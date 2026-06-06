//! Shared harness for end-to-end / smoke tests that drive the real `sextant`
//! binary through a pseudo-terminal.
//!
//! This is the TUI analogue of a browser E2E driver (Playwright): spawn the
//! binary in a PTY (`portable-pty`), parse its ANSI output into a virtual
//! screen (`vt100`), and assert on the rendered text. The environment is
//! hermetic — a temp `HOME`/`XDG_CONFIG_HOME`/`XDG_DATA_HOME` and a seeded
//! SQLite file — so no Docker is required.
//!
//! Assertions auto-wait (poll the parsed screen) instead of sleeping blindly,
//! and keystrokes are paced so a lone `Esc` is not misread as an escape
//! sequence.

// Each test binary that does `mod common;` pulls in this file; not every binary
// uses every helper, so silence cross-binary dead-code warnings.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

pub const ROWS: u16 = 24;
pub const COLS: u16 = 80;

// Raw bytes for the keys we send over the PTY.
pub const ENTER: &[u8] = b"\r";
pub const ESC: &[u8] = b"\x1b";
pub const CTRL_Q: &[u8] = b"\x11"; // quit
pub const CTRL_E: &[u8] = b"\x05"; // run query in the editor
pub const CTRL_S: &[u8] = b"\x13"; // save / commit
pub const SPACE: &[u8] = b" "; // leader key

/// Default timeout for `wait_for` / `wait_exit`.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A running `sextant` process attached to a PTY, with its output parsed into a
/// live `vt100` screen.
pub struct Tui {
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Tui {
    /// Spawn the binary with a hermetic environment rooted at `home`.
    pub fn spawn(home: &Path) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");

        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_sextant"));
        cmd.cwd(home);
        cmd.env("HOME", home);
        cmd.env("XDG_CONFIG_HOME", home.join("config"));
        cmd.env("XDG_DATA_HOME", home.join("data"));
        cmd.env("TERM", "xterm-256color");
        cmd.env("RUST_LOG", "off");
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }

        let child = pair.slave.spawn_command(cmd).expect("spawn sextant");
        drop(pair.slave); // so the reader sees EOF when the child exits

        let mut reader = pair.master.try_clone_reader().expect("reader");
        let writer = pair.master.take_writer().expect("writer");

        let parser = Arc::new(Mutex::new(vt100::Parser::new(ROWS, COLS, 0)));
        let parser_clone = Arc::clone(&parser);
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                parser_clone.lock().unwrap().process(&buf[..n]);
            }
        });

        Tui {
            _master: pair.master,
            writer,
            parser,
            child,
        }
    }

    /// The current visible screen as text.
    pub fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    /// Print the current screen to stderr (visible with `--nocapture`).
    pub fn dump(&self, label: &str) {
        eprintln!(
            "\n########## {label} ##########\n{}\n{}",
            self.screen(),
            "#".repeat(20 + label.len() + 2)
        );
    }

    pub fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().ok();
        // Pace keystrokes so the event loop processes them in order.
        std::thread::sleep(Duration::from_millis(60));
    }

    /// Send a lone `Esc`. Crossterm needs a pause after `0x1b` to tell a
    /// standalone Esc apart from the start of an escape sequence.
    pub fn esc(&mut self) {
        self.send(ESC);
        std::thread::sleep(Duration::from_millis(150));
    }

    pub fn type_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    /// Press the leader key (`Space`) followed by `key`.
    pub fn leader(&mut self, key: &str) {
        self.send(SPACE);
        self.type_str(key);
    }

    /// Poll the screen until it contains `needle`, or panic with a screen dump.
    pub fn wait_for(&self, needle: &str, timeout: Duration) {
        let start = Instant::now();
        loop {
            let screen = self.screen();
            if screen.contains(needle) {
                return;
            }
            if start.elapsed() > timeout {
                panic!(
                    "timed out waiting for {needle:?} after {timeout:?}.\n--- screen ---\n{screen}\n--------------"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Wait for the process to exit and return whether it exited successfully.
    pub fn wait_exit(&mut self, timeout: Duration) -> bool {
        let start = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.success(),
                Ok(None) => {}
                Err(_) => return false,
            }
            if start.elapsed() > timeout {
                let _ = self.child.kill();
                panic!("process did not exit within {timeout:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A hermetic test environment: a temp dir with config + a seeded SQLite db.
pub struct Fixture {
    pub home: tempfile::TempDir,
    pub db: std::path::PathBuf,
}

impl Fixture {
    /// Create a temp environment with one SQLite connection named `conn_name`
    /// pointing at a freshly seeded `test.db`.
    pub fn sqlite(conn_name: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("test.db");
        seed_sqlite(&db);
        write_sqlite_connection(home.path(), conn_name, &db);
        Fixture { home, db }
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// Path to the app's local state database inside this environment.
    pub fn state_db(&self) -> std::path::PathBuf {
        self.home().join("data").join("sextant").join("state.db")
    }

    /// Path to the directory where exported result sets are written.
    pub fn exports_dir(&self) -> std::path::PathBuf {
        self.home().join("data").join("sextant").join("exports")
    }

    /// Spawn the binary against this environment.
    pub fn spawn(&self) -> Tui {
        Tui::spawn(self.home())
    }
}

/// Write a `connections.toml` with a single SQLite connection into the hermetic
/// config dir.
pub fn write_sqlite_connection(home: &Path, name: &str, db_path: &Path) {
    let cfg_dir = home.join("config").join("sextant");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml = format!(
        "[[connection]]\nname = \"{name}\"\ndriver = \"sqlite\"\npath = \"{}\"\n",
        db_path.display()
    );
    std::fs::write(cfg_dir.join("connections.toml"), toml).unwrap();
}

/// Create and seed a SQLite database file the app can connect to and browse.
pub fn seed_sqlite(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
         INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob');",
    )
    .unwrap();
}
