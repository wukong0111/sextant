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

## Order (spec-first)

`SPEC.md` (what + G/W/T) → implement and verify against that criterion → ADR if a
technical decision was made → `ARCHITECTURE.md` if the structure changed →
`plan.md` (log + commit).

## Invariant

`SPEC.md` is **agnostic** (no language, library, or architecture). Never leak
implementation into it — that belongs in `docs/adr/` and `ARCHITECTURE.md`.
