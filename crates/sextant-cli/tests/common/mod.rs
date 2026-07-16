//! Shared harness for end-to-end / smoke tests that drive the real `sextant`
//! binary through a pseudo-terminal.
//!
//! This is the TUI analogue of a browser E2E driver (Playwright): spawn the
//! binary in a PTY (`portable-pty`), parse its ANSI output into a virtual
//! screen (`vt100`), and assert on the rendered text. The environment is
//! hermetic — a temp `HOME`/`XDG_CONFIG_HOME`/`XDG_DATA_HOME` and a seeded
//! database — so no manual setup is required for SQLite.
//!
//! PostgreSQL and MySQL fixtures are also available. When Docker is present,
//! the harness starts the containers defined in the workspace `compose.yml`,
//! waits for them to be healthy, seeds them, and writes a `connections.toml`
//! that points at the local Docker ports. If Docker is unavailable, those
//! fixtures return `None` and their tests are skipped.
//!
//! Assertions auto-wait (poll the parsed screen) instead of sleeping blindly,
//! and keystrokes are paced so a lone `Esc` is not misread as an escape
//! sequence.

// Each test binary that does `mod common;` pulls in this file; not every binary
// uses every helper, so silence cross-binary dead-code warnings.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Global lock that serializes tests which may start Docker containers or
/// mutate shared seeded databases. SQLite-only tests do not need this.
static DOCKER_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Acquire the global Docker test lock.
pub fn docker_test_lock() -> &'static Mutex<()> {
    DOCKER_TEST_LOCK.get_or_init(|| Mutex::new(()))
}

pub const ROWS: u16 = 24;
pub const COLS: u16 = 80;

// Raw bytes for the keys we send over the PTY.
pub const ENTER: &[u8] = b"\r";
pub const ESC: &[u8] = b"\x1b";
pub const CTRL_Q: &[u8] = b"\x11"; // quit
pub const CTRL_E: &[u8] = b"\x05"; // run query in the editor
pub const CTRL_S: &[u8] = b"\x13"; // save / commit
pub const CTRL_SPACE: &[u8] = b"\x00"; // autocomplete trigger
pub const TAB: &[u8] = b"\t"; // accept completion
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
        cmd.env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "off".to_string()),
        );
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

    /// Text of a specific screen row, restricted to a column range.
    pub fn row_text(&self, row: u16, start: u16, width: u16) -> String {
        self.parser
            .lock()
            .unwrap()
            .screen()
            .rows(start, width)
            .nth(row as usize)
            .unwrap_or_default()
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
    pub fn wait_for(&mut self, needle: &str, timeout: Duration) {
        let start = Instant::now();
        loop {
            // If the child has already exited, the needle will never appear.
            // Surface the exit immediately instead of waiting out `timeout` —
            // a sub-second "process exited" panic is far easier to debug than
            // a 30s "timed out waiting for X".
            self.panic_if_exited(needle);
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

    /// Poll the screen until `needle` is absent, or panic with a screen dump.
    pub fn wait_for_absent(&mut self, needle: &str, timeout: Duration) {
        let start = Instant::now();
        loop {
            // Same rationale as `wait_for`: an exited child makes the needle
            // trivially "absent", which would silently pass a test that
            // actually crashed the app.
            self.panic_if_exited(needle);
            let screen = self.screen();
            if !screen.contains(needle) {
                return;
            }
            if start.elapsed() > timeout {
                panic!(
                    "timed out; {needle:?} still present after {timeout:?}.\n--- screen ---\n{screen}\n--------------"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Panic with a screen dump if the child process has exited. Called by the
    /// polling waiters so a crash mid-flow is reported immediately rather than
    /// after their full timeout.
    fn panic_if_exited(&mut self, needle: &str) {
        match self.child.try_wait() {
            Ok(None) => {}
            Ok(Some(status)) => {
                let screen = self.screen();
                panic!(
                    "sextant exited (status: {status}) while waiting for {needle:?}.\n--- screen ---\n{screen}\n--------------"
                );
            }
            Err(e) => {
                let screen = self.screen();
                panic!(
                    "error checking sextant status while waiting for {needle:?}: {e}\n--- screen ---\n{screen}\n--------------"
                );
            }
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

    /// Move the tree selection down until `needle` appears as the selected
    /// node (prefixed by `>` or `v`). Panics if it does not appear within the
    /// given number of steps.
    pub fn select_tree_node(&mut self, needle: &str, max_steps: usize) {
        for _ in 0..max_steps {
            let screen = self.screen();
            if screen.contains(&format!("> {needle}")) || screen.contains(&format!("v {needle}")) {
                return;
            }
            self.type_str("j");
            std::thread::sleep(Duration::from_millis(300));
        }
        let screen = self.screen();
        panic!(
            "could not select tree node {needle:?} within {max_steps} steps.\n--- screen ---\n{screen}\n--------------"
        );
    }
}

/// A hermetic test environment: a temp dir with config + a seeded database.
pub struct Fixture {
    pub home: tempfile::TempDir,
    pub db: Option<PathBuf>,
    pub driver: sextant_core::Driver,
}

impl Fixture {
    /// Create a temp environment with one SQLite connection named `conn_name`
    /// pointing at a freshly seeded `test.db` (minimal schema).
    pub fn sqlite(conn_name: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("test.db");
        seed_sqlite(&db);
        write_sqlite_connection(home.path(), conn_name, &db);
        Fixture {
            home,
            db: Some(db),
            driver: sextant_core::Driver::Sqlite,
        }
    }

    /// Create a temp environment using the full `seeds/sqlite.sql` schema
    /// (13-column `users` table, orders, products, type_samples).
    pub fn sqlite_full(conn_name: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let db = home.path().join("test.db");
        seed_sqlite_full(&db);
        write_sqlite_connection(home.path(), conn_name, &db);
        Fixture {
            home,
            db: Some(db),
            driver: sextant_core::Driver::Sqlite,
        }
    }

    /// Create a temp environment with a SQLite connection whose database file
    /// cannot be opened (its parent directory does not exist). The connect
    /// attempt fails at once — a hermetic, Docker-free way to drive the
    /// `ConnectionFailed` path without sqlx's network retry/timeout.
    pub fn sqlite_broken(conn_name: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let bad_path = home.path().join("missing").join("test.db");
        write_sqlite_connection(home.path(), conn_name, &bad_path);
        Fixture {
            home,
            db: None,
            driver: sextant_core::Driver::Sqlite,
        }
    }

    /// Create a temp environment with a PostgreSQL connection named
    /// `conn_name`. Returns `None` if Docker is unavailable or the container
    /// fails to start. Callers must hold `docker_test_lock()` while calling
    /// this and for the duration of the test, because the container and seed
    /// are shared.
    pub fn postgres(conn_name: &str) -> Option<Self> {
        if !docker_available() {
            return None;
        }
        let runtime = docker_runtime()?;
        start_compose_service(&runtime, "postgres").ok()?;
        seed_postgres().ok()?;

        let home = tempfile::tempdir().unwrap();
        let password =
            std::env::var("SEXTANT_DOCKER_PG_PASSWORD").unwrap_or_else(|_| "sextant".to_string());
        write_postgres_connection(
            home.path(),
            conn_name,
            "localhost",
            5433,
            "sextant",
            "sextant_test",
            &password,
        );

        Some(Fixture {
            home,
            db: None,
            driver: sextant_core::Driver::Postgres,
        })
    }

    /// Create a temp environment with a MySQL connection named `conn_name`.
    /// Returns `None` if Docker is unavailable or the container fails to start.
    /// Callers must hold `docker_test_lock()` while calling this and for the
    /// duration of the test, because the container and seed are shared.
    pub fn mysql(conn_name: &str) -> Option<Self> {
        if !docker_available() {
            return None;
        }
        let runtime = docker_runtime()?;
        start_compose_service(&runtime, "mysql").ok()?;
        seed_mysql().ok()?;

        let home = tempfile::tempdir().unwrap();
        let password = std::env::var("SEXTANT_DOCKER_MYSQL_PASSWORD")
            .unwrap_or_else(|_| "sextant".to_string());
        write_mysql_connection(
            home.path(),
            conn_name,
            "localhost",
            3307,
            "sextant",
            "sextant_test",
            &password,
        );

        Some(Fixture {
            home,
            db: None,
            driver: sextant_core::Driver::Mysql,
        })
    }

    pub fn home(&self) -> &Path {
        self.home.path()
    }

    /// Path to the app's local state database inside this environment.
    pub fn state_db(&self) -> PathBuf {
        self.home().join("data").join("sextant").join("state.db")
    }

    /// Path to the directory where exported result sets are written.
    pub fn exports_dir(&self) -> PathBuf {
        self.home().join("data").join("sextant").join("exports")
    }

    /// Spawn the binary against this environment.
    pub fn spawn(&self) -> Tui {
        Tui::spawn(self.home())
    }

    /// Query the backing database directly using the same credentials the app
    /// would use. SQLite returns a `rusqlite::Connection`; PG/MySQL require
    /// `sqlx` and an async runtime. This is intentionally simple: tests that
    /// need cross-checks call the appropriate helper below.
    pub fn sqlite_conn(&self) -> Option<rusqlite::Connection> {
        self.db
            .as_ref()
            .map(|p| rusqlite::Connection::open(p).unwrap())
    }
}

/// Direct verification helpers for assertions after driving the TUI.
pub struct DbCheck {
    pub driver: sextant_core::Driver,
    pub url: String,
}

impl DbCheck {
    /// Build a verifier for the database backing `fixture`.
    pub fn for_fixture(fixture: &Fixture) -> Option<Self> {
        match fixture.driver {
            sextant_core::Driver::Sqlite => fixture.db.as_ref().map(|p| Self {
                driver: sextant_core::Driver::Sqlite,
                url: p.to_string_lossy().to_string(),
            }),
            sextant_core::Driver::Postgres => Some(Self {
                driver: sextant_core::Driver::Postgres,
                url: format!(
                    "postgres://sextant:{}@localhost:5433/sextant_test",
                    std::env::var("SEXTANT_DOCKER_PG_PASSWORD")
                        .unwrap_or_else(|_| "sextant".to_string())
                ),
            }),
            sextant_core::Driver::Mysql => Some(Self {
                driver: sextant_core::Driver::Mysql,
                url: format!(
                    "mysql://sextant:{}@localhost:3307/sextant_test",
                    std::env::var("SEXTANT_DOCKER_MYSQL_PASSWORD")
                        .unwrap_or_else(|_| "sextant".to_string())
                ),
            }),
        }
    }

    /// Drop all seeded tables and recreate a minimal `users` table so that the
    /// tree has exactly one table under one schema. This makes PTY navigation
    /// identical across SQLite, PostgreSQL and MySQL.
    pub async fn reset_to_minimal_users(&self) {
        match self.driver {
            sextant_core::Driver::Sqlite => {
                let conn = rusqlite::Connection::open(&self.url).unwrap();
                conn.execute("DROP TABLE IF EXISTS users", []).unwrap();
                conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)", [])
                    .unwrap();
                conn.execute(
                    "INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')",
                    [],
                )
                .unwrap();
            }
            sextant_core::Driver::Postgres => {
                let pool = sqlx::postgres::PgPool::connect(&self.url).await.unwrap();
                let _ = sqlx::query(
                    "DROP TABLE IF EXISTS orders, products, type_samples, users CASCADE",
                )
                .execute(&pool)
                .await;
                sqlx::query("CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL)")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            sextant_core::Driver::Mysql => {
                let pool = sqlx::mysql::MySqlPool::connect(&self.url).await.unwrap();
                let _ = sqlx::query("DROP TABLE IF EXISTS orders, products, type_samples, users")
                    .execute(&pool)
                    .await;
                sqlx::query("CREATE TABLE users (id INT PRIMARY KEY, name TEXT NOT NULL)")
                    .execute(&pool)
                    .await
                    .unwrap();
                sqlx::query("INSERT INTO users (id, name) VALUES (1, 'alice'), (2, 'bob')")
                    .execute(&pool)
                    .await
                    .unwrap();
            }
        }
    }

    /// True if a row with the given primary key exists in `users.name`.
    pub async fn user_name(&self, id: i64) -> Option<String> {
        match self.driver {
            sextant_core::Driver::Sqlite => {
                let conn = rusqlite::Connection::open(&self.url).unwrap();
                conn.query_row("SELECT name FROM users WHERE id = ?1", [id], |r| r.get(0))
                    .ok()
            }
            sextant_core::Driver::Postgres => {
                let pool = sqlx::postgres::PgPool::connect(&self.url).await.unwrap();
                sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .unwrap()
            }
            sextant_core::Driver::Mysql => {
                let pool = sqlx::mysql::MySqlPool::connect(&self.url).await.unwrap();
                sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .unwrap()
            }
        }
    }

    /// True if a row with the given primary key exists in `z_import_test.name`.
    pub async fn z_import_test_name(&self, id: i64) -> Option<String> {
        match self.driver {
            sextant_core::Driver::Sqlite => {
                let conn = rusqlite::Connection::open(&self.url).unwrap();
                conn.query_row("SELECT name FROM z_import_test WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .ok()
            }
            sextant_core::Driver::Postgres => {
                let pool = sqlx::postgres::PgPool::connect(&self.url).await.unwrap();
                sqlx::query_scalar::<_, String>("SELECT name FROM z_import_test WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .unwrap()
            }
            sextant_core::Driver::Mysql => {
                let pool = sqlx::mysql::MySqlPool::connect(&self.url).await.unwrap();
                sqlx::query_scalar::<_, String>("SELECT name FROM z_import_test WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .unwrap()
            }
        }
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

/// Write a `connections.toml` with a single PostgreSQL connection.
pub fn write_postgres_connection(
    home: &Path,
    name: &str,
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    password: &str,
) {
    let cfg_dir = home.join("config").join("sextant");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml = format!(
        "[[connection]]\nname = \"{name}\"\ndriver = \"postgres\"\nhost = \"{host}\"\nport = {port}\nuser = \"{user}\"\ndatabase = \"{database}\"\n",
    );
    std::fs::write(cfg_dir.join("connections.toml"), toml).unwrap();
    unsafe {
        std::env::set_var(format!("SEXTANT_{}_PASSWORD", env_name(name)), password);
    }
}

/// Write a `connections.toml` with a single MySQL connection.
pub fn write_mysql_connection(
    home: &Path,
    name: &str,
    host: &str,
    port: u16,
    user: &str,
    database: &str,
    password: &str,
) {
    let cfg_dir = home.join("config").join("sextant");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let toml = format!(
        "[[connection]]\nname = \"{name}\"\ndriver = \"mysql\"\nhost = \"{host}\"\nport = {port}\nuser = \"{user}\"\ndatabase = \"{database}\"\n",
    );
    std::fs::write(cfg_dir.join("connections.toml"), toml).unwrap();
    unsafe {
        std::env::set_var(format!("SEXTANT_{}_PASSWORD", env_name(name)), password);
    }
}

fn env_name(name: &str) -> String {
    name.to_uppercase().replace([' ', '-'], "_")
}

/// Create and seed a SQLite database file the app can connect to and browse.
pub fn seed_sqlite(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, email TEXT, active INTEGER, age INTEGER);
         INSERT INTO users (id, name, email, active, age) VALUES (1, 'alice', 'alice@example.com', 1, 30), (2, 'bob', 'bob@example.com', 0, 25);",
    )
    .unwrap();
}

/// Seed a SQLite database with the full workspace `seeds/sqlite.sql` script.
pub fn seed_sqlite_full(db_path: &Path) {
    let conn = rusqlite::Connection::open(db_path).unwrap();
    let sql = include_str!("../../../../seeds/sqlite.sql");
    conn.execute_batch(sql).unwrap();
}

// ---------------------------------------------------------------------------
// Docker helpers for PG/MySQL fixtures
// ---------------------------------------------------------------------------

fn docker_available() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        Command::new("docker")
            .arg("version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

fn docker_runtime() -> Option<String> {
    let compose = Command::new("docker")
        .args(["compose", "version"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if compose {
        return Some("docker compose".to_string());
    }
    let legacy = Command::new("docker-compose")
        .arg("version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if legacy {
        Some("docker-compose".to_string())
    } else {
        None
    }
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn start_compose_service(runtime: &str, service: &str) -> Result<(), String> {
    let root = workspace_root();

    let up = Command::new("sh")
        .arg("-c")
        .arg(format!("{runtime} up -d {service}"))
        .current_dir(&root)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to start {service}: {e}"))?;
    if !up.status.success() {
        return Err(format!(
            "docker compose up {service} failed: {}",
            String::from_utf8_lossy(&up.stderr)
        ));
    }

    // Wait for healthy. The compose file defines healthchecks; poll `docker compose ps`.
    let start = Instant::now();
    let timeout = Duration::from_secs(60);
    loop {
        let ps = Command::new("sh")
            .arg("-c")
            .arg(format!("{runtime} ps {service}"))
            .current_dir(&root)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("failed to query {service} status: {e}"))?;
        let out = String::from_utf8_lossy(&ps.stdout);
        if out.contains("healthy") {
            return Ok(());
        }
        if start.elapsed() > timeout {
            return Err(format!(
                "{service} did not become healthy within {timeout:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn seed_postgres() -> Result<(), String> {
    let script = workspace_root().join("seeds").join("postgres.sql");
    let out = Command::new("docker")
        .args([
            "exec",
            "-i",
            "sextant-postgres-test",
            "psql",
            "-U",
            "sextant",
            "-d",
            "sextant_test",
            "-q",
        ])
        .stdin(Stdio::from(
            std::fs::File::open(&script).map_err(|e| e.to_string())?,
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to seed postgres: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "postgres seed failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn seed_mysql() -> Result<(), String> {
    let script = workspace_root().join("seeds").join("mysql.sql");
    let password =
        std::env::var("SEXTANT_DOCKER_MYSQL_PASSWORD").unwrap_or_else(|_| "sextant".to_string());
    let out = Command::new("docker")
        .args([
            "exec",
            "-i",
            "sextant-mysql-test",
            "mysql",
            "-u",
            "sextant",
            &format!("-p{password}"),
            "sextant_test",
        ])
        .stdin(Stdio::from(
            std::fs::File::open(&script).map_err(|e| e.to_string())?,
        ))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to seed mysql: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "mysql seed failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}
