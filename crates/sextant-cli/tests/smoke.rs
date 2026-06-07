//! On-demand smoke "screenshots" of the live TUI.
//!
//! These are `#[ignore]`d so they never run in the normal suite — they are a
//! manual tool for eyeballing the real rendered app without a TTY. Run with:
//!
//! ```bash
//! cargo test -p sextant-cli --test smoke -- --ignored --nocapture
//! # or: make smoke
//! ```
//!
//! Each test drives the binary to an interesting state and dumps the parsed
//! screen to stderr, then quits.

mod common;

use std::time::Duration;

use common::{CTRL_Q, ENTER, Fixture};

/// Boot → connect → open editor: a quick tour of the main surfaces.
#[test]
#[ignore = "manual smoke screenshot; run with --ignored --nocapture"]
fn screenshot_tour() {
    let fx = Fixture::sqlite("smoke-db");
    let mut tui = fx.spawn();

    tui.wait_for("smoke-db", Duration::from_secs(10));
    tui.dump("BOOT (sidebar + status line)");

    tui.send(ENTER); // connect
    tui.wait_for("users", Duration::from_secs(15));
    tui.dump("CONNECTED (schema tree introspected)");

    tui.leader("e"); // open editor
    tui.wait_for("insert", Duration::from_secs(10));
    tui.dump("EDITOR OPEN");

    tui.esc(); // close editor
    tui.send(CTRL_Q);
    if !tui.wait_exit(Duration::from_secs(5)) {
        // A dirty/blank buffer may prompt; discard and quit.
        tui.type_str("d");
        let _ = tui.wait_exit(Duration::from_secs(5));
    }
}

/// Browse a table so the focus lands on an editable grid: the status line then
/// shows the contextual edit hints. The help hint must remain visible here too.
#[test]
#[ignore = "manual smoke screenshot; run with --ignored --nocapture"]
fn screenshot_grid_focus_status_line() {
    let fx = Fixture::sqlite("smoke-grid");
    let mut tui = fx.spawn();

    tui.wait_for("smoke-grid", Duration::from_secs(10));
    tui.send(ENTER); // connect
    tui.wait_for("users", Duration::from_secs(15));

    // Navigate to the `users` table and browse it: focus moves to the grid.
    tui.type_str("j");
    tui.type_str("j");
    tui.send(ENTER);
    tui.wait_for("2 rows", Duration::from_secs(10));
    tui.dump("GRID FOCUSED (edit hints + help hint must both show)");

    tui.send(CTRL_Q);
    let _ = tui.wait_exit(Duration::from_secs(5));
}
