//! Reveal: which constructs show their markers for a cursor (C-4).

// Consumed by later tasks (projection, widget); remove once T3+ use every item.
#![allow(dead_code)]

use std::ops::Range;

use crate::parse::{Construct, ConstructKind, Segment};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Block cursor on a grapheme.
    Normal,
    /// Bar between graphemes.
    Insert,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadingPolicy {
    /// Reveal when the cursor is anywhere on the heading line.
    G1,
    /// Reveal only when the cursor is in a marker.
    G2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NestingPolicy {
    /// Innermost: of the constructs whose own condition holds, only the smallest.
    N1,
    /// Ancestor chain: every construct whose own condition holds, plus its ancestors.
    N2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevealPolicy {
    pub heading: HeadingPolicy,
    pub nesting: NestingPolicy,
}

/// Per segment, one flag per construct (parallel to `Segment::constructs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevealSet {
    per_segment: Vec<Vec<bool>>,
}

impl RevealSet {
    fn none(segments: &[Segment]) -> Self {
        Self {
            per_segment: segments
                .iter()
                .map(|s| vec![false; s.constructs.len()])
                .collect(),
        }
    }

    fn mark(&mut self, segment: usize, construct: usize) {
        if let Some(flag) = self
            .per_segment
            .get_mut(segment)
            .and_then(|flags| flags.get_mut(construct))
        {
            *flag = true;
        }
    }

    pub fn is_revealed(&self, segment: usize, construct: usize) -> bool {
        self.for_segment(segment)
            .get(construct)
            .copied()
            .unwrap_or(false)
    }

    /// The flags for one segment's constructs; empty for an unknown segment.
    pub fn for_segment(&self, segment: usize) -> &[bool] {
        self.per_segment.get(segment).map_or(&[], Vec::as_slice)
    }

    pub fn count(&self) -> usize {
        self.per_segment
            .iter()
            .map(|flags| flags.iter().filter(|&&f| f).count())
            .sum()
    }
}

fn strictly_inside(range: &Range<usize>, o: usize) -> bool {
    range.start < o && o < range.end
}

/// A construct's own reveal condition (the C-4 table), ignoring nesting.
fn holds(seg: &Segment, c: &Construct, o: usize, mode: Mode, heading: HeadingPolicy) -> bool {
    match &c.kind {
        ConstructKind::Strong | ConstructKind::Emphasis => match mode {
            Mode::Normal => c.range.contains(&o),
            Mode::Insert => strictly_inside(&c.range, o),
        },
        // The heading construct's range is the segment minus its EOL byte, so its
        // end is the C-4 `line_end` and its start is `line_start`.
        ConstructKind::Heading { hash_run } => match (heading, mode) {
            (HeadingPolicy::G1, Mode::Normal) => seg.range.contains(&o),
            (HeadingPolicy::G1, Mode::Insert) => c.range.start <= o && o <= c.range.end,
            (HeadingPolicy::G2, Mode::Normal) => c.open.contains(&o) || c.close.contains(&o),
            (HeadingPolicy::G2, Mode::Insert) => {
                (c.range.start <= o && o <= hash_run.end)
                    || strictly_inside(&c.open, o)
                    || strictly_inside(&c.close, o)
            }
        },
    }
}

/// Computes the reveal set for a cursor (Normal) or bar (Insert) at `cursor`.
// I-1: `cursor` is a source byte offset; nothing here is in view coordinates.
pub fn reveal(segments: &[Segment], cursor: usize, mode: Mode, policy: RevealPolicy) -> RevealSet {
    let mut set = RevealSet::none(segments);

    // Only segments whose range touches `cursor` can hold a construct that holds there.
    let first = segments.partition_point(|s| s.range.end < cursor);
    let mut holders: Vec<(usize, usize)> = Vec::new();
    for (si, seg) in segments.iter().enumerate().skip(first) {
        if seg.range.start > cursor {
            break;
        }
        for (ci, c) in seg.constructs.iter().enumerate() {
            if holds(seg, c, cursor, mode, policy.heading) {
                holders.push((si, ci));
            }
        }
    }

    match policy.nesting {
        NestingPolicy::N1 => {
            let innermost = holders
                .iter()
                .min_by_key(|&&(si, ci)| segments[si].constructs[ci].range.len());
            if let Some(&(si, ci)) = innermost {
                set.mark(si, ci);
            }
        }
        NestingPolicy::N2 => {
            for (si, ci) in holders {
                let mut at = Some(ci);
                while let Some(i) = at {
                    set.mark(si, i);
                    at = segments[si].constructs[i].parent;
                }
            }
        }
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;
    use proptest::prelude::*;

    const G1: HeadingPolicy = HeadingPolicy::G1;
    const G2: HeadingPolicy = HeadingPolicy::G2;
    const N1: NestingPolicy = NestingPolicy::N1;
    const N2: NestingPolicy = NestingPolicy::N2;

    /// The four policy combinations, in the order the tables list them.
    const COMBOS: [(HeadingPolicy, NestingPolicy); 4] = [(G1, N1), (G1, N2), (G2, N1), (G2, N2)];

    fn policy(heading: HeadingPolicy, nesting: NestingPolicy) -> RevealPolicy {
        RevealPolicy { heading, nesting }
    }

    /// Ranges of the revealed constructs, sorted.
    fn revealed(text: &str, o: usize, mode: Mode, p: RevealPolicy) -> Vec<(usize, usize)> {
        let segs = parse(text, 0).segments;
        let set = reveal(&segs, o, mode, p);
        let mut out = Vec::new();
        for (si, seg) in segs.iter().enumerate() {
            for (ci, c) in seg.constructs.iter().enumerate() {
                if set.is_revealed(si, ci) {
                    out.push((c.range.start, c.range.end));
                }
            }
        }
        out.sort_unstable();
        out
    }

    type Expect<'a> = &'a [(usize, usize)];

    /// `expect` lists the revealed constructs for G1+N1, G1+N2, G2+N1, G2+N2.
    fn check(text: &str, mode: Mode, o: usize, expect: [Expect; 4]) {
        for ((h, n), want) in COMBOS.into_iter().zip(expect) {
            assert_eq!(
                revealed(text, o, mode, policy(h, n)),
                want,
                "{text:?} {mode:?} offset {o} {h:?}+{n:?}"
            );
        }
    }

    /// The same expectation under all four combinations.
    fn check_all(text: &str, mode: Mode, o: usize, expect: Expect) {
        check(text, mode, o, [expect; 4]);
    }

    use Mode::{Insert, Normal};

    // ---- design-flow states ------------------------------------------------

    #[test]
    fn empty_heading_with_bar_after_hashes_is_revealed_under_both() {
        // `##` with the bar at 2: the bar must keep `##` visible while it is typed.
        check_all("##", Insert, 2, &[(0, 2)]);
        check_all("##\n", Insert, 2, &[(0, 2)]);
        check_all("##\n", Insert, 0, &[(0, 2)]);
    }

    #[test]
    fn heading_with_bar_after_the_space_conceals_under_g2_and_g1_keeps_it() {
        // `## ` with the bar at 3: G2 conceals (design flow step 5); G1 gives the whole line.
        for text in ["## ", "## \n"] {
            check(
                text,
                Insert,
                3,
                [&[(0, 3)], &[(0, 3)], &[], &[]], // G1+N1, G1+N2, G2+N1, G2+N2
            );
            // Still revealed under both with the bar at 2, before the space.
            check_all(text, Insert, 2, &[(0, 3)]);
        }
    }

    #[test]
    fn strong_at_end_of_line_bar_13_conceals_and_bar_12_reveals() {
        let text = "**important**";
        check_all(text, Insert, 13, &[]);
        check_all(text, Insert, 12, &[(0, 13)]);
        check_all(text, Insert, 1, &[(0, 13)]);
        check_all(text, Insert, 0, &[]);
        // Normal cursor on the first and last marker byte.
        check_all(text, Normal, 0, &[(0, 13)]);
        check_all(text, Normal, 12, &[(0, 13)]);
    }

    // ---- inline constructs ------------------------------------------------

    #[test]
    fn both_under_n1_and_n2() {
        // `***both***`: pulldown nests Emphasis 0..10 around Strong 1..9 (F-1).
        let text = "***both***\n";
        let (e, s) = ((0, 10), (1, 9));
        // Normal: the outer marker bytes belong only to the outer construct.
        check_all(text, Normal, 0, &[e]);
        check_all(text, Normal, 9, &[e]);
        for o in [1, 5, 8] {
            check(text, Normal, o, [&[s], &[e, s], &[s], &[e, s]]);
        }
        // Insert: strictly inside.
        check_all(text, Insert, 0, &[]);
        check_all(text, Insert, 1, &[e]);
        check_all(text, Insert, 9, &[e]);
        check_all(text, Insert, 10, &[]);
        for o in [2, 5, 8] {
            check(text, Insert, o, [&[s], &[e, s], &[s], &[e, s]]);
        }
    }

    #[test]
    fn adjacent_strongs_reveal_independently() {
        let text = "**a** **b**\n";
        check_all(text, Normal, 1, &[(0, 5)]);
        check_all(text, Normal, 5, &[]); // the space between
        check_all(text, Normal, 6, &[(6, 11)]);
        // A bar between them is strictly inside neither.
        check_all(text, Insert, 5, &[]);
        check_all(text, Insert, 6, &[]);
    }

    #[test]
    fn emphasis_inside_strong() {
        let text = "**a *b* c**\n";
        let (outer, inner) = ((0, 11), (4, 7));
        check(
            text,
            Normal,
            5,
            [&[inner], &[outer, inner], &[inner], &[outer, inner]],
        );
        check_all(text, Normal, 2, &[outer]);
    }

    // ---- headings ----------------------------------------------------------

    /// `## Roadmap with **bold** inside\n`: heading 0..31 (open 0..3), strong 16..24.
    const H2: &str = "## Roadmap with **bold** inside\n";

    #[test]
    fn heading_with_bold_normal() {
        let (h, s) = ((0, 31), (16, 24));
        check_all(H2, Normal, 0, &[h]);
        check_all(H2, Normal, 2, &[h]);
        // Text outside the markers: G1 reveals the heading, G2 does not.
        for o in [3, 5, 15, 24, 30, 31] {
            check(H2, Normal, o, [&[h], &[h], &[], &[]]);
        }
        // Inside bold: N1 reveals only the bold, N2 the chain.
        for o in [16, 18, 23] {
            check(H2, Normal, o, [&[s], &[h, s], &[s], &[h, s]]);
        }
    }

    #[test]
    fn heading_with_bold_insert() {
        let (h, s) = ((0, 31), (16, 24));
        check_all(H2, Insert, 0, &[h]);
        check_all(H2, Insert, 2, &[h]);
        for o in [3, 16, 24, 31] {
            check(H2, Insert, o, [&[h], &[h], &[], &[]]);
        }
        // Between the two `*` of the opening marker: strictly inside the strong.
        for o in [17, 20, 23] {
            check(H2, Insert, o, [&[s], &[h, s], &[s], &[h, s]]);
        }
        // After the EOL byte: past `line_end`.
        check_all(H2, Insert, 32, &[]);
    }

    #[test]
    fn heading_with_closing_sequence() {
        // `### Title ###\n`: opening 0..4, closing 9..13, line_end 13, hash run 0..3.
        let text = "### Title ###\n";
        let h = (0, 13);
        for o in [0, 1, 2, 3] {
            check_all(text, Normal, o, &[h]);
        }
        check(text, Normal, 4, [&[h], &[h], &[], &[]]);
        check(text, Normal, 8, [&[h], &[h], &[], &[]]);
        for o in [9, 12] {
            check_all(text, Normal, o, &[h]);
        }
        check(text, Normal, 13, [&[h], &[h], &[], &[]]); // the EOL cell
        // Insert under G2: after the hash run, or strictly inside either marker.
        check_all(text, Insert, 3, &[h]);
        check(text, Insert, 4, [&[h], &[h], &[], &[]]);
        check(text, Insert, 9, [&[h], &[h], &[], &[]]); // closing marker start: not strictly inside
        check_all(text, Insert, 10, &[h]);
        check_all(text, Insert, 12, &[h]);
        check(text, Insert, 13, [&[h], &[h], &[], &[]]);
        check_all(text, Insert, 14, &[]);
    }

    #[test]
    fn empty_heading_normal() {
        let h = (0, 2);
        check_all("##\n", Normal, 0, &[h]);
        check_all("##\n", Normal, 1, &[h]);
        // The EOL cell is outside both markers: G1 keeps it, G2 conceals.
        check("##\n", Normal, 2, [&[h], &[h], &[], &[]]);
        check_all("##\n", Insert, 3, &[]);
    }

    #[test]
    fn bold_inside_heading_with_both_nested() {
        // `# ***x***\n`: heading 0..9, Emphasis 2..9, Strong 3..8.
        let text = "# ***x***\n";
        let (h, e, s) = ((0, 9), (2, 9), (3, 8));
        check_all(text, Normal, 0, &[h]);
        check(text, Normal, 2, [&[e], &[h, e], &[e], &[h, e]]);
        check(text, Normal, 5, [&[s], &[h, e, s], &[s], &[h, e, s]]);
        check(text, Normal, 8, [&[e], &[h, e], &[e], &[h, e]]);
    }

    // ---- segments ----------------------------------------------------------

    #[test]
    fn only_the_cursors_segment_reveals() {
        // Heading 0..4 (construct 0..3), gap 4..5, paragraph 5..11 (strong 5..10).
        let text = "# a\n\n**b**\n";
        check_all(text, Normal, 0, &[(0, 3)]);
        check(text, Normal, 3, [&[(0, 3)], &[(0, 3)], &[], &[]]);
        check_all(text, Normal, 4, &[]);
        check_all(text, Normal, 7, &[(5, 10)]);
        // A bar on the boundary between segments is strictly inside neither.
        check_all(text, Insert, 4, &[]);
        check_all(text, Insert, 5, &[]);
        check_all(text, Insert, 6, &[(5, 10)]);
    }

    #[test]
    fn raw_and_gap_segments_never_reveal() {
        for text in ["- **a**\n", "```\n**a**\n```\n", "Setext\n---\n", "\n\n\n"] {
            for o in 0..=text.len() {
                for mode in [Normal, Insert] {
                    for (h, n) in COMBOS {
                        assert!(revealed(text, o, mode, policy(h, n)).is_empty());
                    }
                }
            }
        }
    }

    #[test]
    fn empty_document_and_out_of_range_cursor() {
        let p = policy(G1, N2);
        assert_eq!(reveal(&[], 0, Normal, p).count(), 0);
        assert_eq!(reveal(&[], 0, Insert, p).count(), 0);
        let segs = parse("**a**\n", 0).segments;
        assert_eq!(reveal(&segs, 999, Insert, p).count(), 0);
        assert_eq!(reveal(&segs, 999, Normal, p).count(), 0);
        assert!(!reveal(&segs, 0, Normal, p).is_revealed(7, 7));
        assert!(reveal(&segs, 0, Normal, p).for_segment(7).is_empty());
    }

    // ---- exhaustive and property checks --------------------------------------

    /// Every offset of a document, in both modes, under every policy.
    fn check_invariants(text: &str) {
        let segs = parse(text, 0).segments;
        for o in 0..=text.len() + 1 {
            for mode in [Normal, Insert] {
                for h in [G1, G2] {
                    let n1 = reveal(&segs, o, mode, policy(h, N1));
                    let n2 = reveal(&segs, o, mode, policy(h, N2));
                    assert!(
                        n1.count() <= 1,
                        "{text:?} {o} {mode:?} {h:?}: N1 reveals >1"
                    );
                    for (si, seg) in segs.iter().enumerate() {
                        for (ci, c) in seg.constructs.iter().enumerate() {
                            // N1 is a subset of N2.
                            assert!(!n1.is_revealed(si, ci) || n2.is_revealed(si, ci));
                            // N2 is closed under ancestors.
                            if n2.is_revealed(si, ci)
                                && let Some(p) = c.parent
                            {
                                assert!(n2.is_revealed(si, p), "{text:?} {o}: ancestor missing");
                            }
                        }
                    }
                    // The windowed search agrees with a scan over every segment.
                    let oracle = full_scan(&segs, o, mode, policy(h, N2));
                    assert_eq!(n2, oracle, "{text:?} {o} {mode:?} {h:?}: N2 vs full scan");
                    let oracle = full_scan(&segs, o, mode, policy(h, N1));
                    assert_eq!(n1, oracle, "{text:?} {o} {mode:?} {h:?}: N1 vs full scan");
                }
            }
        }
    }

    /// Reference implementation with no windowing: test every construct of every segment.
    fn full_scan(segs: &[Segment], o: usize, mode: Mode, p: RevealPolicy) -> RevealSet {
        let mut set = RevealSet::none(segs);
        let mut holders = Vec::new();
        for (si, seg) in segs.iter().enumerate() {
            for (ci, c) in seg.constructs.iter().enumerate() {
                if holds(seg, c, o, mode, p.heading) {
                    holders.push((si, ci));
                }
            }
        }
        match p.nesting {
            N1 => {
                let mut best: Option<(usize, usize)> = None;
                for &(si, ci) in &holders {
                    let len = segs[si].constructs[ci].range.len();
                    if best.is_none_or(|(bs, bc)| len < segs[bs].constructs[bc].range.len()) {
                        best = Some((si, ci));
                    }
                }
                if let Some((si, ci)) = best {
                    set.mark(si, ci);
                }
            }
            N2 => {
                for (si, ci) in holders {
                    let mut at = Some(ci);
                    while let Some(i) = at {
                        set.mark(si, i);
                        at = segs[si].constructs[i].parent;
                    }
                }
            }
        }
        set
    }

    #[test]
    fn fixture_invariants_at_every_offset() {
        for text in [
            include_str!("../fixtures/spike.md"),
            include_str!("../fixtures/no_trailing_newline.md"),
            include_str!("../fixtures/empty.md"),
        ] {
            check_invariants(text);
        }
    }

    fn atom() -> impl Strategy<Value = &'static str> {
        // The T1 alphabet plus a few multi-byte atoms so constructs and headings appear often.
        prop::sample::select(vec![
            "a", "é", "e\u{301}", "👍🏽", " ", "*", "_", "#", "\n", "\\", "n", "**", "# ", "*a*",
        ])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        #[test]
        fn reveal_invariants_on_generated_documents(
            atoms in prop::collection::vec(atom(), 0..40)
        ) {
            check_invariants(&atoms.concat());
        }
    }
}
