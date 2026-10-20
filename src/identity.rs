//! Block identity (C-10): which new segment is which old one after an edit. Pure functions over
//! parse output; the ids themselves live in `App`, never in a `Segment` (C-2).

use std::mem::discriminant;
use std::ops::Range;

use crate::parse::Segment;

/// An id held by the UI for one segment; unique within a frame, never reused (C-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(pub u64);

/// The matching strategy F5 toggles (C-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The architecture's literal order: position, then kind, then content hash.
    L,
    /// Content first: common prefix and suffix, then an LCS, then in-order pairing.
    C,
}

/// Kind comparison is the discriminant only: Paragraph, Heading (level ignored), Raw, Gap.
fn same_kind(a: &Segment, b: &Segment) -> bool {
    discriminant(&a.kind) == discriminant(&b.kind)
}

/// The `(kind, content hash)` a segment is compared by in strategy C.
fn content_key(s: &Segment) -> (std::mem::Discriminant<crate::parse::SegmentKind>, u64) {
    (discriminant(&s.kind), s.content_hash)
}

fn overlaps(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

/// For each new segment, the index of the old segment it is matched to, if any. Every old
/// segment is matched at most once.
pub fn match_segments(strategy: Strategy, old: &[Segment], new: &[Segment]) -> Vec<Option<usize>> {
    match strategy {
        Strategy::L => match_position_first(old, new),
        Strategy::C => match_content_first(old, new),
    }
}

/// Fresh ids for matched-nothing segments come from `next_id`, a monotonic counter (C-10).
///
/// `old_ids` is parallel to `old`; the result is parallel to `new`.
pub fn assign(
    strategy: Strategy,
    old: &[Segment],
    old_ids: &[BlockId],
    new: &[Segment],
    next_id: &mut u64,
) -> Vec<BlockId> {
    assert_eq!(old.len(), old_ids.len(), "spike: ids parallel to segments");
    match_segments(strategy, old, new)
        .into_iter()
        .map(|matched| {
            matched.map_or_else(
                || {
                    let id = BlockId(*next_id);
                    *next_id += 1;
                    id
                },
                |i| old_ids[i],
            )
        })
        .collect()
}

/// Strategy L. For each new segment, in order: the unused old segments whose ranges overlap
/// its range, of the same kind; an equal content hash wins, else the first remaining one.
fn match_position_first(old: &[Segment], new: &[Segment]) -> Vec<Option<usize>> {
    let mut used = vec![false; old.len()];
    new.iter()
        .map(|n| {
            // Old segments are ordered and disjoint, so the overlapping ones are contiguous.
            let from = old.partition_point(|o| o.range.end <= n.range.start);
            let to = old.partition_point(|o| o.range.start < n.range.end);
            let candidates: Vec<usize> = (from..to)
                .filter(|&i| !used[i] && overlaps(&old[i].range, &n.range) && same_kind(&old[i], n))
                .collect();
            let pick = candidates
                .iter()
                .copied()
                .find(|&i| old[i].content_hash == n.content_hash)
                .or_else(|| candidates.first().copied());
            if let Some(i) = pick {
                used[i] = true;
            }
            pick
        })
        .collect()
}

/// Strategy C. Trim the common prefix and suffix of the `(kind, hash)` sequences, take an LCS
/// of what is left as anchors, and within each gap between anchors pair the remaining old and
/// new segments in order where the kinds are equal (greedy, order-preserving).
fn match_content_first(old: &[Segment], new: &[Segment]) -> Vec<Option<usize>> {
    let mut matched = vec![None; new.len()];
    let limit = old.len().min(new.len());
    let prefix = (0..limit)
        .take_while(|&i| content_key(&old[i]) == content_key(&new[i]))
        .count();
    let suffix = (0..limit - prefix)
        .take_while(|&j| {
            content_key(&old[old.len() - 1 - j]) == content_key(&new[new.len() - 1 - j])
        })
        .count();
    for (i, slot) in matched.iter_mut().enumerate().take(prefix) {
        *slot = Some(i);
    }
    for j in 0..suffix {
        matched[new.len() - 1 - j] = Some(old.len() - 1 - j);
    }

    // The middle, as half-open index ranges.
    let (old_mid, new_mid) = (prefix..old.len() - suffix, prefix..new.len() - suffix);
    let anchors = lcs(&old[old_mid.clone()], &new[new_mid.clone()]);

    // Walk the gaps between anchors (and before the first / after the last).
    let (mut old_from, mut new_from) = (old_mid.start, new_mid.start);
    let boundaries = anchors
        .iter()
        .map(|&(i, j)| (old_mid.start + i, new_mid.start + j))
        .chain(std::iter::once((old_mid.end, new_mid.end)));
    for (old_at, new_at) in boundaries {
        pair_in_order(old, new, old_from..old_at, new_from..new_at, &mut matched);
        if old_at < old_mid.end {
            matched[new_at] = Some(old_at); // the anchor itself
        }
        (old_from, new_from) = (old_at + 1, new_at + 1);
    }
    matched
}

/// Pairs the old and new segments of one gap in order, where the kinds are equal: each new
/// segment takes the first old one after the previous pairing that has its kind.
fn pair_in_order(
    old: &[Segment],
    new: &[Segment],
    old_gap: Range<usize>,
    new_gap: Range<usize>,
    matched: &mut [Option<usize>],
) {
    let mut next_old = old_gap.start;
    for j in new_gap {
        if let Some(k) = (next_old..old_gap.end).find(|&k| same_kind(&old[k], &new[j])) {
            matched[j] = Some(k);
            next_old = k + 1;
        }
    }
}

/// The longest common subsequence of the `(kind, hash)` keys, as `(old index, new index)` pairs
/// relative to the slices, in increasing order.
fn lcs(old: &[Segment], new: &[Segment]) -> Vec<(usize, usize)> {
    let (n, m) = (old.len(), new.len());
    // `len[i][j]`: the LCS length of `old[i..]` and `new[j..]`.
    let mut len = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i][j] = if content_key(&old[i]) == content_key(&new[j]) {
                len[i + 1][j + 1] + 1
            } else {
                len[i + 1][j].max(len[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut anchors = Vec::new();
    while i < n && j < m {
        if content_key(&old[i]) == content_key(&new[j]) {
            anchors.push((i, j));
            (i, j) = (i + 1, j + 1);
        } else if len[i + 1][j] >= len[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    anchors
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{SegmentKind, parse};
    use proptest::prelude::*;
    // Ours, not proptest's `Strategy` trait.
    use super::Strategy;
    use std::collections::HashSet;

    fn segments(text: &str) -> Vec<Segment> {
        parse(text, 0).segments
    }

    /// Fresh ids for every segment of `text`.
    fn first_ids(text: &str, next_id: &mut u64) -> (Vec<Segment>, Vec<BlockId>) {
        let segs = segments(text);
        let ids = assign(Strategy::C, &[], &[], &segs, next_id);
        (segs, ids)
    }

    #[test]
    fn fresh_ids_are_monotonic_and_the_counter_advances() {
        let mut next = 5;
        let (segs, ids) = first_ids("a\n\nb\n", &mut next);
        assert_eq!(segs.len(), 3);
        assert_eq!(ids, [BlockId(5), BlockId(6), BlockId(7)]);
        assert_eq!(next, 8);
        let (segs, ids) = first_ids("", &mut next);
        assert!(segs.is_empty() && ids.is_empty());
        assert_eq!(next, 8);
    }

    #[test]
    fn identical_documents_keep_every_id_under_both_strategies() {
        for text in [
            "",
            "a\n",
            "alpha\n\nbeta\n\n\n# h\n\n- x\n- y\n\n```\ncode\n```\n",
            include_str!("../fixtures/spike.md"),
        ] {
            for strategy in [Strategy::L, Strategy::C] {
                let mut next = 0;
                let (segs, ids) = first_ids(text, &mut next);
                let again = assign(strategy, &segs, &ids, &segs, &mut next);
                assert_eq!(again, ids, "{strategy:?} {text:?}");
            }
        }
    }

    #[test]
    fn kind_comparison_ignores_the_heading_level_but_not_the_kind() {
        let (h2, h3) = (segments("## t\n"), segments("### t\n"));
        let (p, raw) = (segments("t\n"), segments("- t\n"));
        assert!(same_kind(&h2[0], &h3[0]));
        assert!(!same_kind(&h2[0], &p[0]));
        assert!(!same_kind(&p[0], &raw[0]));
        assert_eq!(match_segments(Strategy::C, &h2, &h3), [Some(0)]);
        assert_eq!(match_segments(Strategy::C, &p, &h2), [None]);
        assert_eq!(match_segments(Strategy::L, &h2, &h3), [Some(0)]);
        assert_eq!(match_segments(Strategy::L, &p, &h2), [None]);
    }

    #[test]
    fn strategy_l_prefers_an_equal_hash_among_overlapping_candidates_and_never_reuses() {
        // Old: paragraph `ab` (0..3), a gap (3..4), paragraph `cd` (4..7). New: a two-line gap
        // (0..2) and the paragraph `cd` (2..5), whose range overlaps both old paragraphs. The
        // one with an equal hash (`cd`, the second) wins over the first candidate.
        let old = segments("ab\n\ncd\n");
        let new = segments("\n\ncd\n");
        assert_eq!(
            old.iter().map(|s| s.range.clone()).collect::<Vec<_>>(),
            [0..3, 3..4, 4..7]
        );
        assert_eq!(
            new.iter().map(|s| s.range.clone()).collect::<Vec<_>>(),
            [0..2, 2..5]
        );
        assert_eq!(match_segments(Strategy::L, &old, &new), [None, Some(2)]);
        // With no equal hash among the candidates, the first one is taken.
        let new = segments("\n\nxy\n");
        assert_eq!(match_segments(Strategy::L, &old, &new), [None, Some(0)]);
        // Never the same old segment twice.
        let m = match_segments(Strategy::L, &old, &segments("a\n\na\n\na\n"));
        let used: Vec<usize> = m.iter().flatten().copied().collect();
        assert_eq!(
            used.len(),
            used.iter().collect::<HashSet<_>>().len(),
            "{m:?}"
        );
    }

    #[test]
    fn a_moved_block_loses_its_id_under_c_and_ids_stay_with_positions_under_l() {
        // Characterization (not a defect to fix, F-45): neither strategy detects a move.
        let run = |strategy, old: &str, new: &str| {
            let mut next = 0;
            let (old_segs, old_ids) = first_ids(old, &mut next);
            let new_segs = segments(new);
            let new_ids = assign(strategy, &old_segs, &old_ids, &new_segs, &mut next);
            (
                blocks(&old_segs, &old_ids),
                blocks(&new_segs, &new_ids),
                old_ids,
            )
        };

        // Swap `alpha` and `beta`. C anchors on one block by content (`beta` keeps its id) and
        // the other is new; L hands each position's id to whatever now sits there.
        let (old, new, all_old) = run(Strategy::C, "alpha\n\nbeta\n", "beta\n\nalpha\n");
        assert_eq!(new[0], old[1], "C: beta keeps its id");
        assert!(!all_old.contains(&new[1]), "C: the other block is fresh");
        let (old, new, _) = run(Strategy::L, "alpha\n\nbeta\n", "beta\n\nalpha\n");
        assert_eq!(new, [old[0], old[1]], "L: each position keeps its id");

        // Move `gamma` to the front. C keeps `alpha` and `beta` and gives `gamma` a new id; L
        // gives the ids to positions, so `gamma` inherits the id `alpha` had.
        let (old, new, all_old) = run(
            Strategy::C,
            "alpha\n\nbeta\n\ngamma\n",
            "gamma\n\nalpha\n\nbeta\n",
        );
        assert_eq!((new[1], new[2]), (old[0], old[1]));
        assert!(!all_old.contains(&new[0]));
        let (old, new, _) = run(
            Strategy::L,
            "alpha\n\nbeta\n\ngamma\n",
            "gamma\n\nalpha\n\nbeta\n",
        );
        assert_eq!(new[0], old[0]);
    }

    #[test]
    fn ranges_overlap_only_if_they_share_a_byte() {
        assert!(overlaps(&(0..3), &(2..5)));
        assert!(overlaps(&(2..5), &(0..3)));
        assert!(overlaps(&(0..5), &(1..2)));
        // Touching is not overlapping.
        assert!(!overlaps(&(0..3), &(3..5)));
        assert!(!overlaps(&(3..5), &(0..3)));
        assert!(!overlaps(&(0..3), &(7..9)));
    }

    #[test]
    fn empty_documents_are_handled_by_both_strategies() {
        let one = segments("a\n");
        for strategy in [Strategy::L, Strategy::C] {
            assert_eq!(match_segments(strategy, &[], &one), [None]);
            assert_eq!(
                match_segments(strategy, &one, &[]),
                Vec::<Option<usize>>::new()
            );
            assert!(match_segments(strategy, &[], &[]).is_empty());
        }
    }

    // ---- the case table (C-10) ------------------------------------------------------------

    /// One expectation about the non-Gap segments, indexed in document order among non-Gap
    /// segments only (the contract's table does not constrain Gap ids).
    #[derive(Clone, Copy)]
    enum Expect {
        /// New block `.0` carries the id of old block `.1`.
        Keeps(usize, usize),
        /// New block `.0` has an id no old segment had.
        Fresh(usize),
        /// The id of old block `.0` is not used by any new segment.
        Retired(usize),
        /// At least one new block keeps an old block's id.
        SomeKept,
    }

    struct Case {
        label: &'static str,
        old: &'static str,
        new: &'static str,
        expect: &'static [Expect],
    }

    use Expect::{Fresh, Keeps, Retired, SomeKept};

    const CASES: &[Case] = &[
        Case {
            label: "1  insert a block above B",
            old: "alpha\n\nbeta\n\ngamma\n",
            new: "alpha\n\nxray\n\nbeta\n\ngamma\n",
            expect: &[Keeps(0, 0), Fresh(1), Keeps(2, 1), Keeps(3, 2)],
        },
        Case {
            label: "2  split a paragraph in two",
            old: "head\n\nonetwo\n\ntail\n",
            new: "head\n\none\n\ntwo\n\ntail\n",
            expect: &[Keeps(0, 0), Keeps(1, 1), Fresh(2), Keeps(3, 2)],
        },
        Case {
            label: "3  join two paragraphs",
            old: "head\n\none\n\ntwo\n\ntail\n",
            new: "head\n\nonetwo\n\ntail\n",
            expect: &[Keeps(0, 0), Keeps(1, 1), Retired(2), Keeps(2, 3)],
        },
        Case {
            label: "4  edit in place",
            old: "head\n\nbody text\n\ntail\n",
            new: "head\n\nbody text edited\n\ntail\n",
            expect: &[Keeps(0, 0), Keeps(1, 1), Keeps(2, 2)],
        },
        Case {
            label: "5a paragraph to heading",
            old: "head\n\ntitle\n\ntail\n",
            new: "head\n\n# title\n\ntail\n",
            expect: &[Keeps(0, 0), Fresh(1), Keeps(2, 2)],
        },
        Case {
            label: "5b H2 to H3",
            old: "head\n\n## title\n\ntail\n",
            new: "head\n\n### title\n\ntail\n",
            expect: &[Keeps(0, 0), Keeps(1, 1), Keeps(2, 2)],
        },
        Case {
            label: "6  duplicates: A,X,X,B to A,X,Y,X,B",
            old: "alpha\n\ndup\n\ndup\n\nbeta\n",
            new: "alpha\n\ndup\n\nmid\n\ndup\n\nbeta\n",
            expect: &[Keeps(0, 0), Keeps(1, 1), Fresh(2), Keeps(3, 2), Keeps(4, 3)],
        },
        Case {
            label: "7  delete a block",
            old: "a1\n\nb2\n\nc3\n",
            new: "a1\n\nc3\n",
            expect: &[Keeps(0, 0), Keeps(1, 2)],
        },
        Case {
            label: "8  swap two adjacent blocks",
            old: "alpha\n\nbeta\n",
            new: "beta\n\nalpha\n",
            expect: &[SomeKept],
        },
    ];

    /// The ids of the non-Gap segments of `text`, with the full list for the checks.
    fn blocks(segs: &[Segment], ids: &[BlockId]) -> Vec<BlockId> {
        segs.iter()
            .zip(ids)
            .filter(|(s, _)| s.kind != SegmentKind::Gap)
            .map(|(_, id)| *id)
            .collect()
    }

    /// Runs one case through one strategy: the problems found, empty if it passes.
    fn run_case(case: &Case, strategy: Strategy) -> Vec<String> {
        let mut next = 0;
        let (old, old_ids) = first_ids(case.old, &mut next);
        let new = segments(case.new);
        let new_ids = assign(strategy, &old, &old_ids, &new, &mut next);

        let mut problems = Vec::new();
        let distinct: HashSet<_> = new_ids.iter().collect();
        if distinct.len() != new_ids.len() {
            problems.push("duplicate ids".to_string());
        }
        let (old_blocks, new_blocks) = (blocks(&old, &old_ids), blocks(&new, &new_ids));
        for expect in case.expect {
            match *expect {
                Keeps(n, o) if new_blocks[n] != old_blocks[o] => {
                    problems.push(format!("new {n} should keep old {o}"));
                }
                Fresh(n) if old_ids.contains(&new_blocks[n]) => {
                    problems.push(format!("new {n} should be fresh"));
                }
                Retired(o) if new_ids.contains(&old_blocks[o]) => {
                    problems.push(format!("old {o} should be retired"));
                }
                SomeKept if !new_blocks.iter().any(|id| old_blocks.contains(id)) => {
                    problems.push("no block kept its id".to_string());
                }
                _ => {}
            }
        }
        problems
    }

    /// Prints the pass/fail matrix (`cargo test identity_matrix -- --nocapture`). Strategy C
    /// must pass every case; strategy L is printed and recorded only (C-10).
    #[test]
    fn identity_matrix() {
        println!();
        println!("identity matrix: L = position -> kind -> hash, C = content-first");
        println!("{:<38} {:^6} {:^6}", "case", "L", "C");
        let mut c_failures = Vec::new();
        let mut l_notes = Vec::new();
        for case in CASES {
            let l = run_case(case, Strategy::L);
            let c = run_case(case, Strategy::C);
            let mark = |p: &[String]| if p.is_empty() { "pass" } else { "FAIL" };
            println!("{:<38} {:^6} {:^6}", case.label, mark(&l), mark(&c));
            if !l.is_empty() {
                l_notes.push(format!("  L fails `{}`: {}", case.label, l.join("; ")));
            }
            if !c.is_empty() {
                c_failures.push(format!("`{}`: {}", case.label, c.join("; ")));
            }
        }
        for note in &l_notes {
            println!("{note}");
        }
        assert!(c_failures.is_empty(), "strategy C failed: {c_failures:?}");
    }

    // ---- invariants -----------------------------------------------------------------------

    /// What a matching got wrong, per the C-10 invariants.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct Violations {
        duplicate_ids: bool,
        kind_mismatch: bool,
        crossing: bool,
        lost_prefix_or_suffix: bool,
        reused_retired_id: bool,
    }

    /// Checks one old -> new step against the invariants, with an independent computation of
    /// the untouched prefix and suffix.
    fn violations(
        strategy: Strategy,
        old: &[Segment],
        old_ids: &[BlockId],
        new: &[Segment],
        next: &mut u64,
    ) -> (Vec<BlockId>, Violations) {
        let before = *next;
        let matched = match_segments(strategy, old, new);
        let ids = assign(strategy, old, old_ids, new, next);
        let mut v = Violations::default();
        let distinct: HashSet<_> = ids.iter().collect();
        v.duplicate_ids = distinct.len() != ids.len();
        v.kind_mismatch = matched
            .iter()
            .enumerate()
            .any(|(j, m)| m.is_some_and(|i| !same_kind(&old[i], &new[j])));
        let pairs: Vec<usize> = matched.iter().flatten().copied().collect();
        v.crossing = pairs.windows(2).any(|w| w[0] >= w[1]);
        // A fresh id is never one an old segment had (ids are never reused).
        v.reused_retired_id = ids
            .iter()
            .zip(&matched)
            .any(|(id, m)| m.is_none() && (old_ids.contains(id) || id.0 < before));

        let limit = old.len().min(new.len());
        let key = |s: &Segment| (discriminant(&s.kind), s.content_hash);
        let prefix = (0..limit)
            .take_while(|&i| key(&old[i]) == key(&new[i]))
            .count();
        let suffix = (0..limit - prefix)
            .take_while(|&j| key(&old[old.len() - 1 - j]) == key(&new[new.len() - 1 - j]))
            .count();
        v.lost_prefix_or_suffix = (0..prefix).any(|i| matched[i] != Some(i))
            || (0..suffix).any(|j| matched[new.len() - 1 - j] != Some(old.len() - 1 - j));
        (ids, v)
    }

    /// A small xorshift generator, so the recorded L counts are reproducible.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as usize
        }

        fn below(&mut self, n: usize) -> usize {
            self.next() % n.max(1)
        }
    }

    const BLOCKS: &[&str] = &[
        "alpha",
        "beta",
        "gamma",
        "dup",
        "dup",
        "# Head",
        "## Sub",
        "### Sub",
        "- item\n- item two",
        "```\ncode\n```",
        "> quote",
        "| a | b |\n|---|---|\n| 1 | 2 |",
        "    ",
        "***",
        "a longer paragraph with **bold** and *italic*",
        "setext\n------",
    ];

    const EDIT_ATOMS: &[&str] = &[
        "a", " ", "\n", "\n\n", "# ", "*", "x\n", "alpha", "- ", "```\n", "é", "\n\n\n",
    ];

    fn document(rng: &mut Rng) -> String {
        let blocks: Vec<&str> = (0..rng.below(8))
            .map(|_| BLOCKS[rng.below(BLOCKS.len())])
            .collect();
        let mut text = blocks.join("\n\n");
        if rng.below(4) != 0 && !text.is_empty() {
            text.push('\n');
        }
        text
    }

    /// One random edit on char boundaries: delete 0..3 chars at a random place and insert an atom.
    fn edit(text: &str, rng: &mut Rng) -> String {
        let chars: Vec<char> = text.chars().collect();
        let at = rng.below(chars.len() + 1);
        let delete = rng.below(3).min(chars.len() - at);
        let mut out: String = chars[..at].iter().collect();
        if rng.below(4) != 0 {
            out.push_str(EDIT_ATOMS[rng.below(EDIT_ATOMS.len())]);
        }
        out.extend(&chars[at + delete..]);
        out
    }

    /// Runs `steps` random edits from a random document under `strategy`, carrying the ids
    /// along, and adds up the violations of each step.
    fn walk(strategy: Strategy, rng: &mut Rng, steps: usize) -> Violations {
        let mut next = 0;
        let mut text = document(rng);
        let (mut segs, mut ids) = first_ids(&text, &mut next);
        let mut total = Violations::default();
        for _ in 0..steps {
            text = edit(&text, rng);
            let new = segments(&text);
            let (new_ids, v) = violations(strategy, &segs, &ids, &new, &mut next);
            total.duplicate_ids |= v.duplicate_ids;
            total.kind_mismatch |= v.kind_mismatch;
            total.crossing |= v.crossing;
            total.lost_prefix_or_suffix |= v.lost_prefix_or_suffix;
            total.reused_retired_id |= v.reused_retired_id;
            (segs, ids) = (new, new_ids);
        }
        total
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]

        /// C-10 invariants for strategy C over random edit sequences on generated documents:
        /// ids are unique within a frame, matched pairs have the same kind and keep their
        /// order, segments in an untouched common prefix or suffix keep their ids, and a
        /// fresh id is never one that was used before.
        #[test]
        fn strategy_c_keeps_every_invariant(seed in 1u64.., steps in 1usize..8) {
            let v = walk(Strategy::C, &mut Rng(seed), steps);
            prop_assert_eq!(v, Violations::default());
        }

        /// Strategy L is only recorded (C-10), but its ids are still unique, its pairs still
        /// share a kind, and it never reuses a retired id; order and the untouched prefix and
        /// suffix are what it can lose.
        #[test]
        fn strategy_l_keeps_the_structural_invariants(seed in 1u64.., steps in 1usize..8) {
            let v = walk(Strategy::L, &mut Rng(seed), steps);
            prop_assert!(!v.duplicate_ids && !v.kind_mismatch && !v.reused_retired_id, "{:?}", v);
        }
    }

    /// Records how often strategy L breaks the invariants it is not asserted on (a finding).
    #[test]
    fn strategy_l_violations_are_counted_not_asserted() {
        const TRIALS: usize = 2000;
        let (mut crossing, mut lost) = (0, 0);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..TRIALS {
            // One edit from a fresh document each time.
            let v = walk(Strategy::L, &mut rng, 1);
            crossing += usize::from(v.crossing);
            lost += usize::from(v.lost_prefix_or_suffix);
        }
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut c_bad = 0;
        for _ in 0..TRIALS {
            let v = walk(Strategy::C, &mut rng, 1);
            c_bad += usize::from(v != Violations::default());
        }
        println!(
            "strategy L over {TRIALS} single random edits: {crossing} crossed an order, {lost} lost an untouched prefix or suffix id; strategy C: {c_bad} violations"
        );
        assert_eq!(c_bad, 0);
    }
}
