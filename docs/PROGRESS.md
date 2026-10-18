# PROGRESS

## Status
Current task: T1 — done (awaiting developer review and commit)
Last verification: 2026-10-17 — fmt ✓ clippy ✓ test ✓

## Tasks
- [x] T0 — Scaffold, CLI, fixtures
- [x] T1 — Parse
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
- Sign-off: developer confirmed the T0 MANUAL check, committed and merged to main on 2026-10-17.

### T1 — 2026-10-17
- Built: `src/parse.rs` — `parse(&str, version) -> ParseResult { version, segments }`; `Segment { range, kind, content_hash, constructs }` (no id field); `SegmentKind { Paragraph, Heading { level }, Raw, Gap }`; `Construct { kind: Strong | Emphasis | Heading { hash_run }, range, parent, open, close }`; `Segment::eol`. Top-level blocks from depth-0 pulldown events, line normalization, merge of line-sharing blocks into `Raw`, gap fill, marker validation (`*`/`_` only, else dropped with children re-parented to the nearest kept ancestor). `content_hash` is `DefaultHasher` over the segment's bytes only.
- Tests: 50 total (44 in `parse::tests`, 6 from T0). Property tests (proptest, 2000 cases each, alphabet from T1.md): partition invariant, segments are whole lines on char boundaries, construct shape (parents precede children and contain them; marker widths/bytes; no `\n` in markers; heading construct is the root and spans the segment minus EOL), hash matches segment bytes, parse is deterministic. Fixture assertions: exact marker ranges for plain/closing-sequence/empty/indented headings, Strong/Emphasis/nested/adjacent/line-spanning constructs, `***both***` tree, unclosed/escaped/literal-`\n` cases make no construct, setext/list/fence/quote/table/HTML/rule are `Raw`, spike.md constructs and gaps, content-hash determinism and position independence.
- Verification: `cargo fmt --all -- --check` ✓, `cargo clippy --all-targets -- -D warnings` ✓, `cargo test` ✓ (50 passed).
- MANUAL: none (no GUI surface in T1).
- Notes: `parse.rs` carries a module-level `#![allow(dead_code)]` because nothing outside tests consumes it yet; remove it once T2/T3 use every item. Two places where T1.md/C-2/C-3 were silent or literal-reading was wrong (no contract conflict, so not H3):
  - ATX detection uses "the segment is a single line" rather than only "first non-space byte is `#`", because a setext heading whose text starts with `#` (e.g. `#tag\n---`) would otherwise be misclassified as ATX. The two rules agree on every other input.
  - Depth-0 events that are not `Start` (thematic break) are treated as one-event blocks, so they become `Raw` as C-2 requires.

## Findings
- F-1 (T1): pulldown nests `***both***` as Emphasis (outer, index 0) containing Strong (inner, index 1); the six markers split as Emphasis `[0,1)`/`[9,10)` and Strong `[1,3)`/`[7,9)` — evidence: test `both_is_a_two_level_tree`.
- F-2 (T1): for a heading whose first child is an escape (`# \\*a`), pulldown's Text event starts at the escaped character, so C-3's `content_start` rule puts the backslash inside the opening marker (`[0,3)`); it is concealed when the heading is concealed. Escapes are out of scope, so recorded as a characterization — evidence: test `heading_with_escape_first_conceals_the_backslash`.
- F-3 (T1): a setext heading whose text starts with `#` is misclassified as ATX by C-2's literal "first non-space byte is `#`" rule; handled with a single-line check — evidence: test `setext_heading_starting_with_hash_is_raw`.
- F-4 (T1): the line-sharing merge path is reachable from real input, not only synthetic: `-\n aaaa`, `*\n aaaa` make pulldown report a list and a paragraph whose normalized ranges overlap; both become one `Raw` — evidence: test `real_overlapping_blocks_merge_into_raw` (found by brute-force enumeration over a 18-atom alphabet, length 5).
- F-5 (T1): link reference definitions emit no pulldown events, so their lines become a `Gap` that is not blank (`[a]: /url\n\n`). C-2 says gaps are "such as blank or whitespace-only runs", so this is allowed but later tasks must not assume a Gap is whitespace — evidence: test `link_reference_definition_becomes_a_gap`.

## Halts
