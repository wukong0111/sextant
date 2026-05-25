# AGENTS.md

Agent-facing reference for the `sextant` project. Read this first before making changes.

---

## Project Overview

`sextant` is a keyboard-driven, terminal-based database client for **PostgreSQL**, **MySQL**, and **SQLite**. It targets developers who want a TablePlus/DataGrip equivalent without leaving the shell. Written in Rust, rendered with `ratatui`, with modal editing inspired by **Helix** (selection-first).

- **Language**: Rust (edition 2024, MSRV 1.85+)
- **License**: MIT
- **Author**: wukong0111

### Key Documents

| File | Purpose |
|------|---------|
| `sextant-spec.md` | Full product specification (features, UI layout, keybindings, architecture). |
| `plan.md` | Development roadmap split into phases (Fase 0–3). Check this before starting work. |

---

## Workspace Structure

This is a Cargo workspace. The root `Cargo.toml` defines workspace metadata; all code lives in `crates/`.

```
sextant/
├── Cargo.toml                 # workspace definition
├── plan.md                    # development plan
├── sextant-spec.md            # product specification
└── crates/
    ├── sextant-cli/           # binary entry point (main.rs)
    ├── sextant-core/          # domain types, traits, shared errors
    ├── sextant-config/        # TOML config loading, XDG paths, keymaps
    ├── sextant-db/            # sqlx drivers, query execution, introspection
    └── sextant-ui/            # ratatui components, event loop, layout
```

### Crate Responsibilities

- **`sextant-cli`** — The only crate that produces a binary (`sextant`). Minimal: installs `color_eyre`, sets up `tracing`, and calls `sextant_ui::run()`.
- **`sextant-core`** — Domain primitives (`Driver`, `Connection`, `CellValue`, `QueryResult`, etc.) and the `QueryExecutor` trait. Kept lightweight with few dependencies.
- **`sextant-config`** — Loads `connections.toml`, `config.toml`, and `keys.toml` from XDG-compliant paths (`~/.config/sextant/`). Validates per-driver required fields.
- **`sextant-db`** — Implements `QueryExecutor` via `sqlx`. Manages per-connection connection pools. All DB I/O is async (`tokio`).
- **`sextant-ui`** — Owns the TUI event loop, state machine (`Normal` / `Insert` / `EditorOpen`), and all `ratatui` widgets (tree sidebar, result grid, editor modal, status line).

### Dependency Rules

- `sextant-cli` → `sextant-ui`
- `sextant-ui` → `sextant-core`
- `sextant-db` → `sextant-core`
- `sextant-config` → `sextant-core`
- Service crates (`sextant-db`, `sextant-config`) must compile and be testable without depending on `sextant-ui`.

---

## Build and Run

```bash
# Check the entire workspace
cargo check --workspace

# Run tests
cargo test --workspace

# Run the TUI application
cargo run
```

The binary opens a TUI window. In the current Phase 0 implementation it shows a black screen with a status line. Press `Ctrl+Q` to quit cleanly.

---

## Code Style Guidelines

- **Rust edition 2024**. Use modern syntax where appropriate.
- **Doc comments** in English (`//!` / `///`).
- **Prefer minimal, explicit code.** Do not add speculative abstractions, premature generics, or unused error variants.
- **Surgical changes only.** Do not refactor unrelated code, reformat adjacent lines, or "improve" comments that you didn't touch.
- **Clean up your own orphans.** If your changes leave imports, variables, or functions unused, remove them. Do not remove pre-existing dead code unless asked.
- **Every modified line must trace directly to the user's request.**

---

## Testing Instructions

### Unit Tests

- Use `ratatui::backend::TestBackend` to test widget rendering without a real TTY.
- Example: `crates/sextant-ui/src/lib.rs` contains tests for status-line rendering and `Ctrl+Q` handling.
- Service crates (`sextant-db`, `sextant-config`) should be unit-testable without spinning up the full TUI.
- **Do not write tests that only verify compiler derive macros.** A test that asserts `assert_eq!(Enum::A, Enum::A)` on a `#[derive(PartialEq)]` type, or checks that `#[from] std::io::Error` produces the expected variant, tests the language—not your code. Tests must validate project-specific logic: custom behavior, business rules, non-obvious edge cases, or invariants that are not guaranteed by the type definition alone.

### Integration Tests

- TUI integration tests can use `screen` to create a pseudo-tty, send `\x11` (`Ctrl+Q`), and verify exit code 0.
- Database tests should use temporary SQLite files or test containers for PG/MySQL.

### Integration Tests with Docker

A `compose.yml` provides PostgreSQL and MySQL test containers. Requirements: Docker + Docker Compose v2.

```bash
# Full cycle: start containers, run tests, tear down
make test-db

# Or step by step:
make test-db-up      # start containers and wait for readiness
make test-integration # run cargo test --workspace against Docker DBs
make test-db-down    # stop and remove containers + volumes
```

**Default connection URLs** (can be overridden via environment):
- PostgreSQL: `postgres://sextant:sextant@localhost:5433/sextant_test`
- MySQL: `mysql://sextant:sextant@localhost:3307/sextant_test`

Containers use non-standard ports (5433, 3307) to avoid conflicts with local database instances.

### Connecting the TUI to Docker databases

1. Install the Docker test connection definitions:
   ```bash
   make setup-docker-conns
   ```
   This creates `~/.config/sextant/connections.toml` with entries for `docker-pg` and `docker-mysql`.

2. Ensure the containers are running:
   ```bash
   make test-db-up
   ```

3. Start the TUI:
   ```bash
   cargo run
   ```
   The sidebar will show `docker-pg` and `docker-mysql`. Select one and press `Enter` to connect.

4. Passwords are read from environment variables (already exported by the Makefile targets):
   - `SEXTANT_DOCKER_PG_PASSWORD=sextant`
   - `SEXTANT_DOCKER_MYSQL_PASSWORD=sextant`

### Verification Checklist

Before declaring a task done:
1. `cargo check --workspace` compiles without warnings.
2. `cargo test --workspace` passes.
3. If the change affects the TUI, run `cargo run` and verify the app still starts and quits cleanly.

---

## Security Considerations

- **Passwords never belong in config files.** The `connections.toml` references credentials via `keyring_key`; actual passwords are stored in the OS keyring. For v0.1, a fallback via `SEXTANT_<NAME>_PASSWORD` environment variable is acceptable.
- **Redact connection strings in logs.** Never log full connection URIs with passwords.
- **Destructive operations require confirmation.** `DELETE` / `UPDATE` without `WHERE`, and any DDL, must trigger a confirmation modal by default.
- **Enforce restrictive file permissions on creation:**
  - `state.db`: `0600`
  - `.swp` files: `0600`
  - queries directory: `0700`
  - `.sql` files: `0600`
- Query text on disk (saved queries, swap files, history) is **not encrypted**. The threat model assumes local-machine access only.

---

## Development Workflow

When the user says "vamos con la Fase X" or "implementa el punto Y":

1. **Read `plan.md` first.** Check what is marked ✅ vs ⬜. Do not assume a task is done without verifying it in the code.
2. **Present options before acting.** If multiple approaches exist (library A vs B, architecture X vs Y), present tradeoffs and wait for a decision.
3. **Define the scope.** Ask whether the user wants the full phase or a specific subset.
4. **Code → Verify → Commit.** Each plan task must compile, pass tests, have a verifiable success criterion, and be committed atomically with a descriptive message.
5. **Update the plan immediately.** Mark the task as `[x] ✅` in `plan.md` and add the commit hash to the progress table.
   - **Commit hash ordering**: if `plan.md` itself must reference the commit hash of the changes being committed, do it in **two steps**: (1) commit the code first, (2) update `plan.md` with the real hash in a follow-up commit. Never use `git commit --amend` to inject the hash into the same commit; the amend changes the hash and creates an inconsistent reference.
6. **Plan/code sync: correctness wins.** If the implementation diverges from `plan.md` for technical reasons (compiler constraints, warnings, better practices, discovered blockers), update `plan.md` to reflect the actual code. The plan is a living document; correctness of the code always takes precedence over literal fidelity to the plan. Document the reason for the divergence in the plan or the commit message.
7. **If blocked, stop and report.** Do not improvise solutions to unplanned problems without consulting. Document blockers in the plan or an issue.

---

## Behavioral Guidelines (for LLM Agents)

1. **Think before coding.** State assumptions explicitly. If something is unclear, ask. If there are multiple interpretations, present them.
2. **Simplicity first.** Minimum code that solves the problem. No speculative features, no single-use abstractions, no unnecessary configurability.
3. **Goal-oriented execution.** Transform vague requests into verifiable criteria:
   - "Add validation" → "Write tests for invalid inputs, then make them pass."
   - "Fix the bug" → "Write a test that reproduces it, then make it pass."
   - "Refactor X" → "Ensure tests pass before and after."
4. **Do not hide confusion.** If unsure, name what is confusing and ask before implementing.

---

## Quick Reference

| Command | Purpose |
|---------|---------|
| `cargo check --workspace` | Compile all crates |
| `cargo test --workspace` | Run all tests |
| `cargo run` | Start the TUI |
| `cargo run --bin sextant` | Same as above (explicit binary) |

| File | What to read when... |
|------|----------------------|
| `plan.md` | Starting a new phase or task |
| `sextant-spec.md` | Need product-level context (features, UI, keybindings) |
| `Cargo.toml` (root) | Workspace metadata |
| `crates/*/Cargo.toml` | Crate dependencies and features |
