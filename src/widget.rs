//! The document widget (C-11): per-segment paragraph layout, `ParagraphCache` and drawing.

use std::collections::HashMap;
use std::collections::hash_map::{DefaultHasher, Entry};
use std::hash::{Hash, Hasher};
use std::ops::Range;

use iced::advanced::graphics::text::Paragraph as TextParagraph;
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer::{self, Quad};
use iced::advanced::text::{
    Alignment, LineHeight, Paragraph as _, Renderer as _, Shaping, Span, Text, Wrapping,
};
use iced::advanced::widget::{Tree, tree};
use iced::advanced::{Clipboard, Renderer as _, Shell, Widget};
use iced::font::{Style as FontStyle, Weight};
use iced::{
    Border, Color, Element, Event, Font, Length, Pixels, Point, Rectangle, Size, Theme, alignment,
    keyboard, mouse,
};

use crate::app::{Message, Toggle};
use crate::parse::SegmentKind;
use crate::projection::{Projection, Style};

/// Width used when the parent offers an unbounded one.
const FALLBACK_WIDTH: f32 = 800.0;

// ---- ParagraphCache ---------------------------------------------------------------

/// Hits and misses of one layout pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PassStats {
    pub hits: usize,
    pub misses: usize,
}

struct CacheEntry {
    paragraph: TextParagraph,
    used: bool,
}

/// Content-addressed paragraph cache: the key hashes the visible text, the spans, the
/// base size and the width bits, so identity is not part of it (C-11). It is a plain
/// struct with no widget types, so the headless bench can reuse it (T10).
#[derive(Default)]
pub struct ParagraphCache {
    entries: HashMap<u64, CacheEntry>,
    current: PassStats,
}

impl ParagraphCache {
    /// Starts a layout pass: nothing has been used yet and the counters restart.
    pub fn begin_pass(&mut self) {
        for entry in self.entries.values_mut() {
            entry.used = false;
        }
        self.current = PassStats::default();
    }

    /// The paragraph for `projection` laid out at `width`, built only on a miss.
    pub fn paragraph(&mut self, projection: &Projection, width: f32) -> TextParagraph {
        match self.entries.entry(cache_key(projection, width)) {
            Entry::Occupied(mut hit) => {
                self.current.hits += 1;
                hit.get_mut().used = true;
                hit.get().paragraph.clone()
            }
            Entry::Vacant(slot) => {
                self.current.misses += 1;
                let paragraph = build_paragraph(projection, width);
                slot.insert(CacheEntry {
                    paragraph: paragraph.clone(),
                    used: true,
                });
                paragraph
            }
        }
    }

    /// Ends the pass: entries not used during it are evicted. Returns the pass counters.
    pub fn end_pass(&mut self) -> PassStats {
        self.entries.retain(|_, entry| entry.used);
        self.current
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

fn cache_key(p: &Projection, width: f32) -> u64 {
    let mut hasher = DefaultHasher::new();
    p.visible.hash(&mut hasher);
    for span in &p.spans {
        span.range.start.hash(&mut hasher);
        span.range.end.hash(&mut hasher);
        span.style.size.to_bits().hash(&mut hasher);
        span.style.bold.hash(&mut hasher);
        span.style.italic.hash(&mut hasher);
    }
    p.base_size.to_bits().hash(&mut hasher);
    width.to_bits().hash(&mut hasher);
    hasher.finish()
}

fn font_for(style: &Style) -> Font {
    Font {
        weight: if style.bold {
            Weight::Bold
        } else {
            Weight::Normal
        },
        style: if style.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
        ..Font::DEFAULT
    }
}

/// A zero-width character that gives the last row a line to live on (see `build_paragraph`).
const ROW_ANCHOR: &str = "\u{200B}";

/// One paragraph per segment: bounds `(width, ∞)`, word wrapping, advanced shaping.
pub fn build_paragraph(p: &Projection, width: f32) -> TextParagraph {
    // I-7: size, weight and slant come from the projection's spans, so a revealed marker
    // renders in its construct's style and a heading's `#` is heading-sized.
    let mut spans: Vec<Span<'_, (), Font>> = p
        .spans
        .iter()
        .map(|s| {
            let text = p
                .visible
                .get(s.range.clone())
                .expect("spike: span on char boundaries");
            Span::new(text).size(s.style.size).font(font_for(&s.style))
        })
        .collect();
    // cosmic-text drops the empty line after a final `\n`, but that row exists (C-6): a Gap
    // of k source lines has k rows, the last one holding the EOL cell. Anchor it with a
    // zero-width character so the paragraph has the row. It is not part of `visible`, so
    // no source or visible offset moves, and it adds no width (F-20).
    if p.visible.ends_with('\n') {
        spans.push(Span::new(ROW_ANCHOR).size(Pixels(p.base_size)));
    }
    TextParagraph::with_spans(Text {
        content: spans.as_slice(),
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(p.base_size),
        line_height: LineHeight::default(),
        font: Font::DEFAULT,
        align_x: Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::Word,
    })
}

/// One line at `base_size`: the minimum height of any segment, including empty ones.
fn one_line(base_size: f32) -> f32 {
    LineHeight::default().to_absolute(Pixels(base_size)).0
}

/// Segment height: the paragraph's own height, but never less than one line (C-11).
fn segment_height(min_bounds_height: f32, base_size: f32) -> f32 {
    min_bounds_height.max(one_line(base_size))
}

// ---- layout ---------------------------------------------------------------------------

pub struct SegmentLayout {
    /// Top of the segment, relative to the widget.
    pub y: f32,
    pub height: f32,
    pub paragraph: TextParagraph,
}

#[derive(Default)]
pub struct DocumentLayout {
    pub segments: Vec<SegmentLayout>,
    pub height: f32,
    pub stats: PassStats,
}

/// Lays out every segment through `cache`, stacked with zero spacing (C-11). A pure
/// function of its inputs plus the cache, so it runs headlessly.
pub fn layout_document(
    cache: &mut ParagraphCache,
    projections: &[Projection],
    width: f32,
) -> DocumentLayout {
    cache.begin_pass();
    let mut y = 0.0;
    let mut segments = Vec::with_capacity(projections.len());
    for p in projections {
        let paragraph = cache.paragraph(p, width);
        // An empty segment still has one row of its base line height.
        let height = segment_height(paragraph.min_bounds().height, p.base_size);
        segments.push(SegmentLayout {
            y,
            height,
            paragraph,
        });
        y += height;
    }
    let stats = cache.end_pass();
    DocumentLayout {
        segments,
        height: y,
        stats,
    }
}

/// The segments intersecting the vertical span `top..bottom` (widget coordinates).
pub fn visible_segments(layout: &DocumentLayout, top: f32, bottom: f32) -> Range<usize> {
    let start = layout.segments.partition_point(|s| s.y + s.height <= top);
    let end = layout.segments.partition_point(|s| s.y < bottom);
    start..end.max(start)
}

// ---- the widget -------------------------------------------------------------------------

#[derive(Default)]
struct State {
    cache: ParagraphCache,
    layout: DocumentLayout,
}

/// The single custom document widget (C-11). Segments are laid out and drawn here; the
/// cursor, keys other than F3 and click arrive in later tasks.
pub struct DocumentView {
    projections: Vec<Projection>,
    kinds: Vec<SegmentKind>,
    overlay: bool,
}

impl DocumentView {
    pub fn new(projections: Vec<Projection>, kinds: Vec<SegmentKind>, overlay: bool) -> Self {
        Self {
            projections,
            kinds,
            overlay,
        }
    }
}

fn kind_label(kind: SegmentKind) -> String {
    match kind {
        SegmentKind::Paragraph => "Paragraph".to_string(),
        SegmentKind::Heading { level } => format!("Heading {level}"),
        SegmentKind::Raw => "Raw".to_string(),
        SegmentKind::Gap => "Gap".to_string(),
    }
}

fn overlay_text(content: String, bounds: Size, align_x: Alignment) -> Text<String, Font> {
    Text {
        content,
        bounds,
        size: Pixels(10.0),
        line_height: LineHeight::default(),
        font: Font::MONOSPACE,
        align_x,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::None,
    }
}

impl Widget<Message, Theme, iced::Renderer> for DocumentView {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fill, Length::Shrink)
    }

    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(State::default())
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let state = tree.state.downcast_mut::<State>();
        let width = match limits.max().width {
            w if w.is_finite() => w,
            _ => FALLBACK_WIDTH,
        };
        state.layout = layout_document(&mut state.cache, &self.projections, width);
        let size = limits.resolve(
            Length::Fill,
            Length::Shrink,
            Size::new(width, state.layout.height),
        );
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        _tree: &mut Tree,
        event: &Event,
        _layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        if let Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(keyboard::key::Named::F3),
            ..
        }) = event
        {
            shell.publish(Message::Toggle(Toggle::F3));
            shell.capture_event();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let state = tree.state.downcast_ref::<State>();

        // Only segments that intersect the viewport are drawn. `viewport` and `bounds`
        // share the content coordinate space inside a scrollable.
        let top = viewport.y - bounds.y;
        let bottom = top + viewport.height;
        let drawn = visible_segments(&state.layout, top, bottom);
        for row in &state.layout.segments[drawn.clone()] {
            renderer.fill_paragraph(
                &row.paragraph,
                Point::new(bounds.x, bounds.y + row.y),
                style.text_color,
                *viewport,
            );
        }

        if self.overlay {
            self.draw_overlay(renderer, state, bounds, viewport, drawn);
        }
    }
}

impl DocumentView {
    /// F3: an outline and kind label per visible segment, and the cache counters.
    fn draw_overlay(
        &self,
        renderer: &mut iced::Renderer,
        state: &State,
        bounds: Rectangle,
        viewport: &Rectangle,
        drawn: Range<usize>,
    ) {
        let outline = Color::from_rgba(0.1, 0.4, 0.9, 0.55);
        let label = Color::from_rgba(0.1, 0.3, 0.8, 0.95);
        let stats = state.layout.stats;
        let summary = format!(
            "cache hits {}  misses {}  entries {}\nsegments {}  drawn {}",
            stats.hits,
            stats.misses,
            state.cache.len(),
            state.layout.segments.len(),
            drawn.len(),
        );

        // A separate layer, so the quads sit above the paragraph text.
        renderer.with_layer(*viewport, |renderer| {
            for i in drawn.clone() {
                let row = &state.layout.segments[i];
                let rect = Rectangle::new(
                    Point::new(bounds.x, bounds.y + row.y),
                    Size::new(bounds.width, row.height),
                );
                renderer.fill_quad(
                    Quad {
                        bounds: rect,
                        border: Border {
                            color: outline,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        ..Quad::default()
                    },
                    Color::TRANSPARENT,
                );
                let kind = self
                    .kinds
                    .get(i)
                    .copied()
                    .map_or_else(|| "?".to_string(), kind_label);
                let label_size = Size::new(120.0, 12.0);
                renderer.fill_text(
                    overlay_text(format!("{i}: {kind}"), label_size, Alignment::Right),
                    Point::new(rect.x + rect.width - label_size.width - 2.0, rect.y + 1.0),
                    label,
                    *viewport,
                );
            }

            let panel = Rectangle::new(
                Point::new(viewport.x + 4.0, viewport.y + 4.0),
                Size::new(240.0, 34.0),
            );
            renderer.fill_quad(
                Quad {
                    bounds: panel,
                    ..Quad::default()
                },
                Color::from_rgba(0.0, 0.0, 0.0, 0.75),
            );
            renderer.fill_text(
                overlay_text(summary, Size::new(232.0, 28.0), Alignment::Left),
                Point::new(panel.x + 4.0, panel.y + 3.0),
                Color::WHITE,
                *viewport,
            );
        });
    }
}

impl<'a> From<DocumentView> for Element<'a, Message, Theme, iced::Renderer> {
    fn from(view: DocumentView) -> Self {
        Element::new(view)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use crate::projection::project_all;
    use crate::reveal::{HeadingPolicy, Mode, NestingPolicy, RevealPolicy, reveal};

    const POLICY: RevealPolicy = RevealPolicy {
        heading: HeadingPolicy::G1,
        nesting: NestingPolicy::N1,
    };

    fn projections(text: &str, cursor: usize) -> Vec<Projection> {
        let segs = parse(text, 0).segments;
        let set = reveal(&segs, cursor, Mode::Normal, POLICY);
        project_all(text, &segs, &set)
    }

    fn spike() -> Vec<Projection> {
        projections(include_str!("../fixtures/spike.md"), 0)
    }

    /// Number of visual rows (layout runs) of a paragraph.
    fn rows(p: &TextParagraph) -> usize {
        p.buffer().layout_runs().count()
    }

    // ---- ParagraphCache ------------------------------------------------------------

    #[test]
    fn repeated_passes_with_no_change_are_all_hits() {
        let projs = spike();
        let mut cache = ParagraphCache::default();
        let first = layout_document(&mut cache, &projs, 700.0);
        assert!(first.stats.misses > 0);
        assert_eq!(first.stats.hits + first.stats.misses, projs.len());
        let entries = cache.len();

        for _ in 0..3 {
            let again = layout_document(&mut cache, &projs, 700.0);
            assert_eq!(again.stats.misses, 0, "an unchanged pass must not reshape");
            assert_eq!(again.stats.hits, projs.len());
            assert_eq!(cache.len(), entries);
        }
    }

    #[test]
    fn a_new_width_misses_everything_and_evicts_the_old_width() {
        let projs = spike();
        let mut cache = ParagraphCache::default();
        let first = layout_document(&mut cache, &projs, 700.0);
        let entries = cache.len();
        // Resizing rewraps: every distinct paragraph is rebuilt, none is reused.
        let resized = layout_document(&mut cache, &projs, 450.0);
        assert_eq!(resized.stats.misses, first.stats.misses);
        assert_eq!(resized.stats.hits, first.stats.hits);
        // Entries of the old width were not used in this pass, so they are gone.
        assert_eq!(cache.len(), entries);
        // Going back is a miss again, not a stale hit.
        let back = layout_document(&mut cache, &projs, 700.0);
        assert_eq!(back.stats.misses, first.stats.misses);
    }

    #[test]
    fn entries_not_used_in_a_pass_are_evicted() {
        let mut cache = ParagraphCache::default();
        let both = projections("one\n\ntwo\n", 99);
        layout_document(&mut cache, &both, 500.0);
        let before = cache.len();
        assert!(before >= 2);
        let only_one = projections("one\n", 99);
        let pass = layout_document(&mut cache, &only_one, 500.0);
        assert_eq!(pass.stats, PassStats { hits: 1, misses: 0 });
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn the_cache_is_content_addressed_so_equal_segments_share_an_entry() {
        // Two identical paragraphs and two identical gaps: one paragraph per distinct content.
        let projs = projections("same\n\nother\n\nsame\n\n\n\nother\n", 99);
        let mut cache = ParagraphCache::default();
        let pass = layout_document(&mut cache, &projs, 500.0);
        assert!(pass.stats.hits >= 2, "{:?}", pass.stats);
        assert_eq!(cache.len(), pass.stats.misses);
        assert!(cache.len() < projs.len());
    }

    #[test]
    fn a_style_change_is_a_different_key() {
        // Same visible text, different spans: the bold one must not hit the plain entry.
        let plain = projections("word\n", 99);
        let bold = projections("**word**\n", 99);
        assert_eq!(plain[0].visible, bold[0].visible);
        let mut cache = ParagraphCache::default();
        layout_document(&mut cache, &plain, 500.0);
        let pass = layout_document(&mut cache, &bold, 500.0);
        assert_eq!(pass.stats, PassStats { hits: 0, misses: 1 });
    }

    // ---- layout ---------------------------------------------------------------------

    #[test]
    fn segments_are_stacked_with_zero_spacing() {
        let projs = spike();
        let layout = layout_document(&mut ParagraphCache::default(), &projs, 700.0);
        assert_eq!(layout.segments.len(), projs.len());
        let mut y = 0.0;
        for (s, p) in layout.segments.iter().zip(&projs) {
            assert_eq!(s.y, y);
            assert!(s.height >= one_line(p.base_size));
            assert!(s.height >= s.paragraph.min_bounds().height);
            y += s.height;
        }
        assert_eq!(layout.height, y);
    }

    #[test]
    fn an_empty_segment_still_has_one_row_of_its_base_height() {
        // A concealed `##` leaves no visible text, but its row keeps the heading's line height.
        let projs = projections("##\n\nx\n", 99);
        assert_eq!(projs[0].visible, "");
        let layout = layout_document(&mut ParagraphCache::default(), &projs, 500.0);
        assert_eq!(layout.segments[0].height, one_line(projs[0].base_size));
        assert!(layout.segments[0].height > one_line(14.0));
    }

    #[test]
    fn segment_height_is_never_less_than_one_line() {
        // cosmic-text gives empty text one row, so this guard is not reachable through a
        // real paragraph; feed it a zero height directly.
        for base in [14.0, 22.4, 28.0] {
            assert_eq!(segment_height(0.0, base), one_line(base));
            assert_eq!(
                segment_height(one_line(base) * 3.0, base),
                one_line(base) * 3.0
            );
        }
    }

    #[test]
    fn headings_are_taller_than_body_text_and_levels_order() {
        let projs = projections("# h\n\n## h\n\n###### h\n\nbody\n", 99);
        let layout = layout_document(&mut ParagraphCache::default(), &projs, 500.0);
        let heights: Vec<f32> = layout
            .segments
            .iter()
            .filter(|s| s.height > 0.0)
            .map(|s| s.height)
            .collect();
        // Segments: H1, gap, H2, gap, H6, gap, paragraph.
        let (h1, h2, h6, body) = (heights[0], heights[2], heights[4], heights[6]);
        assert!(h1 > h2 && h2 > h6, "{h1} {h2} {h6}");
        assert!(h6 >= body, "{h6} {body}");
    }

    #[test]
    fn newlines_are_hard_rows_and_literal_backslash_n_is_text() {
        // Gap `\n\n\n` is three rows; a paragraph with an internal newline is two.
        let projs = projections("a\nb\n\n\n\nc \\n d\n", 99);
        let layout = layout_document(&mut ParagraphCache::default(), &projs, 500.0);
        assert_eq!(rows(&layout.segments[0].paragraph), 2, "a / b");
        assert_eq!(projs[1].visible, "\n\n");
        assert_eq!(rows(&layout.segments[1].paragraph), 3, "three blank rows");
        assert_eq!(projs[2].visible, "c \\n d");
        assert_eq!(
            rows(&layout.segments[2].paragraph),
            1,
            "literal \\n is text"
        );
    }

    #[test]
    fn a_gap_of_k_source_lines_has_k_rows_and_the_last_row_is_zero_width() {
        // F-20: cosmic-text alone would give k-1 rows for k >= 2.
        for k in 1..=6usize {
            let text = format!("x\n{}y\n", "\n".repeat(k));
            let projs = projections(&text, 99);
            assert_eq!(projs[1].visible, "\n".repeat(k - 1), "k={k}");
            let layout = layout_document(&mut ParagraphCache::default(), &projs, 500.0);
            let gap = &layout.segments[1];
            assert_eq!(rows(&gap.paragraph), k, "k={k}");
            assert!(
                (gap.height - k as f32 * one_line(14.0)).abs() < 0.01,
                "k={k}"
            );
            // The anchor adds no width, so the EOL cell of the last row sits at x = 0.
            let last = gap
                .paragraph
                .buffer()
                .layout_runs()
                .last()
                .expect("test: a gap has rows");
            assert!(last.line_w < 0.5, "k={k}: last row is {} wide", last.line_w);
        }
    }

    #[test]
    fn blank_lines_inside_a_raw_segment_are_rows_without_an_anchor() {
        let projs = projections("```\na\n\nb\n```\n", 99);
        assert_eq!(projs.len(), 1);
        assert_eq!(projs[0].visible, "```\na\n\nb\n```");
        let layout = layout_document(&mut ParagraphCache::default(), &projs, 500.0);
        assert_eq!(rows(&layout.segments[0].paragraph), 5);
    }

    #[test]
    fn narrowing_the_width_rewraps_a_long_paragraph() {
        let projs = spike();
        let long = projs
            .iter()
            .position(|p| p.visible.starts_with("This paragraph is deliberately long"))
            .expect("test: the long paragraph");
        let wide = layout_document(&mut ParagraphCache::default(), &projs, 1000.0);
        let narrow = layout_document(&mut ParagraphCache::default(), &projs, 300.0);
        assert!(
            rows(&narrow.segments[long].paragraph) > rows(&wide.segments[long].paragraph),
            "narrower width must produce more rows"
        );
        assert!(narrow.height > wide.height);
    }

    #[test]
    fn spans_become_styled_runs_of_the_paragraph() {
        let projs = projections("a **bold** and *it* b\n", 99);
        let p = &projs[0];
        assert_eq!(p.spans.len(), 5);
        let paragraph = build_paragraph(p, 500.0);
        // Every span maps to bounds in the laid-out paragraph.
        for i in 0..p.spans.len() {
            assert!(!paragraph.span_bounds(i).is_empty(), "span {i}");
        }
        // Bold text is not narrower than the same text in regular weight.
        let regular = build_paragraph(&projections("bold\n", 99)[0], 500.0);
        let bold = build_paragraph(&projections("**bold**\n", 99)[0], 500.0);
        assert!(bold.min_bounds().width >= regular.min_bounds().width);
    }

    // ---- viewport culling ---------------------------------------------------------------

    #[test]
    fn culling_selects_exactly_the_intersecting_segments() {
        let projs = spike();
        let layout = layout_document(&mut ParagraphCache::default(), &projs, 700.0);
        let n = layout.segments.len();
        assert_eq!(visible_segments(&layout, 0.0, layout.height), 0..n);
        assert_eq!(visible_segments(&layout, -50.0, layout.height + 50.0), 0..n);

        for (top, bottom) in [
            (0.0, 1.0),
            (40.0, 90.0),
            (layout.height - 5.0, layout.height),
        ] {
            let got = visible_segments(&layout, top, bottom);
            // Brute-force oracle: open-interval intersection.
            let want: Vec<usize> = (0..n)
                .filter(|&i| {
                    let s = &layout.segments[i];
                    s.y + s.height > top && s.y < bottom
                })
                .collect();
            assert_eq!(got.clone().collect::<Vec<_>>(), want, "{top}..{bottom}");
            assert!(!got.is_empty());
            assert!(got.len() < n, "{top}..{bottom} should cull something");
        }
        // Nothing above or below the document.
        assert!(visible_segments(&layout, -100.0, 0.0).is_empty());
        assert!(visible_segments(&layout, layout.height, layout.height + 100.0).is_empty());
    }

    #[test]
    fn culling_an_empty_layout_is_empty() {
        let layout = DocumentLayout::default();
        assert!(visible_segments(&layout, 0.0, 100.0).is_empty());
    }

    // ---- the real widget, headless ---------------------------------------------------------

    #[test]
    fn simulator_lays_out_draws_and_publishes_f3() {
        use crate::app::App;
        let app = App::new(include_str!("../fixtures/spike.md").to_string());
        let mut ui = iced_test::simulator(app.view());
        // Draw through the real renderer: layout, culling and fill_paragraph must not panic.
        ui.snapshot(&Theme::Light).expect("test: snapshot");
        ui.tap_key(keyboard::key::Named::F3);
        let messages: Vec<Message> = ui.into_messages().collect();
        assert!(
            matches!(messages.as_slice(), [Message::Toggle(Toggle::F3)]),
            "{messages:?}"
        );
    }
}
