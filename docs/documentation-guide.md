# Documentation guide — which doc a change must touch

The docs are split by the **question they answer**, so a change updates *only the
docs its kind implies*, not all of them. This guide is the full rule; `AGENTS.md`
carries a short pointer to it.

## Routing table

| Doc | Touch it when… | Don't touch it if… |
|-----|----------------|--------------------|
| `SPEC.md` | the **observable behavior or a contract** changes — new feature, changed default keybinding, export/import format, security rule. Add/adjust the matching **Given/When/Then** criterion in §17. | it's a refactor or an implementation swap with no observable effect. |
| `docs/adr/` | you make a **non-obvious implementation decision** (new library, new mechanism, or superseding a prior one) → write a **new** ADR. ADRs are **immutable**: never edit an accepted one; a new ADR marks the old as *superseded*. | the change carries no noteworthy technical decision. |
| `ARCHITECTURE.md` | the **code structure/wiring** changes — new crate/module, a concern moves, a new invariant. | code moves within what's already documented. |
| `plan.md` | always, as work advances — task status and commit hash (the log). | — |

## Litmus for `SPEC.md`

*Would two independent implementations, in different languages, be forced to
mirror this change?* If yes → spec. Concretely:

- **New feature / deliberate behavior change** → update spec **and** its
  acceptance criterion. A feature without a G/W/T criterion leaves the spec
  incomplete.
- **Bug fix that aligns code with the spec** → **no** spec change (the spec was
  already right; the code was wrong).
- **Bug fix that reveals the spec was wrong** → fix the spec.
- **Refactor / library swap** → spec **no**; ADR **yes** if the decision is
  noteworthy.

## Lifecycle of a change (spec- and test-first)

1. **Spec** — if observable behavior changes, write/adjust the `SPEC.md` §17
   Given/When/Then **first** (see litmus above). The criterion is the prose form
   of the test you're about to write.
2. **Test (red)** — translate each new/changed criterion into the strongest
   feasible tier (UNIT → APP → E2E, see `docs/coverage.md`) and add its row to
   `coverage.md`. The test fails first. For the slice only a human can verify
   (color, real-TTY feel, PG/MySQL), register it in coverage's manual catalog
   instead — that *is* its coverage.
3. **Implement (green)** — the minimal code that makes the tests/criteria pass.
4. **Verify** — `make check` (compile, test, clippy, fmt); `cargo run` if the
   TUI changed.
5. **Document** — ADR if a noteworthy technical decision was made;
   `ARCHITECTURE.md` if the structure/wiring changed; finalize the `coverage.md`
   row (definitive tier).
6. **Log** — `plan.md` status + commit hash, and commit atomically.

**Conditionals.** A refactor or library swap skips steps 1–2 (no observable
change) but still needs an ADR if the decision is noteworthy. A bug fix that
aligns code with the spec skips the spec edit (the spec was already right) but
still **starts with a red regression test**.

## Invariant

`SPEC.md` is **agnostic** (no language, library, or architecture). Never leak
implementation into it — that belongs in `docs/adr/` and `ARCHITECTURE.md`.

## Acceptance-criteria binding

The mapping from `SPEC.md` §17 criteria to this implementation's tests lives in
`docs/coverage.md` (per-implementation, kept out of the agnostic spec). A new
criterion adds a row there; a row at tier **—** is explicit test debt.
