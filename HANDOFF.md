# HANDOFF — start of T7 (written at the end of T6, 2026-10-18)

Throwaway note for the next session. Everything recorded elsewhere is only pointed to. Delete it once T7 is signed off, if the developer agrees.

## Read first (per CLAUDE.md)
`docs/SPIKE_GOAL.md`, `docs/CONTRACTS.md`, `docs/PROGRESS.md`, then `docs/tasks/T7.md` and the C-8 vertical-motion paragraphs. State at hand-off: T0–T6 done and signed off (the developer commits and merges all code themselves — do not commit; stop and report after T7 for their review). `PROGRESS.md` has no open halt.

## Where T7 plugs in (names only; read the code for details)
- `src/geometry.rs` — `SegmentGeometry::{rows, locate, cell, hit}`, `Row`, `Hit::affinity`. Per segment only. **There is no document-level row list yet**: T7 builds it from `DocumentLayout.segments[i].y` plus each segment's `rows()` (`widget.rs`). The tail row (F-27) is not a layout row, so `j`/`Down` from the last row is a no-op by construction.
- `src/widget.rs` — `cursor_rect` and `resolve_click` are the pattern to copy: pure functions over `(DocumentLayout, &[Projection], source, mode, …)`, fully testable headlessly, returning offsets already passed through `doc::normalize_position`. Suggested shape: a pure `vertical_target(...) -> Option<(offset, goal_x)>` used by both `DocumentView::update` and the tests. `DocumentView::new` does not take `goal_x` yet (C-11 lists it as an input); `key_message` has no `j`/`k`/Up/Down yet.
- `src/app.rs` — `Message::MoveTo { offset, goal_x }` already exists and sets `goal_x` (and panics on an invalid position by design, I-1). `goal_x` is stored but has a targeted `#[allow(dead_code)]` to remove when T7 reads it. Rules already implemented: horizontal motion clears it; `Esc` clears it only if the cursor moves; click publishes `None`. Mode and policy live in `reveal.rs`.
- `Projection::{src_to_vis, vis_to_src(v, Affinity)}` and `normalize_position` turn a chosen visible position back into a valid source offset. Use `Hit::affinity`'s rule (click x left of the boundary's x → `Before`) as the model for Insert-mode affinity.

## Things the contracts leave open that T7 will hit
- C-8 says a Normal target past the line end is "the EOL cell or the last grapheme". `resolve_click` already decided: past the end of an internal hard line → that line's `\n` cell; past the last row of a segment with an EOL byte → the EOL byte; a segment with no EOL byte → its last grapheme. Reuse that
  unless the developer says otherwise.
- Which x is `goal_x`? Suggested: the cursor rect's left edge (`cell.x` in Normal, bar x in Insert), in widget coordinates (independent of scroll; every segment starts at x = 0). Heading rows are taller but that does not change x.
- Soft-wrap boundaries (F-26, F-23): a boundary with no swallowed space belongs to the row that starts there, so an Insert target "at the end" of a non-last wrapped row cannot be expressed; boundaries inside swallowed whitespace share one position (use the first). Decide and test rather than rediscover.

## Facts about iced 0.14 / cosmic-text not (fully) in PROGRESS findings
- `Buffer::hit(x, y)`: bounds are inclusive; at or right of a glyph's midpoint it returns the glyph's end, otherwise its start; clusters are split equally per grapheme; past the row end it returns the last glyph's end; above the first run it returns (line, 0); an empty line returns index 0; it can only return glyph starts/ends (never a byte inside swallowed whitespace — F-23). `LayoutRunIter` silently stops at an unshaped line (not a problem for iced-built paragraphs).
- `scrollable` hands the child's `update`/`draw` coordinates in content space, and `cursor.position_in(layout.bounds())` is already content-relative; `viewport` is in the same space.
- Quads draw under text within a layer; use `renderer.with_layer` to put something above text.
- Pixel widths depend on installed fonts (this machine: Noto Sans). Tests must sweep widths or compare relations, never absolute pixels (SPIKE_GOAL rule). `build_paragraph` is `pub` for geometry tests.

## Testing and tooling conventions that cost time to rediscover
- `iced_test`: `iced_test::simulator(app.view())`, `.tap_key(Key)`, `.point_at(Point)`, `.simulate(iced_test::simulator::click())` (not `iced_test::click`), `.snapshot(&Theme::Light)`, `.into_messages()`. The simulator never calls `App::update`; build a new one per state.
- Exhaustive property tests stay fast with a shared `ParagraphCache` and by memoizing on `(segment, reveal flags)`; the full suite is ~8 s in debug. Test helpers to copy: `projections(text, cursor)`, `at(cache, text, cursor, mode, width)`, `check_every_rectangle`, `check_click_round_trip` in `widget.rs` tests; the T3/T4 alphabet plus `**`, `# `, `*a*`, regional indicators.
- `prop_assert!(…)` breaks on a first argument containing `{ .. }` (`matches!(x, Foo { .. })`, `if {…}`): bind it to a variable first.
- Mutation checks (do one per behavior, never commit): copy the file to the scratchpad, `sed -i`, confirm the file actually changed (`cmp -s`; `rustfmt` reflows lines, so patterns miss), run `cargo test`, restore from the copy, and finish with `rm -rf proptest-regressions` (failing runs write seeds there). Compare literals, not the constant under test (the bar-width mutation survived once because of that).
- Launch check: `timeout 8 ./target/debug/marqed-spike fixtures/spike.md` — exit 124 with empty stderr means it stayed open (the per-frame I-6 `debug_assert!` is live in debug builds).

## Environment and process gotchas
- The Bash tool's shell does not word-split an unquoted `$var`, so build argument lists explicitly. Never `pkill -f <name>`: the pattern matches the command line of your own shell.
- Use the Edit/Write tools for file changes. The developer has noticed Python and `printf` edits; `PROGRESS.md` records those slips. `PROGRESS.md` Findings and Log are append-only (editing a past finding's text is a rule violation); only the Status block and checkboxes are rewritable.
- The Edit tool requires the file to have been read with the Read tool in the session first; files also change on disk between your edits (rustfmt, the developer), so re-Read before dependent edits.
- Reply style that worked: report evidence and flag decisions where the contracts were silent; never pick a policy winner (the developer's G1/N1 preference is recorded as a remark, not a verdict).
