//! End-to-end tests that drive the real `sextant` binary.
//!
//! This is the TUI analogue of a browser E2E suite (Playwright): instead of a
//! headless browser we spawn the binary inside a pseudo-terminal, feed it
//! keystrokes, and parse the emitted ANSI stream into a virtual screen grid we
//! can assert on. The environment is hermetic — a temp `XDG_CONFIG_HOME` /
//! `XDG_DATA_HOME` and a seeded SQLite file — so no Docker is required.
//!
//! Like Playwright's auto-waiting, assertions poll the parsed screen until the
//! expected text appears (with a timeout) rather than sleeping blindly.

use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const ROWS: u16 = 24;
const COLS: u16 = 80;

// Raw bytes for the keys we send over the PTY.
const ENTER: &[u8] = b"\r";
const ESC: &[u8] = b"\x1b";
const CTRL_Q: &[u8] = b"\x11"; // quit
const CTRL_E: &[u8] = b"\x05"; // run query in the editor
const SPACE: &[u8] = b" "; // leader key

/// A running `sextant` process attached to a PTY, with its output parsed into a
/// live `vt100` screen.
struct Tui {
    writer: Box<dyn Write + Send>,
    parser: Arc<Mutex<vt100::Parser>>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl Tui {
    /// Spawn the binary with a hermetic environment rooted at `home`.
    fn spawn(home: &Path) -> Self {
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
        // Keep the master alive for the lifetime of the writer.
        std::mem::forget(pair.master);

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
            writer,
            parser,
            child,
        }
    }

    /// The current visible screen as text.
    fn screen(&self) -> String {
        self.parser.lock().unwrap().screen().contents()
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).expect("write to pty");
        self.writer.flush().ok();
        // Pace keystrokes so the event loop processes them in order.
        std::thread::sleep(Duration::from_millis(60));
    }

    /// Send a lone `Esc`. Crossterm needs a pause after `0x1b` to tell a
    /// standalone Esc apart from the start of an escape sequence.
    fn esc(&mut self) {
        self.send(ESC);
        std::thread::sleep(Duration::from_millis(150));
    }

    fn type_str(&mut self, s: &str) {
        self.send(s.as_bytes());
    }

    /// Press the leader key (`Space`) followed by `key`.
    fn leader(&mut self, key: &str) {
        self.send(SPACE);
        self.type_str(key);
    }

    /// Poll the screen until it contains `needle`, or panic with a screen dump.
    fn wait_for(&self, needle: &str, timeout: Duration) {
        let start = Instant::now();
        loop {
            let screen = self.screen();
            if screen.contains(needle) {
                return;
            }
            if start.elapsed() > timeout {
                panic!(
                    "timed out waiting for {needle:?} after {:?}.\n--- screen ---\n{screen}\n--------------",
                    timeout
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Wait for the process to exit and return whether it exited successfully.
    fn wait_exit(&mut self, timeout: Duration) -> bool {
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

/// Write a `connections.toml` with a single SQLite connection into the hermetic
/// config dir.
fn write_sqlite_connection(home: &Path, name: &str, db_path: &Path) {
    let cfg_dir = home.join("config").join("sextant");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml = format!(
        "[[connection]]\nname = \"{name}\"\ndriver = \"sqlite\"\npath = \"{}\"\n",
        db_path.display()
    );
    std::fs::write(cfg_dir.join("connections.toml"), toml).unwrap();
}

/// Create and seed a SQLite database file the app can connect to and browse.
fn seed_sqlite(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT);
         INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob');",
    )
    .unwrap();
}

#[test]
fn boots_renders_connection_and_quits_cleanly() {
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("test.db");
    seed_sqlite(&db);
    write_sqlite_connection(home.path(), "e2e-boot", &db);

    let mut tui = Tui::spawn(home.path());

    // The sidebar lists the configured connection, and the status line shows
    // there is no active connection yet.
    tui.wait_for("e2e-boot", Duration::from_secs(10));
    tui.wait_for("no connection", Duration::from_secs(10));

    // Ctrl+Q quits cleanly (no dirty buffers).
    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit with status 0"
    );
}

#[test]
fn editor_query_is_recorded_in_history() {
    let home = tempfile::tempdir().unwrap();
    let db = home.path().join("test.db");
    seed_sqlite(&db);
    write_sqlite_connection(home.path(), "e2e-hist", &db);

    let mut tui = Tui::spawn(home.path());
    tui.wait_for("e2e-hist", Duration::from_secs(10));

    // Connect (Enter on the selected connection) and wait for introspection to
    // surface the seeded table — proof the async DB round-trip completed.
    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));

    // Open the editor, type a query, run it (Ctrl+E), then close the editor.
    // Each step waits for an on-screen marker before the next, the TUI analogue
    // of Playwright's auto-waiting.
    tui.leader("e");
    tui.wait_for("insert", Duration::from_secs(10)); // editor Normal-mode hint
    tui.type_str("i"); // enter insert mode
    tui.type_str("SELECT 42");
    tui.wait_for("SELECT 42", Duration::from_secs(10)); // text rendered in editor
    tui.esc(); // back to Normal
    tui.wait_for("<C-e> run", Duration::from_secs(10)); // Normal-mode hint is back
    tui.send(CTRL_E); // execute
    tui.wait_for("rows", Duration::from_secs(10)); // result returned
    tui.esc(); // close the editor modal
    tui.wait_for("history", Duration::from_secs(10)); // main status hint

    // The history picker (<Space>h) lists the query we just ran.
    tui.leader("h");
    tui.wait_for("Query history", Duration::from_secs(10));
    tui.wait_for("SELECT 42", Duration::from_secs(10));

    // Dismiss the picker and quit (the dirty buffer triggers the quit prompt;
    // `d` discards and quits).
    tui.esc();
    tui.send(CTRL_Q);
    tui.wait_for("Unsaved buffers", Duration::from_secs(10));
    tui.type_str("d");
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );

    // Cross-check the backend: the query landed in the on-disk state.db.
    let state_db = home.path().join("data").join("sextant").join("state.db");
    assert!(state_db.exists(), "state.db should have been created");
    let conn = rusqlite::Connection::open(&state_db).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM query_history WHERE sql = ?1",
            ["SELECT 42"],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "the executed query must be recorded in history");
}
