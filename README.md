# sextant

A keyboard-driven, terminal-based database client for **PostgreSQL**, **MySQL**, and **SQLite**.

sextant is built for developers who want a TablePlus/DataGrip-style workflow without leaving the shell. It uses a **modal** interaction model inspired by Helix/vi — navigate the schema tree, edit SQL in a floating editor, and inspect results in a grid, all with the keyboard.

> **Status:** v0.1.0 — early, functional, and under active development.

---

## Features

- **Multiple drivers** — PostgreSQL, MySQL, and SQLite in one client.
- **Modal editing** — Normal/Insert modes, Helix-style selection and navigation.
- **Schema tree** — browse connections → schemas → tables → columns.
- **SQL editor** — floating editor with tabs/buffers, autocomplete, and history.
- **Result grid** — navigate with `hjkl`, edit cells inline, add/delete rows, and commit changes.
- **Transaction awareness** — session transactions are visible in the status line.
- **Secure credential handling** — passwords are never stored in config; resolved from the OS keyring, env vars, or an interactive prompt.
- **Destructive-op guardrails** — `DELETE`/`UPDATE` without `WHERE` and DDL trigger a confirmation modal by default.
- **Local state** — query history, recent files, and snippets are tracked in a private local database.

---

## Requirements

- [Rust](https://rustup.rs/) **1.85+**
- A terminal with truecolor support (recommended)

---

## Installation

```bash
git clone https://github.com/wukong0111/sextant.git
cd sextant
cargo build --release
```

The binary is available at `target/release/sextant`. To install it into `~/.cargo/bin`:

```bash
cargo install --path crates/sextant-cli
```

---

## Quick start

Create the config directory and copy the example connection file:

```bash
mkdir -p ~/.config/sextant
cp connections.example.toml ~/.config/sextant/connections.toml
```

Edit `~/.config/sextant/connections.toml` to match your databases. For example:

```toml
[[connection]]
name = "local-pg"
driver = "postgres"
host = "127.0.0.1"
port = 5432
user = "postgres"
database = "myapp"
keyring_key = "local-pg"   # read password from OS keyring

[[connection]]
name = "local-sqlite"
driver = "sqlite"
path = "test.db"
```

Then launch sextant:

```bash
sextant
```

Use `Enter` to activate a connection, `Space e` to open the SQL editor, and `Ctrl+E` (or `Ctrl+Enter`) to run the current buffer.

### Password resolution

Passwords are resolved in this order:

1. **OS keyring** — if the connection declares `keyring_key`.
2. **Environment variable** — `SEXTANT_<NAME>_PASSWORD`, where `<NAME>` is the connection name uppercased with spaces/`-` replaced by `_`.
3. **Interactive prompt** — when a `keyring_key` is declared but no secret is found.

Passwords are **never written to `connections.toml`**.

---

## Default key bindings

Normal-mode bindings (customizable via `~/.config/sextant/keys.toml`):

| Keys | Action |
|------|--------|
| `Enter` | Activate selected connection / browse table / edit cell |
| `Tab` | Switch focus between tree and grid |
| `Space e` | Open/close SQL editor |
| `Space h` | Query history |
| `Space r` | Recent files |
| `Space f` | Find table |
| `Space :` | Command palette |
| `Space ?` | Help overlay |
| `j` / `k` | Move down / up |
| `h` / `l` | Collapse/expand tree or move left/right |
| `gg` / `G` | Go to top / bottom |
| `o` / `dd` | Add / delete row (grid) |
| `Ctrl+s` | Commit grid edits |
| `Ctrl+z` | Discard grid edits |
| `Ctrl+q` | Quit |

Editor-specific shortcuts (run, save, buffer cycle, etc.) are shown in the editor help or defined in `crates/sextant-ui/src/editor_modal.rs`.

---

## Development

Run the full workspace verification:

```bash
make check
```

Run the PTY end-to-end tests (SQLite only, no Docker):

```bash
make e2e
```

Run integration tests against Docker PostgreSQL/MySQL:

```bash
make test-db
```

Seed the local SQLite test database:

```bash
make seed-sqlite
```

For a full list of targets:

```bash
make help
```

---

## Project documentation

- [`SPEC.md`](SPEC.md) — product specification and acceptance criteria.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — code map, data flow, and crate rules.
- [`docs/adr/`](docs/adr/) — architecture decision records.
- [`docs/coverage.md`](docs/coverage.md) — test coverage mapping.
- [`AGENTS.md`](AGENTS.md) — contributor workflow and conventions.

---

## License

MIT © wukong0111
