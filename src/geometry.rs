//! Geometry (C-8): visible bytes to rows and pixels, and points back to visible bytes,
//! computed from the concrete paragraph's `cosmic_text::Buffer` and nothing else.
//!
//! A segment's paragraph holds one `BufferLine` per hard line of its visible text, so a
//! line-start table over the visible text maps a visible byte to `(line, byte in line)`.
//! A paragraph whose visible text ends in `\n` has one more line, the row anchor that
//! `build_paragraph` adds (F-20): it starts at `visible.len()` and holds no visible byte.

use iced::Pixels;
use iced::Point;
use iced::advanced::graphics::text::Paragraph as TextParagraph;
use iced::advanced::graphics::text::cosmic_text::LayoutRun;
use iced::advanced::text::LineHeight;
use unicode_segmentation::UnicodeSegmentation;

use crate::doc::next_grapheme_boundary;
use crate::projection::Affinity;

/// One line at `base_size`: the minimum height of any row or segment.
pub fn one_line(base_size: f32) -> f32 {
    LineHeight::default().to_absolute(Pixels(base_size)).0
}

/// How far (in pixels) a column may sit left of a boundary and still count as at it. A
/// sticky `goal_x` is usually exactly a cell's left edge, read from another row, and a cell's
/// right edge (`x + width`) can differ from the next cell's `x` by rounding.
pub const COLUMN_EPS: f32 = 0.01;

/// One visual row: a layout run of the paragraph.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The hard line (`BufferLine`) this row belongs to.
    pub line: usize,
    /// Top of the row, relative to the segment.
    pub top: f32,
    pub height: f32,
    /// Right edge of the row's content.
    pub width: f32,
    /// First visible byte on the row.
    pub vis_start: usize,
    /// End of the row's visible bytes. For the last row of a hard line this is the end of
    /// the line: the offset of its `\n`, or `visible.len()`.
    pub vis_end: usize,
    pub last_of_line: bool,
}

/// Where a visible byte sits: its row and the pixel position of its left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// Index into [`SegmentGeometry::rows`].
    pub row: usize,
    pub x: f32,
    pub top: f32,
    pub height: f32,
}

/// The cell of a visible byte: a grapheme's bounds, or the EOL cell at the end of a row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cell {
    pub row: usize,
    pub x: f32,
    pub width: f32,
    pub top: f32,
    pub height: f32,
}

/// The visible boundary nearest a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hit {
    pub vis: usize,
    /// The point is over the grapheme before the boundary: left of the boundary's x, or the
    /// boundary starts the next row. Used to pick the affinity across a concealed run and the
    /// grapheme a Normal-mode click lands on.
    pub left_of_boundary: bool,
}

impl Hit {
    /// `Before` when the click was left of the boundary (nearer the preceding visible byte),
    /// otherwise `After` (nearer the following one).
    pub fn affinity(&self) -> Affinity {
        if self.left_of_boundary {
            Affinity::Before
        } else {
            Affinity::After
        }
    }
}

/// Geometry queries over one segment's paragraph.
pub struct SegmentGeometry<'a> {
    paragraph: &'a TextParagraph,
    visible: &'a str,
    base_size: f32,
    line_starts: Vec<usize>,
}

impl<'a> SegmentGeometry<'a> {
    pub fn new(paragraph: &'a TextParagraph, visible: &'a str, base_size: f32) -> Self {
        let line_starts = std::iter::once(0)
            .chain(visible.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self {
            paragraph,
            visible,
            base_size,
            line_starts,
        }
    }

    /// The first visible byte of each hard line.
    pub fn line_starts(&self) -> &[usize] {
        &self.line_starts
    }

    /// The hard line holding visible byte `vis`.
    pub fn line_of(&self, vis: usize) -> usize {
        self.line_starts.partition_point(|&s| s <= vis) - 1
    }

    /// End of a hard line: the offset of its `\n`, or `visible.len()` for the last line.
    pub fn line_end(&self, line: usize) -> usize {
        match self.line_starts.get(line + 1) {
            Some(next) => next - 1,
            None => self.visible.len(),
        }
    }

    /// All rows of the paragraph, ordered by `top`. An empty paragraph still has one row
    /// of one base line at x = 0 (C-8).
    pub fn rows(&self) -> Vec<Row> {
        let runs: Vec<LayoutRun<'_>> = self.paragraph.buffer().layout_runs().collect();
        if runs.is_empty() {
            return vec![Row {
                line: 0,
                top: 0.0,
                height: one_line(self.base_size),
                width: 0.0,
                vis_start: 0,
                vis_end: self.line_end(0),
                last_of_line: true,
            }];
        }
        // Where each row starts within its line: the first row at the line start, a wrapped
        // row at its first glyph. cosmic-text emits no glyph for whitespace swallowed at a
        // soft wrap (F-23), so that space sits between one row's last glyph and the next
        // row's first; it hangs off the end of the earlier row.
        let mut starts = Vec::with_capacity(runs.len());
        let mut previous_end = 0;
        for (i, run) in runs.iter().enumerate() {
            let first = i == 0 || runs[i - 1].line_i != run.line_i;
            let start = if first {
                0
            } else {
                run.glyphs.first().map_or(previous_end, |g| g.start)
            };
            starts.push(start);
            previous_end = run.glyphs.last().map_or(start, |g| g.end);
        }

        let mut rows = Vec::with_capacity(runs.len());
        for (i, run) in runs.iter().enumerate() {
            let last = i + 1 == runs.len() || runs[i + 1].line_i != run.line_i;
            let line = run.line_i.min(self.line_starts.len() - 1);
            let (line_start, line_end) = (self.line_starts[line], self.line_end(line));
            let right_edge = run.glyphs.last().map_or(0.0, |g| g.x + g.w);
            rows.push(Row {
                line,
                top: run.line_top,
                height: run.line_height,
                width: if last { run.line_w } else { right_edge },
                vis_start: (line_start + starts[i]).min(line_end),
                // Rows of a line tile its bytes: each ends where the next one starts.
                vis_end: if last {
                    line_end
                } else {
                    (line_start + starts[i + 1]).min(line_end)
                },
                last_of_line: last,
            });
        }
        rows
    }

    /// The row and left-edge position of visible byte `vis` (clamped to `visible.len()`).
    ///
    /// A byte at a soft-wrap boundary belongs to the row that starts there; the end of a
    /// hard line belongs to that line's last row, at the row's content width.
    pub fn locate(&self, vis: usize) -> Located {
        let rows = self.rows();
        self.locate_in(&rows, vis)
    }

    fn locate_in(&self, rows: &[Row], vis: usize) -> Located {
        let vis = vis.min(self.visible.len());
        let line = self.line_of(vis);
        let row = rows
            .iter()
            .position(|r| r.line == line && r.vis_start <= vis && vis < r.vis_end)
            .or_else(|| rows.iter().rposition(|r| r.line == line))
            .unwrap_or(0);
        Located {
            row,
            x: self.x_in_row(rows, row, vis),
            top: rows[row].top,
            height: rows[row].height,
        }
    }

    /// The cell at `vis`: the grapheme's bounds, or an EOL cell of `space_width` at the end
    /// of a hard line (the cell of its `\n`, or the end of the segment).
    pub fn cell(&self, vis: usize, space_width: f32) -> Cell {
        self.cell_in(&self.rows(), vis, space_width)
    }

    fn cell_in(&self, rows: &[Row], vis: usize, space_width: f32) -> Cell {
        let vis = vis.min(self.visible.len());
        let at = self.locate_in(rows, vis);
        let at_line_end = vis >= self.visible.len() || self.visible.as_bytes()[vis] == b'\n';
        let width = if at_line_end {
            space_width
        } else {
            let end = next_grapheme_boundary(self.visible, vis).unwrap_or(self.visible.len());
            // The right edge on this row; a grapheme at the end of a wrapped row stops there.
            let end = end.min(rows[at.row].vis_end);
            let extent = (self.x_in_row(rows, at.row, end) - at.x).max(0.0);
            // A grapheme with no glyph (whitespace swallowed at a soft wrap) has no extent
            // of its own: it hangs off the end of its row with the width of a space.
            if extent > f32::EPSILON {
                extent
            } else {
                space_width
            }
        };
        Cell {
            row: at.row,
            x: at.x,
            width,
            top: at.top,
            height: at.height,
        }
    }

    /// The grapheme starts a row owns, in order: the row's own graphemes, then (on the last
    /// row of a hard line) `vis_end`, the end-of-line position.
    fn row_boundaries(&self, row: &Row) -> Vec<usize> {
        let mut at = Vec::new();
        let mut vis = row.vis_start;
        while vis < row.vis_end {
            at.push(vis);
            match next_grapheme_boundary(self.visible, vis) {
                Some(next) if next > vis => vis = next,
                _ => break,
            }
        }
        if row.last_of_line {
            at.push(row.vis_end);
        }
        at
    }

    /// Normal-mode landing on row `row` of the paragraph for a column `goal_x`: the grapheme
    /// whose cell `[x, x + w)` contains it, else the nearest cell (the first on a tie), so a
    /// point past the line end lands on the EOL cell. Returns its visible byte, which is
    /// `visible.len()` for the segment's own EOL cell.
    ///
    /// A hard line's `\n` is always a cell; the end of the visible text is one only if
    /// `eol_cell_at_end` (the segment has an EOL byte), otherwise the last grapheme is the
    /// last cell. `None` when the row has no cell at all (an empty segment with no EOL byte).
    pub fn pick_cell(
        &self,
        row: usize,
        goal_x: f32,
        space_width: f32,
        eol_cell_at_end: bool,
    ) -> Option<usize> {
        let rows = self.rows();
        let r = rows.get(row)?;
        let goal_x = goal_x + COLUMN_EPS;
        let mut best: Option<(f32, usize)> = None;
        for vis in self.row_boundaries(r) {
            if vis >= self.visible.len() && !eol_cell_at_end {
                continue; // no EOL byte: the end of the text is not a cell
            }
            let cell = self.cell_in(&rows, vis, space_width);
            if cell.x <= goal_x && goal_x < cell.x + cell.width {
                return Some(vis);
            }
            let distance = if goal_x < cell.x {
                cell.x - goal_x
            } else {
                goal_x - (cell.x + cell.width)
            };
            if best.is_none_or(|(d, _)| distance < d) {
                best = Some((distance, vis));
            }
        }
        best.map(|(_, vis)| vis)
    }

    /// Insert-mode landing on row `row` for a column `goal_x`: the boundary nearest to it
    /// (the first on a tie, which also picks the first of several boundaries that share one
    /// position, F-23). The `Hit` says whether `goal_x` is left of the boundary, which picks
    /// the affinity across a concealed run exactly as a click does.
    ///
    /// A boundary at a soft wrap belongs to the row that starts there (F-26), so the end of a
    /// wrapped row is reachable only through swallowed whitespace.
    pub fn pick_boundary(&self, row: usize, goal_x: f32) -> Option<Hit> {
        let rows = self.rows();
        let r = rows.get(row)?;
        let mut best: Option<(f32, usize, f32)> = None;
        for vis in self.row_boundaries(r) {
            let x = self.x_in_row(&rows, row, vis);
            let distance = (goal_x - x).abs();
            if best.is_none_or(|(d, _, _)| distance < d) {
                best = Some((distance, vis, x));
            }
        }
        best.map(|(_, vis, x)| Hit {
            vis,
            left_of_boundary: goal_x + COLUMN_EPS < x,
        })
    }

    /// The visible boundary nearest `point` (relative to the paragraph's top-left).
    pub fn hit(&self, point: Point) -> Hit {
        let Some(cursor) = self.paragraph.buffer().hit(point.x, point.y) else {
            return Hit {
                vis: 0,
                left_of_boundary: false,
            };
        };
        let line = cursor.line.min(self.line_starts.len() - 1);
        let vis = (self.line_starts[line] + cursor.index).min(self.line_end(line));

        // The row the point is on (above the first row or below the last clamps to it).
        let rows = self.rows();
        let clicked_row = rows.iter().rposition(|r| point.y >= r.top).unwrap_or(0);
        let boundary = self.locate_in(&rows, vis);
        Hit {
            vis,
            // A boundary on a later row is the end of the clicked row (a wrap with no
            // swallowed space): the point is over the last cell before it, whatever its x.
            left_of_boundary: boundary.row > clicked_row || point.x < boundary.x,
        }
    }

    /// The x of `vis` within row `row`: the glyph's left edge, a grapheme's share of a
    /// cluster (as `Buffer::hit` splits it), or the row's right edge past its last glyph.
    fn x_in_row(&self, rows: &[Row], row: usize, vis: usize) -> f32 {
        let Some(run) = self.paragraph.buffer().layout_runs().nth(row) else {
            return 0.0; // the synthetic row of an empty paragraph
        };
        let byte = vis.saturating_sub(self.line_starts[rows[row].line]);
        for glyph in run.glyphs {
            if byte < glyph.start {
                return glyph.x;
            }
            if byte < glyph.end {
                let cluster = run.text.get(glyph.start..glyph.end).unwrap_or("");
                let share = glyph.w / cluster.graphemes(true).count().max(1) as f32;
                for (i, (at, grapheme)) in cluster.grapheme_indices(true).enumerate() {
                    if byte < glyph.start + at + grapheme.len() {
                        return glyph.x + share * i as f32;
                    }
                }
                return glyph.x;
            }
        }
        rows[row].width
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::grapheme_starts;
    use crate::parse::parse;
    use crate::projection::{Projection, StyleSpan, project_all};
    use crate::reveal::{HeadingPolicy, Mode, NestingPolicy, RevealPolicy, reveal};
    use crate::widget::build_paragraph;
    use proptest::prelude::*;

    const POLICY: RevealPolicy = RevealPolicy {
        heading: HeadingPolicy::G1,
        nesting: NestingPolicy::N1,
    };

    /// Projections with the cursor far away, so every marker is concealed.
    fn projections(text: &str) -> Vec<Projection> {
        let segs = parse(text, 0).segments;
        let set = reveal(&segs, usize::MAX, Mode::Normal, POLICY);
        project_all(text, &segs, &set)
    }

    /// Grapheme boundaries of the visible text, including its end.
    fn boundaries(visible: &str) -> Vec<usize> {
        let mut b = grapheme_starts(visible);
        b.push(visible.len());
        b.dedup();
        b
    }

    /// Everything the geometry must satisfy for one projection at one width.
    fn check(p: &Projection, width: f32) {
        let paragraph = build_paragraph(p, width);
        let geometry = SegmentGeometry::new(&paragraph, &p.visible, p.base_size);
        let rows = geometry.rows();
        assert!(!rows.is_empty());

        // Rows are y-ordered, contiguous, and tile each hard line's visible bytes.
        assert_eq!(rows[0].top, 0.0);
        for pair in rows.windows(2) {
            assert!(pair[1].top > pair[0].top, "{:?}", rows);
            assert!((pair[0].top + pair[0].height - pair[1].top).abs() < 0.01);
            if pair[0].line == pair[1].line {
                assert_eq!(pair[0].vis_end, pair[1].vis_start, "{:?}", rows);
                assert!(!pair[0].last_of_line);
            } else {
                assert!(pair[0].last_of_line);
                assert_eq!(pair[1].line, pair[0].line + 1);
            }
        }
        for line in 0..geometry.line_starts().len() {
            let mine: Vec<&Row> = rows.iter().filter(|r| r.line == line).collect();
            assert!(!mine.is_empty(), "line {line} has no row: {:?}", p.visible);
            assert_eq!(mine[0].vis_start, geometry.line_starts()[line]);
            assert_eq!(
                mine.last().expect("test: nonempty").vis_end,
                geometry.line_end(line)
            );
        }

        // Every visible boundary round-trips vis -> point -> vis, modulo coincident positions
        // (the developer's resolution of H3, option A): the hit lands on the boundary's own
        // pixel position, and on the boundary itself unless another boundary shares that
        // position. Whitespace swallowed at a soft wrap has no glyph, so its boundaries
        // collapse onto one position and `Buffer::hit` cannot tell them apart (F-23).
        let bounds = boundaries(&p.visible);
        let places: Vec<Located> = bounds.iter().map(|&b| geometry.locate(b)).collect();
        let same_place = |a: &Located, b: &Located| a.row == b.row && (a.x - b.x).abs() < 0.01;
        for (i, &b) in bounds.iter().enumerate() {
            let at = places[i];
            let point = Point::new(at.x, at.top + at.height / 2.0);
            let hit = geometry.hit(point);
            assert!(
                same_place(&at, &geometry.locate(hit.vis)),
                "{:?} width {width}: boundary {b} at {at:?} came back as {} at {:?}",
                p.visible,
                hit.vis,
                geometry.locate(hit.vis)
            );
            let shared = places
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && same_place(&at, other));
            if !shared {
                assert_eq!(
                    hit.vis, b,
                    "{:?} width {width}: boundary {b} at {at:?} is alone at its position",
                    p.visible
                );
            }
            assert!(
                !hit.left_of_boundary,
                "x equals the boundary: not left of it"
            );
            assert_eq!(hit.affinity(), Affinity::After);
        }

        // A hit on hard line k returns a byte on line k, wherever along the row it lands.
        for row in &rows {
            for x in [-50.0, 0.0, row.width / 2.0, row.width, row.width + 500.0] {
                let hit = geometry.hit(Point::new(x, row.top + row.height / 2.0));
                assert_eq!(
                    geometry.line_of(hit.vis),
                    row.line,
                    "{:?}: x={x} on a row of line {} gave byte {}",
                    p.visible,
                    row.line,
                    hit.vis
                );
            }
        }

        // Cells: a grapheme's right edge is the next boundary's x when both are on its row.
        for pair in boundaries(&p.visible).windows(2) {
            let cell = geometry.cell(pair[0], 5.0);
            if p.visible.as_bytes()[pair[0]] == b'\n' {
                assert_eq!(cell.width, 5.0, "an internal newline is an EOL cell");
                continue;
            }
            let next = geometry.locate(pair[1]);
            if next.row == cell.row {
                if (next.x - cell.x).abs() < 0.05 {
                    // No extent of its own (collapsed whitespace, F-23): a space's width.
                    assert_eq!(cell.width, 5.0, "{:?} cell {cell:?}", p.visible);
                } else {
                    assert!(
                        (cell.x + cell.width - next.x).abs() < 0.05,
                        "{:?} cell {cell:?} vs next {next:?}",
                        p.visible
                    );
                }
            }
            assert!(cell.width >= 0.0);
        }
        // The EOL cell: the end of the visible text, on the last row, at its content width.
        let end = geometry.locate(p.visible.len());
        let last = rows.last().expect("test: nonempty");
        assert_eq!(end.row, rows.len() - 1);
        assert!((end.x - last.width).abs() < 0.01);
        assert_eq!(geometry.cell(p.visible.len(), 7.0).width, 7.0);
    }

    const WRAPPED: &str = "A wrapped paragraph with **bold words** and a good many more words \
        so that it needs several rows at a narrow width, ending the first hard line\n\
        the second hard line has *italic* text, an emoji 👍🏽 and é e\u{301} too\n\
        third\n";

    #[test]
    fn wrapped_multi_line_paragraph_at_several_widths() {
        let projs = projections(WRAPPED);
        assert_eq!(projs.len(), 1);
        for width in [90.0, 140.0, 260.0, 900.0] {
            check(&projs[0], width);
        }
        // Narrow widths really wrap: more rows than hard lines.
        let paragraph = build_paragraph(&projs[0], 120.0);
        let g = SegmentGeometry::new(&paragraph, &projs[0].visible, projs[0].base_size);
        assert!(g.rows().len() > g.line_starts().len());
    }

    #[test]
    fn headings_and_unicode_and_gaps() {
        let text = "## A heading that is long enough to wrap when the width is narrow\n\n\n\n\
                    é e\u{301} 👍🏽 中文漢字 **日本語**\n\n";
        for p in projections(text) {
            for width in [100.0, 300.0] {
                check(&p, width);
            }
        }
    }

    #[test]
    fn mixed_size_spans_in_one_paragraph() {
        // Hand-built mixed sizes: no parsed segment has them, but the geometry must cope.
        let mut p = projections(
            "plain words then LARGE words and more plain words that wrap around a bit\nsecond line here\n",
        )
        .remove(0);
        let len = p.visible.len();
        let (a, b) = (
            p.visible.find("LARGE").expect("test: text"),
            p.visible.find(" words and").expect("test: text"),
        );
        let style = p.spans[0].style;
        let mixed = |size: f32, bold: bool| crate::projection::Style {
            size,
            bold,
            italic: false,
        };
        p.spans = vec![
            StyleSpan { range: 0..a, style },
            StyleSpan {
                range: a..b,
                style: mixed(32.0, true),
            },
            StyleSpan {
                range: b..len,
                style,
            },
        ];
        for width in [120.0, 220.0, 800.0] {
            check(&p, width);
        }
        // The row holding the large span is taller than a row of body text.
        let paragraph = build_paragraph(&p, 800.0);
        let g = SegmentGeometry::new(&paragraph, &p.visible, p.base_size);
        let rows = g.rows();
        assert!(rows[0].height > rows[1].height, "{rows:?}");
        assert!(g.locate(a).height > g.locate(len).height);
    }

    #[test]
    fn eol_and_empty_segment_handling() {
        // An empty paragraph (a concealed `##`) still has one row of its base line at x = 0.
        let projs = projections("##\n\nx\n");
        let empty = &projs[0];
        assert_eq!(empty.visible, "");
        let paragraph = build_paragraph(empty, 400.0);
        let g = SegmentGeometry::new(&paragraph, &empty.visible, empty.base_size);
        let rows = g.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].top, 0.0);
        assert!(rows[0].height >= one_line(empty.base_size) - 0.01);
        let at = g.locate(0);
        assert_eq!((at.row, at.x), (0, 0.0));
        let cell = g.cell(0, 6.0);
        assert_eq!((cell.x, cell.width), (0.0, 6.0));
        check(empty, 400.0);

        // The EOL cell of a paragraph sits just past its last glyph.
        let projs = projections("abc\n\nx\n");
        let p = &projs[0];
        let paragraph = build_paragraph(p, 400.0);
        let g = SegmentGeometry::new(&paragraph, &p.visible, p.base_size);
        let last_glyph = g.locate(2);
        let eol = g.cell(3, 6.0);
        assert!(eol.x > last_glyph.x);
        assert_eq!(eol.row, last_glyph.row);

        // Gap of three source lines: three rows, every one starting at x = 0.
        let projs = projections("x\n\n\n\ny\n");
        let gap = &projs[1];
        assert_eq!(gap.visible, "\n\n");
        let paragraph = build_paragraph(gap, 400.0);
        let g = SegmentGeometry::new(&paragraph, &gap.visible, gap.base_size);
        assert_eq!(g.rows().len(), 3);
        for vis in 0..=gap.visible.len() {
            assert_eq!(g.locate(vis).x, 0.0, "vis {vis}");
            assert_eq!(g.locate(vis).row, vis, "each gap byte starts its own row");
        }
        check(gap, 400.0);
    }

    #[test]
    fn a_soft_wrap_boundary_belongs_to_the_row_that_starts_there() {
        let projs = projections("one two three four five six seven eight nine ten eleven\n");
        let p = &projs[0];
        let paragraph = build_paragraph(p, 100.0);
        let g = SegmentGeometry::new(&paragraph, &p.visible, p.base_size);
        let rows = g.rows();
        assert!(rows.len() >= 3);
        for pair in rows.windows(2) {
            let at = g.locate(pair[1].vis_start);
            assert_eq!(
                (at.row, at.x),
                (
                    rows.iter().position(|r| r == &pair[1]).expect("test: row"),
                    0.0
                )
            );
        }
    }

    #[test]
    fn click_left_or_right_of_a_boundary_picks_the_affinity() {
        let projs = projections("abcdef\n");
        let p = &projs[0];
        let paragraph = build_paragraph(p, 400.0);
        let g = SegmentGeometry::new(&paragraph, &p.visible, p.base_size);
        let at = g.locate(3);
        let y = at.top + at.height / 2.0;
        let left = g.hit(Point::new(at.x - 1.0, y));
        let right = g.hit(Point::new(at.x + 1.0, y));
        assert_eq!((left.vis, right.vis), (3, 3));
        assert_eq!(left.affinity(), Affinity::Before);
        assert_eq!(right.affinity(), Affinity::After);
        // Above and below the paragraph clamp to its first and last rows.
        assert_eq!(g.hit(Point::new(0.0, -40.0)).vis, 0);
        assert_eq!(g.hit(Point::new(9999.0, 9999.0)).vis, 6);
    }

    // ---- characterization of F-23: whitespace swallowed at a soft wrap ----------------------
    //
    // These document current behavior on purpose. Pixel widths depend on the machine's
    // fonts, so each test sweeps widths for the shape instead of hard-coding one.

    fn geometry_at<'a>(paragraph: &'a TextParagraph, p: &'a Projection) -> SegmentGeometry<'a> {
        SegmentGeometry::new(paragraph, &p.visible, p.base_size)
    }

    #[test]
    fn several_swallowed_spaces_at_a_row_end_share_one_position() {
        let projs = projections("ab  cd  ef  gh\n");
        let p = &projs[0];
        let mut coincident = 0;
        for width in (10..120).map(|w| w as f32) {
            let paragraph = build_paragraph(p, width);
            let g = geometry_at(&paragraph, p);
            let rows = g.rows();
            for (i, pair) in rows.windows(2).enumerate() {
                // The bytes between the last glyph of a row and the next row's start have
                // no glyph (this is a single hard line, so line offsets are visible offsets).
                let (end, next_start) = (pair[0].vis_end, pair[1].vis_start);
                assert_eq!(end, next_start, "rows tile the line at width {width}");
                let glyph_end = paragraph
                    .buffer()
                    .layout_runs()
                    .nth(i)
                    .and_then(|run| run.glyphs.last().map(|gl| gl.end))
                    .expect("test: a wrapped row has glyphs");
                if end - glyph_end < 2 {
                    continue;
                }
                let first = glyph_end;
                // All boundaries from the first swallowed space to the row end coincide ...
                let at = g.locate(first);
                for b in first..end {
                    let here = g.locate(b);
                    assert_eq!(
                        (here.row, here.x),
                        (at.row, at.x),
                        "width {width}, byte {b}"
                    );
                }
                // ... so `hit` cannot return the later ones: it returns one of them, and the
                // row start after them is unaffected.
                let hit = g.hit(Point::new(at.x, at.top + at.height / 2.0));
                assert!((first..end).contains(&hit.vis), "width {width}: {hit:?}");
                let next = g.locate(next_start);
                let back = g.hit(Point::new(next.x, next.top + next.height / 2.0));
                assert_eq!(back.vis, next_start, "width {width}");
                coincident += 1;
            }
        }
        assert!(coincident > 0, "no width wrapped inside a run of spaces");
    }

    #[test]
    fn leading_whitespace_swallowed_at_a_narrow_width_has_no_position_of_its_own() {
        let projs = projections(" wordwordword other\n");
        let p = &projs[0];
        let mut swallowed = 0;
        for width in (10..200).map(|w| w as f32) {
            let paragraph = build_paragraph(p, width);
            let g = geometry_at(&paragraph, p);
            let first_glyph = paragraph
                .buffer()
                .layout_runs()
                .next()
                .and_then(|run| run.glyphs.first().map(|g| g.start));
            if first_glyph != Some(1) {
                continue; // the leading space kept its glyph at this width
            }
            swallowed += 1;
            // Byte 0 has no glyph: it shares x = 0 with byte 1, and `hit` returns byte 1.
            let (zero, one) = (g.locate(0), g.locate(1));
            assert_eq!((zero.row, zero.x), (one.row, one.x), "width {width}");
            let hit = g.hit(Point::new(zero.x, zero.top + zero.height / 2.0));
            assert_eq!(hit.vis, 1, "width {width}");
        }
        assert!(swallowed > 0, "no width swallowed the leading space");
    }

    #[test]
    fn a_single_swallowed_space_hangs_at_the_end_of_the_earlier_row() {
        // The fix for the first failures: the swallowed space belongs to the row before the
        // wrap, at that row's right edge, and the next row starts at its own first glyph.
        let projs = projections("one two three four five six seven eight nine ten\n");
        let p = &projs[0];
        let mut hanging = 0;
        for width in (30..160).map(|w| w as f32) {
            let paragraph = build_paragraph(p, width);
            let g = geometry_at(&paragraph, p);
            let rows = g.rows();
            for (i, pair) in rows.windows(2).enumerate() {
                let end = pair[0].vis_end;
                if p.visible.as_bytes()[end - 1] != b' ' {
                    continue;
                }
                let before = g.locate(end - 1);
                let glyph_after_space = paragraph
                    .buffer()
                    .layout_runs()
                    .nth(i)
                    .and_then(|run| run.glyphs.last().map(|gl| gl.end));
                if glyph_after_space != Some(end - 1) {
                    continue; // this wrap kept the space as a glyph
                }
                hanging += 1;
                assert_eq!(
                    before.row, i,
                    "width {width}: the space belongs to the earlier row"
                );
                assert!((before.x - pair[0].width).abs() < 0.01, "width {width}");
                let cell = g.cell(end - 1, 4.0);
                assert_eq!(
                    (cell.row, cell.width),
                    (i, 4.0),
                    "width {width}: a space-width cell"
                );
            }
        }
        assert!(hanging > 0, "no width swallowed a single space at a wrap");
    }

    #[test]
    fn pickers_choose_by_column_with_a_tolerance_at_cell_edges() {
        let projs = projections("abcdef\n");
        let p = &projs[0];
        let paragraph = build_paragraph(p, 400.0);
        let g = geometry_at(&paragraph, p);
        let sw = 4.0;
        let edge = g.cell(3, sw).x;
        let inside = g.cell(2, sw);
        assert!((inside.x + inside.width - edge).abs() < 0.01, "cells tile");
        // A column inside a cell, and past the line end (the EOL cell).
        assert_eq!(g.pick_cell(0, edge + 1.0, sw, true), Some(3));
        assert_eq!(g.pick_cell(0, edge - 1.0, sw, true), Some(2));
        assert_eq!(g.pick_cell(0, 1000.0, sw, true), Some(6));
        assert_eq!(g.pick_cell(0, 1000.0, sw, false), Some(5));
        assert_eq!(g.pick_cell(0, -5.0, sw, true), Some(0));
        // Exactly on an edge, or within the tolerance left of it, belongs to the cell on the
        // right; clearly left of it, to the one on the left.
        assert_eq!(g.pick_cell(0, edge, sw, true), Some(3));
        assert_eq!(g.pick_cell(0, edge - COLUMN_EPS / 2.0, sw, true), Some(3));
        assert_eq!(g.pick_cell(0, edge - 2.0 * COLUMN_EPS, sw, true), Some(2));
        // Boundaries: the nearest, and the affinity flips only beyond the tolerance.
        let nearest = g.pick_boundary(0, edge + 0.1).expect("test: a boundary");
        assert_eq!(nearest.vis, 3);
        let on_it = g.pick_boundary(0, edge).expect("test: a boundary");
        assert_eq!((on_it.vis, on_it.affinity()), (3, Affinity::After));
        let within = g
            .pick_boundary(0, edge - COLUMN_EPS / 2.0)
            .expect("test: a boundary");
        assert_eq!((within.vis, within.affinity()), (3, Affinity::After));
        let left = g
            .pick_boundary(0, edge - 2.0 * COLUMN_EPS)
            .expect("test: a boundary");
        assert_eq!((left.vis, left.affinity()), (3, Affinity::Before));
        // A row that does not exist has no landing spot.
        assert_eq!(g.pick_cell(7, 0.0, sw, true), None);
        assert_eq!(g.pick_boundary(7, 0.0), None);
    }

    fn atom() -> impl Strategy<Value = &'static str> {
        prop::sample::select(vec![
            "a", "é", "e\u{301}", "👍🏽", " ", " ", "word ", "*", "_", "#", "\n", "\\", "n", "**",
            "# ", "*a*",
        ])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        #[test]
        fn geometry_holds_on_random_documents(
            atoms in prop::collection::vec(atom(), 0..40),
            width in prop::sample::select(vec![50.0f32, 120.0, 300.0]),
        ) {
            let text = atoms.concat();
            for p in projections(&text) {
                check(&p, width);
            }
        }
    }
}
