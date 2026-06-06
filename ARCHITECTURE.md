# ARCHITECTURE.md

A code map and orientation guide for `sextant`. Read this alongside `AGENTS.md`
(project facts, workflow) and `sextant-spec.md` (product spec). The goal is to
save you from re-deriving the non-obvious wiring every session.

Symbols are referenced **by name** so you can `grep` for them — line numbers
drift, names don't.

---

## Data flow

```
sextant-cli/main.rs
   │  color_eyre + tracing setup, then:
   ▼
sextant_ui::run()                      (ui/src/lib.rs)
   │  builds a tokio runtime, calls run_async()
   ▼
run_async()  ── event loop ──┐
   │  tokio::select! { biased; ... }
   │                         │
   ├─ crossterm key events ──┤──► App::handle_key_event(key, &tx)
   ├─ AppMsg from channel ───┤──► App::handle_msg(msg)
   └─ 250ms tick ────────────┘    (animations / spinners)
                                  │
            background work (connect / query) is tokio::spawn'd
            and reports back by sending an AppMsg on `tx`
                                  │
                                  ▼
                    sextant_db::SqlxExecutor::execute()  (async, sqlx)
```

The UI thread never blocks on I/O. Connecting and querying happen in spawned
tasks; their results return as `AppMsg` variants over an
`mpsc::UnboundedSender<AppMsg>`.

---

## Where things live

| Concern | File |
|---------|------|
| Event loop, `App` state, render, key handling, message handling | `crates/sextant-ui/src/lib.rs` |
| Sidebar tree (connections → schemas → tables), connection state | `crates/sextant-ui/src/tree_pane.rs` |
| Result grid (read-only, `hjkl`/`gg`/`G` nav) | `crates/sextant-ui/src/result_grid.rs` |
| SQL editor modal (`tui-textarea`, Normal/Insert) | `crates/sextant-ui/src/editor_modal.rs` |
| SQL execution + row→`CellValue` mapping per backend | `crates/sextant-db/src/executor.rs` |
| Connection pools per active connection | `crates/sextant-db/src/connection_manager.rs` |
| Schema/table introspection | `crates/sextant-db/src/introspection.rs` |
| Connection-URL construction per driver | `crates/sextant-db/src/url_builder.rs` |
| Config load / validation / XDG paths / password lookup | `crates/sextant-config/src/{lib,parser,validation,paths}.rs` |
| Local `state.db` (query history, recent-files ring) | `crates/sextant-state/src/lib.rs` |
| Domain types & `QueryExecutor` trait | `crates/sextant-core/src/lib.rs` |

Public DB surface is re-exported from `sextant-db/src/lib.rs`:
`DbPool`, `SqlxExecutor`, `ConnectionManager`, `build_connection_url`,
`introspection::Schema`.

---

## Invariants & gotchas

These are the things that bite you if you don't know them.

- **`DbPool` is typed-per-backend, not `sqlx::Any`.** `DbPool` is an enum:
  `Postgres(PgPool)` / `MySql(MySqlPool)` / `Sqlite(SqlitePool)`
  (`executor.rs`). `sqlx::Any` was abandoned because it can't decode SQLite's
  `BOOLEAN` (`SqliteTypeInfo(Bool)`), which made `fetch_all` fail. See
  `plan.md` (Fase 1.2 notes, "fix SQLite BOOLEAN"). When adding a backend you
  add a `DbPool` variant and a `match` arm — there is no generic path.

- **Row mapping uses an ordered fallback.** In `executor.rs`, each cell is
  decoded `bool` (only when `type_info` says boolean) → `i64` → `f64` →
  `String` → `Bytes`, producing a `sextant_core::CellValue`.

- **`execute()` distinguishes SELECT from DML/DDL.** `is_select_query()` routes
  SELECTs to a full `QueryResult { columns, rows }`; everything else returns
  `rows_affected`. There is **no transaction state** — every `execute()` is
  independent. (Relevant for Fase 2 grid editing.)

- **Rendering is message-driven.** The terminal only redraws when
  `App::needs_redraw` is true. It is set on key events, on every `AppMsg`, and
  on the 250ms tick (`run_async` in `lib.rs`), then cleared after the draw.
  If a state change doesn't show up, you forgot to set `needs_redraw`.

- **Executors are cached by connection name.** `App::executors:
  HashMap<String, SqlxExecutor>`. A successful async connect delivers
  `AppMsg::Connected { name, executor, schemas }`; `handle_msg` inserts the
  executor and populates the schema tree. `run_editor_sql` looks the executor
  up by the active connection name and `clone()`s it (pools are `Arc` inside).

- **Editor buffers persist per connection.** `App::saved_buffers:
  HashMap<String, String>` keeps each connection's SQL text across editor
  open/close, keyed by connection name.

- **Introspection is async and front-loaded.** It runs on connect
  (`introspection::introspect_schemas_and_tables`) before the tree is
  populated; on large schemas this latency is user-visible.

- **Password resolution (v0.1).** `connection_password(name)` reads env var
  `SEXTANT_<NAME>_PASSWORD`, where `<NAME>` is the connection name uppercased
  with spaces and `-` replaced by `_` (`sextant-config/src/lib.rs`). Keyring is
  planned (Fase 3) but **not implemented** — `keyring_key` is parsed but unused.

- **State machine.** `Mode { Normal, Insert }` (editing text), `Focus { Tree,
  Grid }` (Tab toggles), and the boolean `editor_open` (modal overlay). These
  are independent axes.

---

## How to add X

- **A new `AppMsg`:** add the variant to `enum AppMsg` (`lib.rs`), then handle
  it in `App::handle_msg`. If background work produces it, send it on the `tx`
  cloned into the spawned task.

- **A keybinding:** add a branch in `App::handle_key_event` (mind the current
  `Mode`/`Focus` and `pending_leader`/`pending_g` chord state). Document it in
  `sextant-spec.md` §9.

- **A driver capability:** enable the `sqlx` feature in the workspace
  `Cargo.toml`, extend `build_connection_url` (`url_builder.rs`), add the
  `DbPool` variant + `execute` match arms + a row-mapper (`executor.rs`), and
  add the introspection query (`introspection.rs`).

- **A widget:** add a `mod` + `use` in `lib.rs`, render it inside `App`'s draw
  path, and route input through `handle_key_event` honoring `Focus`.

---

## Testing entry points

- UI: `ratatui::backend::TestBackend` (no TTY). Examples live at the bottom of
  `sextant-ui/src/lib.rs`.
- DB: SQLite in-memory needs no external service. PG/MySQL integration tests
  are gated on `SEXTANT_TEST_PG_URL` / `SEXTANT_TEST_MYSQL_URL` and skip when
  unset. See `AGENTS.md` → Testing, and the `make test-db*` / `make seed*`
  targets.
