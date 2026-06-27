//! End-to-end tests that drive the real `sextant` binary through a PTY.
//!
//! See `tests/common/mod.rs` for the harness. These are the TUI analogue of a
//! browser E2E suite: spawn the binary, send keystrokes, assert on the parsed
//! screen, and cross-check the on-disk `state.db`.

mod common;

use std::time::Duration;

use common::{CTRL_E, CTRL_Q, CTRL_SPACE, ENTER, Fixture, SPACE, TAB};

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
    // Leftmost main-view hint: survives narrow-terminal truncation, unlike the
    // later leader hints (history/recent/export), which the help cell can clip.
    tui.wait_for("<Space>e editor", Duration::from_secs(10));

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
fn leader_shows_which_key_menu() {
    let fx = Fixture::sqlite("e2e-whichkey");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-whichkey", Duration::from_secs(10));

    // Press the leader alone (no second key): the which-key popup appears,
    // listing the continuations with their actions. "open SQL editor" only
    // shows in this menu (the status hint reads "<Space>e editor").
    tui.send(SPACE);
    tui.wait_for("open SQL editor", Duration::from_secs(10));
    tui.wait_for("query history", Duration::from_secs(10));

    // Completing the chord (`e`) dismisses the menu and opens the editor.
    tui.type_str("e");
    tui.wait_for("insert", Duration::from_secs(10)); // editor Normal-mode hint

    tui.esc(); // close editor
    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
}

#[test]
fn grid_columns_resize_on_full_schema() {
    // Use the real seeds/sqlite.sql which has a 13-column users table.
    let fx = Fixture::sqlite_full("e2e-resize-full");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-resize-full", Duration::from_secs(10));

    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));
    // Navigate past main schema to reach users (orders, products, sqlite_sequence, type_samples, users).
    for _ in 0..6 {
        tui.type_str("j");
    }
    tui.send(ENTER);
    tui.wait_for("Alice", Duration::from_secs(10));

    // Move cursor id -> name.
    tui.type_str("l");
    std::thread::sleep(Duration::from_millis(100));

    let before_narrow_name = tui.row_text(1, 20, 60);

    // Narrow name well below its content to force truncation.
    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    let after_narrow_name = tui.row_text(1, 20, 60);
    assert_ne!(
        before_narrow_name, after_narrow_name,
        "narrowing 'name' should change the data row; before={before_narrow_name:?} after={after_narrow_name:?}"
    );

    // Restore name with auto-fit.
    tui.leader("w");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before_narrow_name,
        tui.row_text(1, 20, 60),
        "restoring 'name' should restore the data row"
    );

    // Move cursor name -> email.
    tui.type_str("l");
    std::thread::sleep(Duration::from_millis(100));

    let before_narrow_email = tui.row_text(1, 20, 60);

    // Narrow email well below its content to force truncation.
    for _ in 0..8 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    let after_narrow_email = tui.row_text(1, 20, 60);
    assert_ne!(
        before_narrow_email, after_narrow_email,
        "narrowing 'email' should change the data row; before={before_narrow_email:?} after={after_narrow_email:?}"
    );

    // Restore email with auto-fit.
    tui.leader("w");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before_narrow_email,
        tui.row_text(1, 20, 60),
        "restoring 'email' should restore the data row"
    );

    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
}

#[test]
fn grid_columns_can_be_resized() {
    let fx = Fixture::sqlite("e2e-resize");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-resize", Duration::from_secs(10));

    // Connect and browse the users table.
    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));
    tui.type_str("j");
    tui.type_str("j");
    tui.send(ENTER);
    tui.wait_for("alice", Duration::from_secs(10));

    // --- id column (cursor already here) ---
    let before = tui.row_text(0, 20, 60);
    for _ in 0..5 {
        tui.type_str(">");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_ne!(
        before,
        tui.row_text(0, 20, 60),
        "widening 'id' should change layout"
    );

    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before,
        tui.row_text(0, 20, 60),
        "narrowing 'id' should restore layout"
    );

    // --- name column ---
    tui.type_str("l");
    std::thread::sleep(Duration::from_millis(100));
    let before = tui.row_text(0, 20, 60);
    for _ in 0..5 {
        tui.type_str(">");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_ne!(
        before,
        tui.row_text(0, 20, 60),
        "widening 'name' should change layout"
    );

    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before,
        tui.row_text(0, 20, 60),
        "narrowing 'name' should restore layout"
    );

    // --- email column ---
    tui.type_str("l");
    std::thread::sleep(Duration::from_millis(100));
    let before_hdr = tui.row_text(0, 20, 60);
    let before_data = tui.row_text(1, 20, 60);
    for _ in 0..5 {
        tui.type_str(">");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_ne!(
        before_hdr,
        tui.row_text(0, 20, 60),
        "widening 'email' should change layout"
    );

    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before_hdr,
        tui.row_text(0, 20, 60),
        "narrowing 'email' should restore layout"
    );
    assert_eq!(
        before_data,
        tui.row_text(1, 20, 60),
        "data row should also be restored"
    );

    // Narrow email below its content length to force visible truncation.
    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    let truncated = tui.row_text(1, 20, 60);
    assert_ne!(
        before_data, truncated,
        "narrowing 'email' below auto-width should truncate cell content"
    );

    // Restore email.
    for _ in 0..5 {
        tui.type_str(">");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before_data,
        tui.row_text(1, 20, 60),
        "restoring 'email' should restore data"
    );

    // --- active column ---
    tui.type_str("l");
    std::thread::sleep(Duration::from_millis(100));
    let before = tui.row_text(0, 20, 60);
    for _ in 0..5 {
        tui.type_str(">");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_ne!(
        before,
        tui.row_text(0, 20, 60),
        "widening 'active' should change layout"
    );

    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before,
        tui.row_text(0, 20, 60),
        "narrowing 'active' should restore layout"
    );

    // --- age column (last visible) ---
    // Widening the last visible column only adds empty space to its right,
    // which is indistinguishable in the header row. We verify the data row
    // still contains the expected values instead.
    tui.type_str("l");
    std::thread::sleep(Duration::from_millis(100));
    let before = tui.row_text(1, 20, 60);
    for _ in 0..5 {
        tui.type_str(">");
    }
    std::thread::sleep(Duration::from_millis(200));
    let after = tui.row_text(1, 20, 60);
    assert!(
        after.contains("30"),
        "widening 'age' should not hide its content"
    );

    for _ in 0..5 {
        tui.type_str("<");
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        before,
        tui.row_text(1, 20, 60),
        "narrowing 'age' should restore data row"
    );

    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
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
    // Leftmost main-view hint: survives narrow-terminal truncation (the later
    // `<Space>x export` hint can be clipped by the pinned help cell).
    tui.wait_for("<Space>e editor", Duration::from_secs(10));

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
    let conn = fx.sqlite_conn().unwrap();
    let name: String = conn
        .query_row("SELECT name FROM users WHERE id = 3", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "carol", "the imported row must be committed");
}

#[test]
fn browse_table_renders_rows_with_limit() {
    let fx = Fixture::sqlite("e2e-browse");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-browse", Duration::from_secs(10));

    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));

    tui.type_str("j");
    tui.type_str("j");
    tui.send(ENTER);
    tui.wait_for("alice", Duration::from_secs(10));
    tui.wait_for("bob", Duration::from_secs(10));
    tui.wait_for("2 rows", Duration::from_secs(10));

    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
}

#[test]
fn autocomplete_inserts_table_name() {
    let fx = Fixture::sqlite("e2e-ac");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-ac", Duration::from_secs(10));

    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));

    tui.leader("e");
    tui.wait_for("insert", Duration::from_secs(10));
    tui.type_str("i");
    tui.type_str("SELECT * FROM ");
    tui.wait_for("SELECT * FROM ", Duration::from_secs(10));

    tui.send(CTRL_SPACE);
    tui.wait_for("users", Duration::from_secs(5));
    tui.send(TAB);
    tui.wait_for("SELECT * FROM users", Duration::from_secs(5));

    tui.esc();
    tui.wait_for("<C-e> run", Duration::from_secs(10));
    tui.send(CTRL_E);
    tui.wait_for("2 rows", Duration::from_secs(10));

    tui.esc();
    tui.send(CTRL_Q);
    tui.wait_for("Unsaved buffers", Duration::from_secs(10));
    tui.type_str("d");
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
}

#[test]
fn schema_viewer_shows_columns_in_tree() {
    let fx = Fixture::sqlite("e2e-schema");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-schema", Duration::from_secs(10));

    tui.send(ENTER);
    tui.wait_for("users", Duration::from_secs(15));

    tui.type_str("j");
    tui.type_str("j");
    tui.type_str("l");
    tui.wait_for("INTEGER", Duration::from_secs(10));
    tui.wait_for("id", Duration::from_secs(10));
    tui.wait_for("TEXT", Duration::from_secs(10));

    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
}

#[test]
fn connection_error_is_dismissable_with_esc() {
    // A SQLite connection whose database file cannot be opened fails at once
    // and surfaces an error in the status line. Esc must clear it.
    let fx = Fixture::sqlite_broken("e2e-down");
    let mut tui = fx.spawn();
    tui.wait_for("e2e-down", Duration::from_secs(10));

    // Activate the connection: it cannot connect, so an error surfaces.
    tui.send(ENTER);
    tui.wait_for("ERR:", Duration::from_secs(15));
    tui.wait_for("failed to connect", Duration::from_secs(5));

    // Esc dismisses the status-line error; it must disappear from the screen.
    tui.esc();
    tui.wait_for_absent("ERR:", Duration::from_secs(5));
    tui.wait_for_absent("failed to connect", Duration::from_secs(5));

    tui.send(CTRL_Q);
    assert!(
        tui.wait_exit(Duration::from_secs(10)),
        "sextant should exit"
    );
}
