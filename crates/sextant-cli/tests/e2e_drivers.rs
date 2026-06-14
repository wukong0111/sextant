//! End-to-end tests that exercise the real `sextant` binary against every
//! supported database driver.
//!
//! These mirror the SQLite-only PTY suite in `e2e.rs`, but run against
//! PostgreSQL and MySQL when Docker is available. If Docker is unavailable the
//! TCP fixtures return `None` and that driver is skipped.
//!
//! Tests inside this file are serialized with a global lock because they share
//! Docker containers and seeded databases; the lock also prevents multiple PTY
//! sessions from running concurrently, which can destabilize timing-sensitive
//! assertions.

mod common;

use std::time::Duration;

use common::{CTRL_E, CTRL_Q, CTRL_SPACE, DbCheck, ENTER, Fixture, TAB, docker_test_lock};

fn lock_guard() -> std::sync::MutexGuard<'static, ()> {
    docker_test_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn all_fixtures(prefix: &str) -> Vec<(Option<Fixture>, &'static str, String)> {
    vec![
        (
            Some(Fixture::sqlite(&format!("{prefix}-sq"))),
            "SQLite",
            format!("{prefix}-sq"),
        ),
        (
            Fixture::postgres(&format!("{prefix}-pg")),
            "PostgreSQL",
            format!("{prefix}-pg"),
        ),
        (
            Fixture::mysql(&format!("{prefix}-my")),
            "MySQL",
            format!("{prefix}-my"),
        ),
    ]
}

#[test]
fn connect_and_query_records_in_history() {
    let _guard = lock_guard();
    for (fx, label, conn_name) in all_fixtures("conn") {
        let Some(fx) = fx else {
            eprintln!("{label}: skipped (fixture unavailable)");
            continue;
        };
        let mut tui = fx.spawn();
        tui.wait_for(&conn_name, Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("users", Duration::from_secs(20));

        tui.leader("e");
        tui.wait_for("insert", Duration::from_secs(10));
        tui.type_str("i");
        tui.type_str("SELECT 42 AS answer");
        tui.wait_for("SELECT 42 AS answer", Duration::from_secs(10));
        tui.esc();
        tui.wait_for("<C-e> run", Duration::from_secs(10));
        tui.send(CTRL_E);
        tui.wait_for("rows", Duration::from_secs(15));
        tui.esc();
        tui.wait_for("<Space>e editor", Duration::from_secs(10));

        tui.leader("h");
        tui.wait_for("Query history", Duration::from_secs(10));
        tui.wait_for("SELECT 42 AS answer", Duration::from_secs(10));
        tui.esc();

        tui.send(CTRL_Q);
        tui.wait_for("Unsaved buffers", Duration::from_secs(10));
        tui.type_str("d");
        assert!(
            tui.wait_exit(Duration::from_secs(10)),
            "{label}: sextant should exit"
        );

        let state_db = fx.state_db();
        assert!(state_db.exists(), "{label}: state.db should exist");
        let conn = rusqlite::Connection::open(&state_db).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM query_history WHERE sql = ?1",
                ["SELECT 42 AS answer"],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "{label}: the executed query must be recorded in history"
        );
    }
}

#[test]
fn browse_table_renders_rows() {
    let _guard = lock_guard();
    for (fx, label, conn_name) in all_fixtures("browse") {
        let Some(fx) = fx else {
            eprintln!("{label}: skipped (fixture unavailable)");
            continue;
        };

        {
            let db = DbCheck::for_fixture(&fx).unwrap();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(db.reset_to_minimal_users());
        }

        let mut tui = fx.spawn();
        tui.wait_for(&conn_name, Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("users", Duration::from_secs(20));

        // connection → schema (pg/mysql) / main (sqlite) → users
        tui.type_str("j");
        tui.type_str("j");
        tui.send(ENTER);
        tui.wait_for("alice", Duration::from_secs(15));
        tui.wait_for("bob", Duration::from_secs(15));
        tui.wait_for("rows", Duration::from_secs(15));

        tui.send(CTRL_Q);
        assert!(
            tui.wait_exit(Duration::from_secs(10)),
            "{label}: sextant should exit"
        );
    }
}

#[test]
fn autocomplete_inserts_table_name() {
    let _guard = lock_guard();
    for (fx, label, conn_name) in all_fixtures("ac") {
        let Some(fx) = fx else {
            eprintln!("{label}: skipped (fixture unavailable)");
            continue;
        };

        {
            let db = DbCheck::for_fixture(&fx).unwrap();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(db.reset_to_minimal_users());
        }

        let mut tui = fx.spawn();
        tui.wait_for(&conn_name, Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("users", Duration::from_secs(20));

        tui.leader("e");
        tui.wait_for("insert", Duration::from_secs(10));
        tui.type_str("i");
        tui.type_str("SELECT * FROM us");
        tui.wait_for("SELECT * FROM us", Duration::from_secs(10));

        tui.send(CTRL_SPACE);
        tui.wait_for("users", Duration::from_secs(10));
        tui.send(TAB);
        tui.wait_for("SELECT * FROM users", Duration::from_secs(10));

        tui.esc();
        tui.wait_for("<C-e> run", Duration::from_secs(10));
        tui.send(CTRL_E);
        tui.wait_for("rows", Duration::from_secs(15));

        tui.esc();
        tui.send(CTRL_Q);
        tui.wait_for("Unsaved buffers", Duration::from_secs(10));
        tui.type_str("d");
        assert!(
            tui.wait_exit(Duration::from_secs(10)),
            "{label}: sextant should exit"
        );
    }
}

#[test]
fn exports_result_set_to_csv_file() {
    let _guard = lock_guard();
    for (fx, label, conn_name) in all_fixtures("export") {
        let Some(fx) = fx else {
            eprintln!("{label}: skipped (fixture unavailable)");
            continue;
        };

        {
            let db = DbCheck::for_fixture(&fx).unwrap();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(db.reset_to_minimal_users());
        }

        let mut tui = fx.spawn();
        tui.wait_for(&conn_name, Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("users", Duration::from_secs(20));

        tui.leader("e");
        tui.wait_for("insert", Duration::from_secs(10));
        tui.type_str("i");
        tui.type_str("SELECT id, name FROM users");
        tui.wait_for("SELECT id, name FROM users", Duration::from_secs(10));
        tui.esc();
        tui.wait_for("<C-e> run", Duration::from_secs(10));
        tui.send(CTRL_E);
        tui.wait_for("rows", Duration::from_secs(15));
        tui.esc();
        tui.wait_for("<Space>e editor", Duration::from_secs(10));

        tui.leader("x");
        tui.wait_for("Export as", Duration::from_secs(10));
        tui.wait_for("CSV", Duration::from_secs(10));
        tui.send(ENTER);
        tui.wait_for("exported", Duration::from_secs(15));

        tui.send(CTRL_Q);
        tui.wait_for("Unsaved buffers", Duration::from_secs(10));
        tui.type_str("d");
        assert!(
            tui.wait_exit(Duration::from_secs(10)),
            "{label}: sextant should exit"
        );

        let dir = fx.exports_dir();
        let csv = std::fs::read_dir(&dir)
            .expect("{label}: exports dir should exist")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|x| x == "csv"))
            .expect("{label}: a .csv export should have been written");
        let contents = std::fs::read_to_string(&csv).unwrap();
        assert!(
            contents.starts_with("id,name\n"),
            "{label}: CSV needs a header row"
        );
        assert!(
            contents.to_lowercase().contains("1,alice"),
            "{label}: CSV must contain the rows"
        );
        assert!(
            contents.to_lowercase().contains("2,bob"),
            "{label}: CSV must contain bob"
        );
    }
}

#[test]
fn imports_csv_into_selected_table() {
    let _guard = lock_guard();
    for (fx, label, conn_name) in all_fixtures("import") {
        let Some(fx) = fx else {
            eprintln!("{label}: skipped (fixture unavailable)");
            continue;
        };

        // Create a simple target table with no constraints beyond a PK. The
        // `z_` prefix makes it alphabetically last, so a small fixed number of
        // `j` steps selects it reliably across drivers.
        {
            let db = DbCheck::for_fixture(&fx).unwrap();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                db.reset_to_minimal_users().await;
                match db.driver {
                    sextant_core::Driver::Sqlite => {
                        let conn = rusqlite::Connection::open(&db.url).unwrap();
                        conn.execute("DROP TABLE IF EXISTS z_import_test", [])
                            .unwrap();
                        conn.execute(
                            "CREATE TABLE z_import_test (id INTEGER PRIMARY KEY, name TEXT)",
                            [],
                        )
                        .unwrap();
                    }
                    sextant_core::Driver::Postgres => {
                        let pool = sqlx::postgres::PgPool::connect(&db.url).await.unwrap();
                        let _ = sqlx::query("DROP TABLE IF EXISTS z_import_test CASCADE")
                            .execute(&pool)
                            .await;
                        sqlx::query("CREATE TABLE z_import_test (id INT PRIMARY KEY, name TEXT)")
                            .execute(&pool)
                            .await
                            .unwrap();
                    }
                    sextant_core::Driver::Mysql => {
                        let pool = sqlx::mysql::MySqlPool::connect(&db.url).await.unwrap();
                        let _ = sqlx::query("DROP TABLE IF EXISTS z_import_test")
                            .execute(&pool)
                            .await;
                        sqlx::query("CREATE TABLE z_import_test (id INT PRIMARY KEY, name TEXT)")
                            .execute(&pool)
                            .await
                            .unwrap();
                    }
                }
            });
        }

        let csv = fx.home().join("import.csv");
        std::fs::write(&csv, "id,name\n5,carol\n").unwrap();

        let mut tui = fx.spawn();
        tui.wait_for(&conn_name, Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("z_import_test", Duration::from_secs(20));

        // connection → schema (pg/mysql) / main (sqlite) → users → z_import_test
        for _ in 0..4 {
            tui.type_str("j");
        }
        tui.leader("i");
        tui.wait_for("Import into z_import_test", Duration::from_secs(10));

        tui.type_str(csv.to_str().unwrap());
        tui.send(ENTER);
        tui.wait_for("Confirm import", Duration::from_secs(10));
        tui.wait_for("Insert 1 row", Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("imported", Duration::from_secs(15));

        tui.send(CTRL_Q);
        assert!(
            tui.wait_exit(Duration::from_secs(10)),
            "{label}: sextant should exit"
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        let name = rt
            .block_on(DbCheck::for_fixture(&fx).unwrap().z_import_test_name(5))
            .expect("{label}: imported row must exist");
        assert_eq!(name, "carol", "{label}: the imported row must be committed");
    }
}

#[test]
fn schema_viewer_shows_columns_in_tree() {
    let _guard = lock_guard();
    for (fx, label, conn_name) in all_fixtures("schema") {
        let Some(fx) = fx else {
            eprintln!("{label}: skipped (fixture unavailable)");
            continue;
        };

        {
            let db = DbCheck::for_fixture(&fx).unwrap();
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(db.reset_to_minimal_users());
        }

        let mut tui = fx.spawn();
        tui.wait_for(&conn_name, Duration::from_secs(10));

        tui.send(ENTER);
        tui.wait_for("users", Duration::from_secs(20));

        tui.type_str("j");
        tui.type_str("j");
        tui.type_str("l");
        tui.wait_for("id", Duration::from_secs(15));
        tui.wait_for("name", Duration::from_secs(15));

        tui.send(CTRL_Q);
        assert!(
            tui.wait_exit(Duration::from_secs(10)),
            "{label}: sextant should exit"
        );
    }
}
