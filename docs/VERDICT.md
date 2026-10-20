# Projection spike verdict — 2026-10-20

Written after T10, as T11 requires. Everything under "Objective evidence" is measured or tested by the agent and points at a test, a log entry or a finding in `docs/PROGRESS.md` (F-n). It records evidence; it does not pick a winner between policies, and the go/no-go is left to the developer. Visual and "feel" checks are the developer's and are listed under "Manual check script".

State of the repository when this was written: branch `task-11`, `cargo fmt --all -- --check` ✓, `cargo clippy --all-targets -- -D warnings` ✓, `cargo test` ✓ (241 passed, 0 failed, 0 ignored), `cargo test identity_matrix -- --nocapture` ✓, `cargo run --release -- fixtures/spike.md --repeat 110 --bench 50` ✓ (exit 0). All of T0 to T10 are ticked in `PROGRESS.md`; the only halt of the spike (H3 at T6) is resolved; H1, H2, H4 and H5 never fired. Ground rules, checked against the tree: no `unsafe`, no `unwrap()`, no threads or async, and `Cargo.toml` unchanged since the skeleton commits (no dependency changes).

## Objective evidence (agent)

### Mapping properties P1–P5 and the I-6 invariant: pass, no counterexample

- **What is checked.** P1 `vis → src → vis` is the identity; P2 `src_to_vis` is monotone; P3 the I-6 frame invariant (a Normal cursor is never on a concealed byte; an Insert bar is never strictly inside a concealed run); P4 the run table covers the segment range exactly for every reveal set; P5 interleaving the runs reconstructs the source bytes exactly (T3). Plus literal tests on fixture lines, a per-byte style oracle, and span checks.
- **Cases run (T3).** At every valid Normal cursor and Insert bar of all three fixtures under G1/G2 × N1/N2; 1,000 random documents with random valid cursors under all four policies (proptest, alphabet `a é e\u{301} 👍🏽 space * _ # \n \ n` plus `**`, `# `, `*a*`); every cursor of 500 small random documents; every reveal-flag combination of four construct-heavy lines; 500 random documents with arbitrary reveal flags (P4 on sets no cursor would produce). Result: all pass (F-12).
- **I-6 beyond T3.** It is also a `debug_assert!` in `App::view`, so every test that builds a view re-checks it each frame. After a move it is checked again in: 1,000 random horizontal-motion sequences of up to 29 motions with an independent brute-force oracle (T4, F-19); the vertical-motion oracle at every step (T7, F-36); 500 random Insert sessions (T8, F-41); 300 random key sequences of the `goal_x` rules (T7).
- **Shrunk counterexamples.** None for P1–P5 or I-6.
- **One reading that matters (F-10, F-18).** I-6 holds with one concealed run per marker range. If adjacent concealed markers were merged into one maximal run, it would not: in `**a**__b__` an Insert bar at 5 reveals neither strong, so the closing `**` and the opening `__` are adjacent and concealed, and the bar would sit strictly inside the merged run. Insert `Right` does reach that bar (F-18). Choosing the maximal-run reading would need a contract change.

### Geometry via `Paragraph::buffer()`: works; limits hit

- **Surface used.** Only `paragraph.buffer()` and, on it, `Buffer::layout_runs()` and `Buffer::hit()` (grep of `src/geometry.rs`). `hit_test` and `grapheme_position` are not used anywhere. H2 never fired, and no API in the verified surface differed from its documentation (F-22).
- **Works.** `vis → (row, x, row top, height)`, a cell per grapheme, `point → (visible byte, affinity)`, document rows across segments, the cursor block and bar, click resolution, and vertical motion are all built from it. Properties: every visible boundary lands back on its own pixel position (300 random documents, fixtures, hand-built mixed-size paragraphs, T6); every valid cursor and bar of the fixtures and 100 random documents round-trips through a click (T6); the widget works against the real renderer through `iced_test` (F-21).
- **Limits hit.**
  - *Swallowed whitespace (F-23, F-24).* cosmic-text emits no glyph for whitespace it swallows at a soft wrap, so those bytes are in no row's glyph list and `Buffer::hit` can never return them. The T6 task file's "every visible boundary round-trips `vis → point → vis`" is therefore unachievable for boundaries inside collapsed whitespace. This was the spike's one halt (H3 @ T6). The developer chose to restate the property modulo coincident positions: every boundary still lands back on its own pixel position, and exact identity is still required wherever no other boundary shares that position. In a 4,000-document enumeration 137 of 104,174 boundaries (0.13%) were affected, all coincident whitespace and none involving a non-whitespace byte.
  - *Empty trailing line (F-20).* cosmic-text's rich-text buffer drops the empty line after a final `\n`, so a Gap of k lines came out with k-1 rows. A zero-width anchor character (in the paragraph only, not in the visible text) restores the row.
  - *Wrap boundaries (F-25, F-26).* `Buffer::hit` returns the glyph's end at or right of its midpoint, which for the last glyph of a wrapped row is the next row's start; a boundary at a wrap with no swallowed whitespace belongs to the row that starts there, so the visual end of the upper row cannot be expressed for the bar.
  - *Wrapping and fonts (F-33).* `Wrapping::Word` never breaks a long run of letters (60 `a`s are one row at widths 50 to 100); breaks occur after `-` and `/` and between CJK characters. Pixel widths depend on the installed fonts (Noto Sans on this machine), so geometry tests assert relations and sweep widths, never absolute pixels.

### Identity matrix (L vs C)

`cargo test identity_matrix -- --nocapture` (strategy C is asserted to pass every case; L is printed only, as C-10 says):

```
case                                     L      C
1  insert a block above B               FAIL   pass
2  split a paragraph in two             FAIL   pass
3  join two paragraphs                  pass   pass
4  edit in place                        FAIL   pass
5a paragraph to heading                 pass   pass
5b H2 to H3                             pass   pass
6  duplicates: A,X,X,B to A,X,Y,X,B     FAIL   pass
7  delete a block                       FAIL   pass
8  swap two adjacent blocks             pass   pass
```

- **Why L fails (F-43).** L compares old and new byte ranges as raw offsets in two different coordinate systems (C-10 gives no mapping); any length-changing edit shifts every later block, and a block keeps its id only while its new range still overlaps its old one. Over 2,000 seeded single random edits, L lost the id of a block in an untouched common prefix or suffix 631 times (32%) and never crossed an order; C had no violation in the same 2,000 and in 1,000 random edit chains.
- **In ordinary typing (F-47).** A one-byte edit shifts later segments by one byte. Blocks stay overlapped and keep their ids under both strategies; each one-byte Gap only touches its old range, so L gives every Gap below the edit a fresh id on every keystroke and C keeps them. The matrix constrains non-Gap ids only. The developer saw exactly this in the overlay.
- **Moves (F-45).** Neither strategy detects a move. In a swap, C keeps one block by content and gives the other a fresh id; L hands each position's id to whatever now sits there.
- **Invariants for C (1,000 random edit chains, F-46 and T9 log).** Ids unique within a frame, matched pairs share a kind and keep their order, an untouched common prefix or suffix keeps its ids, and a fresh id is never reused. Through 500 random Insert sessions that also toggle F5, the app keeps one id per segment, unique, below the counter, and no retired id comes back.
- **Cost (F-50).** At about 3,600 segments, release: identity is 0.36 to 0.38 ms per edit under L and 0.028 ms under C (about 13 times cheaper); either is small next to parse.

### Performance (release, 5,168 lines)

`cargo build --release`, then `./target/release/marqed-spike fixtures/spike.md --repeat 110 --bench 50` (stdout of run 1 of 3; the three runs agree to about 10% on every median; this laptop had `rust-analyzer` running, so absolute numbers are not clean; times in ms):

```
bench: fixtures/spike.md --repeat 110
  143218 bytes, 5168 lines, 3629 segments; layout width 800 px; 50 single-character inserts at the midpoint (byte 71609), then 50 at the end
  times in ms; reshapes = paragraphs shaped by the layout; cold first layout of the document, fonts not yet loaded (excluded below): 32.274 ms

strategy L
  initial full layout: 6.361 ms (22 paragraphs shaped for 3629 segments)
  final text: 143318 bytes (started at 143218)
  phase     stat        snapshot        parse     identity  reveal+proj    app total       layout  edit+layout     reshapes
  midpoint  median         0.037        2.081        0.381        1.703        4.261        0.590        4.849        1.000
  midpoint  max            0.131        2.785        0.424        2.559        5.773        3.568        7.882        1.000
  end       median         0.038        2.054        0.363        1.700        4.217        0.596        4.817        1.000
  end       max            0.046        2.307        0.401        1.940        4.953        0.658        5.584        1.000

strategy C
  initial full layout: 6.297 ms (22 paragraphs shaped for 3629 segments)
  final text: 143318 bytes (started at 143218)
  phase     stat        snapshot        parse     identity  reveal+proj    app total       layout  edit+layout     reshapes
  midpoint  median         0.036        2.045        0.028        1.689        3.853        0.574        4.424        1.000
  midpoint  max            0.042        2.102        0.039        1.749        3.930        0.596        4.508        1.000
  end       median         0.035        2.054        0.028        1.711        3.884        0.591        4.473        1.000
  end       max            0.044        2.165        0.034        2.295        4.481        0.643        5.085        1.000
```

- **Initial layout.** 6.3 ms is flattering (F-51): the 110 copies of `spike.md` are identical and the cache is content-addressed, so 3,629 segments shaped only 22 paragraphs. A generated all-distinct document (5,320 lines, 3,800 segments, scratch file, not in the repository) shaped 2,281 paragraphs in 116 to 119 ms (about 0.05 ms each), which is also what a window resize costs because the width is in the cache key. Cold first layout (fonts loading): 32 ms for the repeated fixture, 182 to 189 ms for the distinct one (F-53).
- **Per edit (F-49).** Median edit plus layout 4.4 to 5.6 ms across runs and both inputs (L 4.8 to 5.6, C 4.4 to 5.2; the repeated fixture alone gives 4.4 to 5.2), the same at the midpoint and at the end, about a third of a 60 fps frame. Where it goes (L, run 1, midpoint median 4.85 ms): parse 2.08 ms (43%), reveal+projection 1.70 ms (35%), layout 0.59 ms (12%), identity 0.38 ms (8%), snapshot 0.04 ms (under 1%). Parse and reveal+projection are whole-document passes (pulldown needs the whole `String`; `project_all` runs for every segment, as C-1 requires each frame). Nothing was optimized (out of scope). The max (7.9 ms) is the first edit of a run.
- **Reshapes (F-52).** Median 1 paragraph per single-character edit in every row; max 2, where the edit also flips a neighbour's reveal state. Reveal reflows only its own block.

### `iced_test` Simulator smoke: works, no blocker

`simulator(app.view())` builds the App's real view (the custom `DocumentView` inside a `scrollable`), `snapshot` draws it through the real renderer, and `tap_key` for `l` yields exactly `[Message::MoveRight]` (test `iced_test_simulator_drives_the_custom_widget`, F-54). The same simulator drove clicks, `typewrite`, Backspace/Enter, arrows and the F-keys in the T6 to T8 tests (F-21).

### Findings

Each item has an F-number in `PROGRESS.md`, with the test that shows it.

- **G2 marker reachability (open question 2).**
  - Under G2 `h`/`l` cannot reach the markers of a plain heading: with the cursor on the text `## ` is concealed, so `h` skips to the line above and `l` from above lands on the text (F-14).
  - Once a Normal cursor starts in the markers (reachable by Esc) and steps out, it cannot come back: a one-way door (F-14).
  - Under G1 `h` walks onto the markers but `l` from the line above still skips them (F-14).
  - Insert `Left` from the bar after `## ` is a no-op under G2 (F-16).
  - With the Normal cursor on a heading line's EOL cell G2 conceals the markers and G1 reveals them (F-9).
  - Typing `## Roadmap`: the segment is a heading from the first `#`; G1 keeps it revealed at every key; G2 reveals it through `##` and conceals it from the space on, so the text appears without its hashes (F-6, F-38).
- **N1 outer-marker reachability (open question 3).**
  - Under N1 the outer markers of `***x***` (Emphasis around Strong) are unreachable by `h`/`l` from the text; under N2 both are reachable; from outside they are skipped under every policy (F-17).
  - Under N1 a heading's `#` stay concealed with the cursor inside bold even under G1, because only the innermost construct reveals (F-7).
  - In a heading whose first child is bold (`## **b** x`) the `## ` markers are reachable by `h` only under N2 (F-15).
- **Backspace into concealed markers (F-40).** With the bar right after a concealed closing `**` (`**b**|`, shown as `b`), Backspace deletes the hidden final `*`, as C-9 says; the strong is gone but the display only changes from `b` to `*b` (a literal `*` plus an emphasis `*b*` with concealed markers). A second Backspace leaves `**b`, plain source. The developer confirmed this in the T8 manual check as expected.
- **`***x***` behaviour (F-1, F-17).** pulldown nests it as Emphasis (outer) containing Strong (inner): markers `[0,1)`/`[9,10)` and `[1,3)`/`[7,9)` for `***both***`. N1 vs N2 reachability is above.
- **Escapes render as source.** Out of scope by `SPIKE_GOAL.md` §7. `\*not bold\*`, `C:\new\path` and the two-character text `\` `n` render as plain text and produce no construct. One quirk: for a heading whose first child is an escape (`# \*a`) pulldown's text event starts at the escaped character, so C-3's `content_start` rule puts the backslash inside the opening marker and it is concealed with the heading (F-2).
- **String-snapshot cost.** 0.035 to 0.046 ms median (0.13 to 0.23 ms max, the first edit of a run) for a 143 to 150 KB document of about 5,200 lines, under 1% of an edit; parse (2.1 ms) and the whole-document projection (1.7 ms) are the larger whole-document costs (F-49).
- **Anything else.**
  - *Typing and clusters (F-37).* C-9's "the bar advances by `t.len()`" can land inside a grapheme cluster when typed text merges with its neighbour; the bar goes to the end of that cluster. The developer accepted this.
  - *Typing bold (F-39).* No marker is concealed until the 12th key of `**important**`; `**important*` parses as a literal `*` plus an emphasis, so one visible `*` disappears, and at the 13th key the strong takes effect.
  - *Motions are not inverses (F-13).* In `a**b**c`, `l` from 0 jumps to 3 but `h` from 3 walks back through the now-revealed markers.
  - *Reveal by click or vertical motion (F-28, F-34).* A click, or an Insert `Up`/`Down`, on either side of a concealed run resolves to its two ends by the position relative to the collapse point; the side after the run puts the bar strictly inside the construct and reveals its markers.
  - *Tail row (F-11, F-27, F-29).* An Insert bar at the end of a text that ends in `\n` belongs to no segment; it is drawn on a one-line tail row below the last segment. Vertical motion treats that row as outside the document rows: `Up` leaves it, `Down` from the last text row does not enter it (the developer confirmed this).
  - *Vertical motion details (F-30 to F-32, F-35).* `goal_x` is the cell's left edge in Normal mode and the bar's boundary in Insert mode, with a 0.01 px tolerance at cell edges; coincident boundaries resolve to the first; a row with no landing spot is skipped; the visual end of a wrapped row with no swallowed whitespace is not a bar position.
  - *Parse quirks (F-3, F-4, F-5).* A setext heading whose text starts with `#` would be misread as ATX by C-2's literal rule (handled with a single-line check); line-sharing blocks that need the merge-to-Raw path occur in real input (`-\n aaaa`); link reference definitions produce a Gap that is not blank.
  - *Dead code (F-55).* When this report was written the five module-level `#![allow(dead_code)]` from T1 to T4 were still in the tree. Without them the build warned about five items, all test-only helpers except `VisualPos`, which is never constructed (the code passes segment, visible byte and affinity as separate values). Resolved afterwards on the `dead_code-fix` branch: the attributes are gone, the test-only helpers are `#[cfg(test)]`, and `VisualPos` was deleted at the developer's request (the tree now has no `allow` attributes at all).
  - *Test suite.* 241 tests; property tests and brute-force oracles for parse, reveal, projection, motion, geometry, vertical motion, editing and identity; mutation checks (not committed) at the end of T2 to T10, each caught, with the equivalent survivors named in the logs.

### Limits of this evidence

- Subjective feel, visual layout and the look of the overlay are the developer's; I could not see the GUI. The developer's manual checks were confirmed in the sign-offs of T0, T5, T6, T7, T8 and T9; T10 had none.
- One machine (Noto Sans, `rust-analyzer` running during the timings); absolute pixel and millisecond values depend on it. The tests assert relations, and the bench numbers are medians of repeated runs.
- The bench input is `spike.md` repeated (identical copies) plus one generated all-distinct file; neither is a real five-thousand-line note.
- Out of scope and not tested: CRLF, IME, auto-scroll, undo, save, other motions, concealing anything except Strong, Emphasis and ATX headings.
- The identity strategies were judged on the C-10 table and on random edit chains; real editing sessions beyond the developer's few manual edits were not run. The preference for C stated in the T9 review is the developer's subjective remark, not part of this evidence.

## Manual check script (developer)

1. Open fixtures/spike.md. Move with h/l through `**bold**`, `***both***`, and bold inside the H2 under each F1/F2 combination.
2. Insert: type `## Roadmap` on a new line, then `**important**`; watch reveal/conceal at each key under G1 and G2.
3. j/k through the wrapping paragraph, the headings and the blank-line runs; check that goal_x is sticky.
4. F3 overlay: confirm ids stay stable while typing (F5 to compare L vs C); confirm reveal only reflows its own block.

Evidence to compare against, per step: step 1 F-13, F-14, F-15, F-17; step 2 F-6, F-38, F-39; step 3 F-29 to F-32 and the stickiness tests; step 4 F-42 to F-47 and F-52 (the overlay shows `<index>: <kind> #<id>` and, after an edit, the layout time and the edit line).

## Developer verdict (developer fills in)

- Go / no-go: I think it's a go. The projection spike reveals the Marktext-style conceal/reveal rendering model is feasible (though parsing will need to be optimized to prevent lag on large documents).
- Heading granularity (G1/G2): G1 is the better option. G2 requires either extra motions to be able to reveal and land on the markers or to blindly delete via Backspace in Insert mode. 
- Nesting (N1/N2): N2 is the better option. Revealing both outer and inner markers makes it clear what I'm editing. 
- Identity strategy: There was no detectable difference in performance during usage, but strategy C (content-first) seems cleaner and more reliable at scale. 

## Draft decision-log entries (for developer approval)

Drafts only: each lists what the spike showed and what is left open. None of them is a decision, and none picks between policies.

**1. Spike verdict.**
- *Question:* does the Marktext-style conceal/reveal rendering model with Helix-style modal editing work on `iced 0.14.0`?
- *Evidence for:* a single custom widget with a per-segment content-addressed paragraph cache runs the whole model against the real iced 0.14.0 renderer; the source↔visual mapping, I-6 and geometry from `Paragraph::buffer()` hold over every property and oracle run; the full pipeline (parse, identity, reveal+projection, layout) costs about 4.4 to 5.6 ms per edit at 5,200 lines in release, with one paragraph reshaped per edit; headless testing works through `iced_test`; no halt other than the T6 property restatement (H3), no H1/H2.
- *Evidence of cost or limit:* the whole-document parse and projection per edit (about 3.8 ms of the 4.4 to 5.6); a full re-layout of a distinct 5k-line document on resize (116 to 119 ms); cosmic-text quirks that need workarounds (F-20, F-23, F-25, F-26); the one-way doors, non-inverse motions and click-reveal effects under the snapping contract.
- *Open for the developer:* go / no-go, and whether the whole-document passes need localizing before production.

**2. Nested construct reveal (N1 innermost, N2 ancestor chain).**
- *Evidence:* F-7, F-15, F-17. N1 reveals only the smallest holder, so the outer markers of `***x***` and a heading's `#` around bold are unreachable by `h`/`l`; N2 reveals every holder and its ancestors, so they are reachable and more markers show at once. Behaviour is identical for plain constructs.
- *Open for the developer:* N1 or N2, and whether unreachable outer markers are acceptable under N1.

**3. Heading reveal granularity (G1 line, G2 marker).**
- *Evidence:* F-6, F-9, F-14, F-16, F-38. G1 reveals through the whole line while the cursor is on it; G2 reveals only at the markers, so a Normal cursor on plain heading text cannot walk onto them (`h` skips them), Insert `Left` from after `## ` is a no-op, and typing `## ` hides the hashes from the space on. Characterization tests pin these defects as current behaviour.
- *Recorded, not a verdict:* the developer's T6 impression that the starting G1/N1 felt the most natural.
- *Open for the developer:* G1 or G2.

**4. Block-identity heuristic (including identity as an optimization hint?).**
- *Evidence:* the matrix above, F-43, F-47, F-45, F-50. C passes every case of the table and every invariant; L fails five cases (every failure a block after the edit renumbered) and renumbers every Gap below an edit in ordinary typing; neither detects a move. C is about 13 times cheaper per edit at 3,600 segments.
- *On identity as an optimization hint:* in this design the paragraph cache is keyed by content (visible text, spans, base size, width), not by id, and still reshapes exactly one paragraph per edit under either strategy (F-52). Ids were used only for display (title, overlay), so the spike has no case where an id improves a cache hit; whatever else ids would carry in production (stable UI state across edits) was not exercised.
- *Recorded, not a verdict:* the developer's T9 impression that C seems the better choice because its ids stay stable and predictable.
- *Open for the developer:* L or C, whether ids are kept at all, and whether an L with positions mapped through the edit is worth specifying.

**5. `Block` has no id; ids are held by the UI.**
- *Evidence:* `Segment` has no id field (C-2); parse output is identity-free and position-independent (`content_hash` over the segment's bytes only); `App` holds `Vec<BlockId>` parallel to the segments and a monotonic counter; `identity.rs` is a pure function of old and new parse output; ids stayed one-per-segment, unique and never reused through random sessions (F-46).
- *Open for the developer:* approve for the production architecture.

**6. `VisualPos` and the snapping contract.**
- *Evidence:* all persistent positions were source byte offsets (I-1); the only stored view coordinate is the transient `goal_x`; motions, `j`/`k` and clicks resolve against the projection computed before the move and the reveal and reflow that follow are accepted as-is, which held under every property. Consequences to weigh: `h`/`l` are not inverses (F-13); G2 has one-way doors (F-14); a click or vertical move can reveal a construct depending on which side of its start it lands (F-28, F-34); an Insert bar can sit between two adjacent concealed markers (F-10, F-18); typed text can merge into a cluster (F-37). The `VisualPos` type that C-5 names was never constructed: the code passes `(segment, visible byte, affinity)` as separate values and derives pixels on demand (F-55). Its affinity plays no part in deriving pixels (C-8's `vis → (row, x, …)` does not take one; it only matters going back from a visible byte to a source offset), which is part of why the type was not needed. The developer had it deleted from the code after this report was first written.
- *Open for the developer:* keep the snapping contract as specified. C-5's sentence defining `VisualPos` no longer matches the code; the developer is editing `CONTRACTS.md`.

**7. Vertical motion as a UI geometry query with a transient `goal_x`.**
- *Evidence:* a pure function over the per-segment paragraphs (document rows are the layout runs in y order; Normal picks the cell holding `goal_x` or the nearest, Insert the nearest boundary with the click's affinity rule) matched a brute-force oracle at every step of random walks (400 cases committed, 10,000 run once) and needed nothing beyond `Paragraph::buffer()`; `goal_x` stays sticky across short lines and is cleared by horizontal motion, edits and clicks; the developer confirmed one-row moves, stickiness and the mode-specific keys by hand. Details to settle: the tail row (F-29), the tolerance at cell edges (F-30), coincident boundaries (F-31), rows with no landing spot (F-32).
- *Open for the developer:* approve vertical motion as a UI query rather than a model operation, with `goal_x` as the one transient view coordinate.
