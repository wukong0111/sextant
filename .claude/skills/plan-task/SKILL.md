---
name: plan-task
description: Guided workflow to implement a feature or roadmap/backlog item following sextant's spec- and test-first process. Use when the user says "implementa X", asks to pick up a backlog item from plan.md, or starts a new feature.
---

# plan-task

Drive a feature or `plan.md` backlog item through the project's spec- and
test-first lifecycle. This operationalizes the "Development Workflow" in
`AGENTS.md` and the lifecycle in `docs/documentation-guide.md`.

## Steps

1. **Read `plan.md` and `SPEC.md` first.** Identify the item; don't assume it's
   done or undone — verify in the code. Read `ARCHITECTURE.md` for where the
   relevant concern lives.

2. **Confirm scope.** If multiple technical approaches exist, present trade-offs
   and wait for a decision before coding.

3. **Spec (if behavior changes).** Add/adjust the `SPEC.md` §17 Given/When/Then
   first. The criterion is the prose form of the test. A pure refactor skips
   this; a bug fix that aligns code with the spec skips it too.

4. **Test (red).** Translate the criterion into the strongest feasible tier
   (UNIT → APP → E2E) and add its row to `docs/coverage.md`. It fails first.
   Slice that only a human can verify → the manual catalog in `coverage.md`.

5. **Code (green).** Minimal, surgical changes that make the tests pass. Every
   modified line traces to the item; clean up orphans you introduce.

6. **Verify.** Run the `workspace-check` skill (check, test, fmt --check,
   clippy). For TUI changes, also `cargo run` and confirm clean start/quit
   (`Ctrl+Q`).

7. **Commit atomically** with a descriptive message. Then document: ADR if a
   noteworthy technical decision was made, `ARCHITECTURE.md` if structure
   changed, and `plan.md` **only if a roadmap item changed status** (no
   per-commit hash — git is the log).

8. **If blocked,** stop and report; don't improvise solutions to unplanned
   problems.

## Sync rule

If the implementation must diverge from the spec/plan for technical reasons,
update the doc to match the code and note why. Correctness of the code wins over
literal fidelity to the plan.
