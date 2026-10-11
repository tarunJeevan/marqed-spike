# Contracts
 
## C-1 Coordinates
 
- **Stored state.** App state stores only:
  - the cursor/bar as a source byte offset on a grapheme boundary;
  - the rope;
  - `version: u64`;
  - mode, debug flags and identity state;
  - `goal_x: Option<f32>`, the one exception — transient view state.
  Visible offsets and pixels are recomputed every frame.
- **Grapheme boundaries** come from `unicode-segmentation` over the per-parse `String` snapshot.

## C-2 Segments (parse output)
 
`parse(&str, version) -> ParseResult { version, segments: Vec<Segment> }`.
 
- **Top-level blocks.** These are the depth-0 `Start`/`End` pairs from `into_offset_iter`.
- **Normalization.**
  - Extend each block's start to the start of its first line.
  - Extend each block's end to just past its last line's `\n`, or to EOF.
  - If two normalized ranges share any line, merge them into one `Raw` segment.
- **Gaps.** Bytes not covered by any normalized block become `Gap` segments. Gaps therefore consist of whole lines only, such as blank or whitespace-only runs.
- **Kinds.**
  - `Paragraph`.
  - `Heading { level }` — ATX only: the first non-space byte of the segment is `#`.
  - `Raw` — every other block: setext headings, lists, fences, quotes, tables, HTML, thematic breaks.
  - `Gap`.
- **Each segment has:**
  - `range`;
  - `kind`;
  - `content_hash: u64` — `std::hash::DefaultHasher` over the segment's source bytes only, so it does not depend on position or kind;
  - `constructs` (C-3; empty for Raw and Gap).
  There is **no id field.** Parse output is identity-free; ids live in App (C-10).
- **EOL byte.** A segment's final byte, if it is `\n`, is its **EOL byte**. It is addressable but is never part of visible text.
- **Partition invariant.** Segments are ordered, non-overlapping and cover `[0, len)` exactly. An empty document has zero segments.

## C-3 Constructs and markers
 
Constructs exist only in `Paragraph` and `Heading` segments. They form a tree by range containment, with a parent index stored.
 
- **Inline constructs.** Only `Strong` and `Emphasis`, at any depth. Their range `s..e` is the pulldown event range.
  - `Strong` markers are `[s, s+2)` and `[e-2, e)`.
  - `Emphasis` markers are `[s, s+1)` and `[e-1, e)`.
  - Every marker byte must be `*` or `_`. If not, drop the construct, render it plain, and record the case in a test.
  - **Do not** derive markers as "range minus children". Continuation-line indentation would be misclassified as markers.
  - Every other inline element — code spans, links, strikethrough, escapes, HTML — produces no construct. Its bytes are plain visible text.
- **Heading construct.**
  - Its range is the segment range minus the EOL byte.
  - The opening marker is `[line_start, content_start)`, where `content_start` is the start of the first child event's range. With no children, it is the whole line content.
  - The closing marker is `[content_end, line_end)`, where `content_end` is the end of the last child event's range. It covers any closing `#` sequence and trailing whitespace, and may be empty.
  - `hash_run` is the contiguous `#` run inside the opening marker.
- **No marker range includes a `\n`.**

## C-4 Reveal
 
`reveal(segments, cursor_offset, mode, policy) -> RevealSet`. The policy is `{ heading: G1 | G2, nesting: N1 | N2 }`.
 
**Each construct's own reveal condition** (`o` is the cursor or bar offset):
 
| Construct | Normal (block cursor) | Insert (bar) |
|---|---|---|
| Inline `[s,e)` | `s ≤ o < e` | `s < o < e` |
| Heading, G1 (line) | `seg.start ≤ o < seg.end` (includes the EOL byte) | `seg.start ≤ o ≤ line_end` (`line_end` = EOL byte offset, or `seg.end`) |
| Heading, G2 (marker) | `o` ∈ opening ∪ closing marker ranges | `line_start ≤ o ≤ hash_run.end`, **or** `o` strictly inside the opening or closing marker range |
 
The G2 Insert rule is there because `##` on its own is a valid empty ATX heading. The bar must keep `##` visible while it is being typed, and must conceal once the space is typed (design flow step 5).
 
**Nesting policies:**
 
- **N1 (innermost):** of the constructs whose own condition holds, reveal only the one with the smallest range.
- **N2 (ancestor chain):** reveal every construct whose own condition holds, plus all ancestors of each.

## C-5 Projection and mapping
 
`project(segment, reveal_set) -> Projection`. It produces visible text, style spans and a run table.
 
- **Concealed bytes** are the union of the marker ranges of the segment's constructs that are not revealed.
- **Visible text** is the segment's bytes minus the concealed bytes and minus the EOL byte. Every other byte is kept, including internal `\n` bytes.
- **Run table.** An ordered list of `Visible { src, vis_start } | Concealed { src } | Eol { src }` that covers `segment.range` exactly.
- **Styles (I-7):**
  - Base size is `14.0`, or `14.0 × [2.0, 1.6, 1.3, 1.15, 1.05, 1.0][level-1]` for every byte of a Heading segment.
  - Bold applies inside any `Strong` range; italic inside any `Emphasis` range.
  - Revealed markers therefore carry their construct's style automatically.
  - Spans are maximal runs of equal style.
- **`src_to_vis(o)`:**
  - in a Visible run: `vis_start + (o - src.start)`;
  - in a Concealed run: the visible length preceding the run (the collapse point);
  - the EOL byte: `visible_len`, flagged `eol`.
- **`vis_to_src(v, affinity)`:**
  - If no concealed run sits between `v`'s neighbours, the result is unique.
  - Otherwise, `Before` gives the offset immediately after the preceding visible byte, and `After` gives the offset of the following visible byte.
  - At `v == visible_len`: `Before` gives the offset after the last visible byte; `After` gives the EOL byte offset, or `seg.end` if there is no EOL byte.
- **`VisualPos`** is `(segment index, visible byte, affinity)`. Pixels are derived from it on demand (C-8).

## C-6 Cursor model, frame invariant, newline rule
 
- **Normal cursor.** The first byte of a grapheme in `[0, len)`, or `0` if the document is empty. `\n` counts as a grapheme (an EOL cell).
- **Insert bar.** A grapheme boundary in `[0, len]`.
- **I-6 frame invariant.** Under the projection computed *for the current cursor*:
  - the Normal cursor's byte is never concealed;
  - the Insert bar is never strictly inside a concealed run.
  Check it with `debug_assert!` every frame, and property-test it (T3, T4).
- **Snapping contract.** Motions, `j`/`k` and clicks are resolved against the projection *before* the move. The reveal and reflow that follow are accepted as-is.
- **Newline rule.**
  - A segment-internal `\n` is a hard line in that segment's paragraph.
  - An EOL byte is the end of the segment's last row.
  - Each Gap line is a row.
  - A literal `\` followed by `n` renders as the text `\n`.
- **Modes.** The only transitions are:
  - Normal `i` → Insert, with the bar at the cursor offset;
  - Insert `Esc` → Normal, with the cursor at the grapheme starting at the bar if `bar < len`, else at the last grapheme, else `0`.
  `:` and every other key do nothing.

## C-7 Horizontal motions (pure, module `motion`)
 
These are computed against the projection for the current cursor. A grapheme counts as visible if its first byte is not concealed; EOL cells are visible.
 
- **Normal `h`/`l`** move to the previous or next visible grapheme start, crossing segments in document order. No-op at either end of the document.
- **Insert `Left`/`Right`:**
  - `Right` moves to the boundary immediately after the next visible grapheme after the bar.
  - `Left` moves to the boundary immediately before the previous visible grapheme before the bar.
  - No-op at the ends.
- **Every horizontal motion, edit or click clears `goal_x`.**

## C-8 Geometry, vertical motion, click (module `geometry` plus the widget)
 
- **Geometry functions** (from the concrete paragraph's `cosmic_text::Buffer`):
  - `vis → (row, x, row_top, row_height)`: use a line-start table to map the visible byte to `(BufferLine, byte-in-line)`, then `layout_runs()` to find the run and glyph, giving the x position. A byte at end of line gives the run's `line_w`.
  - `point → (visible byte, affinity)`: use `Buffer::hit`, then the line-start table.
  - An empty-text segment still has one row of its base line height at `x = 0`.
- **Document rows** are all layout runs of all segments, ordered by document y (`segment y + line_top`).
- **Vertical motion.** Normal `j`/`k` and Insert `Down`/`Up` move to the row below or above, across segments and gap rows. No-op at the first or last row.
  - If `goal_x` is `None`, first set it to the cursor's current x.
  - On the target row, Normal mode picks the grapheme whose `[x, x+w)` contains `goal_x`. Failing that, it picks the nearest grapheme; past the line end that is the EOL cell or the last grapheme.
  - Insert mode picks the boundary nearest `goal_x`, with affinity per C-5.
  - `goal_x` persists across consecutive vertical moves.
- **Message routing.** The widget resolves these motions and publishes `Message::MoveTo { offset, goal_x }`. App checks that the offset is a grapheme boundary (`expect` with an invariant message) and applies it.
- **Click.** Only active while the debug click toggle (F4) is on. A left press resolves to the grapheme under the point in Normal mode, or to a boundary with affinity in Insert mode. It publishes `MoveTo { offset, goal_x: None }`.

## C-9 Editing and the reparse loop
 
- **Insert-mode keys.**
  - A `KeyPressed` with `text: Some(t)`, no ctrl/alt/logo modifier, and no control characters in `t` inserts `t` at the bar; the bar advances by `t.len()`.
  - `Backspace` deletes the source grapheme immediately before the bar, regardless of concealment. Deleting a concealed marker degrades the construct to source — that is intended.
  - `Enter` inserts `"\n"`.
  - Everything else is ignored.
- **After every edit:**
  1. `version += 1`.
  2. Snapshot the rope to a `String`.
  3. Run `parse`.
  4. App applies the result only if `result.version == version`, and otherwise discards it (I-8 shape).
  5. Run the identity match (C-10).
- **No undo, no save, no Command mode.**

## C-10 Block identity (module `identity`, pure)
 
- **Ids.** `BlockId(u64)` comes from a monotonic counter in App. Ids are assigned to every segment, held in a `Vec<BlockId>` parallel to the segments.
- **Kind comparison** uses the discriminant only: Paragraph, Heading (level ignored), Raw, Gap.
- **Strategy L (the architecture's literal order: position → kind → content hash).** For each new segment, in order:
  1. Take the unused old segments whose ranges overlap the new range.
  2. Keep only those of the same kind.
  3. Prefer an equal `content_hash`; otherwise take the first remaining candidate.
  4. If none remain, assign a fresh id.
- **Strategy C (content-first).**
  1. Trim the common prefix and suffix of the `(kind, hash)` sequences.
  2. Run an LCS on the middle to get anchors.
  3. Within each gap between consecutive anchors, pair unmatched old and new segments in order where the kinds are equal, greedy and order-preserving.
  4. Everything left over gets a fresh id.
- **Expected-outcome contract.** It applies to non-Gap segments; Gap ids are only checked for uniqueness.

| # | Case | Expected |
|---|---|---|
| 1 | Insert a block above B (`A,B,C` → `A,X,B,C`) | A, B, C keep their ids; X fresh |
| 2 | Split a paragraph P into P1 and P2 (Enter twice mid-paragraph) | P1 keeps P's id; P2 fresh |
| 3 | Join P1 and P2 into P | P takes P1's id; P2's id is retired |
| 4 | Edit in place (same kind, new hash) | Keeps its id |
| 5a | Paragraph → heading, same text | Fresh id |
| 5b | H2 → H3 | Keeps its id |
| 6 | `A,X,X,B` → `A,X,Y,X,B` (duplicates) | Both Xs keep their own ids, in order; Y fresh |
| 7 | Delete a block | Remaining blocks keep their ids |
| 8 | Swap two adjacent distinct blocks | No duplicate ids; at least one block keeps its id |
 
- **Invariants** (proptest over random edit sequences on generated documents):
  - ids are unique within a frame;
  - matched pairs have the same kind;
  - matched pairs preserve order (no crossings);
  - segments in an untouched common prefix or suffix keep their ids.
- **The matrix test (`identity_matrix`)** runs every case through both strategies and prints a pass/fail matrix (visible with `cargo test identity_matrix -- --nocapture`).
  - It asserts that Strategy C passes every case.
  - Strategy L outcomes are printed and recorded only, not asserted.
  - If Strategy C fails a case, that is H3: report it, don't tune the case to pass.
- **Live display.** F5 toggles the active strategy; the debug overlay shows each segment's id.

## C-11 Rendering (module `widget`)
 
- **`DocumentView`.** A single custom widget, wrapped in `scrollable`. Its inputs are:
  - per-segment projections, kinds and ids;
  - cursor and mode;
  - `goal_x`;
  - debug flags.
- **Layout.**
  - Width is the available width.
  - One concrete paragraph per segment, built with `with_spans`, bounds `(width, ∞)`, `Wrapping::Word`, `Shaping::Advanced`.
  - Segment height is `max(min_bounds.height, one line at its base size)`.
  - Segments are stacked with zero spacing; gaps render as empty rows.
- **`ParagraphCache`.**
  - A plain struct, so it is reusable headlessly in T10, stored in the widget's tree state.
  - It maps `key → Paragraph`, where the key hashes the visible text, the spans, the base size and the width bits.
  - It is content-addressed: identity is not part of the key.
  - Entries not used during a layout pass are evicted at the end of the pass.
  - It counts hits and misses per pass.
- **Draw.**
  - Only segments that intersect the viewport are drawn.
  - The Normal cursor is a translucent quad over the grapheme's bounds; an EOL cell is the width of a space at that size.
  - The Insert bar is a 2 px quad of the row's height.
- **Keys** are handled in the widget's `update` and published as semantic messages:
  - `MoveLeft` and `MoveRight` (App resolves these through `motion`);
  - `MoveTo`, `EnterInsert`, `ExitInsert`, `InsertText(String)`, `Backspace`, `Newline`;
  - `Toggle(F1..F5)`.
- **Debug toggles.**

| Key | Toggles |
|---|---|
| F1 | Heading policy G1 ↔ G2 |
| F2 | Nesting policy N1 ↔ N2 |
| F3 | Debug overlay |
| F4 | Click enabled |
| F5 | Identity strategy L ↔ C |
 
  - The overlay shows: segment kind and id, a tick at each concealed collapse point, cache hits and misses, the last perf line, and the active policies.
  - The window title or a status line shows mode and policies.

## C-12 Perf instrumentation
 
- **Per edit, App times** the String snapshot, parse, identity, reveal and projection, and the total. The widget's layout reports reshapes, cache hits and layout time.
- **Output.** Write one line per edit to stderr, and show the latest line in the overlay.
- **Headless bench mode:** `marqed-spike <file> --repeat N --bench K`.
  - It runs the full pipeline without a window, including paragraph construction through `ParagraphCache`, for K single-character inserts at the document midpoint followed by K at the end.
  - It prints the median and max per stage, plus the initial full-layout time, then exits.
