# ARCHITECTURE.md

A code map and orientation guide for `sextant`. Read this alongside `AGENTS.md`
(project facts, workflow) and `SPEC.md` (agnostic product spec). The goal is to
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

## Crates & dependency rules

All code lives in `crates/` (the workspace tree is in `AGENTS.md`).

- **`sextant-cli`** — The only crate that produces a binary (`sextant`). Minimal:
  installs `color_eyre`, sets up `tracing`, calls `sextant_ui::run()`.
- **`sextant-core`** — Domain primitives (`Driver`, `Connection`, `CellValue`,
  `QueryResult`) and the `QueryExecutor` trait. Lightweight, few deps.
- **`sextant-config`** — Loads `connections.toml` / `config.toml` / `keys.toml`
  from XDG paths (`~/.config/sextant/`); validates per-driver required fields.
- **`sextant-db`** — Implements `QueryExecutor` via `sqlx`; per-connection pools.
  All DB I/O is async (`tokio`).
- **`sextant-state`** — Owns the app's private `state.db` (query history,
  recent-files ring). Async (`sqlx`/SQLite), independent of user connections.
- **`sextant-ui`** — TUI event loop, state machine (`Normal`/`Insert` modes +
  `editor_open` overlay; see Invariants), and all `ratatui` widgets.

Dependency edges: `cli → ui → core`; `db`, `config`, `state` each `→ core`.
The service crates (`db`, `config`, `state`) must compile and be testable
**without** depending on `sextant-ui`.

---

## Where things live

| Concern | File |
|---------|------|
| Event loop, `App` state, render, key handling, message handling | `crates/sextant-ui/src/lib.rs` |
| Sidebar tree (connections → schemas → tables), connection state | `crates/sextant-ui/src/tree_pane.rs` |
| Result grid (`hjkl`/`gg`/`G` nav + inline editing, optimistic commit) | `crates/sextant-ui/src/result_grid.rs` |
| SQL editor modal (`tui-textarea`, Normal/Insert, multi-buffer tabs) | `crates/sextant-ui/src/editor_modal.rs` |
| Editor autocomplete (table/column candidates from the schema cache) | `crates/sextant-ui/src/autocomplete.rs` |
| Fuzzy pickers (command palette, find table, open file, snippets) | `crates/sextant-ui/src/fuzzy.rs` |
| Keymap: default bindings, user remap, chord resolver | `crates/sextant-ui/src/keymap.rs` |
| Theme → `ratatui` palette resolution | `crates/sextant-ui/src/palette.rs` |
| Swap files (crash recovery) | `crates/sextant-ui/src/swap.rs` |
| SQL execution, transaction state + row→`CellValue` mapping per backend | `crates/sextant-db/src/executor.rs` |
| SQL generation: quoting, `CREATE TABLE` skeleton, DML by PK, destructive-op detection | `crates/sextant-db/src/sql.rs` |
| Export serialization (CSV/TSV/JSON/SQL) · import parsing + column mapping | `crates/sextant-db/src/{export,import}.rs` |
| Connection pools per active connection | `crates/sextant-db/src/connection_manager.rs` |
| Schema/table introspection | `crates/sextant-db/src/introspection.rs` |
| Connection-URL construction per driver | `crates/sextant-db/src/url_builder.rs` |
| Config load / validation / XDG paths / password lookup / themes / keymaps | `crates/sextant-config/src/{lib,parser,validation,paths}.rs` |
| Local `state.db` (query history, recent-files ring, snippets) | `crates/sextant-state/src/lib.rs` |
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
  `BOOLEAN` (`SqliteTypeInfo(Bool)`), which made `fetch_all` fail (git: commit
  `4a2636e`). When adding a backend you
  add a `DbPool` variant and a `match` arm — there is no generic path.

- **Row mapping uses an ordered fallback.** In `executor.rs`, each cell is
  decoded `bool` (only when `type_info` says boolean) → `i64` → `f64` →
  `String` → `Bytes`, producing a `sextant_core::CellValue`.

- **`execute()` distinguishes SELECT from DML/DDL.** `is_select_query()` routes
  SELECTs to a full `QueryResult { columns, rows }`; everything else returns
  `rows_affected`.

- **Transactions are session-held, not per-statement.** By default each
  `execute()` is autocommit and independent. On `BEGIN`/`START TRANSACTION`,
  `SqlxExecutor` pulls a `PoolConnection` from the pool and **retains** it
  (`HeldConn`); subsequent statements run on that held connection — seeing
  uncommitted changes — until `COMMIT`/`ROLLBACK` returns it to the pool. The
  `active: AtomicBool` flag (`in_transaction()`, lock-free) is read at render to
  show `txn: ACTIVE`. Grid commits use their own one-shot transaction
  (`execute_transaction`), independent of any session transaction. See ADR-0003.

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

- **Password resolution.** On connect, `start_connection` (`ui/src/lib.rs`) looks
  up the keyring via the injected `App::credentials: Arc<dyn CredentialStore>`
  (`core`) and the env-var fallback `connection_password(name)`
  (`SEXTANT_<NAME>_PASSWORD`, name uppercased with spaces/`-` → `_`), then the
  pure `resolve_password` (`sextant-config`) decides the cascade: keyring wins
  over env; with neither, a TCP driver declaring a `keyring_key` prompts,
  otherwise it connects passwordless. On a successful connect the prompt-entered
  password is persisted: `confirm_password_prompt` stashes it in
  `App::pending_credential`, and the `AppMsg::Connected` handler calls
  `persist_pending_credential`, which writes it via the `CredentialStore` (a
  failed connect discards it). The production store is `KeyringStore`
  (`sextant-config`, service `"sextant"`); tests inject an in-memory double. This
  seam is what makes the cascade + save testable — see ADR-0005.

- **State machine.** `Mode { Normal, Insert }` (editing text), `Focus { Tree,
  Grid }` (Tab toggles), and the boolean `editor_open` (modal overlay). These
  are independent axes.

---

## How to add X

- **A new `AppMsg`:** add the variant to `enum AppMsg` (`lib.rs`), then handle
  it in `App::handle_msg`. If background work produces it, send it on the `tx`
  cloned into the spawned task.

- **A keybinding:** add the chord → `Action` mapping in `Keymap::defaults`
  (`keymap.rs`) and dispatch the `Action` in `App::dispatch` (mind the current
  `Mode`/`Focus`); multi-key chords resolve through `ChordState`. Document it in
  `SPEC.md` §12 (interaction model / default keymap).

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
