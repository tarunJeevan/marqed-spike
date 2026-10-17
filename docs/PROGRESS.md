# PROGRESS

## Status
Current task: T0 — done (awaiting developer review and commit)
Last verification: 2026-10-16 — fmt ✓ clippy ✓ test ✓

## Tasks
- [x] T0 — Scaffold, CLI, fixtures
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
### T0 — 2026-10-16
- Built: `src/app.rs` (`App` state, `update`, `view`: stock `text` in a `scrollable`; empty `Message` enum for now); thin `src/main.rs` (CLI `<path> [--repeat N] [--bench K]`, UTF-8 file load, `--repeat` joins N copies with `"\n\n"`, iced `application(boot, update, view)`); fixtures `spike.md`, `no_trailing_newline.md`, `empty.md`.
- Tests: 6 inline in `main.rs` — arg parsing, bad args, repeat join, all fixtures load (plus no-trailing-newline / empty properties), `spike.md` contains every construct T0 requires, unreadable and non-UTF-8 files error.
- Verification: `cargo fmt --all -- --check` ✓, `cargo clippy --all-targets -- -D warnings` ✓, `cargo test` ✓ (6 passed). Launched the debug binary on all three fixtures: window stays open until killed by `timeout` (exit 124), no panic. Missing file and non-UTF-8 file print an action-phrased error and exit 1.
- MANUAL: `cargo run -- fixtures/spike.md` displays the file's raw text in a scrollable window (I cannot see the GUI).
- Notes: iced 0.14.0 `application` boot is `Fn() -> (State, Task)` with no arguments, so the loaded text is captured by a `move` closure (clone per boot). Choices where T0 was silent: `--repeat 0` is rejected as an error; `--bench K` is parsed but only prints a "not implemented yet" note and opens the window (bench lands in T10). Other skeleton modules remain empty.

## Findings

## Halts
