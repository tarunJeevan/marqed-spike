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

use crate::app::{DebugFlags, Message, Toggle};
use crate::doc::{normalize_position, prev_grapheme_boundary};
use crate::geometry::{SegmentGeometry, one_line};
use crate::parse::SegmentKind;
use crate::projection::{Affinity, BASE_SIZE, Projection, Run, Style};
use crate::reveal::Mode;

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
    paragraph_from_spans(&spans, p.base_size, width)
}

fn paragraph_from_spans(spans: &[Span<'_, (), Font>], base_size: f32, width: f32) -> TextParagraph {
    TextParagraph::with_spans(Text {
        content: spans,
        bounds: Size::new(width, f32::INFINITY),
        size: Pixels(base_size),
        line_height: LineHeight::default(),
        font: Font::DEFAULT,
        align_x: Alignment::Default,
        align_y: alignment::Vertical::Top,
        shaping: Shaping::Advanced,
        wrapping: Wrapping::Word,
    })
}

/// The advance of a space at `size`: the width of an EOL cell (C-11).
pub fn space_width(size: f32) -> f32 {
    let space = [Span::new(" ").size(Pixels(size))];
    paragraph_from_spans(&space, size, 4096.0)
        .min_bounds()
        .width
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
    /// Sum of the segment heights.
    pub height: f32,
    /// One base line below the last segment when the document ends in an EOL byte: the
    /// row an Insert bar at the end of the text sits on (F-11). Zero otherwise.
    pub tail: f32,
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
    let ends_in_eol = projections.last().is_some_and(|p| p.eol().is_some());
    DocumentLayout {
        segments,
        height: y,
        tail: if ends_in_eol {
            one_line(BASE_SIZE)
        } else {
            0.0
        },
        stats,
    }
}

/// The segments intersecting the vertical span `top..bottom` (widget coordinates).
pub fn visible_segments(layout: &DocumentLayout, top: f32, bottom: f32) -> Range<usize> {
    let start = layout.segments.partition_point(|s| s.y + s.height <= top);
    let end = layout.segments.partition_point(|s| s.y < bottom);
    start..end.max(start)
}

// ---- keys, cursor and click (pure) ----------------------------------------------------------

/// Width of the Insert bar (C-11).
const BAR_WIDTH: f32 = 2.0;

/// What a key press means (C-11 keys). `None` when the key does nothing in `mode`.
///
/// The F-keys toggle in either mode. Normal mode knows `h`, `l` and `i`; Insert mode knows
/// Escape and the Left/Right arrows. Everything else is ignored, including `:` (C-6).
pub fn key_message(
    mode: Mode,
    key: &keyboard::Key,
    modifiers: keyboard::Modifiers,
) -> Option<Message> {
    use keyboard::Key;
    use keyboard::key::Named;

    if let Key::Named(named) = key {
        let toggle = match named {
            Named::F1 => Some(Toggle::F1),
            Named::F2 => Some(Toggle::F2),
            Named::F3 => Some(Toggle::F3),
            Named::F4 => Some(Toggle::F4),
            Named::F5 => Some(Toggle::F5),
            _ => None,
        };
        if let Some(toggle) = toggle {
            return Some(Message::Toggle(toggle));
        }
    }
    if modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match (mode, key) {
        (Mode::Normal, Key::Character(c)) => match c.as_str() {
            "h" => Some(Message::MoveLeft),
            "l" => Some(Message::MoveRight),
            "i" => Some(Message::EnterInsert),
            _ => None,
        },
        (Mode::Insert, Key::Named(Named::Escape)) => Some(Message::ExitInsert),
        (Mode::Insert, Key::Named(Named::ArrowLeft)) => Some(Message::MoveLeft),
        (Mode::Insert, Key::Named(Named::ArrowRight)) => Some(Message::MoveRight),
        _ => None,
    }
}

/// The segment holding source offset `o`, and `o` as a visible byte of it. An offset on the
/// boundary between two segments belongs to the second; the end of the text belongs to the
/// last segment only if it has no EOL byte.
fn owner(projections: &[Projection], o: usize) -> Option<(usize, crate::projection::SrcVis)> {
    let after = projections.partition_point(|p| p.range.end <= o);
    [after, after.wrapping_sub(1)]
        .into_iter()
        .filter(|&i| i < projections.len())
        .find_map(|i| projections[i].src_to_vis(o).map(|v| (i, v)))
}

/// The cursor's rectangle in widget coordinates: the grapheme's cell in Normal mode (an EOL
/// cell is `space_width` at the segment's base size), a 2 px bar of the row's height in
/// Insert mode (C-11). `None` for an empty document.
///
/// An Insert bar at the end of a text that ends in an EOL byte has no segment; it is drawn
/// at the start of the tail row below the last segment (F-11).
// I-1: `cursor` is a source offset; the pixels are derived from it here, on demand.
pub fn cursor_rect(
    layout: &DocumentLayout,
    projections: &[Projection],
    cursor: usize,
    mode: Mode,
    space_width: &dyn Fn(f32) -> f32,
) -> Option<Rectangle> {
    let Some((si, at)) = owner(projections, cursor) else {
        let text_len = projections.last()?.range.end;
        return (mode == Mode::Insert && cursor == text_len && layout.tail > 0.0).then(|| {
            Rectangle::new(
                Point::new(0.0, layout.height),
                Size::new(BAR_WIDTH, layout.tail),
            )
        });
    };
    let (segment, p) = (&layout.segments[si], &projections[si]);
    let geometry = SegmentGeometry::new(&segment.paragraph, &p.visible, p.base_size);
    Some(match mode {
        Mode::Normal => {
            let cell = geometry.cell(at.vis, space_width(p.base_size));
            Rectangle::new(
                Point::new(cell.x, segment.y + cell.top),
                Size::new(cell.width, cell.height),
            )
        }
        Mode::Insert => {
            let place = geometry.locate(at.vis);
            Rectangle::new(
                Point::new((place.x - BAR_WIDTH / 2.0).max(0.0), segment.y + place.top),
                Size::new(BAR_WIDTH, place.height),
            )
        }
    })
}

/// A left click at `point` (widget coordinates) as a source offset valid for `mode` (C-8).
///
/// It is resolved against `projections`, the ones computed for the cursor before the click
/// (the snapping contract, C-6). Normal mode picks the grapheme under the point, Insert mode
/// the nearest boundary with the affinity from [`crate::geometry::Hit::affinity`]. The result is
/// snapped to a grapheme boundary of `source`, so it can never be an invalid position.
/// `None` when there is nothing to land on.
// I-1
pub fn resolve_click(
    layout: &DocumentLayout,
    projections: &[Projection],
    source: &str,
    mode: Mode,
    point: Point,
) -> Option<usize> {
    if layout.segments.is_empty() {
        return Some(0);
    }
    if mode == Mode::Insert && layout.tail > 0.0 && point.y >= layout.height {
        return Some(source.len()); // the tail row: the end of the text
    }
    let si = layout
        .segments
        .partition_point(|s| s.y + s.height <= point.y)
        .min(layout.segments.len() - 1);
    let (segment, p) = (&layout.segments[si], &projections[si]);
    let geometry = SegmentGeometry::new(&segment.paragraph, &p.visible, p.base_size);
    let hit = geometry.hit(Point::new(point.x, point.y - segment.y));

    let raw = match mode {
        Mode::Insert => p.vis_to_src(hit.vis, hit.affinity()),
        Mode::Normal => {
            // A point left of the boundary is over the grapheme before it, unless the
            // boundary starts the hard line.
            let line_start = geometry.line_starts()[geometry.line_of(hit.vis)];
            let vis = if hit.left_of_boundary && hit.vis > line_start {
                prev_grapheme_boundary(&p.visible, hit.vis).unwrap_or(hit.vis)
            } else {
                hit.vis
            };
            if vis < p.visible_len() {
                p.vis_to_src(vis, Affinity::After)
            } else if let Some(eol) = p.eol() {
                eol // the EOL cell
            } else {
                // No EOL byte: the last visible grapheme, if there is one.
                let last = prev_grapheme_boundary(&p.visible, vis)?;
                p.vis_to_src(last, Affinity::After)
            }
        }
    };
    Some(normalize_position(source, raw, mode))
}

// ---- the widget -------------------------------------------------------------------------

#[derive(Default)]
struct State {
    cache: ParagraphCache,
    layout: DocumentLayout,
    /// Space advance per base size (keyed by the size's bits), for EOL cells.
    space_widths: HashMap<u32, f32>,
}

impl State {
    fn space_width(&self, size: f32) -> f32 {
        self.space_widths
            .get(&size.to_bits())
            .copied()
            .unwrap_or(size * 0.3)
    }
}

/// The single custom document widget (C-11): layout, drawing, the cursor, keys and click.
pub struct DocumentView<'a> {
    source: &'a str,
    projections: Vec<Projection>,
    kinds: Vec<SegmentKind>,
    cursor: usize,
    mode: Mode,
    flags: DebugFlags,
}

impl<'a> DocumentView<'a> {
    pub fn new(
        source: &'a str,
        projections: Vec<Projection>,
        kinds: Vec<SegmentKind>,
        cursor: usize,
        mode: Mode,
        flags: DebugFlags,
    ) -> Self {
        Self {
            source,
            projections,
            kinds,
            cursor,
            mode,
            flags,
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

impl Widget<Message, Theme, iced::Renderer> for DocumentView<'_> {
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
        for p in &self.projections {
            state
                .space_widths
                .entry(p.base_size.to_bits())
                .or_insert_with(|| space_width(p.base_size));
        }
        let size = limits.resolve(
            Length::Fill,
            Length::Shrink,
            Size::new(width, state.layout.height + state.layout.tail),
        );
        layout::Node::new(size)
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        _viewport: &Rectangle,
    ) {
        match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key,
                modifiers,
                ..
            }) => {
                if let Some(message) = key_message(self.mode, modified_key, *modifiers) {
                    shell.publish(message);
                    shell.capture_event();
                }
            }
            // Click is a debug aid, active only under F4 (C-8).
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) if self.flags.click => {
                let state = tree.state.downcast_ref::<State>();
                let offset = cursor.position_in(layout.bounds()).and_then(|point| {
                    resolve_click(
                        &state.layout,
                        &self.projections,
                        self.source,
                        self.mode,
                        point,
                    )
                });
                if let Some(offset) = offset {
                    shell.publish(Message::MoveTo {
                        offset,
                        goal_x: None,
                    });
                    shell.capture_event();
                }
            }
            _ => {}
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

        // The cursor: a translucent block over the grapheme in Normal mode, a bar in Insert.
        let rect = cursor_rect(
            &state.layout,
            &self.projections,
            self.cursor,
            self.mode,
            &|size| state.space_width(size),
        );
        if let Some(rect) = rect {
            let rect = Rectangle {
                x: bounds.x + rect.x,
                y: bounds.y + rect.y,
                ..rect
            };
            if rect.intersects(viewport) {
                let color = match self.mode {
                    Mode::Normal => Color::from_rgba(0.15, 0.45, 0.95, 0.35),
                    Mode::Insert => Color::from_rgb(0.1, 0.3, 0.9),
                };
                renderer.fill_quad(
                    Quad {
                        bounds: rect,
                        ..Quad::default()
                    },
                    color,
                );
            }
        }

        if self.flags.overlay {
            self.draw_overlay(renderer, state, bounds, viewport, drawn);
        }
    }
}

impl DocumentView<'_> {
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
        let tick = Color::from_rgba(0.9, 0.1, 0.1, 0.8);
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

                // A tick at each concealed run's collapse point.
                let p = &self.projections[i];
                let geometry = SegmentGeometry::new(&row.paragraph, &p.visible, p.base_size);
                for run in &p.runs {
                    let Run::Concealed { src } = run else {
                        continue;
                    };
                    let Some(collapse) = p.src_to_vis(src.start) else {
                        continue;
                    };
                    let at = geometry.locate(collapse.vis);
                    renderer.fill_quad(
                        Quad {
                            bounds: Rectangle::new(
                                Point::new(bounds.x + at.x, bounds.y + row.y + at.top),
                                Size::new(1.0, at.height),
                            ),
                            ..Quad::default()
                        },
                        tick,
                    );
                }
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

impl<'a> From<DocumentView<'a>> for Element<'a, Message, Theme, iced::Renderer> {
    fn from(view: DocumentView<'a>) -> Self {
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

    // ---- keys -------------------------------------------------------------------------------

    use keyboard::Key;
    use keyboard::key::Named;

    fn ch(c: &str) -> Key {
        Key::Character(c.into())
    }

    fn msg(mode: Mode, key: &Key) -> Option<Message> {
        key_message(mode, key, keyboard::Modifiers::default())
    }

    #[test]
    fn normal_mode_keys() {
        assert_eq!(msg(Mode::Normal, &ch("h")), Some(Message::MoveLeft));
        assert_eq!(msg(Mode::Normal, &ch("l")), Some(Message::MoveRight));
        assert_eq!(msg(Mode::Normal, &ch("i")), Some(Message::EnterInsert));
        // Everything else does nothing: `:`, the unmapped motions, arrows and Escape.
        for key in [
            ch(":"),
            ch("j"),
            ch("k"),
            ch("w"),
            ch("H"),
            ch("L"),
            ch("I"),
            Key::Named(Named::Escape),
            Key::Named(Named::ArrowLeft),
            Key::Named(Named::ArrowRight),
            Key::Named(Named::Enter),
            Key::Named(Named::Backspace),
        ] {
            assert_eq!(msg(Mode::Normal, &key), None, "{key:?}");
        }
    }

    #[test]
    fn insert_mode_keys() {
        assert_eq!(
            msg(Mode::Insert, &Key::Named(Named::Escape)),
            Some(Message::ExitInsert)
        );
        assert_eq!(
            msg(Mode::Insert, &Key::Named(Named::ArrowLeft)),
            Some(Message::MoveLeft)
        );
        assert_eq!(
            msg(Mode::Insert, &Key::Named(Named::ArrowRight)),
            Some(Message::MoveRight)
        );
        // Typed text is ignored until T8: no `h`/`l`/`i` motions in Insert mode.
        for key in [
            ch("h"),
            ch("l"),
            ch("i"),
            ch("a"),
            ch(":"),
            Key::Named(Named::Enter),
        ] {
            assert_eq!(msg(Mode::Insert, &key), None, "{key:?}");
        }
    }

    #[test]
    fn f_keys_toggle_in_both_modes_and_with_modifiers() {
        for (named, toggle) in [
            (Named::F1, Toggle::F1),
            (Named::F2, Toggle::F2),
            (Named::F3, Toggle::F3),
            (Named::F4, Toggle::F4),
            (Named::F5, Toggle::F5),
        ] {
            for mode in [Mode::Normal, Mode::Insert] {
                assert_eq!(
                    msg(mode, &Key::Named(named)),
                    Some(Message::Toggle(toggle)),
                    "{named:?} {mode:?}"
                );
                assert_eq!(
                    key_message(mode, &Key::Named(named), keyboard::Modifiers::CTRL),
                    Some(Message::Toggle(toggle))
                );
            }
        }
    }

    #[test]
    fn ctrl_alt_and_logo_turn_letters_into_nothing() {
        for modifiers in [
            keyboard::Modifiers::CTRL,
            keyboard::Modifiers::ALT,
            keyboard::Modifiers::LOGO,
        ] {
            for letter in ["h", "l", "i"] {
                assert_eq!(key_message(Mode::Normal, &ch(letter), modifiers), None);
            }
            assert_eq!(
                key_message(Mode::Insert, &Key::Named(Named::Escape), modifiers),
                None
            );
        }
    }

    // ---- cursor rectangles ------------------------------------------------------------------

    fn projections_for(text: &str, cursor: usize, mode: Mode) -> Vec<Projection> {
        let segs = parse(text, 0).segments;
        let set = reveal(&segs, cursor, mode, POLICY);
        project_all(text, &segs, &set)
    }

    fn rect_of(
        layout: &DocumentLayout,
        projs: &[Projection],
        cursor: usize,
        mode: Mode,
    ) -> Option<Rectangle> {
        cursor_rect(layout, projs, cursor, mode, &space_width)
    }

    /// Layout of the projections computed for `cursor`, sharing `cache` between calls.
    fn at(
        cache: &mut ParagraphCache,
        text: &str,
        cursor: usize,
        mode: Mode,
        width: f32,
    ) -> (Vec<Projection>, DocumentLayout) {
        let projs = projections_for(text, cursor, mode);
        let layout = layout_document(cache, &projs, width);
        (projs, layout)
    }

    #[test]
    fn normal_cursor_is_a_cell_and_the_eol_cell_is_a_space_wide() {
        let mut cache = ParagraphCache::default();
        let text = "abc\n";
        let mut rect = |cursor| {
            let (projs, layout) = at(&mut cache, text, cursor, Mode::Normal, 500.0);
            rect_of(&layout, &projs, cursor, Mode::Normal).expect("test: a rect")
        };
        let (a, b, c, eol) = (rect(0), rect(1), rect(2), rect(3));
        // Cells tile the row left to right.
        assert!(a.x == 0.0 && b.x > a.x && c.x > b.x && eol.x > c.x);
        assert!((a.x + a.width - b.x).abs() < 0.05 && (b.x + b.width - c.x).abs() < 0.05);
        assert!((c.x + c.width - eol.x).abs() < 0.05);
        assert!(a.y == b.y && b.y == eol.y && a.height == eol.height);
        // The EOL cell is the width of a space at the segment's base size.
        assert_eq!(eol.width, space_width(BASE_SIZE));
    }

    #[test]
    fn eol_cell_of_a_heading_uses_the_heading_size() {
        let mut cache = ParagraphCache::default();
        let text = "## Title\n\nbody\n";
        let (projs, layout) = at(&mut cache, text, 8, Mode::Normal, 500.0);
        let eol = rect_of(&layout, &projs, 8, Mode::Normal).expect("test: a rect");
        assert_eq!(eol.width, space_width(projs[0].base_size));
        assert!(eol.width > space_width(BASE_SIZE));
        assert!(eol.height > one_line(BASE_SIZE));
    }

    #[test]
    fn a_revealed_marker_has_its_own_cell() {
        // `a**b**c`, cursor on the first `*` of the bold: the markers are visible, so the
        // cell sits between `a` and `b`, with its own width.
        let mut cache = ParagraphCache::default();
        let text = "a**b**c";
        let mut rect = |cursor| {
            let (projs, layout) = at(&mut cache, text, cursor, Mode::Normal, 500.0);
            rect_of(&layout, &projs, cursor, Mode::Normal).expect("test: a rect")
        };
        let (a, star, b) = (rect(0), rect(1), rect(3));
        assert!(star.x > a.x && b.x > star.x && star.width > 0.0);
    }

    #[test]
    fn insert_bar_is_two_pixels_wide_and_a_row_tall() {
        let mut cache = ParagraphCache::default();
        let text = "abc\n\nde\n";
        for bar in [0, 1, 3, 4, 5, 7] {
            let (projs, layout) = at(&mut cache, text, bar, Mode::Insert, 500.0);
            let r = rect_of(&layout, &projs, bar, Mode::Insert).expect("test: a rect");
            assert_eq!(r.width, 2.0, "C-11: the Insert bar is a 2 px quad");
            assert!((r.height - one_line(BASE_SIZE)).abs() < 0.01, "bar {bar}");
        }
        // The bar moves right along a row.
        let xs: Vec<f32> = (0..=3)
            .map(|bar| {
                let (projs, layout) = at(&mut cache, text, bar, Mode::Insert, 500.0);
                rect_of(&layout, &projs, bar, Mode::Insert)
                    .expect("test: a rect")
                    .x
            })
            .collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "{xs:?}");
    }

    #[test]
    fn a_bar_at_the_end_after_a_final_newline_sits_on_the_tail_row() {
        // F-11: no segment owns offset 3 of `ab\n`; the bar is drawn on a row below the text.
        let mut cache = ParagraphCache::default();
        let (projs, layout) = at(&mut cache, "ab\n", 3, Mode::Insert, 500.0);
        assert_eq!(layout.tail, one_line(BASE_SIZE));
        let r = rect_of(&layout, &projs, 3, Mode::Insert).expect("test: a rect");
        assert_eq!(
            (r.x, r.y, r.width, r.height),
            (0.0, layout.height, 2.0, layout.tail)
        );
        // No tail, and the bar belongs to the last segment, when the text has no final newline.
        let (projs, layout) = at(&mut cache, "ab", 2, Mode::Insert, 500.0);
        assert_eq!(layout.tail, 0.0);
        let r = rect_of(&layout, &projs, 2, Mode::Insert).expect("test: a rect");
        assert!(r.y < layout.height && r.x > 0.0);
        // Normal mode never has a cursor there.
        let (projs, layout) = at(&mut cache, "ab\n", 2, Mode::Normal, 500.0);
        assert!(rect_of(&layout, &projs, 3, Mode::Normal).is_none());
    }

    #[test]
    fn an_empty_document_has_no_cursor_rectangle() {
        let mut cache = ParagraphCache::default();
        for mode in [Mode::Normal, Mode::Insert] {
            let (projs, layout) = at(&mut cache, "", 0, mode, 500.0);
            assert_eq!(layout.tail, 0.0);
            assert!(rect_of(&layout, &projs, 0, mode).is_none());
        }
    }

    #[test]
    fn the_tail_row_exists_only_when_the_text_ends_in_an_eol_byte() {
        let mut cache = ParagraphCache::default();
        for (text, tail) in [("a\n", true), ("a", false), ("", false), ("a\n\n\n", true)] {
            let projs = projections(text, 99);
            let layout = layout_document(&mut cache, &projs, 500.0);
            assert_eq!(layout.tail > 0.0, tail, "{text:?}");
        }
    }

    /// Every valid cursor and bar has a rectangle inside the document, in its own segment.
    fn check_every_rectangle(text: &str, width: f32) {
        let mut cache = ParagraphCache::default();
        for mode in [Mode::Normal, Mode::Insert] {
            let positions = if mode == Mode::Insert {
                crate::doc::insert_bars(text)
            } else {
                crate::doc::normal_cursors(text)
            };
            for o in positions {
                let (projs, layout) = at(&mut cache, text, o, mode, width);
                let rect = rect_of(&layout, &projs, o, mode);
                if text.is_empty() {
                    assert!(rect.is_none());
                    continue;
                }
                let r = rect.unwrap_or_else(|| panic!("{text:?} {mode:?} {o}: no rectangle"));
                let limit = layout.height + layout.tail + 0.01;
                assert!(
                    r.x >= 0.0 && r.y >= 0.0 && r.y + r.height <= limit,
                    "{r:?} in {limit}"
                );
                assert!(r.width > 0.0 && r.height > 0.0 && r.x.is_finite());
                match owner(&projs, o) {
                    Some((si, _)) => {
                        let s = &layout.segments[si];
                        assert!(
                            r.y >= s.y - 0.01 && r.y + r.height <= s.y + s.height + 0.01,
                            "{text:?} {mode:?} {o}: {r:?} outside segment {si}"
                        );
                    }
                    None => assert_eq!(r.y, layout.height, "the tail row"),
                }
            }
        }
    }

    #[test]
    fn every_valid_cursor_has_a_rectangle_in_the_fixtures() {
        for text in [
            include_str!("../fixtures/spike.md"),
            include_str!("../fixtures/no_trailing_newline.md"),
            include_str!("../fixtures/empty.md"),
        ] {
            check_every_rectangle(text, 700.0);
            check_every_rectangle(text, 220.0);
        }
    }

    // ---- click ------------------------------------------------------------------------------------

    fn click(text: &str, cursor: usize, mode: Mode, width: f32, point: Point) -> Option<usize> {
        let mut cache = ParagraphCache::default();
        let (projs, layout) = at(&mut cache, text, cursor, mode, width);
        resolve_click(&layout, &projs, text, mode, point)
    }

    #[test]
    fn normal_click_picks_the_grapheme_under_the_point() {
        let text = "abc\n";
        let mut cache = ParagraphCache::default();
        let (projs, layout) = at(&mut cache, text, 0, Mode::Normal, 500.0);
        let cell = |o| rect_of(&layout, &projs, o, Mode::Normal).expect("test: a rect");
        let hit = |p: Point| resolve_click(&layout, &projs, text, Mode::Normal, p);
        for o in 0..3 {
            let r = cell(o);
            let y = r.y + r.height / 2.0;
            assert_eq!(
                hit(Point::new(r.x + r.width * 0.25, y)),
                Some(o),
                "left of {o}"
            );
            assert_eq!(
                hit(Point::new(r.x + r.width * 0.75, y)),
                Some(o),
                "right of {o}"
            );
        }
        let y = cell(0).y + 2.0;
        assert_eq!(hit(Point::new(-30.0, y)), Some(0), "left of the row");
        assert_eq!(
            hit(Point::new(900.0, y)),
            Some(3),
            "right of the row: the EOL cell"
        );
        assert_eq!(hit(Point::new(900.0, 900.0)), Some(3), "below the text");
        assert_eq!(hit(Point::new(5.0, -50.0)), Some(0), "above the text");
    }

    #[test]
    fn normal_click_past_an_internal_line_end_picks_its_newline() {
        // One paragraph, two hard lines: the newline between them is a cell at the row end.
        let text = "ab\ncd\n";
        let p = click(text, 0, Mode::Normal, 500.0, Point::new(900.0, 5.0));
        assert_eq!(p, Some(2));
        let p = click(text, 0, Mode::Normal, 500.0, Point::new(900.0, 25.0));
        assert_eq!(p, Some(5), "the last row's EOL cell");
    }

    #[test]
    fn normal_click_without_an_eol_byte_picks_the_last_grapheme() {
        assert_eq!(
            click("ab", 0, Mode::Normal, 500.0, Point::new(900.0, 5.0)),
            Some(1)
        );
        // A concealed empty heading with no newline has nothing to land on.
        let text = "x\n\n##";
        assert_eq!(
            click(text, 0, Mode::Normal, 500.0, Point::new(10.0, 90.0)),
            None
        );
    }

    #[test]
    fn insert_click_picks_the_nearest_boundary() {
        let text = "abc\n";
        let mut cache = ParagraphCache::default();
        let (projs, layout) = at(&mut cache, text, 0, Mode::Insert, 500.0);
        let bar = |o| rect_of(&layout, &projs, o, Mode::Insert).expect("test: a rect");
        let hit = |p: Point| resolve_click(&layout, &projs, text, Mode::Insert, p);
        for o in 0..=3 {
            let r = bar(o);
            let (x, y) = (r.x + r.width / 2.0, r.y + r.height / 2.0);
            assert_eq!(hit(Point::new(x, y)), Some(o), "on bar {o}");
        }
        assert_eq!(
            hit(Point::new(900.0, 5.0)),
            Some(3),
            "past the row end: before the newline"
        );
        // Below the text, the tail row is the end of the text.
        assert_eq!(hit(Point::new(5.0, 900.0)), Some(4));
    }

    #[test]
    fn clicking_either_side_of_a_concealed_run_picks_the_affinity() {
        // `a **b** c`: with the bar at 0 the markers are concealed. The collapse point
        // between `a ` and `b` is the boundary after source byte 1 or before source byte 4.
        let text = "a **b** c\n";
        let mut cache = ParagraphCache::default();
        let (projs, layout) = at(&mut cache, text, 0, Mode::Insert, 500.0);
        let at_b = projs[0].src_to_vis(4).expect("test: in segment").vis;
        let g = SegmentGeometry::new(&layout.segments[0].paragraph, &projs[0].visible, 14.0);
        let place = g.locate(at_b);
        let y = place.top + place.height / 2.0;
        let click = |dx: f32| {
            resolve_click(
                &layout,
                &projs,
                text,
                Mode::Insert,
                Point::new(place.x + dx, y),
            )
        };
        assert_eq!(
            click(-1.5),
            Some(2),
            "left of the boundary: before the concealed `**`"
        );
        assert_eq!(click(1.5), Some(4), "right of it: after the concealed `**`");
    }

    #[test]
    fn click_in_an_empty_document_is_offset_zero() {
        for mode in [Mode::Normal, Mode::Insert] {
            assert_eq!(click("", 0, mode, 500.0, Point::new(10.0, 10.0)), Some(0));
        }
    }

    /// Clicking a cursor's own cell resolves back to that cursor, or to one with the same
    /// position: boundaries inside collapsed whitespace share a place (F-23).
    fn check_click_round_trip(text: &str, width: f32) {
        let mut cache = ParagraphCache::default();
        for mode in [Mode::Normal, Mode::Insert] {
            let positions = if mode == Mode::Insert {
                crate::doc::insert_bars(text)
            } else {
                crate::doc::normal_cursors(text)
            };
            for o in positions {
                let (projs, layout) = at(&mut cache, text, o, mode, width);
                let Some(r) = rect_of(&layout, &projs, o, mode) else {
                    continue;
                };
                let point = Point::new(r.x + r.width / 2.0, r.y + r.height / 2.0);
                let got = resolve_click(&layout, &projs, text, mode, point)
                    .unwrap_or_else(|| panic!("{text:?} {mode:?} {o}: click found nothing"));
                assert!(
                    crate::doc::is_valid_position(text, got, mode),
                    "{text:?} {mode:?} {o}: {got} is not a valid position"
                );
                let there = rect_of(&layout, &projs, got, mode).expect("test: a rect");
                assert!(
                    got == o || ((there.x - r.x).abs() < 0.01 && (there.y - r.y).abs() < 0.01),
                    "{text:?} {mode:?} {width}: clicked {o} at {point:?}, got {got} at {there:?} (wanted {r:?})"
                );
            }
        }
    }

    #[test]
    fn click_round_trips_on_the_fixtures() {
        for text in [
            include_str!("../fixtures/spike.md"),
            include_str!("../fixtures/no_trailing_newline.md"),
        ] {
            check_click_round_trip(text, 700.0);
            check_click_round_trip(text, 220.0);
        }
    }

    fn atom() -> impl proptest::strategy::Strategy<Value = &'static str> {
        proptest::sample::select(vec![
            "a", "é", "e\u{301}", "👍🏽", " ", " ", "word ", "*", "_", "#", "\n", "\\", "n", "**",
            "# ", "*a*",
        ])
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(100))]

        #[test]
        fn rectangles_and_click_round_trip_on_random_documents(
            atoms in proptest::collection::vec(atom(), 0..30),
            width in proptest::sample::select(vec![60.0f32, 150.0, 400.0]),
        ) {
            let text = atoms.concat();
            check_every_rectangle(&text, width);
            check_click_round_trip(&text, width);
        }

        #[test]
        fn a_click_anywhere_is_never_an_invalid_position(
            atoms in proptest::collection::vec(atom(), 0..30),
            width in proptest::sample::select(vec![60.0f32, 150.0, 400.0]),
            points in proptest::collection::vec((-50.0f32..500.0, -50.0f32..400.0), 1..12),
            cursor in proptest::prelude::any::<proptest::sample::Index>(),
        ) {
            let text = atoms.concat();
            for mode in [Mode::Normal, Mode::Insert] {
                let positions = if mode == Mode::Insert {
                    crate::doc::insert_bars(&text)
                } else {
                    crate::doc::normal_cursors(&text)
                };
                let o = positions[cursor.index(positions.len())];
                for &(x, y) in &points {
                    if let Some(got) = click(&text, o, mode, width, Point::new(x, y)) {
                        proptest::prop_assert!(
                            crate::doc::is_valid_position(&text, got, mode),
                            "{:?} {:?}: click ({}, {}) gave {}", text, mode, x, y, got
                        );
                    }
                }
            }
        }
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

    // ---- keys and click through the real widget -----------------------------------------------

    use crate::app::App;

    const SPIKE: &str = include_str!("../fixtures/spike.md");

    /// The messages the real widget publishes for one key tap.
    fn tapped(app: &App, key: Key) -> Vec<Message> {
        let mut ui = iced_test::simulator(app.view());
        ui.tap_key(key);
        ui.into_messages().collect()
    }

    /// An app in Insert mode: `i` pressed once.
    fn inserting(text: &str) -> App {
        let mut app = App::new(text.to_string());
        app.update(Message::EnterInsert);
        app
    }

    #[test]
    fn normal_mode_keys_reach_the_app_as_messages() {
        let app = App::new(SPIKE.to_string());
        assert_eq!(tapped(&app, ch("l")), [Message::MoveRight]);
        assert_eq!(tapped(&app, ch("h")), [Message::MoveLeft]);
        assert_eq!(tapped(&app, ch("i")), [Message::EnterInsert]);
        for key in [ch("x"), ch(":"), Key::Named(Named::Escape)] {
            assert_eq!(tapped(&app, key.clone()), [], "{key:?}");
        }
        for (named, toggle) in [
            (Named::F1, Toggle::F1),
            (Named::F2, Toggle::F2),
            (Named::F3, Toggle::F3),
            (Named::F4, Toggle::F4),
            (Named::F5, Toggle::F5),
        ] {
            assert_eq!(tapped(&app, Key::Named(named)), [Message::Toggle(toggle)]);
        }
    }

    #[test]
    fn insert_mode_keys_reach_the_app_as_messages() {
        let app = inserting(SPIKE);
        assert_eq!(
            tapped(&app, Key::Named(Named::Escape)),
            [Message::ExitInsert]
        );
        assert_eq!(
            tapped(&app, Key::Named(Named::ArrowRight)),
            [Message::MoveRight]
        );
        assert_eq!(
            tapped(&app, Key::Named(Named::ArrowLeft)),
            [Message::MoveLeft]
        );
        assert_eq!(
            tapped(&app, ch("l")),
            [],
            "typing is T8; `l` is not a motion here"
        );
        assert_eq!(
            tapped(&app, Key::Named(Named::F3)),
            [Message::Toggle(Toggle::F3)]
        );
    }

    /// The messages one left click at `point` produces.
    fn clicked(app: &App, point: Point) -> Vec<Message> {
        let mut ui = iced_test::simulator(app.view());
        ui.point_at(point);
        ui.simulate(iced_test::simulator::click());
        ui.into_messages().collect()
    }

    #[test]
    fn click_publishes_move_to_only_while_f4_is_on() {
        let mut app = App::new(SPIKE.to_string());
        assert_eq!(
            clicked(&app, Point::new(1.0, 5.0)),
            [],
            "click is off by default"
        );
        app.update(Message::Toggle(Toggle::F4));
        // The first row's first cell: the `#` of the revealed H1 at offset 0.
        assert_eq!(
            clicked(&app, Point::new(1.0, 5.0)),
            [Message::MoveTo {
                offset: 0,
                goal_x: None
            }]
        );
        app.update(Message::Toggle(Toggle::F4));
        assert_eq!(clicked(&app, Point::new(1.0, 5.0)), []);
    }

    #[test]
    fn clicks_anywhere_publish_a_valid_position_in_both_modes() {
        for mut app in [App::new(SPIKE.to_string()), inserting(SPIKE)] {
            app.update(Message::Toggle(Toggle::F4));
            let mode = if app.title().contains("Insert") {
                Mode::Insert
            } else {
                Mode::Normal
            };
            for point in [
                Point::new(1.0, 5.0),
                Point::new(300.0, 60.0),
                Point::new(900.0, 130.0),
                Point::new(40.0, 400.0),
                Point::new(1000.0, 760.0),
            ] {
                let messages = clicked(&app, point);
                let [
                    Message::MoveTo {
                        offset,
                        goal_x: None,
                    },
                ] = messages.as_slice()
                else {
                    panic!("{point:?}: {messages:?}");
                };
                assert!(
                    crate::doc::is_valid_position(SPIKE, *offset, mode),
                    "{mode:?} {point:?}: {offset}"
                );
            }
        }
    }

    #[test]
    fn a_click_applied_to_the_app_keeps_the_frame_invariant() {
        // The whole loop: click -> MoveTo -> update -> view (per-frame I-6 assert).
        let mut app = App::new(SPIKE.to_string());
        app.update(Message::Toggle(Toggle::F4));
        for point in [
            Point::new(120.0, 20.0),
            Point::new(500.0, 200.0),
            Point::new(60.0, 330.0),
        ] {
            for message in clicked(&app, point) {
                app.update(message);
                let _ = app.view();
            }
        }
    }

    #[test]
    fn draws_every_mode_flag_and_cursor_state_without_panicking() {
        // Cursor, bar, tail-row bar and overlay ticks, through the real renderer.
        let states: [(&str, &[Message]); 6] = [
            (SPIKE, &[]),
            (
                SPIKE,
                &[Message::MoveRight, Message::MoveRight, Message::EnterInsert],
            ),
            (
                SPIKE,
                &[Message::Toggle(Toggle::F3), Message::Toggle(Toggle::F4)],
            ),
            (
                SPIKE,
                &[
                    Message::Toggle(Toggle::F3),
                    Message::EnterInsert,
                    Message::MoveRight,
                ],
            ),
            (
                "ab\n",
                &[
                    Message::EnterInsert,
                    Message::MoveRight,
                    Message::MoveRight,
                    Message::MoveRight,
                ],
            ),
            ("", &[Message::Toggle(Toggle::F3), Message::EnterInsert]),
        ];
        for (text, messages) in states {
            let mut app = App::new(text.to_string());
            for m in messages {
                app.update(*m);
            }
            iced_test::simulator(app.view())
                .snapshot(&Theme::Light)
                .unwrap_or_else(|e| panic!("{text:.10?} {messages:?}: {e:?}"));
        }
    }
}
