# `sextant` — Terminal Database Manager Specification

> Named after the nautical instrument used for navigation by celestial observation — a tool for finding your position. Same idea here, but for data.

## 1. Overview

A keyboard-driven, terminal-based database client for **PostgreSQL**, **MySQL**, and **SQLite**. Targets developers who live in the terminal and want a TablePlus/DataGrip equivalent without leaving the shell. Written in Rust, rendered with `ratatui`, modal editing inspired by **Helix** (selection-first).

## 2. Goals

- Manage multiple database connections (PG, MySQL, SQLite) from a single TUI.
- Execute SQL with syntax highlighting and context-aware autocomplete.
- Browse tables and edit rows inline with immediate feedback.
- Inspect schemas (tables, columns, indexes, constraints, FKs) and emit DDL.
- Export/import data in CSV, JSON, and SQL dump formats.
- Stay responsive: async queries, cancellable, non-blocking UI.

## 3. Non-Goals (v1)

- NoSQL (Mongo, Redis), MSSQL, Oracle.
- ORM-like visual query builders or graphical join designers.
- Mouse-first UX (mouse may work in places, but is never required).
- Remote collaboration / multi-user sessions.
- ER diagrams (deferred).
- Plugin or scripting system (deferred).
- Multi-cursor editing (single cursor by design — not a deferral).

## 4. Target User

Backend developers and power users comfortable with modal editors (Helix, Kakoune, vim), running terminal-first workflows, who want fast introspection and querying without a heavy GUI.

## 5. Core Features (v1)

### 5.1 Connection Management

- Saved connections defined in a TOML file.
- Per-connection fields: host, port, user, database, SSL mode (PG/MySQL) or path (SQLite).
- Credentials stored via OS keyring (`keyring` crate), **never plaintext** in config.
- Connection pooling per active database (`sqlx::Pool`).
- Multiple simultaneous connections; switch with `<leader>w`.

### 5.2 SQL Editor

- Multi-line modal editor (Helix bindings).
- Syntax highlighting via `tree-sitter` + `tree-sitter-sql`.
- Context-aware autocomplete:
  - Tables and views in the active schema(s).
  - Columns scoped to tables present in `FROM` / `JOIN`.
  - SQL keywords and dialect-specific functions.
- Multi-statement execution (selected range, or whole buffer if no selection).
- Cancellable execution (`<C-c>` interrupts long-running queries).
- One editor buffer per tab; multiple tabs per connection.

### 5.3 Table CRUD

- Browse rows in a paginated grid (lazy load on scroll).
- **Editability requires a primary key** (single or composite). Tables without a PK, views, and ad-hoc query results display as read-only with a 🔒 indicator in the status line.
  - Rationale: without a PK, the only way to identify a row in `WHERE` is by all columns — which can silently affect multiple duplicates or fail on `NULL` (`NULL != NULL` in SQL). We refuse rather than risk it. Opt-in escape hatch deferred to post-v1 if real-world use demands it.
- Inline cell editing: `Enter` on a cell → Insert mode → `<Esc>` commits to a pending change set.
- Insert new row via the empty bottom row.
- Soft-delete: marked rows show pending state; explicit commit applies `DELETE`.
- Optimistic concurrency: row-version check before commit; conflicts surfaced.
- All changes batched and applied in a single transaction on commit (see §5.6).

### 5.4 Schema Viewer + DDL

- Tree pane hierarchy: `Connection → Database → Schema → {Tables, Views, Functions, Sequences} → {Columns, Indexes, Constraints}`.
- Detail panel shows column types, nullability, defaults, FKs, indexes, constraints.
- DDL generation from the tree:
  - `CREATE TABLE` / `CREATE INDEX` skeletons emitted into the editor.
  - Drop / rename / alter via guided forms that produce DDL **into the editor** — never auto-executed.
- `\d` / `DESCRIBE`-equivalent always one keypress away.

### 5.5 Export / Import

- Export current result set or full table to:
  - **CSV** (RFC 4180, configurable delimiter).
  - **JSON** (array of objects, optional NDJSON).
  - **SQL dump** (`INSERT` statements; optionally schema-only / data-only).
- Import from CSV / JSON / SQL dump with a column-mapping preview.
- Export/import runs asynchronously with a progress indicator and cancel.

### 5.6 Transaction Model

Hybrid, psql-style. Two regimes, always visible in the status line:

- `txn: auto` (gray) — default. Each executed statement auto-commits.
- `txn: ACTIVE` (amber) — manual mode. Entered when the user runs `BEGIN` or `START TRANSACTION`. Subsequent statements stay uncommitted until `COMMIT` or `ROLLBACK`.

The status line is authoritative — the user is never surprised about which regime is active.

Grid edits (§5.3) are always batched and applied in a single transaction on `<C-s>`, independent of this regime.

Destructive operations (`DELETE` / `UPDATE` without `WHERE`, any DDL) trigger a confirmation modal regardless of regime (see §12).

### 5.7 Buffer Management & Saved Queries

- Editor buffers are **volatile in memory** — like vim, helix, sublime, every real editor.
- `<C-s>` saves the active buffer to a `.sql` file. First save prompts for a filename; subsequent saves overwrite silently.
- `:w <path>` saves to an explicit path.
- Multiple buffers (tabs) inside the modal — cycle with `<Tab>`. Dirty buffers marked with `●` in the tab label.
- Quitting with dirty buffers prompts: save / discard / cancel.
- **Crash recovery** via swap files (vim-style). While a buffer is dirty, sextant writes a `.swp` companion every 30 seconds (configurable). On startup, orphan `.swp` files trigger a recovery prompt. Clean quit + save removes them. This is the **only** mechanism that writes buffer content to disk without explicit user action.
- File navigation:
  - `<Space>o` — open file (fuzzy finder over the queries directory).
  - `<Space>r` — recent files for the active connection.
- A saved query is just a `.sql` file. sextant imposes no metadata, no special header. Power users organize directories however they want; the files are editable with any external tool, version-controllable, shareable.

## 6. Tech Stack

| Concern | Choice | Version (May 2026) |
| --- | --- | --- |
| Language | Rust (edition 2024) | MSRV 1.85+ |
| TUI | `ratatui` | 0.30 |
| Terminal backend | `crossterm` | 0.29 |
| Async runtime | `tokio` (multi-thread) | latest stable |
| DB drivers | `sqlx` with `postgres`, `mysql`, `sqlite` features | 0.8 |
| Syntax highlight | `tree-sitter` + `tree-sitter-sql` | latest |
| SQL parsing (autocomplete) | `sqlparser-rs` | latest |
| Config | `serde` + `toml` | latest |
| Credentials | `keyring` | latest |
| CSV | `csv` | latest |
| JSON | `serde_json` | latest |
| Local app state | SQLite via `sqlx-sqlite` | 0.8 |
| Logging | `tracing` + `tracing-subscriber` | latest |
| Errors | `thiserror` (libs) + `color-eyre` (bin) | latest |

## 7. Architecture

```
┌────────────────────────────────────────────┐
│           UI Layer (ratatui)               │
│  Components: Tree, Editor, ResultGrid,     │
│  StatusLine, CommandPalette, Overlays      │
└──────────────┬─────────────────────────────┘
               │ Messages (mpsc)
┌──────────────▼─────────────────────────────┐
│         App State / Event Loop             │
│  - Modal state machine (Normal/Insert/Sel) │
│  - Command dispatcher                      │
│  - Pending-operation tracker               │
└──────────────┬─────────────────────────────┘
               │ Commands
┌──────────────▼─────────────────────────────┐
│         Service Layer (async)              │
│  - QueryExecutor (per-connection)          │
│  - SchemaIntrospector                      │
│  - ExportService / ImportService           │
│  - AutocompleteEngine                      │
└──────────────┬─────────────────────────────┘
               │
┌──────────────▼─────────────────────────────┐
│        Driver Layer (sqlx pools)           │
│   postgres / mysql / sqlite                │
└────────────────────────────────────────────┘
```

- **TEA-inspired loop**: render → poll events → produce messages → update state → render.
- Long-running operations dispatched via `tokio::spawn`; results returned through `mpsc::UnboundedSender`.
- Render coalesces redraws; target ~60 FPS, drop frames if behind.
- Each service is independent and unit-testable without a TUI.

## 8. UI Layout

The persistent layout is intentionally minimal — a **narrow left sidebar for connections** and the **rest of the viewport for the results grid**. The SQL editor is a **floating modal overlay**, summoned on demand.

### Default View (editor closed)

```
┌─ Connections ───────┬─ Results ──────────────────────────┐
│ ▾ local-pg          │  id │ email          │ created_at  │
│   ▾ public          │ 1   │ a@b.c          │ 2026-05-19  │
│     • users         │ 2   │ d@e.f          │ 2026-05-20  │
│     • orders        │ 3   │ g@h.i          │ 2026-05-20  │
│ ▸ prod-mysql        │ 4   │ j@k.l          │ 2026-05-21  │
│ ▸ scratch.db        │ ...                                │
│                     │                                    │
│                     │                                    │
├─────────────────────┴────────────────────────────────────┤
│ NOR │ local-pg/public │ 142 rows / 38ms │ <space>e       │
└──────────────────────────────────────────────────────────┘
```

### Editor Modal Open (centered over the full viewport)

```
┌─ Connections ───────┬─ Results ──────────────────────────┐
│ ▾ local-pg          │  id │ email   ┌─ SQL Editor ─────┐ │
│   ▾ public          │ 1   │ a@b.c   │ 1 SELECT id, em… │ │
│     • users         │ 2   │ d@e.f   │ 2 FROM users     │ │
│     • orders        │ 3   │ g@h.i   │ 3 WHERE …        │ │
│ ▸ prod-mysql        │ 4   │ j@k.l   │ 4   - INTERVAL…  │ │
│ ▸ scratch.db        │ ...           │                  │ │
│                     │               │ <C-Enter> run    │ │
│                     │               └──────────────────┘ │
├─────────────────────┴────────────────────────────────────┤
│ EDT │ local-pg/public │ 142 rows / 38ms                  │
└──────────────────────────────────────────────────────────┘
```

### Components

- **Connections sidebar** (left, default 20% width, configurable): tree of connections → databases → schemas → objects (tables, views, functions, sequences). Width persisted per session.
- **Results pane** (right, remaining space): rows from the last executed statement. Always full height of the main area. This is where you spend most of your visual attention.
- **SQL editor modal**: floating, centered on the full viewport, configurable size (default 80% width × 60% height). Implemented as a `Clear` widget plus the editor widget rendered over the base layout.
- **Status line** (bottom, full width): mode (`NOR` / `INS` / `SEL` / `EDT`), current connection, query stats, contextual hint.
- **Other floating overlays**: command palette (`<Space>:`), connection picker (`<Space>w`), help (`<Space>?`), autocomplete popup (renders above the editor modal).

### Modal Behavior

- Opens with `<Space>e` (or any binding that requires an editor: `<Space>:` for SQL palette, `D` in tree for DDL).
- **Persists across executions**: pressing `<C-Enter>` runs the query but **does not close the modal**. Results stream into the results pane behind it.
- Dismiss with `<Esc>` (from Normal mode) — buffer content, cursor position, selection, undo history all preserved.
- Reopen restores the exact previous state. Multiple buffers supported (cycle with `<Tab>` inside the modal).
- Optional config flag `editor.auto_dismiss_on_run = true` for users who prefer the run-and-close workflow.
- When dismissed, the editor's state is part of the per-connection session (see §11).

## 9. Keybindings (Helix-Inspired)

### Modes

- **Normal** — navigation, selection, commands.
- **Insert** — text input in editor / cells.
- **Select** — extended / range selection (and multi-selection where applicable).

### Selection-First Philosophy

Like Helix: **select first, then act**. `w` selects the next word; `d` deletes the current selection. No verb-then-motion composition.

### Global

| Key | Action |
| --- | --- |
| `<Space>` | Leader prefix |
| `<Space>e` | Toggle SQL editor modal |
| `<Space>o` | Open `.sql` file (fuzzy finder over queries dir) |
| `<Space>r` | Recent files for active connection |
| `<Space>h` | Query history picker |
| `<Space>x` | Export current result set (CSV / JSON / SQL) |
| `<Space>f` | Fuzzy-find tables / columns |
| `<Space>w` | Switch connection |
| `<Space>:` | Command palette |
| `<Space>?` | Help / cheatsheet |
| `<C-q>` | Quit (prompts if dirty buffers exist) |
| `<Tab>` / `<S-Tab>` | Cycle panes (tree ↔ results when modal closed) |

### Editor (SQL)

| Key | Action |
| --- | --- |
| `i` / `a` | Insert before / after selection |
| `w` / `b` / `e` | Select next / prev word / end of word |
| `x` | Select current line |
| `d` / `c` / `y` | Delete / change / yank selection |
| `p` | Paste after selection |
| `u` / `U` | Undo / redo |
| `<C-c>` | Cancel running query |
| `<C-Enter>` | Execute selection (or full buffer) — modal stays open |
| `<C-s>` | Save current buffer (prompts for filename on first save) |
| `<C-Space>` | Trigger autocomplete |
| `<Esc>` (from Normal) | Dismiss editor modal (state preserved) |
| `<Tab>` / `<S-Tab>` | Cycle buffers within the modal |
| `<C-t>` | Open a new (empty) buffer |

### Result Grid

| Key | Action |
| --- | --- |
| `h j k l` | Move cell |
| `gg` / `G` | Top / bottom |
| `f` | Filter column inline |
| `s` | Sort by column |
| `<Enter>` | Edit cell (Insert mode) |
| `o` | Insert new row below |
| `dd` | Mark current row for deletion |
| `<C-s>` | Commit pending changes |
| `<C-z>` | Discard pending changes |

### Tree Pane

| Key | Action |
| --- | --- |
| `h` / `l` | Collapse / expand node |
| `<Enter>` | Open object (browse rows or show schema) |
| `D` | Emit DDL skeleton into editor |

All bindings remappable via `keys.toml`.

## 10. Configuration

XDG-compliant paths (Linux/macOS):

- Config:        `~/.config/sextant/config.toml`
- Keymap:        `~/.config/sextant/keys.toml`
- Connections:   `~/.config/sextant/connections.toml`
- Themes:        `~/.config/sextant/themes/*.toml`
- Saved queries: `~/.local/share/sextant/queries/*.sql`
- App state:     `~/.local/share/sextant/state.db`
- Swap files:    `~/.local/state/sextant/swap/*.swp`
- Logs:          `~/.local/state/sextant/sextant.log`

Example `config.toml`:

```toml
[ui]
theme                = "dark"     # "dark" | "light" | name of custom theme file
themes_dir           = "~/.config/sextant/themes"
row_limit_default    = 1000
result_pagination    = 500
sidebar_width_pct    = 20

[editor]
tab_width                  = 2
autocomplete_trigger_chars = [".", " "]
modal_width_pct            = 80
modal_height_pct           = 60
auto_dismiss_on_run        = false
queries_dir                = "~/.local/share/sextant/queries"
swap_interval_seconds      = 30

[behavior]
confirm_destructive_ddl = true
confirm_row_delete      = true
auto_commit_edits       = false
```

### Themes

- Two themes ship built-in: `"dark"` (default) and `"light"`.
- Custom themes loaded from `themes_dir` as `.toml` files.
- No hot-reload in v1.

Example `connections.toml` (credentials referenced by keyring key, not stored here):

```toml
[[connection]]
name        = "local-pg"
driver      = "postgres"
host        = "127.0.0.1"
port        = 5432
user        = "dan"
database    = "scratch"
ssl_mode    = "prefer"
keyring_key = "sextant:local-pg"

[[connection]]
name   = "scratch"
driver = "sqlite"
path   = "~/db/scratch.sqlite"
```

## 11. Persistence Layout

Three distinct stores, by purpose:

### Saved Queries — Filesystem

- Plain `.sql` files under `~/.local/share/sextant/queries/` (configurable via `editor.queries_dir`).
- Written only on explicit save (`<C-s>` or `:w <path>`).
- Editable with any external tool (vim, code, etc.), version-controllable, shareable.
- sextant imposes no directory structure — user organizes freely.
- Permissions: directory `0700`, files `0600`.

### Buffer Recovery — Swap Files

- While a buffer is dirty, sextant writes a `.swp` companion every `swap_interval_seconds` (default 30).
- Format: the in-progress `.sql` content + a sidecar JSON with cursor / selection state.
- For unnamed buffers, swap files live in `~/.local/state/sextant/swap/`.
- On startup, sextant scans for orphan `.swp` files (no clean-exit marker) and prompts:
  > "Found unsaved changes from previous session for `<name>`. [R]ecover / [D]iscard / [I]gnore."
- Removed on clean quit + save.
- Permissions: `0600`.
- This is the **only** mechanism that writes buffer content to disk without explicit user action.

### App State — `state.db`

SQLite database at `~/.local/share/sextant/state.db`. Stores:

- Query execution history (timestamp, connection, statement, duration, error).
- Recent files list per connection (paths only, bounded ring of 20).
- Last layout / pane sizes per connection.
- Active connection at last quit (for restore on next start).

Permissions: `0600`. **Never** stores buffer content — that lives in `.sql` (saved) or `.swp` (recovery) files only.

## 12. Security

- Passwords never written to config files; OS keyring only.
- Connection strings redacted in logs (`postgres://user:***@host`).
- `DELETE` / `UPDATE` without `WHERE`, and any DDL, trigger a confirmation modal by default.
- TLS: `prefer` by default for PG/MySQL; `require` enforced if configured.
- **File permissions enforced on creation:**
  - `state.db`: `0600`
  - `.swp` files: `0600`
  - Queries directory: `0700`; `.sql` files inside: `0600`
  - Config files: respect existing permissions (don't widen)
- Query text on disk (saved queries, swap files, history in state.db) is **not encrypted**. Local-machine threat model only — if your machine is compromised, this content is exposed. Don't put unredacted production data into committed `.sql` files.

## 13. Project Structure (Cargo Workspace)

```
sextant/
├── Cargo.toml                 # workspace
├── crates/
│   ├── sextant-core/              # domain types, traits, command bus
│   ├── sextant-db/                # sqlx drivers, introspection
│   ├── sextant-sql/               # parsing, autocomplete, highlight
│   ├── sextant-ui/                # ratatui components, layout
│   ├── sextant-config/            # config + keymap loading
│   ├── sextant-state/             # local SQLite state store
│   └── sextant-cli/               # binary entry point
└── docs/
```

Service crates (`sextant-db`, `sextant-sql`, `sextant-state`) must compile and be tested without `sextant-ui`. Enforces architectural separation.

## 14. Roadmap

### v0.1 — MVP
- PG + SQLite drivers.
- Connection list, basic query editor (no autocomplete), read-only result grid.
- Normal / Insert modes.

### v0.2
- MySQL driver.
- Autocomplete + syntax highlighting.
- Inline row editing with commit/discard.
- Schema viewer + DDL generation.

### v0.3 — v1
- Export/import (CSV, JSON, SQL).
- Query history + snippets.
- Polished keymap, themes, help overlay.

### Post-v1 (out of scope here)
- ER diagrams, `EXPLAIN` plan visualizer.
- Plugin system (Lua or WASM).
- SSH tunneling.
- Additional drivers (MSSQL, Oracle, ClickHouse).
