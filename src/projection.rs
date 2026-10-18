//! Projection: a segment's visible text, styles and source↔visible mapping (C-5, C-6).

// Consumed by later tasks (motion, geometry, widget); remove once they use every item.
#![allow(dead_code)]

use std::ops::Range;

use crate::parse::{ConstructKind, Segment, SegmentKind};
use crate::reveal::{Mode, RevealSet};

pub const BASE_SIZE: f32 = 14.0;
const HEADING_SCALE: [f32; 6] = [2.0, 1.6, 1.3, 1.15, 1.05, 1.0];

/// Base font size of a segment: every byte of a heading uses its level's size (I-7).
pub fn base_size(kind: SegmentKind) -> f32 {
    match kind {
        SegmentKind::Heading { level } => {
            BASE_SIZE * HEADING_SCALE[usize::from(level.clamp(1, 6)) - 1]
        }
        _ => BASE_SIZE,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
}

/// A maximal run of equal style over the visible text.
#[derive(Debug, Clone, PartialEq)]
pub struct StyleSpan {
    pub range: Range<usize>,
    pub style: Style,
}

/// One entry of the run table. Together the runs cover the segment range exactly.
// I-1: every `src` is a source byte range; `vis_start` is the only view coordinate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Visible {
        src: Range<usize>,
        vis_start: usize,
    },
    /// One marker range. Adjacent concealed markers stay separate runs (see F-10).
    Concealed {
        src: Range<usize>,
    },
    Eol {
        src: Range<usize>,
    },
}

impl Run {
    pub fn src(&self) -> &Range<usize> {
        match self {
            Run::Visible { src, .. } | Run::Concealed { src } | Run::Eol { src } => src,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Before,
    After,
}

/// `(segment index, visible byte, affinity)`. Pixels are derived from it on demand (C-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisualPos {
    pub segment: usize,
    pub vis: usize,
    pub affinity: Affinity,
}

/// Result of `src_to_vis`: a visible byte offset, flagged when the source byte is the EOL byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrcVis {
    pub vis: usize,
    pub eol: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    /// The segment's source range.
    pub range: Range<usize>,
    pub visible: String,
    pub spans: Vec<StyleSpan>,
    pub runs: Vec<Run>,
    pub base_size: f32,
    eol: Option<usize>,
    /// Visible bytes preceding each run: the collapse point of a `Concealed` run.
    vis_before: Vec<usize>,
}

/// Projects every segment with the reveal set computed for the current cursor.
pub fn project_all(text: &str, segments: &[Segment], revealed: &RevealSet) -> Vec<Projection> {
    segments
        .iter()
        .enumerate()
        .map(|(i, seg)| project(text, seg, revealed.for_segment(i)))
        .collect()
}

/// The run table under construction, with the visible text and collapse points it implies.
#[derive(Default)]
struct Table {
    runs: Vec<Run>,
    vis_before: Vec<usize>,
    visible: String,
}

impl Table {
    fn visible_run(&mut self, text: &str, src: Range<usize>) {
        self.vis_before.push(self.visible.len());
        self.runs.push(Run::Visible {
            vis_start: self.visible.len(),
            src: src.clone(),
        });
        self.visible.push_str(
            text.get(src)
                .expect("spike: run boundaries are char boundaries"),
        );
    }

    fn concealed_run(&mut self, src: Range<usize>) {
        self.vis_before.push(self.visible.len());
        self.runs.push(Run::Concealed { src });
    }

    fn eol_run(&mut self, at: usize) {
        self.vis_before.push(self.visible.len());
        self.runs.push(Run::Eol { src: at..at + 1 });
    }
}

/// Projects one segment. `revealed` is parallel to `seg.constructs` (missing flags mean concealed).
pub fn project(text: &str, seg: &Segment, revealed: &[bool]) -> Projection {
    let eol = seg.eol(text);
    let content_end = eol.unwrap_or(seg.range.end);

    // Concealed bytes: the markers of every construct that is not revealed.
    let mut concealed: Vec<Range<usize>> = Vec::new();
    for (i, c) in seg.constructs.iter().enumerate() {
        if !revealed.get(i).copied().unwrap_or(false) {
            concealed.extend([c.open.clone(), c.close.clone()]);
        }
    }
    concealed.sort_by_key(|r| (r.start, r.end));

    // Run table. Markers are disjoint and exclude the EOL byte; the clamps keep the
    // table a partition even if that were ever violated.
    let mut table = Table::default();
    let mut at = seg.range.start;
    for marker in concealed {
        let marker = marker.start.max(at)..marker.end.min(content_end);
        if marker.start >= marker.end {
            continue;
        }
        if at < marker.start {
            table.visible_run(text, at..marker.start);
        }
        // I-6: a concealed run is zero-width to motions; callers never land inside it.
        at = marker.end;
        table.concealed_run(marker);
    }
    if at < content_end {
        table.visible_run(text, at..content_end);
    }
    if let Some(eol) = eol {
        table.eol_run(eol);
    }
    let Table {
        runs,
        vis_before,
        visible,
    } = table;

    let base = base_size(seg.kind);
    let spans = spans(seg, &runs, base);
    Projection {
        range: seg.range.clone(),
        visible,
        spans,
        runs,
        base_size: base,
        eol,
        vis_before,
    }
}

/// Spans of equal style over the visible text.
// I-7: styling comes from construct ranges over source bytes, so a revealed marker
// is styled by its construct and a heading's `#` is heading-sized.
fn spans(seg: &Segment, runs: &[Run], size: f32) -> Vec<StyleSpan> {
    let mut breaks: Vec<usize> = seg
        .constructs
        .iter()
        .filter(|c| !matches!(c.kind, ConstructKind::Heading { .. }))
        .flat_map(|c| [c.range.start, c.range.end])
        .collect();
    breaks.sort_unstable();
    breaks.dedup();

    let style_at = |o: usize| {
        let inside = |kind: &ConstructKind| {
            seg.constructs
                .iter()
                .any(|c| &c.kind == kind && c.range.contains(&o))
        };
        Style {
            size,
            bold: inside(&ConstructKind::Strong),
            italic: inside(&ConstructKind::Emphasis),
        }
    };

    let mut out: Vec<StyleSpan> = Vec::new();
    for run in runs {
        let Run::Visible { src, vis_start } = run else {
            continue;
        };
        let inner = breaks.partition_point(|&b| b <= src.start);
        let cuts = breaks[inner..].iter().copied().take_while(|&b| b < src.end);
        let mut from = src.start;
        for to in cuts.chain([src.end]) {
            let range = vis_start + (from - src.start)..vis_start + (to - src.start);
            let style = style_at(from);
            match out.last_mut() {
                Some(prev) if prev.style == style && prev.range.end == range.start => {
                    prev.range.end = range.end;
                }
                _ => out.push(StyleSpan { range, style }),
            }
            from = to;
        }
    }
    out
}

impl Projection {
    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    /// Source offset of the EOL byte, if the segment ends in one.
    pub fn eol(&self) -> Option<usize> {
        self.eol
    }

    /// Maps a source offset in `[range.start, range.end]` to a visible byte offset.
    ///
    /// `range.end` maps only when there is no EOL byte (otherwise it belongs to the
    /// next segment); `None` for anything outside.
    pub fn src_to_vis(&self, o: usize) -> Option<SrcVis> {
        if o < self.range.start || o > self.range.end {
            return None;
        }
        let i = self.runs.partition_point(|r| r.src().end <= o);
        let at = |vis, eol| Some(SrcVis { vis, eol });
        match self.runs.get(i) {
            Some(Run::Visible { src, vis_start }) => at(vis_start + (o - src.start), false),
            // The collapse point: the visible length preceding the run.
            Some(Run::Concealed { .. }) => at(self.vis_before[i], false),
            Some(Run::Eol { .. }) => at(self.visible.len(), true),
            None if self.eol.is_none() => at(self.visible.len(), false),
            None => None,
        }
    }

    /// Maps a visible byte offset (clamped to `visible_len`) back to a source offset.
    ///
    /// Unique unless concealed runs sit between the neighbouring visible bytes; then
    /// `Before` is the offset right after the preceding visible byte and `After` the
    /// offset of the following visible byte (the EOL byte, or the segment end, at the end).
    pub fn vis_to_src(&self, v: usize, affinity: Affinity) -> usize {
        let v = v.min(self.visible.len());
        let mut prev_end = None;
        let mut next_start = None;
        for run in &self.runs {
            let Run::Visible { src, vis_start } = run else {
                continue;
            };
            let end = vis_start + src.len();
            if *vis_start < v && v < end {
                return src.start + (v - vis_start);
            }
            if end == v {
                prev_end = Some(src.end);
            }
            if *vis_start == v {
                next_start = Some(src.start);
            }
        }
        match affinity {
            Affinity::Before => prev_end.unwrap_or(self.range.start),
            Affinity::After => next_start.unwrap_or(self.eol.unwrap_or(self.range.end)),
        }
    }
}

/// The I-6 frame invariant under the projections computed for `cursor`:
/// a Normal cursor's byte is never concealed, and an Insert bar is never strictly
/// inside a concealed run. Callers `debug_assert!` this every frame.
// I-6
pub fn check_frame_invariant(
    projections: &[Projection],
    cursor: usize,
    mode: Mode,
) -> Result<(), String> {
    for (si, p) in projections.iter().enumerate() {
        if cursor < p.range.start || cursor > p.range.end {
            continue;
        }
        for run in &p.runs {
            let Run::Concealed { src } = run else {
                continue;
            };
            let violated = match mode {
                Mode::Normal => src.contains(&cursor),
                Mode::Insert => src.start < cursor && cursor < src.end,
            };
            if violated {
                return Err(format!(
                    "I-6: {mode:?} cursor at {cursor} is inside concealed run {src:?} of segment {si}"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{insert_bars, normal_cursors};
    use crate::parse::{ConstructKind, parse};
    use crate::reveal::{HeadingPolicy, NestingPolicy, RevealPolicy, reveal};
    use proptest::prelude::*;

    const POLICIES: [RevealPolicy; 4] = [
        RevealPolicy {
            heading: HeadingPolicy::G1,
            nesting: NestingPolicy::N1,
        },
        RevealPolicy {
            heading: HeadingPolicy::G1,
            nesting: NestingPolicy::N2,
        },
        RevealPolicy {
            heading: HeadingPolicy::G2,
            nesting: NestingPolicy::N1,
        },
        RevealPolicy {
            heading: HeadingPolicy::G2,
            nesting: NestingPolicy::N2,
        },
    ];
    const G1N1: RevealPolicy = POLICIES[0];
    const G1N2: RevealPolicy = POLICIES[1];
    const G2N1: RevealPolicy = POLICIES[2];

    /// Projections of every segment for a cursor under a policy.
    fn projected(text: &str, o: usize, mode: Mode, p: RevealPolicy) -> Vec<Projection> {
        let segs = parse(text, 0).segments;
        let set = reveal(&segs, o, mode, p);
        project_all(text, &segs, &set)
    }

    /// The projection of the segment holding byte `o`.
    fn seg_at(text: &str, o: usize, mode: Mode, p: RevealPolicy, at: usize) -> Projection {
        let projs = projected(text, o, mode, p);
        projs
            .into_iter()
            .find(|pr| pr.range.contains(&at))
            .expect("test: a segment holds the byte")
    }

    fn style(size: f32, bold: bool, italic: bool) -> Style {
        Style { size, bold, italic }
    }

    fn span(range: Range<usize>, size: f32, bold: bool, italic: bool) -> StyleSpan {
        StyleSpan {
            range,
            style: style(size, bold, italic),
        }
    }

    fn vis(src: Range<usize>, vis_start: usize) -> Run {
        Run::Visible { src, vis_start }
    }

    fn conc(src: Range<usize>) -> Run {
        Run::Concealed { src }
    }

    fn eol(at: usize) -> Run {
        Run::Eol { src: at..at + 1 }
    }

    // ---- invariants checked on any projection ---------------------------------

    /// P1, P2, P4, P5 plus span and visible-text consistency, for one projection.
    fn check_projection(text: &str, seg: &Segment, p: &Projection) {
        // P4: the run table covers the segment range exactly, in order, with no empty runs.
        let mut at = seg.range.start;
        let mut vis_len = 0;
        for run in &p.runs {
            assert_eq!(
                run.src().start,
                at,
                "{text:?}: runs not contiguous: {:?}",
                p.runs
            );
            assert!(run.src().end > run.src().start, "{text:?}: empty run");
            if let Run::Visible { src, vis_start } = run {
                assert_eq!(*vis_start, vis_len, "{text:?}: vis_start drifted");
                vis_len += src.len();
            }
            at = run.src().end;
        }
        assert_eq!(
            at, seg.range.end,
            "{text:?}: runs stop short of the segment end"
        );
        assert_eq!(vis_len, p.visible.len());
        assert!(
            p.runs
                .iter()
                .filter(|r| matches!(r, Run::Eol { .. }))
                .count()
                <= 1,
            "{text:?}: more than one EOL run"
        );
        assert_eq!(
            matches!(p.runs.last(), Some(Run::Eol { .. })),
            seg.eol(text).is_some(),
            "{text:?}: EOL run does not match the EOL byte"
        );

        // P5: interleaving the runs reconstructs the source exactly; visible runs
        // reconstruct the visible text.
        let mut source = String::new();
        let mut visible = String::new();
        for run in &p.runs {
            source.push_str(&text[run.src().clone()]);
            if let Run::Visible { src, .. } = run {
                visible.push_str(&text[src.clone()]);
            }
        }
        assert_eq!(source, text[seg.range.clone()], "P5");
        assert_eq!(visible, p.visible, "P5 visible");

        // P1: vis -> src -> vis is the identity, for both affinities, at every visible byte.
        for v in 0..=p.visible.len() {
            for aff in [Affinity::Before, Affinity::After] {
                let src = p.vis_to_src(v, aff);
                assert!(
                    seg.range.start <= src && src <= seg.range.end,
                    "{text:?}: vis_to_src({v},{aff:?}) = {src} outside the segment"
                );
                let back = p
                    .src_to_vis(src)
                    .unwrap_or_else(|| panic!("{text:?}: src_to_vis({src}) is None (v={v})"));
                assert_eq!(back.vis, v, "{text:?}: P1 at v={v} {aff:?} via src {src}");
            }
        }

        // P2: src_to_vis is non-decreasing over the segment's source offsets.
        let mut last = 0;
        for o in seg.range.start..seg.range.end {
            let m = p.src_to_vis(o).expect("test: offset inside the segment");
            assert!(m.vis >= last, "{text:?}: P2 at src {o}");
            last = m.vis;
        }
        let end = p.src_to_vis(seg.range.end);
        assert_eq!(
            end.is_some(),
            seg.eol(text).is_none(),
            "{text:?}: range.end mapping"
        );
        if let Some(end) = end {
            assert!(end.vis >= last);
        }

        // Visible bytes agree with their source bytes through the mapping.
        for run in &p.runs {
            if let Run::Visible { src, vis_start } = run {
                for (k, o) in src.clone().enumerate() {
                    assert_eq!(
                        p.src_to_vis(o),
                        Some(SrcVis {
                            vis: vis_start + k,
                            eol: false
                        })
                    );
                    assert_eq!(p.vis_to_src(vis_start + k, Affinity::After), o);
                }
            }
        }
        if let Some(e) = seg.eol(text) {
            assert_eq!(
                p.src_to_vis(e),
                Some(SrcVis {
                    vis: p.visible.len(),
                    eol: true
                })
            );
        }

        // Spans: contiguous, nonempty, maximal, on char boundaries, covering the visible text.
        let mut at = 0;
        for (i, s) in p.spans.iter().enumerate() {
            assert_eq!(s.range.start, at, "{text:?}: spans not contiguous");
            assert!(s.range.end > s.range.start);
            assert!(p.visible.is_char_boundary(s.range.start));
            assert!(p.visible.is_char_boundary(s.range.end));
            assert_eq!(s.style.size, base_size(seg.kind), "I-7: size");
            if i > 0 {
                assert_ne!(p.spans[i - 1].style, s.style, "{text:?}: spans not maximal");
            }
            at = s.range.end;
        }
        assert_eq!(
            at,
            p.visible.len(),
            "{text:?}: spans do not cover the visible text"
        );

        // Style oracle: bold/italic of each visible byte follow the construct ranges
        // over its source byte.
        for run in &p.runs {
            if let Run::Visible { src, vis_start } = run {
                for (k, o) in src.clone().enumerate() {
                    let in_kind = |kind: &ConstructKind| {
                        seg.constructs
                            .iter()
                            .any(|c| &c.kind == kind && c.range.contains(&o))
                    };
                    let covering = p
                        .spans
                        .iter()
                        .find(|s| s.range.contains(&(vis_start + k)))
                        .expect("test: spans cover every visible byte");
                    assert_eq!(
                        covering.style.bold,
                        in_kind(&ConstructKind::Strong),
                        "bold at {o}"
                    );
                    assert_eq!(
                        covering.style.italic,
                        in_kind(&ConstructKind::Emphasis),
                        "italic at {o}"
                    );
                }
            }
        }
    }

    /// `(segment index, reveal flags)` pairs whose projection has already been checked.
    /// A projection depends only on those, so repeating one adds nothing.
    type Checked = std::collections::HashSet<(usize, Vec<bool>)>;

    fn check_document(
        text: &str,
        segs: &[Segment],
        o: usize,
        mode: Mode,
        policy: RevealPolicy,
        checked: &mut Checked,
    ) {
        let set = reveal(segs, o, mode, policy);
        let projs = project_all(text, segs, &set);
        for (si, (seg, p)) in segs.iter().zip(&projs).enumerate() {
            if checked.insert((si, set.for_segment(si).to_vec())) {
                check_projection(text, seg, p);
            }
        }
        // P3 / I-6.
        if let Err(e) = check_frame_invariant(&projs, o, mode) {
            panic!("{text:?} {mode:?} {o} {policy:?}: P3 {e}");
        }
    }

    // ---- literal tests on fixture lines ---------------------------------------

    const H2: &str = "## Roadmap with **bold** inside\n";
    const HS: f32 = BASE_SIZE * 1.6;

    #[test]
    fn heading_with_bold_fully_concealed() {
        // The cursor sits in another segment: nothing is revealed.
        let text = format!("{H2}\nfar away\n");
        let far = text.len() - 3;
        let p = seg_at(&text, far, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "Roadmap with bold inside");
        assert_eq!(
            p.runs,
            vec![
                conc(0..3),
                vis(3..16, 0),
                conc(16..18),
                vis(18..22, 13),
                conc(22..24),
                vis(24..31, 17),
                eol(31),
            ]
        );
        assert_eq!(
            p.spans,
            vec![
                span(0..13, HS, false, false),
                span(13..17, HS, true, false),
                span(17..24, HS, false, false),
            ]
        );
        assert_eq!(p.base_size, HS);
    }

    #[test]
    fn revealed_markers_keep_their_constructs_style() {
        // I-7: cursor in the bold, N1: only the strong is revealed. Its markers are
        // bold and heading-sized; the heading's `## ` stays concealed.
        let p = seg_at(H2, 18, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "Roadmap with **bold** inside");
        assert_eq!(p.runs, vec![conc(0..3), vis(3..31, 0), eol(31)]);
        assert_eq!(
            p.spans,
            vec![
                span(0..13, HS, false, false),
                span(13..21, HS, true, false),
                span(21..28, HS, false, false),
            ]
        );
        // N2 reveals the heading as well: its `#` markers are heading-sized, not bold.
        let p = seg_at(H2, 18, Mode::Normal, G1N2, 0);
        assert_eq!(p.visible, "## Roadmap with **bold** inside");
        assert_eq!(p.runs, vec![vis(0..31, 0), eol(31)]);
        assert_eq!(
            p.spans,
            vec![
                span(0..16, HS, false, false),
                span(16..24, HS, true, false),
                span(24..31, HS, false, false),
            ]
        );
    }

    #[test]
    fn heading_under_g1_and_g2_with_the_cursor_in_plain_text() {
        let p = seg_at(H2, 5, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "## Roadmap with bold inside");
        let p = seg_at(H2, 5, Mode::Normal, G2N1, 0);
        assert_eq!(p.visible, "Roadmap with bold inside");
    }

    #[test]
    fn heading_levels_scale_the_base_size() {
        for (level, scale) in [2.0, 1.6, 1.3, 1.15, 1.05, 1.0].into_iter().enumerate() {
            let text = format!("{} h\n", "#".repeat(level + 1));
            let p = seg_at(&text, 0, Mode::Normal, G1N1, 0);
            assert_eq!(p.base_size, BASE_SIZE * scale, "level {}", level + 1);
            assert!(p.spans.iter().all(|s| s.style.size == BASE_SIZE * scale));
        }
        assert_eq!(base_size(SegmentKind::Paragraph), BASE_SIZE);
        assert_eq!(base_size(SegmentKind::Raw), BASE_SIZE);
        assert_eq!(base_size(SegmentKind::Gap), BASE_SIZE);
    }

    #[test]
    fn both_concealed_and_revealed() {
        let text = "***both***\n";
        let p = seg_at(text, 20, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "both");
        assert_eq!(p.spans, vec![span(0..4, BASE_SIZE, true, true)]);
        // Normal cursor on `b`, N2: everything revealed; italic markers italic-only,
        // strong markers bold and italic.
        let p = seg_at(text, 5, Mode::Normal, G1N2, 0);
        assert_eq!(p.visible, "***both***");
        assert_eq!(
            p.spans,
            vec![
                span(0..1, BASE_SIZE, false, true),
                span(1..9, BASE_SIZE, true, true),
                span(9..10, BASE_SIZE, false, true),
            ]
        );
        // N1 reveals only the strong: the outer `*` markers stay concealed.
        let p = seg_at(text, 5, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "**both**");
        assert_eq!(p.runs, vec![conc(0..1), vis(1..9, 0), conc(9..10), eol(10)]);
        assert_eq!(p.spans, vec![span(0..8, BASE_SIZE, true, true)]);
    }

    #[test]
    fn unclosed_escaped_and_literal_backslash_n_are_plain() {
        for text in [
            "an unclosed **x marker\n",
            "the escape \\*not bold\\*\n",
            "literal \\n here and C:\\new\\path\n",
        ] {
            let p = seg_at(text, 0, Mode::Normal, G1N1, 0);
            assert_eq!(p.visible, text.trim_end_matches('\n'));
            assert_eq!(p.runs.len(), 2);
            assert_eq!(
                p.spans,
                vec![span(0..p.visible.len(), BASE_SIZE, false, false)]
            );
        }
    }

    #[test]
    fn raw_and_gap_segments_keep_their_text_minus_the_eol_byte() {
        let text = "```\n**a**\n```\n\n\n\nafter\n";
        let projs = projected(text, 0, Mode::Normal, G1N1);
        // Fence: internal newlines stay, only the final one is the EOL byte.
        assert_eq!(projs[0].visible, "```\n**a**\n```");
        assert_eq!(projs[0].runs, vec![vis(0..13, 0), eol(13)]);
        // Gap: three blank lines are three rows: two internal newlines and an EOL byte.
        assert_eq!(projs[1].visible, "\n\n");
        assert_eq!(projs[1].runs, vec![vis(14..16, 0), eol(16)]);
        assert!(projs[1].spans.iter().all(|s| s.style.size == BASE_SIZE));
    }

    #[test]
    fn empty_and_marker_only_headings_have_empty_visible_text() {
        // Concealed `##` leaves nothing visible but the EOL cell.
        let text = "##\n\nx\n";
        let p = seg_at(text, 4, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "");
        assert_eq!(p.runs, vec![conc(0..2), eol(2)]);
        assert!(p.spans.is_empty());
        assert_eq!(p.src_to_vis(0), Some(SrcVis { vis: 0, eol: false }));
        assert_eq!(p.src_to_vis(1), Some(SrcVis { vis: 0, eol: false }));
        assert_eq!(p.src_to_vis(2), Some(SrcVis { vis: 0, eol: true }));
        assert_eq!(p.src_to_vis(3), None);
        assert_eq!(p.vis_to_src(0, Affinity::Before), 0);
        assert_eq!(p.vis_to_src(0, Affinity::After), 2);
        // `## ` with trailing space, no newline: the whole line is the opening marker.
        // The bar at 3 (after the space) conceals it under G2; at 0 it would be revealed.
        let p = seg_at("## ", 3, Mode::Insert, G2N1, 0);
        assert_eq!(p.visible, "");
        assert_eq!(p.runs, vec![conc(0..3)]);
        assert_eq!(p.src_to_vis(3), Some(SrcVis { vis: 0, eol: false }));
        assert_eq!(p.vis_to_src(0, Affinity::After), 3);
    }

    #[test]
    fn closing_sequence_and_indented_heading_are_concealed() {
        let text = "### Title ###\n\nz\n";
        let p = seg_at(text, 16, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "Title");
        assert_eq!(p.runs, vec![conc(0..4), vis(4..9, 0), conc(9..13), eol(13)]);
        let text = "   ### Indented heading\n\nz\n";
        let p = seg_at(text, 26, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "Indented heading");
        assert_eq!(p.runs[0], conc(0..7));
    }

    #[test]
    fn mapping_across_concealed_markers() {
        // `a **b** c\n` with everything concealed: a0 ' '1 *2 *3 b4 *5 *6 ' '7 c8 \n9.
        let text = "a **b** c\n\nz\n";
        let p = seg_at(text, 12, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "a b c");
        assert_eq!(
            p.runs,
            vec![
                vis(0..2, 0),
                conc(2..4),
                vis(4..5, 2),
                conc(5..7),
                vis(7..9, 3),
                eol(9)
            ]
        );
        // src -> vis: collapse points for concealed bytes, EOL flagged.
        let want = [0, 1, 2, 2, 2, 3, 3, 3, 4, 5];
        for (o, v) in want.into_iter().enumerate() {
            let got = p.src_to_vis(o).expect("test: in range");
            assert_eq!(got.vis, v, "src {o}");
            assert_eq!(got.eol, o == 9);
        }
        // vis -> src: unique inside a run, affinity at concealed boundaries.
        assert_eq!(p.vis_to_src(1, Affinity::Before), 1);
        assert_eq!(p.vis_to_src(1, Affinity::After), 1);
        assert_eq!(p.vis_to_src(2, Affinity::Before), 2);
        assert_eq!(p.vis_to_src(2, Affinity::After), 4);
        assert_eq!(p.vis_to_src(3, Affinity::Before), 5);
        assert_eq!(p.vis_to_src(3, Affinity::After), 7);
        // At the end of the visible text: after the last byte, or the EOL byte.
        assert_eq!(p.vis_to_src(5, Affinity::Before), 9);
        assert_eq!(p.vis_to_src(5, Affinity::After), 9);
        // Out-of-range visible offsets clamp.
        assert_eq!(p.vis_to_src(99, Affinity::After), 9);
    }

    #[test]
    fn trailing_concealed_marker_splits_the_end_affinities() {
        // `a **b**` at EOF, no EOL byte: after `b` (5) vs the segment end (7).
        let text = "a **b**";
        let p = seg_at(text, 0, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "a b");
        assert_eq!(
            p.runs,
            vec![vis(0..2, 0), conc(2..4), vis(4..5, 2), conc(5..7)]
        );
        assert_eq!(p.vis_to_src(3, Affinity::Before), 5);
        assert_eq!(p.vis_to_src(3, Affinity::After), 7);
        assert_eq!(p.src_to_vis(7), Some(SrcVis { vis: 3, eol: false }));
        assert_eq!(p.src_to_vis(8), None);
        // With an EOL byte the segment end belongs to the next segment.
        let text = "a **b**\n\nz\n";
        let p = seg_at(text, 10, Mode::Normal, G1N1, 0);
        assert_eq!(p.vis_to_src(3, Affinity::After), 7);
        assert_eq!(p.src_to_vis(7), Some(SrcVis { vis: 3, eol: true }));
        assert_eq!(p.src_to_vis(8), None);
    }

    #[test]
    fn unicode_bytes_map_one_to_one() {
        let text = "x **thumbs 👍🏽 up** é\n\nz\n";
        let p = seg_at(text, text.len() - 2, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "x thumbs 👍🏽 up é");
        check_projection(text, &parse(text, 0).segments[0], &p);
        // The emoji's bytes are one visible run; every byte maps back to itself.
        let emoji = text.find('👍').expect("test: emoji");
        let v = p.visible.find('👍').expect("test: emoji");
        assert_eq!(p.src_to_vis(emoji).map(|m| m.vis), Some(v));
        assert_eq!(p.vis_to_src(v + 4, Affinity::Before), emoji + 4);
    }

    #[test]
    fn spans_split_at_construct_boundaries_across_runs() {
        // Two bold constructs separated by plain text: spans alternate, markers concealed.
        let text = "**a** b **c**\n\nz\n";
        let p = seg_at(text, 16, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "a b c");
        assert_eq!(
            p.spans,
            vec![
                span(0..1, BASE_SIZE, true, false),
                span(1..4, BASE_SIZE, false, false),
                span(4..5, BASE_SIZE, true, false),
            ]
        );
    }

    #[test]
    fn emphasis_inside_strong_splits_spans_at_the_inner_boundaries() {
        // Bold text with an italic word inside: three spans, markers concealed.
        let text = "**a *b* c**\n\nz\n";
        let p = seg_at(text, 16, Mode::Normal, G1N1, 0);
        assert_eq!(p.visible, "a b c");
        assert_eq!(
            p.spans,
            vec![
                span(0..2, BASE_SIZE, true, false),
                span(2..3, BASE_SIZE, true, true),
                span(3..5, BASE_SIZE, true, false),
            ]
        );
    }

    // ---- I-6 -------------------------------------------------------------------

    #[test]
    fn frame_invariant_holds_on_marker_bytes_and_catches_a_violation() {
        let text = "a **b** c\n";
        // Normal cursor on each marker byte: that construct is revealed.
        for o in [2, 3, 5, 6] {
            let projs = projected(text, o, Mode::Normal, G1N1);
            assert_eq!(
                check_frame_invariant(&projs, o, Mode::Normal),
                Ok(()),
                "{o}"
            );
        }
        // Projections computed for a *different* cursor conceal the byte: the check fires.
        let projs = projected(text, 0, Mode::Normal, G1N1);
        let err = check_frame_invariant(&projs, 2, Mode::Normal).expect_err("test: I-6 violated");
        assert!(err.contains("I-6"), "{err}");
        // An Insert bar strictly inside a concealed marker is a violation; at its edge it is not.
        assert!(check_frame_invariant(&projs, 3, Mode::Insert).is_err());
        assert_eq!(check_frame_invariant(&projs, 2, Mode::Insert), Ok(()));
        assert_eq!(check_frame_invariant(&projs, 4, Mode::Insert), Ok(()));
    }

    #[test]
    fn adjacent_concealed_markers_at_a_bar_boundary() {
        // Characterization (F-10): `**a**__b__` with the bar at 5 reveals neither strong
        // (strict rule), so the close `**` (3..5) and open `__` (5..7) are both concealed
        // and adjacent. With one run per marker the bar sits on a run edge and I-6 holds;
        // if concealed runs were merged into 3..7 the bar would be strictly inside it.
        let text = "**a**__b__\n";
        let segs = parse(text, 0).segments;
        assert_eq!(segs[0].constructs.len(), 2, "{:?}", segs[0].constructs);
        let projs = projected(text, 5, Mode::Insert, G1N1);
        assert_eq!(check_frame_invariant(&projs, 5, Mode::Insert), Ok(()));
        let concealed: Vec<_> = projs[0]
            .runs
            .iter()
            .filter_map(|r| match r {
                Run::Concealed { src } => Some(src.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(concealed, vec![0..2, 3..5, 5..7, 8..10]);
        let merged_zone = 3..7;
        assert!(merged_zone.start < 5 && 5 < merged_zone.end);
    }

    #[test]
    fn project_tolerates_missing_flags_and_stray_inputs() {
        let text = "**a**\n";
        let seg = &parse(text, 0).segments[0];
        // No flags at all: everything concealed.
        let p = project(text, seg, &[]);
        assert_eq!(p.visible, "a");
        // Too many flags are ignored.
        let p = project(text, seg, &[true, true, true]);
        assert_eq!(p.visible, "**a**");
        assert_eq!(p.src_to_vis(99), None);
    }

    // ---- exhaustive and property checks -----------------------------------------

    fn check_every_cursor(text: &str) {
        let segs = parse(text, 0).segments;
        let mut checked = Checked::new();
        for policy in POLICIES {
            for o in normal_cursors(text) {
                check_document(text, &segs, o, Mode::Normal, policy, &mut checked);
            }
            for o in insert_bars(text) {
                check_document(text, &segs, o, Mode::Insert, policy, &mut checked);
            }
        }
    }

    #[test]
    fn fixtures_hold_every_property_at_every_valid_cursor() {
        for text in [
            include_str!("../fixtures/spike.md"),
            include_str!("../fixtures/no_trailing_newline.md"),
            include_str!("../fixtures/empty.md"),
        ] {
            check_every_cursor(text);
        }
    }

    #[test]
    fn random_reveal_flags_keep_the_run_table_a_partition() {
        // P4 for reveal sets that no cursor would produce: every flag combination on
        // a few constructs-heavy lines.
        for text in [
            "# **a** *b* ***c*** ##\n",
            "x **a *b* c** y\n",
            "**a**__b__\n",
            "   ## **h**\n",
        ] {
            let seg = &parse(text, 0).segments[0];
            let n = seg.constructs.len();
            for mask in 0..1u32 << n {
                let flags: Vec<bool> = (0..n).map(|i| mask >> i & 1 == 1).collect();
                check_projection(text, seg, &project(text, seg, &flags));
            }
        }
    }

    fn atom() -> impl Strategy<Value = &'static str> {
        // The T3 alphabet plus a few multi-byte atoms so constructs and headings are common.
        prop::sample::select(vec![
            "a", "é", "e\u{301}", "👍🏽", " ", "*", "_", "#", "\n", "\\", "n", "**", "# ", "*a*",
        ])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        #[test]
        fn all_properties_with_random_valid_cursors(
            atoms in prop::collection::vec(atom(), 0..40),
            normal in any::<prop::sample::Index>(),
            insert in any::<prop::sample::Index>(),
        ) {
            let text = atoms.concat();
            let normals = normal_cursors(&text);
            let inserts = insert_bars(&text);
            let n = normals[normal.index(normals.len())];
            let i = inserts[insert.index(inserts.len())];
            let segs = parse(&text, 0).segments;
            let mut checked = Checked::new();
            for policy in POLICIES {
                check_document(&text, &segs, n, Mode::Normal, policy, &mut checked);
                check_document(&text, &segs, i, Mode::Insert, policy, &mut checked);
            }
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        #[test]
        fn every_cursor_on_small_documents(atoms in prop::collection::vec(atom(), 0..14)) {
            check_every_cursor(&atoms.concat());
        }

        #[test]
        fn arbitrary_flags_keep_a_partition(
            atoms in prop::collection::vec(atom(), 0..30),
            flags in prop::collection::vec(any::<bool>(), 0..8),
        ) {
            let text = atoms.concat();
            let segs = parse(&text, 0).segments;
            for seg in &segs {
                let f: Vec<bool> = (0..seg.constructs.len())
                    .map(|i| flags.get(i % flags.len().max(1)).copied().unwrap_or(false))
                    .collect();
                check_projection(&text, seg, &project(&text, seg, &f));
            }
        }
    }
}
