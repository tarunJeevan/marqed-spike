# marqed projection spike - implementation goal

## 0. Context
 
`marqed` is a planned Rust markdown editor. It combines Helix-style modal editing with Marktext-style live in-place rendering: markdown markers conceal when the cursor is outside a construct and reveal (keeping styling) when the cursor enters it.
 
This repository is a **throwaway spike**. It decides go/no-go for that rendering model on `iced 0.14.0`, and it produces evidence for four open design questions:
 
1. the source↔visual mapping / snapping contract;
2. heading reveal granularity;
3. nested-construct reveal;
4. the block-identity heuristic.

You build the mechanism, the tests, the debug toggles and the measurements. The developer judges subjective "feel" and makes all decisions. Never pick a winner between policies; record evidence.

The developer has already decided the following:
 
- **Mouse:** click hit-testing is a debug aid only, behind a toggle. Hit-testing is exercised through `j`/`k` vertical motion.
- **Minimal Insert mode is in scope:** typing, Backspace, Enter, arrow keys, Esc.
- **Newlines:** every `\n` byte in the source renders as a visual line break, with no CommonMark soft-break-as-space. The two-character text `\` `n` is ordinary text.
- **Widget structure:** a single custom document widget with an internal per-segment paragraph cache. No `keyed_column`.

## 1. Ground rules
 
- **Stack.** Use exactly the dependencies in `Cargo.toml`:
  - `iced =0.14.0` with the `advanced` feature;
  - `pulldown-cmark =0.13.4` with no default features;
  - `ropey =1.6.1`;
  - `unicode-segmentation 1.13`;
  - dev-dependencies `proptest 1.11` and `iced_test =0.14.0`.

  Adding, removing or re-versioning a dependency is a halt condition (H4).

- **Runtime.** No threads, no async, and no `unsafe`.
- **Panics.**
  - No bare `unwrap()`.
  - `expect("…")` is allowed only for internal invariants. Its message names the invariant, e.g. `expect("I-6: cursor on concealed byte")` or `expect("spike: segment partition")`.
  - Nothing may panic on any document content or any key sequence.
- **Offsets.**
  - All persistent offsets are source byte offsets into the rope (I-1).
  - Rope byte↔char conversion goes through ropey APIs.
  - Text is sliced only at offsets already validated as char boundaries.
- **Comments.** Code implementing a marqed invariant carries a comment naming it: `// I-1`, `// I-6`, `// I-7`, `// I-8`.
- **Waived for the spike.** Doc-comment coverage and typed error enums. `eprintln!` plus a non-zero exit is fine for CLI errors.
- **Tests.**
  - Unit and property tests live inline in each module (`#[cfg(test)] mod tests`).
  - Geometry tests assert relations (ordering, line membership, round trips), never absolute pixel values.
- **Every task ends green:**

```bash
  cargo fmt --all -- --check
  cargo clippy --all-targets -- -D warnings
  cargo test
```

## 2. Verified API surface (iced 0.14.0 / cosmic-text 0.15.0 / pulldown-cmark 0.13.4)
 
These were read from the published crate sources; use them as given. For anything not listed here, check the 0.14.0 source or docs before using it. If it doesn't exist as you expected, that is halt condition H1. Do not substitute APIs from another iced version.
 
- **Text paragraphs.**
  - `iced::advanced::text::Paragraph` (trait): `with_spans`, `resize`, `min_bounds`, `hit_test`, `hit_span`, `span_bounds`, `grapheme_position`.
  - **Do not use** `hit_test` or `grapheme_position` for mapping. `hit_test` drops the line index, and `grapheme_position` takes a layout-run index plus a grapheme count.
- **Concrete paragraph.** `iced::advanced::graphics::text::Paragraph` is `iced::Renderer`'s paragraph type. It has `buffer() -> &cosmic_text::Buffer`. cosmic-text is re-exported at `iced::advanced::graphics::text::cosmic_text`.
  - Write the widget as `impl Widget<Message, Theme, iced::Renderer>`. Do not make it generic over the renderer.
- **cosmic-text.**
  - `Buffer::hit(x, y) -> Option<Cursor { line, index /* byte in BufferLine */, affinity }>`.
  - `Buffer::layout_runs()` yields `LayoutRun { line_i, text, glyphs: &[LayoutGlyph { start, end, x, w, .. }], line_top, line_height, line_y, line_w, .. }`.
- **Widget trait.**
  - `Widget::update(&mut self, tree, &Event, layout, cursor, renderer, clipboard, shell, viewport)` returns `()`.
  - Use `shell.publish(msg)`, `shell.capture_event()`, `shell.request_redraw()` and `shell.invalidate_layout()`.
- **Drawing.** `text::Renderer::fill_paragraph(&paragraph, position, color, clip_bounds)` and `Renderer::fill_quad(quad, background)`.
- **Keyboard.** `keyboard::Event::KeyPressed { key, modified_key, physical_key, location, modifiers, text: Option<SmolStr>, repeat }`.
- **Application.** `iced::application(boot, update, view)` — boot is a function, not a title.
- **Shaping.** `text::Shaping` defaults to `Auto` (basic shaping for ASCII). The spike always sets `Shaping::Advanced` explicitly.
- **iced_test.** `simulator(element)`, `click`, `tap_key`, `typewrite`, `into_messages`.
- **pulldown-cmark.**
  - `Parser::new_ext(text, opts).into_offset_iter()`.
  - Flags: `ENABLE_TABLES | ENABLE_STRIKETHROUGH | ENABLE_TASKLISTS | ENABLE_GFM`.
  - It requires a contiguous `&str`. Snapshot the rope to a `String` per parse and measure that cost (T10).

## 5. Verification
 
```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test identity_matrix -- --nocapture
cargo run --release -- fixtures/spike.md --repeat <N> --bench 50
cargo run -- fixtures/spike.md        # manual checks are the developer's (§8)
```
 
## 6. Halt conditions — stop, report, and do not work around
 
- **H1.** A required API from §2 is missing or behaves differently. Report the compiler error and the source location. No vendoring, forking, `unsafe`, or version changes.
- **H2.** Geometry needs anything beyond the public `Paragraph::buffer()` surface. Treat this as no-go evidence: write `VERDICT.md` with it and stop.
- **H3.** A contract is contradictory or unimplementable, or a property fails in a way that needs a contract change rather than a bug fix. Report the minimal counterexample. Do not edit contracts or weaken tests.
- **H4.** A dependency would need to be added or changed.
- **H5.** A test can only pass by being weakened, ignored or deleted.

## 7. Out of scope
 
- Command mode, undo, save, file watching, config;
- `w`/`b` and all other motions;
- concealing anything except Strong, Emphasis and ATX headings (escapes, links, code spans and the rest render as plain source);
- CRLF handling (fixtures are LF-only);
- IME;
- auto-scroll to the cursor;
- syntax highlighting;
- tracing;
- any optimization beyond `ParagraphCache` and viewport culling;
- changing the marqed production repository.

## 8. `VERDICT.md` template
 
```markdown
# Projection spike verdict — <date>
 
## Objective evidence (agent)

- Mapping properties P1–P5 and I-6 invariant: <pass/fail, cases run, any shrunk counterexamples>
- Geometry via Paragraph::buffer(): <works / limits hit>
- Identity matrix (L vs C): <table pasted from identity_matrix>
- Perf (release, <lines> lines): initial layout <ms>; per-edit median/max — snapshot, parse, identity, reveal+project, layout/reshapes, total
- iced_test Simulator smoke: <result / blocker>
- Findings: <G2 marker reachability, N1 outer-marker reachability, Backspace into concealed markers,
  ***x*** behaviour, escapes render as source, String-snapshot cost, anything else observed>
 
## Manual check script (developer)

1. Open fixtures/spike.md. Move with h/l through `**bold**`, `***both***`, and bold inside the H2 under each F1/F2 combination.
2. Insert: type `## Roadmap` on a new line, then `**important**`; watch reveal/conceal at each key under G1 and G2.
3. j/k through the wrapping paragraph, the headings and the blank-line runs; check that goal_x is sticky.
4. F3 overlay: confirm ids stay stable while typing (F5 to compare L vs C); confirm reveal only reflows its own block.
 
## Developer verdict (developer fills in)

- Go / no-go:
- Heading granularity (G1/G2):
- Nesting (N1/N2):
- Identity strategy:
 
## Draft decision-log entries (for developer approval)

<one draft per item: spike verdict; nested reveal; heading granularity; identity heuristic
 (incl. identity as optimization hint?); Block has no id — ids held by ui; VisualPos + snapping contract;
 vertical motion as a ui geometry query with transient goal_x>
```

