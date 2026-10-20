//! Horizontal motions (C-7): pure functions resolved against the projection for the current cursor.

use crate::doc::{next_grapheme_boundary, prev_grapheme_boundary};
use crate::projection::{Projection, Run};
use crate::reveal::Mode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
}

/// Whether the byte at `o` lies in a concealed run of the projections.
fn is_concealed(projections: &[Projection], o: usize) -> bool {
    let i = projections.partition_point(|p| p.range.end <= o);
    let Some(p) = projections.get(i).filter(|p| p.range.contains(&o)) else {
        return false;
    };
    let r = p.runs.partition_point(|r| r.src().end <= o);
    matches!(p.runs.get(r), Some(Run::Concealed { .. }))
}

/// The first visible grapheme starting at or after `from`, as `(start, end)`.
// I-6: a grapheme counts as visible if its first byte is not concealed (EOL cells
// are visible), so a concealed marker is never a landing spot.
fn first_visible_from(
    text: &str,
    projections: &[Projection],
    from: usize,
) -> Option<(usize, usize)> {
    let mut start = from;
    while let Some(end) = next_grapheme_boundary(text, start) {
        if !is_concealed(projections, start) {
            return Some((start, end));
        }
        start = end;
    }
    None
}

/// The start of the last visible grapheme that ends at or before `from`.
// I-6
fn last_visible_before(text: &str, projections: &[Projection], from: usize) -> Option<usize> {
    let mut end = from;
    while let Some(start) = prev_grapheme_boundary(text, end) {
        if !is_concealed(projections, start) {
            return Some(start);
        }
        end = start;
    }
    None
}

/// One horizontal motion from `cursor`, resolved against `projections`, which must be
/// the projections computed for `cursor` (the snapping contract, C-6).
///
/// - Normal `h`/`l` move to the previous/next visible grapheme start.
/// - Insert `Left` moves to the boundary before the previous visible grapheme, `Right`
///   to the boundary after the next visible grapheme.
///
/// Returns `cursor` unchanged at either end of the document. The caller clears
/// `goal_x` for every horizontal motion, moved or not (C-7).
// I-1: `cursor` and the result are source byte offsets on grapheme boundaries.
pub fn move_horizontal(
    text: &str,
    projections: &[Projection],
    cursor: usize,
    mode: Mode,
    dir: Direction,
) -> usize {
    let moved = match (dir, mode) {
        // Moving left lands on the previous visible grapheme's start in both modes.
        (Direction::Left, _) => last_visible_before(text, projections, cursor),
        (Direction::Right, Mode::Normal) => next_grapheme_boundary(text, cursor)
            .and_then(|after| first_visible_from(text, projections, after))
            .map(|(start, _)| start),
        (Direction::Right, Mode::Insert) => {
            first_visible_from(text, projections, cursor).map(|(_, end)| end)
        }
    };
    moved.unwrap_or(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{grapheme_starts, insert_bars, normal_cursors};
    use crate::parse::{ConstructKind, Segment, parse};
    use crate::projection::{check_frame_invariant, project_all};
    use crate::reveal::{HeadingPolicy, NestingPolicy, RevealPolicy, reveal};
    use proptest::prelude::*;
    use std::collections::HashSet;

    use Direction::{Left as L, Right as R};
    use Mode::{Insert, Normal};

    const fn pol(heading: HeadingPolicy, nesting: NestingPolicy) -> RevealPolicy {
        RevealPolicy { heading, nesting }
    }

    const G1N1: RevealPolicy = pol(HeadingPolicy::G1, NestingPolicy::N1);
    const G1N2: RevealPolicy = pol(HeadingPolicy::G1, NestingPolicy::N2);
    const G2N1: RevealPolicy = pol(HeadingPolicy::G2, NestingPolicy::N1);
    const G2N2: RevealPolicy = pol(HeadingPolicy::G2, NestingPolicy::N2);
    const POLICIES: [RevealPolicy; 4] = [G1N1, G1N2, G2N1, G2N2];

    /// One motion, with the projections recomputed for the cursor first.
    fn step(
        text: &str,
        segs: &[Segment],
        cursor: usize,
        mode: Mode,
        policy: RevealPolicy,
        dir: Direction,
    ) -> usize {
        let set = reveal(segs, cursor, mode, policy);
        let projs = project_all(text, segs, &set);
        move_horizontal(text, &projs, cursor, mode, dir)
    }

    /// The cursor after each motion, starting with `start`.
    fn walk(
        text: &str,
        mode: Mode,
        policy: RevealPolicy,
        start: usize,
        dirs: &[Direction],
    ) -> Vec<usize> {
        let segs = parse(text, 0).segments;
        let mut at = vec![start];
        for &d in dirs {
            let last = *at.last().expect("test: seeded");
            at.push(step(text, &segs, last, mode, policy, d));
        }
        at
    }

    /// Checks one walk under every policy.
    fn walk_all(text: &str, mode: Mode, start: usize, dirs: &[Direction], want: &[usize]) {
        for policy in POLICIES {
            assert_eq!(
                walk(text, mode, policy, start, dirs),
                want,
                "{text:?} {mode:?} {policy:?}"
            );
        }
    }

    // ---- literal sequences on `a**b**c` -----------------------------------------

    /// `a`0 `*`1 `*`2 `b`3 `*`4 `*`5 `c`6
    const ABC: &str = "a**b**c";

    #[test]
    fn normal_l_walks_over_revealed_markers_and_skips_concealed_ones() {
        // 0 -> 3 skips the concealed `**`; once the cursor is in the bold they are visible.
        walk_all(ABC, Normal, 0, &[R; 5], &[0, 3, 4, 5, 6, 6]);
    }

    #[test]
    fn normal_h_from_the_end_lands_on_the_bold_then_walks_the_markers() {
        walk_all(ABC, Normal, 6, &[L; 5], &[6, 3, 2, 1, 0, 0]);
    }

    #[test]
    fn insert_right_sequence() {
        // 1 -> 4: from the bar before the concealed `**`, the next visible grapheme is `b`.
        walk_all(ABC, Insert, 0, &[R; 6], &[0, 1, 4, 5, 6, 7, 7]);
    }

    #[test]
    fn insert_left_sequence() {
        walk_all(ABC, Insert, 7, &[L; 6], &[7, 6, 3, 2, 1, 0, 0]);
    }

    #[test]
    fn horizontal_motions_are_not_inverses_of_each_other() {
        // `l` from 0 jumps to 3; `h` from 3 walks back through 2 and 1 (F-13).
        walk_all(ABC, Normal, 0, &[R, L], &[0, 3, 2]);
        // Insert: Right 1 -> 4, but Left from 4 goes to 3.
        walk_all(ABC, Insert, 1, &[R, L], &[1, 4, 3]);
    }

    // ---- crossing segments, EOL cells, Unicode ---------------------------------

    #[test]
    fn motions_cross_segments_through_eol_cells_and_gap_rows() {
        // a0 b1 \n2 | \n3 (gap) | c4 d5 \n6
        let text = "ab\n\ncd\n";
        walk_all(text, Normal, 0, &[R; 7], &[0, 1, 2, 3, 4, 5, 6, 6]);
        walk_all(text, Normal, 6, &[L; 7], &[6, 5, 4, 3, 2, 1, 0, 0]);
        walk_all(text, Insert, 0, &[R; 8], &[0, 1, 2, 3, 4, 5, 6, 7, 7]);
        walk_all(text, Insert, 7, &[L; 8], &[7, 6, 5, 4, 3, 2, 1, 0, 0]);
    }

    #[test]
    fn motions_land_on_grapheme_starts_only() {
        // e + combining acute (0..3), thumbs-up with skin tone (3..11), x (11).
        let text = "e\u{301}👍🏽x";
        walk_all(text, Normal, 0, &[R; 3], &[0, 3, 11, 11]);
        walk_all(text, Normal, 11, &[L; 3], &[11, 3, 0, 0]);
        walk_all(text, Insert, 0, &[R; 4], &[0, 3, 11, 12, 12]);
        walk_all(text, Insert, 12, &[L; 4], &[12, 11, 3, 0, 0]);
    }

    #[test]
    fn nothing_moves_in_an_empty_document_or_from_a_stray_offset() {
        for mode in [Normal, Insert] {
            for dir in [L, R] {
                assert_eq!(move_horizontal("", &[], 0, mode, dir), 0);
            }
        }
        // Beyond the end and mid-character offsets are no-ops, not panics.
        let text = "é";
        let segs = parse(text, 0).segments;
        let projs = project_all(text, &segs, &reveal(&segs, 0, Normal, G1N1));
        for mode in [Normal, Insert] {
            for dir in [L, R] {
                assert_eq!(move_horizontal(text, &projs, 99, mode, dir), 99);
                assert_eq!(move_horizontal(text, &projs, 1, mode, dir), 1);
            }
        }
    }

    // ---- characterization: heading markers ---------------------------------------

    // These tests document current behavior on purpose (CLAUDE.md): do not "fix" them.

    #[test]
    fn g1_normal_h_walks_onto_heading_markers_but_l_from_above_skips_them() {
        // a0 \n1 \n2 #3 #4 ' '5 T6 \n7
        let text = "a\n\n## T\n";
        // Entering from the line above: the heading is concealed, so `l` lands on `T`.
        for policy in [G1N1, G1N2, G2N1, G2N2] {
            assert_eq!(walk(text, Normal, policy, 2, &[R]), [2, 6], "{policy:?}");
        }
        // Under G1 the cursor on `T` reveals the heading and `h` walks through `## `.
        for policy in [G1N1, G1N2] {
            assert_eq!(walk(text, Normal, policy, 6, &[L; 4]), [6, 5, 4, 3, 2]);
        }
    }

    #[test]
    fn g2_normal_h_cannot_reach_heading_markers() {
        let text = "a\n\n## T\n";
        // Under G2 the cursor on `T` is not in a marker, so `## ` stays concealed
        // and `h` skips straight to the line above.
        for policy in [G2N1, G2N2] {
            assert_eq!(walk(text, Normal, policy, 6, &[L]), [6, 2], "{policy:?}");
        }
        // At the start of the document there is nothing visible to the left: no-op.
        let text = "## Title\n";
        for policy in [G2N1, G2N2] {
            assert_eq!(
                walk(text, Normal, policy, 3, &[L, L]),
                [3, 3, 3],
                "{policy:?}"
            );
        }
        for policy in [G1N1, G1N2] {
            assert_eq!(walk(text, Normal, policy, 3, &[L; 4]), [3, 2, 1, 0, 0]);
        }
    }

    #[test]
    fn g2_one_way_door_once_in_the_markers_h_and_l_leave_them_for_good() {
        // Starting on a marker (reachable by Esc, not by h/l): the heading is revealed,
        // the walk crosses the markers, and after leaving them they cannot be re-entered.
        let text = "## Title\n";
        for policy in [G2N1, G2N2] {
            assert_eq!(
                walk(text, Normal, policy, 0, &[R; 4]),
                [0, 1, 2, 3, 4],
                "{policy:?}"
            );
            assert_eq!(walk(text, Normal, policy, 3, &[L]), [3, 3], "{policy:?}");
        }
    }

    #[test]
    fn g2_insert_left_from_after_the_space_cannot_enter_the_markers() {
        // Bar at 3 in `## Title`: G2 conceals the markers, so Left has nothing to move over.
        let text = "## Title\n";
        for policy in [G2N1, G2N2] {
            assert_eq!(walk(text, Insert, policy, 3, &[L]), [3, 3], "{policy:?}");
            // With the bar at 2, before the space, the heading is revealed.
            assert_eq!(
                walk(text, Insert, policy, 2, &[L, L]),
                [2, 1, 0],
                "{policy:?}"
            );
        }
        for policy in [G1N1, G1N2] {
            assert_eq!(walk(text, Insert, policy, 3, &[L; 4]), [3, 2, 1, 0, 0]);
        }
    }

    #[test]
    fn bold_first_heading_reaches_its_markers_only_under_n2() {
        // #0 #1 ' '2 *3 *4 b5 *6 *7 ' '8 x9 \n10: walk left from the space after the bold.
        let text = "## **b** x\n";
        let leftwards = |policy| walk(text, Normal, policy, 8, &[L; 7]);
        // N2: the bold's ancestor chain reveals the heading, so `h` walks `## ` too.
        assert_eq!(leftwards(G2N2), [8, 5, 4, 3, 2, 1, 0, 0]);
        assert_eq!(leftwards(G1N2), [8, 5, 4, 3, 2, 1, 0, 0]);
        // N1: only the bold is revealed, so the heading's `## ` stays concealed.
        assert_eq!(leftwards(G2N1), [8, 5, 4, 3, 3, 3, 3, 3]);
        assert_eq!(leftwards(G1N1), [8, 5, 4, 3, 3, 3, 3, 3]);
    }

    // ---- characterization: nested constructs ----------------------------------------

    #[test]
    fn n1_h_cannot_reach_the_outer_marker_of_both() {
        // *0 *1 *2 x3 *4 *5 *6 \n7: Emphasis 0..7 around Strong 1..6 (F-1).
        let text = "***x***\n";
        // N1 reveals only the strong, so the emphasis's `*` at 0 is skipped and `h` stops at 1.
        for heading in [HeadingPolicy::G1, HeadingPolicy::G2] {
            assert_eq!(
                walk(text, Normal, pol(heading, NestingPolicy::N1), 3, &[L; 4]),
                [3, 2, 1, 1, 1]
            );
            assert_eq!(
                walk(text, Normal, pol(heading, NestingPolicy::N2), 3, &[L; 4]),
                [3, 2, 1, 0, 0]
            );
        }
    }

    #[test]
    fn n1_l_skips_the_outer_closing_marker_of_both() {
        let text = "***x***\n";
        // N1: from 5 the emphasis's closing `*` at 6 is concealed, so `l` jumps to the EOL cell.
        assert_eq!(walk(text, Normal, G1N1, 3, &[R; 4]), [3, 4, 5, 7, 7]);
        assert_eq!(walk(text, Normal, G1N2, 3, &[R; 5]), [3, 4, 5, 6, 7, 7]);
    }

    #[test]
    fn outer_markers_are_skipped_from_outside_under_every_policy() {
        // a0 ' '1 *2 *3 *4 x5 *6 *7 *8 \n9
        let text = "a ***x***\n";
        walk_all(text, Normal, 1, &[R], &[1, 5]);
        walk_all(text, Normal, 9, &[L], &[9, 5]);
    }

    #[test]
    fn insert_right_reaches_the_bar_between_adjacent_concealed_markers() {
        // Confirms F-10: `**a**__b__` has two strongs whose markers are adjacent at 5.
        // *0 *1 a2 *3 *4 _5 _6 b7 _8 _9 \n10
        let text = "**a**__b__\n";
        let segs = parse(text, 0).segments;
        assert_eq!(segs[0].constructs.len(), 2);
        for policy in POLICIES {
            // Right through the revealed closing `**` lands on the bar at 5 ...
            assert_eq!(walk(text, Insert, policy, 2, &[R, R, R]), [2, 3, 4, 5]);
            // ... where both strongs are concealed and I-6 still holds (per-marker runs).
            let set = reveal(&segs, 5, Insert, policy);
            let projs = project_all(text, &segs, &set);
            assert_eq!(check_frame_invariant(&projs, 5, Insert), Ok(()));
            // From there the next visible grapheme is `b`, and Left goes back to before `a`.
            assert_eq!(step(text, &segs, 5, Insert, policy, R), 8);
            assert_eq!(step(text, &segs, 5, Insert, policy, L), 2);
        }
    }

    // ---- properties --------------------------------------------------------------

    /// Brute-force C-7: concealed bytes from the run tables, graphemes from the full list.
    fn oracle(
        text: &str,
        projs: &[Projection],
        cursor: usize,
        mode: Mode,
        dir: Direction,
    ) -> usize {
        oracle_with(
            text.len(),
            &grapheme_starts(text),
            &concealed_bytes(projs),
            cursor,
            mode,
            dir,
        )
    }

    fn concealed_bytes(projs: &[Projection]) -> HashSet<usize> {
        projs
            .iter()
            .flat_map(|p| p.runs.iter())
            .filter_map(|r| match r {
                Run::Concealed { src } => Some(src.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    /// The oracle with the grapheme list and concealed set supplied by the caller.
    fn oracle_with(
        len: usize,
        starts: &[usize],
        concealed: &HashSet<usize>,
        cursor: usize,
        mode: Mode,
        dir: Direction,
    ) -> usize {
        let visible = |s: usize| !concealed.contains(&s);
        match (dir, mode) {
            (L, _) => starts
                .iter()
                .rev()
                .copied()
                .find(|&s| s < cursor && visible(s))
                .unwrap_or(cursor),
            (R, Normal) => starts
                .iter()
                .copied()
                .find(|&s| s > cursor && visible(s))
                .unwrap_or(cursor),
            (R, Insert) => match starts.iter().position(|&s| s >= cursor && visible(s)) {
                Some(i) => starts.get(i + 1).copied().unwrap_or(len),
                None => cursor,
            },
        }
    }

    /// Bytes inside a heading construct's opening or closing marker.
    fn heading_marker_bytes(segs: &[Segment]) -> HashSet<usize> {
        segs.iter()
            .flat_map(|s| &s.constructs)
            .filter(|c| matches!(c.kind, ConstructKind::Heading { .. }))
            .flat_map(|c| c.open.clone().chain(c.close.clone()))
            .collect()
    }

    fn atom() -> impl Strategy<Value = &'static str> {
        // The T4 alphabet plus multi-byte atoms, and regional indicators that pair into flags.
        prop::sample::select(vec![
            "a",
            "é",
            "e\u{301}",
            "👍🏽",
            " ",
            "*",
            "_",
            "#",
            "\n",
            "\\",
            "n",
            "**",
            "# ",
            "*a*",
            "\u{1F1FA}",
            "\u{1F1F8}",
        ])
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        #[test]
        fn random_motion_sequences_match_the_oracle_and_keep_i6(
            atoms in prop::collection::vec(atom(), 0..40),
            start in any::<prop::sample::Index>(),
            moves in prop::collection::vec(any::<bool>(), 1..30),
            policy in prop::sample::select(POLICIES.to_vec()),
            insert in any::<bool>(),
        ) {
            let text = atoms.concat();
            let segs = parse(&text, 0).segments;
            let mode = if insert { Insert } else { Normal };
            let valid = if insert { insert_bars(&text) } else { normal_cursors(&text) };
            let mut cursor = valid[start.index(valid.len())];

            for go_right in moves {
                let dir = if go_right { R } else { L };
                let set = reveal(&segs, cursor, mode, policy);
                let projs = project_all(&text, &segs, &set);
                let next = move_horizontal(&text, &projs, cursor, mode, dir);

                // C-7 against the brute-force oracle.
                prop_assert_eq!(next, oracle(&text, &projs, cursor, mode, dir));
                // Grapheme alignment: the result is a valid cursor or bar.
                prop_assert!(valid.contains(&next), "{:?}: {} is not a valid {:?} position", text, next, mode);
                // Direction.
                let ordered = if go_right { next >= cursor } else { next <= cursor };
                prop_assert!(ordered);
                // I-6 under the projection computed for the new cursor.
                let set = reveal(&segs, next, mode, policy);
                let projs = project_all(&text, &segs, &set);
                prop_assert_eq!(check_frame_invariant(&projs, next, mode), Ok(()));
                cursor = next;
            }
        }

        #[test]
        fn g2_n1_normal_motions_never_land_in_a_heading_marker(
            atoms in prop::collection::vec(atom(), 0..40),
            start in any::<prop::sample::Index>(),
            moves in prop::collection::vec(any::<bool>(), 1..30),
        ) {
            // Under G2+N1 a heading is revealed only while the cursor is in its markers,
            // so a cursor that starts outside every marker can never step into one.
            let text = atoms.concat();
            let segs = parse(&text, 0).segments;
            let markers = heading_marker_bytes(&segs);
            let valid = normal_cursors(&text);
            let mut cursor = valid[start.index(valid.len())];
            prop_assume!(!markers.contains(&cursor));
            for go_right in moves {
                cursor = step(&text, &segs, cursor, Normal, G2N1, if go_right { R } else { L });
                prop_assert!(!markers.contains(&cursor), "{:?}: landed in a marker at {}", text, cursor);
            }
        }
    }

    #[test]
    fn spike_fixture_motions_match_the_oracle_everywhere() {
        let text = include_str!("../fixtures/spike.md");
        let segs = parse(text, 0).segments;
        let starts = grapheme_starts(text);
        // Projections and the concealed set depend only on the reveal flags, and most
        // cursors share the same flags, so compute each distinct set once.
        type Cached = (Vec<Projection>, HashSet<usize>);
        let mut cache: std::collections::HashMap<Vec<bool>, Cached> = Default::default();
        for policy in POLICIES {
            for (mode, positions) in [(Normal, normal_cursors(text)), (Insert, insert_bars(text))] {
                for cursor in positions {
                    let set = reveal(&segs, cursor, mode, policy);
                    let key: Vec<bool> = (0..segs.len())
                        .flat_map(|i| set.for_segment(i).iter().copied().chain([false]))
                        .collect();
                    let (projs, concealed) = cache.entry(key).or_insert_with(|| {
                        let projs = project_all(text, &segs, &set);
                        let concealed = concealed_bytes(&projs);
                        (projs, concealed)
                    });
                    for dir in [L, R] {
                        assert_eq!(
                            move_horizontal(text, projs, cursor, mode, dir),
                            oracle_with(text.len(), &starts, concealed, cursor, mode, dir),
                            "{policy:?} {mode:?} {dir:?} at {cursor}"
                        );
                    }
                }
            }
        }
    }
}
