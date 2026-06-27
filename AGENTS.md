# AGENTS.md

Agent-facing reference for the `sextant` project. Read this first before making changes.

---

## Project Overview

`sextant` is a keyboard-driven, terminal-based database client for **PostgreSQL**, **MySQL**, and **SQLite**. It targets developers who want a TablePlus/DataGrip equivalent without leaving the shell. Written in Rust, rendered with `ratatui`, with modal editing inspired by **Helix** (selection-first).

- **Language**: Rust (edition 2024, MSRV 1.85+) · **License**: MIT · **Author**: wukong0111

### Key Documents

| File | Purpose |
|------|---------|
| `ARCHITECTURE.md` | Code map: data flow, crates & dependency rules, where each concern lives, invariants/gotchas, "how to add X". Read before touching code. |
| `SPEC.md` | Implementation-agnostic product spec: behavior, observable contracts, §17 Given/When/Then acceptance criteria, §16 security requirements, product rationale. The canonical "what". |
| `docs/adr/` | Implementation decision records (the "how/why-of-how"). Each links its requirement in `SPEC.md`. |
| `plan.md` | Forward-looking roadmap: deferred backlog + post-v1 work. Shipped phases live in git history. Check before starting new work. |
| `docs/coverage.md` | Binding from `SPEC.md` §17 criteria to concrete tests, plus the catalog of manual/visual-only checks. |
| `docs/documentation-guide.md` | Which doc a change must touch, the litmus, and ordering rules. |

> Project workflows are slash-command skills in `.agents/skills/`:
> `db-setup`, `connect-tui`, `workspace-check`, `plan-task`.

---

## Workspace Structure

Cargo workspace; the root `Cargo.toml` is metadata, all code lives in `crates/`.

```
sextant/
├── Cargo.toml                 # workspace definition
├── plan.md                    # roadmap / backlog (post-v1)
├── SPEC.md                    # agnostic product spec
├── docs/adr/                  # implementation decision records
└── crates/
    ├── sextant-cli/           # binary entry point (main.rs)
    ├── sextant-core/          # domain types, traits, shared errors
    ├── sextant-config/        # TOML config, XDG paths, keymaps
    ├── sextant-db/            # sqlx drivers, query exec, introspection
    ├── sextant-state/         # local state.db (history, recent files)
    └── sextant-ui/            # ratatui components, event loop, layout
```

Crate responsibilities and dependency rules live in **`ARCHITECTURE.md`** (§ Crates & dependency rules).

---

## Code Style Guidelines

- **Rust edition 2024**. Use modern syntax where appropriate.
- **Doc comments** in English (`//!` / `///`).
- **Prefer minimal, explicit code.** No speculative abstractions, premature generics, or unused error variants.
- **Prefer `let`-else over method chaining.** Use `let Some(x) = ... else { ... };` (and similar `let`-else forms) to unwrap one level at a time and keep the happy path left-aligned, instead of long `.and_then` / `.map` chains.
- **Surgical changes only.** Don't refactor unrelated code, reformat adjacent lines, or "improve" comments you didn't touch.
- **Clean up your own orphans.** Remove imports/variables/functions your change leaves unused. Don't remove pre-existing dead code unless asked.
- **Every modified line must trace directly to the user's request.**

---

## Testing

- **Unit (no TTY).** UI widgets render through `ratatui::backend::TestBackend`; service crates (`sextant-db`, `sextant-config`) are unit-testable standalone. **Don't test compiler-derived behavior** — `assert_eq!(Enum::A, Enum::A)` on a `#[derive(PartialEq)]`, or `#[from]` plumbing, tests the language, not your code. Test project logic, edge cases, and invariants the type doesn't already guarantee.
- **End-to-end (PTY).** The harness in `crates/sextant-cli/tests/common/mod.rs` (`Tui` + `Fixture`) drives the **real binary** through a pseudo-terminal: hermetic (temp `HOME`/XDG + seeded SQLite via `rusqlite`, no Docker), auto-waits with `Tui::wait_for(needle, timeout)` instead of sleeping, paces keystrokes (a lone `Esc` needs an extra pause), and can cross-check `state.db` after driving the UI. `tests/e2e.rs` holds assertions; `tests/smoke.rs` holds `#[ignore]`d on-demand screenshots. Run `make e2e` / `make smoke`.
- **Databases & manual runs.** The `db-setup` skill brings up the Docker PG/MySQL containers (non-standard ports 5433/3307) and seeds data; `connect-tui` wires the TUI to a DB. See the `make test-db*` / `make seed*` targets.
- **Before declaring done.** Run the `workspace-check` skill (or `make check`): `cargo check` / `test` / `clippy` / `fmt --check` across the workspace. If the change touches the TUI, also `cargo run` and confirm it starts and quits cleanly (`Ctrl+Q`).

---

## Security (non-negotiables)

Full requirements in **`SPEC.md` §16**. Do not regress these:

- **Passwords never in config files** — only the OS keyring (or the `SEXTANT_<NAME>_PASSWORD` env fallback for v0.1). Redact connection strings in logs (never log a URI with a password).
- **Destructive ops require confirmation** — `DELETE` / `UPDATE` without `WHERE`, and any DDL, trigger a confirmation modal by default.
- **Restrictive file perms on creation** — `0600` for `state.db` / `.swp` / `.sql`, `0700` for the queries dir. Query text on disk is **not** encrypted; the threat model assumes local-machine access only.

---

## Development Workflow

**Any feature, bug fix, or refactor that changes observable behavior must follow this loop.** Do not write code until the spec and tests are in place.

> **Self-enforcement:** These steps must be triggered proactively by the agent. Do not wait for the user to ask for `plan-task` or `done-checklist`; run them automatically whenever the task changes observable behavior.

- **Before starting:** run the **`plan-task`** skill. It forces reading `SPEC.md`/`plan.md` first, confirming scope, and writing the §17 criterion.
- **Before declaring done:** run the **`done-checklist`** skill. It verifies tests, docs, and `make check` were completed in the right order.

The lifecycle is:

1. **Read `plan.md` and `SPEC.md` first.** Don't assume something is done or undone without verifying it in the code.
2. **Present options before acting.** If multiple approaches exist (library A vs B, architecture X vs Y), present tradeoffs and wait for a decision.
3. **Define the scope.** Confirm how much the user wants done.
4. **Spec → Test → Code → Verify → Commit.** Follow the spec- and test-first lifecycle in **`docs/documentation-guide.md`**: behavior change → §17 criterion → failing test (or manual-catalog entry) → minimal code → `make check` → atomic commit. Every task needs a verifiable success criterion.
5. **Update `plan.md` only if a roadmap item changed status** (shipped, started, or newly deferred). No per-commit hash — git is the log.
6. **If blocked, stop and report.** Don't improvise solutions to unplanned problems without consulting.

**Documentation discipline:** a change touches **only the docs its kind implies** — `SPEC.md` for observable behavior/contracts (with a matching §17 Given/When/Then), a **new immutable** ADR for noteworthy implementation decisions, `ARCHITECTURE.md` for structural changes, `plan.md` only when a roadmap item changes status (git is the per-commit log). Spec-first; keep `SPEC.md` agnostic. Full rules in **`docs/documentation-guide.md`**.

> Turn vague requests into verifiable criteria before coding ("fix the bug" → "write a failing test, then make it pass"). State assumptions; if something is unclear or has multiple readings, ask instead of guessing.

---

## Agent Operating Autonomy

The user expects the agent to act with **maximum sensible autonomy** once a task is accepted. This section hardens the workflow above into explicit defaults.

- **Run `make check` after every non-trivial change.** Do not report completion until it passes. If it fails, fix it in the same turn if the fix is clearly within scope.
- **Run Docker-backed tests when Docker is available.** Prefer `make test-db`; otherwise ensure `docker compose` is up and run `cargo test --workspace`. Multi-driver tests skip cleanly without Docker, so availability is the default assumption.
- **Do not ask permission for routine, low-risk steps** such as formatting, adjusting imports, renaming local variables, adding tests, or re-running the test suite.
- **When multiple technical options exist, choose the recommended one and document the trade-off.** Only stop and ask when the choice affects architecture, security, public API, or the product contract in `SPEC.md`.
- **If a bug surfaces during verification, fix it in the same cycle** when it is obviously related to the accepted task. Report it as a discovered issue, not as a blocker.
- **Keep the user informed, not consulted.** Summarize what was done and why; do not request confirmation for each step.

---

## Quick Reference

| Command | Purpose |
|---------|---------|
| `cargo check --workspace` | Compile all crates |
| `cargo test --workspace` | Run all tests |
| `cargo test -p <crate>` | Run one crate's tests (e.g. `-p sextant-db`) |
| `cargo run` | Start the TUI (`cargo run --bin sextant` is explicit) |
| `make check` | Full verification: compile, test, fmt check, clippy |
| `make e2e` | PTY end-to-end tests (SQLite only, no Docker) |
| `make smoke` | Print live "screenshots" of the running TUI (no TTY) |
| `make seed-sqlite` / `make seed` | Seed local SQLite / all DBs |
| `make test-db` | Full cycle: start Docker DBs, run tests, tear down |

(Which doc to read when is the **Key Documents** table above.)
