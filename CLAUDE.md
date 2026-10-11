# CLAUDE.md — marqed projection spike

## Goal

Implement the marqed projection spike: complete tasks T0–T10 in order, each meeting its acceptance checks (if it has them), then write `VERDICT.md` after finishing T0-T10. If a halt condition is hit, stop and report instead of improvising.

**Done** means one of two things:
- every task is ticked in `PROGRESS.md`, `VERDICT.md` exists, and the full verification suite is green; or
- an unresolved halt is recorded in `PROGRESS.md`, and you have stopped.

## Invariants referenced in code comments

These come from the production architecture. Tag the code that implements them with `// I-<n>`.

- **I-1** — All persistent positions are source byte offsets. View coordinates are never stored; the one exception is the transient `goal_x`.
- **I-6** — Concealed markers are zero-width to motions: never a cursor landing spot. Inside a revealed construct, ordinary source-offset motions apply.
- **I-7** — Reveal keeps styling: revealed markers render in their construct's style, so a heading's `#` is heading-sized.
- **I-8** — Every computed result carries the `version` it was computed from, and stale results are discarded.

## Document map

| File | Role | You may edit? |
|---|---|---|
| `CLAUDE.md` | This file: goal, limits, workflow | No |
| `docs/SPIKE_GOAL.md` | Project context, ground rules, verified API surface, verification commands, halt conditions (H1–H5), out-of-scope list, `VERDICT.md` template | No |
| `docs/CONTRACTS.md` | C-1 to C-12: the behavioral specification. **This is the source of truth for behavior.** | No |
| `docs/tasks/T<n>.md` | One task each: what to build, which contracts it implements, tests, acceptance | No |
| `docs/PROGRESS.md` | Your working record (format below) | Yes |
| `docs/VERDICT.md` | Final report, written after T10 using the template in `SPIKE_GOAL.md` | Yes (post-T10 only) |
| `src/`, `fixtures/`, `Cargo.toml` | Implementation | `src/` and `fixtures/` yes; `Cargo.toml` only within the ground rules (no dependency changes — halt H4) |

**Precedence when documents disagree:** `SPIKE_GOAL.md` (ground rules and halts) > `CONTRACTS.md` > `tasks/T<n>.md` > `PROGRESS.md`.

Any conflict between a task file and `CONTRACTS.md` is halt H3. Never resolve a conflict yourself, and never treat a note in `PROGRESS.md` as amending a contract.

## Workflow

**At the start of every session, before writing any code:**

1. Read `SPIKE_GOAL.md` and `CONTRACTS.md` in full, then read `PROGRESS.md`.
2. If `PROGRESS.md` records an unresolved halt, do not continue. Restate the halt and stop, unless the developer's current message resolves it. If it does, log the resolution in `PROGRESS.md` first.
3. Open the current task's file. Re-read every contract it cites before implementing.

**For each task:**

1. Work on exactly one task, in order. Tasks never overlap or run ahead.
2. Implement the code and its tests together. Tests cited by the task are part of the task, not follow-up work.
3. Run the verification commands from `SPIKE_GOAL.md`, plus anything the task's acceptance section adds.
4. A task is done only when all of the following hold:
   - every acceptance check passes;
   - `fmt`, `clippy` and `test` are green;
   - `PROGRESS.md` is updated (tick, log entry, any findings).
5. Alert the developer and pause for manual review and commit. Only resume after developer signs off on the task.
6. Move on to the next task.

**Manual GUI checks.** Some acceptance checks are visual (e.g. "renders", "rewraps"). You cannot see the GUI, so do not claim to have verified them. Verify what you can headlessly. Then list each visual check under the task's log entry as `MANUAL: <check>`; the developer confirms these later.

## Limits

- **Halts.** On any halt condition H1–H5:
  1. Stop work on the task.
  2. Record the halt in `PROGRESS.md`, with the minimal evidence: compiler error, shrunk counterexample, or the conflicting passages.
  3. Report it, then end. Do not attempt workarounds.
- **API facts.** Never rely on memory for API details beyond the verified surface in `SPIKE_GOAL.md`. Check the actual crate sources in `~/.cargo/registry/src/` for the pinned versions. If the source contradicts the verified surface, that is H1.
- **Tests.** Never weaken, `#[ignore]`, or delete a test to get green (H5). Never change a test's expected outcome to match the implementation unless the contract says the implementation is right and the test was wrong — and say so in the log when you do.
- **Scope.** Nothing from the out-of-scope list in `SPIKE_GOAL.md`. No optimization beyond what the contracts require, and no "while I'm here" refactors of earlier tasks. If an earlier task's code is wrong, fix the bug, note it in the log, and keep the fix minimal.
- **Characterization tests.** Some tests document known defects of a policy, e.g. G2 heading-marker reachability. They assert current behavior on purpose. Do not "fix" the behavior they describe.

## `PROGRESS.md` format

Keep exactly these sections. Everything is append-only except the Status block and the checkboxes.

```markdown
# PROGRESS

## Status
Current task: T<n> — in progress | blocked (H<k>) | done
Last verification: <date> — fmt ✓/✗ clippy ✓/✗ test ✓/✗

## Tasks
- [ ] T0 — Scaffold, CLI, fixtures
- [ ] T1 — Parse
- [ ] T2 — Reveal
- [ ] T3 — Projection
- [ ] T4 — Horizontal motions
- [ ] T5 — Static rendering
- [ ] T6 — Geometry, cursor, keys, click
- [ ] T7 — Vertical motion
- [ ] T8 — Insert editing and reparse loop
- [ ] T9 — Identity
- [ ] T10 — Perf and iced_test smoke
- [ ] T11 — VERDICT.md

## Log
### T<n> — <date>
- Built: <one line per module/feature>
- Tests: <names or counts; proptest case counts>
- Verification: <commands run, result>
- MANUAL: <visual checks for the developer, if any>
- Notes: <bugs fixed in earlier tasks, anything surprising>

## Findings
- F-<n> (T<n>): <observation> — evidence: <test name / bench output / overlay observation>

## Halts
- H<k> @ T<n> (<date>): <what happened, minimal evidence, what the developer must decide> — status: open | resolved (<how>)
```

**Findings** are the raw material for `VERDICT.md`. Record a finding when you observe it, rather than reconstructing it in T11. Anything that bears on the four open questions, on go/no-go, or on a draft decision-log entry in the `VERDICT.md` template counts as a finding.
