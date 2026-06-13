---
name: done-checklist
description: Gatekeeper checklist to run before declaring a task complete. Verifies the spec-and-test-first workflow was followed end-to-end.
---

# done-checklist

Run this skill **before telling the user a task is done**. It verifies that the
spec-and-test-first workflow was actually followed, not skipped or retrofitted.

## When to use

Use after coding is finished and `make check` passes, but **before** saying
"listo", "done", "complete", or pushing a commit.

## Checklist

Answer each question honestly. If any answer is **No**, do that step now before
reporting completion.

1. **Did you read `SPEC.md` and `plan.md` before writing code?**
   - If this was a pure refactor or a bug fix that only aligns code with an
     existing spec criterion, you may answer Yes retroactively only if you
     verified the criterion exists.

2. **Is there a `SPEC.md` §17 Given/When/Then criterion for the changed behavior?**
   - New/changed behavior → criterion must exist.
   - Pure refactor or spec-aligned bug fix → skip.

3. **Were tests written *before* or alongside the implementation, not after?**
   - The test should fail (red) before the fix/feature makes it pass (green).
   - If you wrote tests after the code, run them against the pre-fix commit to
     confirm they would have failed.

4. **Is the test mapped in `docs/coverage.md`?**
   - Add the row linking the §17 criterion to the concrete test file/function.
   - If the test is visual-only (must be verified by a human), add it to the
     manual catalog in `coverage.md`.

5. **Is there an ADR if a noteworthy technical decision was made?**
   - Examples: replacing a library widget with manual rendering, changing a
     public API, introducing a new dependency, altering a data flow invariant.
   - ADRs are immutable; if the decision evolved, create a new one that
     supersedes the old.

6. **Does `make check` pass?**
   - `cargo check --workspace`, `cargo test --workspace`, `cargo fmt --check`,
     `cargo clippy --workspace`.

7. **If the TUI changed, did you `cargo run` and confirm clean start/quit?**
   - Start the app, verify the change is visible, quit with `Ctrl+Q`.
   - Do not skip this for grid, layout, or input-handling changes.

8. **Is the commit atomic and descriptive?**
   - One logical change per commit. No unrelated cleanups mixed in.
   - Commit message explains *why*, not just *what*.

## Outcome

Only after **all** applicable questions are Yes may you report the task as
complete. If you had to retroactively add tests, docs, or ADRs, note that in
your summary to the user.
