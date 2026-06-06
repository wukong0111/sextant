//! End-to-end tests that drive the real `sextant` binary through a PTY.
//!
//! See `tests/common/mod.rs` for the harness. These are the TUI analogue of a
//! browser E2E suite: spawn the binary, send keystrokes, assert on the parsed
//! screen, and cross-check the on-disk `state.db`.

mod common;

use std::time::Duration;

use common::{CTRL_E, CTRL_Q, ENTER, Fixture};

#[test]
fn boots_renders_connection_and_quits_cleanly() {
    let fx = Fixture::sqlite("e2e-boot");
    let mut tui = fx.spawn();

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
    let fx = Fixture::sqlite("e2e-hist");
    let mut tui = fx.spawn();
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
    let state_db = fx.state_db();
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

#[test]
fn exports_result_set_to_csv_file() {
    let fx = Fixture::sqlite("e2e-export");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-export", Duration::from_secs(10));

    // Connect and run a query that returns the seeded rows.
    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));
    tui.leader("e");
    tui.wait_for("insert", Duration::from_secs(10));
    tui.type_str("i");
    tui.type_str("SELECT id, name FROM users");
    tui.wait_for("SELECT id, name FROM users", Duration::from_secs(10));
    tui.esc();
    tui.wait_for("<C-e> run", Duration::from_secs(10));
    tui.send(CTRL_E);
    tui.wait_for("rows", Duration::from_secs(10));
    tui.esc(); // close the editor modal
    tui.wait_for("export", Duration::from_secs(10)); // main status hint

    // Open the export menu (<Space>x), pick the first format (CSV) with Enter.
    tui.leader("x");
    tui.wait_for("Export as", Duration::from_secs(10));
    tui.wait_for("CSV", Duration::from_secs(10));
    tui.send(ENTER);
    tui.wait_for("exported", Duration::from_secs(10)); // success notice

    // Quit (the dirty editor buffer triggers the prompt; `d` discards).
    tui.send(CTRL_Q);
    tui.wait_for("Unsaved buffers", Duration::from_secs(10));
    tui.type_str("d");
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );

    // Cross-check the backend: a CSV file was written with the seeded data.
    let dir = fx.exports_dir();
    let csv = std::fs::read_dir(&dir)
        .expect("exports dir should exist")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().is_some_and(|x| x == "csv"))
        .expect("a .csv export should have been written");
    let contents = std::fs::read_to_string(&csv).unwrap();
    assert!(contents.starts_with("id,name\n"), "CSV needs a header row");
    assert!(contents.contains("1,alice"), "CSV must contain the rows");
    assert!(contents.contains("2,bob"));
}

#[test]
fn imports_csv_into_selected_table() {
    let fx = Fixture::sqlite("e2e-import");
    // A CSV holding a new row to import into the seeded `users` table.
    let csv = fx.home().join("import.csv");
    std::fs::write(&csv, "id,name\n3,carol\n").unwrap();

    let mut tui = fx.spawn();
    tui.wait_for("e2e-import", Duration::from_secs(10));

    // Connect; the seeded table surfaces once introspection completes.
    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));

    // Move the tree selection down from the connection to the `users` table
    // (connection → schema → table), then start the import.
    tui.type_str("j");
    tui.type_str("j");
    tui.leader("i");
    tui.wait_for("Import into users", Duration::from_secs(10));

    // Type the absolute path to the CSV and load it into the preview.
    tui.type_str(csv.to_str().unwrap());
    tui.send(ENTER);
    tui.wait_for("Confirm import", Duration::from_secs(10));
    tui.wait_for("Insert 1 row", Duration::from_secs(10));

    // Confirm the import; the success notice appears in the status line.
    tui.send(ENTER);
    tui.wait_for("imported", Duration::from_secs(10));

    // Quit (no dirty buffer to prompt about).
    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );

    // Cross-check the backend: the imported row is now in the user's database.
    let conn = rusqlite::Connection::open(&fx.db).unwrap();
    let name: String = conn
        .query_row("SELECT name FROM users WHERE id = 3", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "carol", "the imported row must be committed");
}
