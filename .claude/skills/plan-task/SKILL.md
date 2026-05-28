---
name: plan-task
description: Guided workflow to implement a task from plan.md following sextant's development process. Use when the user says "vamos con la Fase X", "implementa el punto Y", or otherwise asks to pick up a roadmap task.
---

# plan-task

Drive a `plan.md` task through the project's Code → Verify → Commit → update-plan
loop. This operationalizes the "Development Workflow" in `AGENTS.md`.

## Steps

1. **Read `plan.md` first.** Identify the requested task and confirm whether it
   is `⬜` (pending) or already `✅`. Do not assume it's undone — verify in the
   code. Read `ARCHITECTURE.md` for where the relevant concern lives.

2. **Confirm scope.** Ask whether the user wants the full phase or a specific
   subset. If multiple technical approaches exist, present trade-offs and wait
   for a decision before coding.

3. **Code.** Make minimal, surgical changes. Every modified line should trace to
   the task. Clean up orphaned imports/vars you introduce.

4. **Verify.** Run the `workspace-check` skill (check, test, fmt --check,
   clippy). For TUI-affecting changes, also `cargo run` and confirm clean
   start/quit (`Ctrl+Q`).

5. **Commit atomically** with a descriptive message scoped to the task.

6. **Update `plan.md`.** Mark the task `[x] ✅` and add the commit hash to the
   progress table.
   - **Commit-hash ordering:** if `plan.md` must reference the hash of the code
     just committed, do it in **two commits** — (1) commit the code, (2) commit
     the `plan.md` update with the real hash. **Never `git commit --amend`** to
     inject the hash; amending changes the hash and creates a stale reference.

7. **If blocked,** stop and report. Document the blocker in `plan.md`; don't
   improvise solutions to unplanned problems.

## Sync rule

If the implementation must diverge from `plan.md` for technical reasons, update
`plan.md` to match the code and note why. Correctness of the code wins over
literal fidelity to the plan.
