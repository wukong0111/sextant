---
name: connect-tui
description: Launch the sextant TUI wired to a database (Docker PostgreSQL/MySQL or local SQLite test.db). Use when the user wants to run the app against real data, manually try a connection, or see a change working end-to-end in the TUI.
---

# connect-tui

Get the TUI running against a database. The app reads connections from
`~/.config/sextant/connections.toml` and resolves passwords from
`SEXTANT_<NAME>_PASSWORD` env vars (name uppercased, `-`/space → `_`).

## 1. Ensure a connections file exists

Check `~/.config/sextant/connections.toml`. If absent:

- **For the Docker DBs:** `make setup-docker-conns` installs entries
  `docker-pg` and `docker-mysql` from `connections.example.toml`.
- **For local SQLite:** a `[[connection]]` with `driver = "sqlite"` and
  `path = "/abs/path/to/test.db"` is enough. A `test-sqlite` entry pointing at
  this repo's `test.db` may already exist.

## 2. Ensure the target DB is reachable

- SQLite: make sure the file exists and is seeded (`make seed-sqlite`).
- Docker PG/MySQL: `make test-db-up` (and `make seed` for data). Passwords are
  `sextant`, already exported by the Makefile / `.claude/settings.json`.

## 3. Run

```bash
cargo run
```

The sidebar lists the configured connections. Select one, press `Enter` to
connect, `Tab` to move focus between tree and grid, `Space e` to open the SQL
editor, and `Ctrl+Q` to quit.

> This is more specific than the built-in `run` skill because it also handles
> connection config and DB readiness. For a non-interactive smoke test in CI-like
> contexts, prefer asserting the app starts and exits on `Ctrl+Q` (`\x11`).
