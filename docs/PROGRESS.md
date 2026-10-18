# PROGRESS

## Status
Current task: T3 — done (awaiting developer review and commit)
Last verification: 2026-10-18 — fmt ✓ clippy ✓ test ✓

## Tasks
- [x] T0 — Scaffold, CLI, fixtures
- [x] T1 — Parse
- [x] T2 — Reveal
- [x] T3 — Projection
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
- Sign-off: developer reviewed T2, committed and merged to main on 2026-10-17.

### T3 — 2026-10-18
- Built:
  - `src/projection.rs` — `project(text, seg, flags) -> Projection` and `project_all(text, segments, &RevealSet)`. `Projection { range, visible, spans, runs, base_size }` with `src_to_vis(o) -> Option<SrcVis { vis, eol }>` and `vis_to_src(v, Affinity) -> usize`. Types `Run { Visible { src, vis_start } | Concealed { src } | Eol { src } }`, `Affinity`, `VisualPos`, `Style`, `StyleSpan`; `base_size(kind)` (14.0 × `[2.0,1.6,1.3,1.15,1.05,1.0]` for headings); `check_frame_invariant(projections, cursor, mode) -> Result` (the I-6 check; callers `debug_assert!` it per frame from T5).
  - `src/doc.rs` — `grapheme_starts`, `normal_cursors`, `insert_bars` (valid cursor/bar positions from `unicode-segmentation`, C-1/C-6), needed by the T3 properties and reused by T4.
  - The signature differs from C-5's `project(segment, reveal_set)` only in also taking the text and a per-segment `&[bool]` slice (`RevealSet::for_segment`).
- Tests: 24 new (22 in `projection::tests`, 2 in `doc::tests`).
  - Literal tests on fixture lines: heading+bold concealed and revealed under N1/N2 (I-7: revealed markers carry heading size and bold), G1 vs G2 heading, all six heading sizes, `***both***` concealed/N1/N2, unclosed/escaped/literal-`\n` plain, fence and gap segments, empty and marker-only headings, closing-sequence and indented headings, affinity tables across concealed markers, trailing marker with and without an EOL byte, Unicode, span splitting, the I-6 checker including a deliberately violated case.
  - P1, P2, P4, P5 (plus spans contiguous/maximal/char-boundary, a per-byte style oracle, and visible-text consistency) are checked by `check_projection`. P3 is checked by `check_frame_invariant`. Both run: at every valid Normal cursor and Insert bar of all three fixtures under all four policies; on 1000 random documents with random valid cursors under all four policies (proptest); at every cursor of 500 small random documents; on every reveal-flag combination of four construct-heavy lines; and on 500 random documents with arbitrary flags (P4 for reveal sets no cursor would produce). Alphabet is the T3 alphabet plus `**`, `# `, `*a*` atoms. Result: all pass, no counterexample.
- Verification: `cargo fmt --all -- --check` ✓, `cargo clippy --all-targets -- -D warnings` ✓, `cargo test` ✓ (90 passed, ~1 s). Mutation check (not committed): off-by-one collapse point, swapped affinities, ignoring heading size, and a reveal that never reveals a Normal inline cursor each made several tests fail; the last one is only caught by the P3 checks.
- MANUAL: none (no GUI surface in T3).
- Notes:
  - One interpretation call, flagged for the developer (see F-10): the run table has one `Concealed` run per marker range and does not merge adjacent concealed markers. C-5 does not say whether runs are maximal.
  - The exhaustive fixture check memoizes on `(segment index, reveal flags)`: a projection depends only on those, so repeating one adds nothing, and it cut the suite from ~20 s to ~1 s with the same coverage. I-6 is still checked at every cursor.
  - `projection.rs` and `doc.rs` carry a module-level `#![allow(dead_code)]` like `parse.rs` and `reveal.rs`; all come off once T4+ consume the items. A `proptest-regressions/` directory created by my mutation runs was deleted, not committed (its seeds only failed against the mutated code).

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
- F-10 (T3): I-6 depends on how "concealed run" is read. With one run per marker range, I-6 holds at every valid cursor under all four policies. If adjacent concealed markers were merged into one maximal run, it would not: in `**a**__b__` an Insert bar at 5 reveals neither strong (strict rule, F-8), so the close `**` (3..5) and open `__` (5..7) are both concealed and adjacent; merged they form 3..7 with the bar strictly inside. Insert `Right` through the revealed closing `**` should reach that bar (T4 to confirm). Typing there would put text at the collapse point between two concealed markers. If the developer wants the maximal-run reading, that needs a contract change (H3), not a bug fix — evidence: test `adjacent_concealed_markers_at_a_bar_boundary`.
- F-11 (T3): an Insert bar at `len` when the document ends with `\n` (e.g. after `abc\n`, the start of a new last line) belongs to no segment: `src_to_vis(seg.end)` is `None` for a segment with an EOL byte, because that offset belongs to the next segment and there is none. C-6 allows the bar there and C-11 has no phantom segment or row for it, so T6 (bar drawing, `VisualPos`) and T8 (Enter at the end of the document) must decide how it renders — evidence: test `trailing_concealed_marker_splits_the_end_affinities` (`src_to_vis(8) == None`).
- F-12 (T3): the mapping properties P1–P5 and the I-6 frame invariant hold at every valid Normal cursor and Insert bar of the three fixtures under G1/G2 × N1/N2, and on the random documents above — evidence: `fixtures_hold_every_property_at_every_valid_cursor`, `all_properties_with_random_valid_cursors`, `every_cursor_on_small_documents`.

## Halts
