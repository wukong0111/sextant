---
name: db-setup
description: Start and seed sextant's test databases. Use when the user wants to bring up the Docker PostgreSQL/MySQL containers, seed test data, or prepare the local SQLite test.db for development or integration tests.
---

# db-setup

Prepare test databases for `sextant`. There are two tiers — pick based on what
the user needs and what's available.

## SQLite only (no Docker required)

```bash
make seed-sqlite   # seeds ./test.db from seeds/sqlite.sql
```

Use this when the user just wants data to click through in the TUI, or Docker
isn't available.

## Full stack (Docker PostgreSQL + MySQL + SQLite)

```bash
make test-db-up    # starts containers, waits until both are healthy
make seed          # seeds PostgreSQL, MySQL, and SQLite
```

`make test-db-up` blocks until `pg_isready` / `mysqladmin ping` report healthy.
Test passwords are `sextant` (already exported by the Makefile and present in
`.claude/settings.json` env). Ports are non-standard: PG `5433`, MySQL `3307`.

When done:

```bash
make test-db-down  # stops containers and removes volumes
```

## Notes

- If `docker compose` isn't available, fall back to `make seed-sqlite` and tell
  the user PG/MySQL were skipped.
- Seeds create `users`, `orders`, `products`, and `type_samples` tables — see
  `seeds/{postgres,mysql,sqlite}.sql`.
- To point the TUI at these databases, use the `connect-tui` skill.
