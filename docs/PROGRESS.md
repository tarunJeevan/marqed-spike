# PROGRESS

## Status
Current task: T2 — done (awaiting developer review and commit)
Last verification: 2026-10-17 — fmt ✓ clippy ✓ test ✓

## Tasks
- [x] T0 — Scaffold, CLI, fixtures
- [x] T1 — Parse
- [x] T2 — Reveal
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
- Sign-off: developer reviewed T1, committed and merged to main on 2026-10-17.

### T2 — 2026-10-17
- Built: `src/reveal.rs` — `reveal(segments, cursor, mode, policy) -> RevealSet`; types `Mode { Normal, Insert }`, `HeadingPolicy { G1, G2 }`, `NestingPolicy { N1, N2 }`, `RevealPolicy`. `RevealSet` is one `Vec<bool>` per segment, parallel to `Segment::constructs` (`is_revealed`, `for_segment`, `count`). `holds()` is the C-4 table (each construct's own condition); N1 keeps the smallest-range holder, N2 keeps every holder plus its `parent` chain. Only segments whose range touches the cursor are searched (`partition_point`). `line_end` for the heading Insert rule is the heading construct's `range.end` (segment minus EOL), so `reveal` needs no text.
- Tests: 16 in `reveal::tests`. Table-driven over G1N1/G1N2/G2N1/G2N2: `##` bar at 2; `## ` bar at 3 (and 2); `**important**` bar at 13/12 (plus 0/1, Normal 0/12); `***both***` Normal and Insert at every boundary; adjacent strongs; emphasis in strong; `## Roadmap with **bold** inside` Normal and Insert; `### Title ###` markers and closing sequence; empty heading Normal; `# ***x***` (heading + two-level inline); multi-segment (only the cursor's segment reveals; bar on a segment boundary); Raw/Gap never reveal; empty document and out-of-range cursor. Exhaustive invariant check over every offset (0..=len+1), both modes, both heading policies on all three fixtures, and a proptest (500 cases, T1 alphabet plus `**`, `# `, `*a*` atoms): N1 reveals ≤ 1 construct, N1 ⊆ N2, N2 is ancestor-closed, and both N1 and N2 equal an independent un-windowed full scan.
- Verification: `cargo fmt --all -- --check` ✓, `cargo clippy --all-targets -- -D warnings` ✓, `cargo test` ✓ (66 passed). Mutation check (not committed): making the Insert inline rule non-strict, and dropping `hash_run.end` from the G2 Insert rule, each made tests fail.
- MANUAL: none (no GUI surface in T2).
- Notes: `Mode` and the policy enums live in `reveal.rs` for now; later tasks import them from there. `reveal.rs` has a module-level `#![allow(dead_code)]` like `parse.rs`; both come off once T3+ consume them. For T4 to check, not yet observed: reveal depends only on the cursor, but under I-6 a concealed marker is never a landing spot, so how a Normal cursor ever reaches a heading marker under G2 is a T4 question.

## Findings
- F-1 (T1): pulldown nests `***both***` as Emphasis (outer, index 0) containing Strong (inner, index 1); the six markers split as Emphasis `[0,1)`/`[9,10)` and Strong `[1,3)`/`[7,9)` — evidence: test `both_is_a_two_level_tree`.
- F-2 (T1): for a heading whose first child is an escape (`# \\*a`), pulldown's Text event starts at the escaped character, so C-3's `content_start` rule puts the backslash inside the opening marker (`[0,3)`); it is concealed when the heading is concealed. Escapes are out of scope, so recorded as a characterization — evidence: test `heading_with_escape_first_conceals_the_backslash`.
- F-3 (T1): a setext heading whose text starts with `#` is misclassified as ATX by C-2's literal "first non-space byte is `#`" rule; handled with a single-line check — evidence: test `setext_heading_starting_with_hash_is_raw`.
- F-4 (T1): the line-sharing merge path is reachable from real input, not only synthetic: `-\n aaaa`, `*\n aaaa` make pulldown report a list and a paragraph whose normalized ranges overlap; both become one `Raw` — evidence: test `real_overlapping_blocks_merge_into_raw` (found by brute-force enumeration over a 18-atom alphabet, length 5).
- F-5 (T1): link reference definitions emit no pulldown events, so their lines become a `Gap` that is not blank (`[a]: /url\n\n`). C-2 says gaps are "such as blank or whitespace-only runs", so this is allowed but later tasks must not assume a Gap is whitespace — evidence: test `link_reference_definition_becomes_a_gap`.
- F-6 (T2): `## ` with the bar at 3 (just after the space is typed): G1 keeps the heading revealed (the bar is on the heading line); G2 conceals it. With the bar at 2 both reveal. This is the design-flow step 5 difference between the policies — evidence: test `heading_with_bar_after_the_space_conceals_under_g2_and_g1_keeps_it`.
- F-7 (T2): with the cursor inside bold in a heading, N1 reveals only the bold, so the heading's `#` stay concealed even under G1; N2 reveals heading and bold. Under G2 the cursor in bold text never satisfies the heading's own condition, so N1 vs N2 is the only thing that reveals the `#` there — evidence: tests `heading_with_bold_normal`, `heading_with_bold_insert`, `bold_inside_heading_with_both_nested`.
- F-8 (T2): the Insert rule is strict for inline constructs, so a bar immediately before an opening marker or after a closing marker (`|**x**`, `**x**|`) reveals nothing, while a bar between the two `*` of a marker (`*|*x**`) does — evidence: tests `strong_at_end_of_line_bar_13_conceals_and_bar_12_reveals`, `heading_with_bold_insert`.
- F-9 (T2): under G2 the Normal cursor on a heading line's EOL cell conceals the markers (`##\n` at offset 2 reveals nothing), while G1 reveals through the EOL cell — evidence: test `empty_heading_normal`.

## Halts
