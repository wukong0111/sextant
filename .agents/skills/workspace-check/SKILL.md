---
name: workspace-check
description: Run sextant's full verification checklist (compile, test, format check, lint) across the Cargo workspace. Use before declaring a task done, before committing, or when the user asks whether the code is ready / passing.
---

# workspace-check

Run the project's "before declaring a task done" checklist, in order. Stop and
report at the first failure — don't paper over it.

```bash
cargo check --workspace
cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets
```

## Interpreting results

- **`cargo check`**: must compile with **no warnings** (project rule).
- **`cargo test`**: PostgreSQL/MySQL integration tests are gated on
  `SEXTANT_TEST_PG_URL` / `SEXTANT_TEST_MYSQL_URL` and **skip** when unset — a
  pass without Docker only covers SQLite + UI. If the user needs full coverage,
  run the `db-setup` skill (or `make test-db`) first.
- **`cargo fmt --all --check`**: reports formatting drift without rewriting. To
  fix, run `cargo fmt --all`.
- **`cargo clippy`**: clippy ships with the toolchain; no config needed.

Report a concise pass/fail summary per step, with the failing output when a
step fails.
