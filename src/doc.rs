//! Document-level helpers over the per-parse `String` snapshot (C-1).

// Consumed by later tasks (motion, geometry); remove once they use every item.
#![allow(dead_code)]

use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

use crate::reveal::Mode;

/// Start offset of every extended grapheme cluster in `text` (`\n` is its own grapheme).
// I-1: offsets are source bytes on grapheme boundaries.
pub fn grapheme_starts(text: &str) -> Vec<usize> {
    text.grapheme_indices(true).map(|(i, _)| i).collect()
}

/// The grapheme boundary after the one starting at `o`; `None` at or past the end.
/// `o` must be a grapheme boundary (anything else is treated as "no boundary").
// I-1
pub fn next_grapheme_boundary(text: &str, o: usize) -> Option<usize> {
    if o >= text.len() || !text.is_char_boundary(o) {
        return None;
    }
    GraphemeCursor::new(o, text.len(), true)
        .next_boundary(text, 0)
        .ok()
        .flatten()
}

/// The start of the grapheme ending at `o`; `None` at `0` or past the end.
/// `o` must be a grapheme boundary (anything else is treated as "no boundary").
// I-1
pub fn prev_grapheme_boundary(text: &str, o: usize) -> Option<usize> {
    if o == 0 || o > text.len() || !text.is_char_boundary(o) {
        return None;
    }
    GraphemeCursor::new(o, text.len(), true)
        .prev_boundary(text, 0)
        .ok()
        .flatten()
}

/// Whether `o` is a grapheme boundary of `text` (`0` and `len` always are).
// I-1
pub fn is_grapheme_boundary(text: &str, o: usize) -> bool {
    if o > text.len() || !text.is_char_boundary(o) {
        return false;
    }
    GraphemeCursor::new(o, text.len(), true)
        .is_boundary(text, 0)
        .unwrap_or(false)
}

/// Whether `o` is a valid Normal cursor or Insert bar position (C-6).
// I-1
pub fn is_valid_position(text: &str, o: usize, mode: Mode) -> bool {
    match mode {
        Mode::Normal if text.is_empty() => o == 0,
        Mode::Normal => o < text.len() && is_grapheme_boundary(text, o),
        Mode::Insert => is_grapheme_boundary(text, o),
    }
}

/// The largest grapheme boundary at or before `o` (`o` is clamped to the text).
// I-1
pub fn snap_to_grapheme_start(text: &str, o: usize) -> usize {
    let mut o = o.min(text.len());
    while !text.is_char_boundary(o) {
        o -= 1;
    }
    if is_grapheme_boundary(text, o) {
        o
    } else {
        prev_grapheme_boundary(text, o).unwrap_or(0)
    }
}

/// Turns a raw offset (a click result, say) into a valid position for `mode`.
// I-1
pub fn normalize_position(text: &str, o: usize, mode: Mode) -> usize {
    let o = snap_to_grapheme_start(text, o);
    match mode {
        Mode::Insert => o,
        // A Normal cursor sits on a grapheme, so the end of the text is the last grapheme.
        Mode::Normal if o >= text.len() => prev_grapheme_boundary(text, text.len()).unwrap_or(0),
        Mode::Normal => o,
    }
}

/// Every position a Normal cursor may take: a grapheme start in `[0, len)`, or `0` when empty (C-6).
pub fn normal_cursors(text: &str) -> Vec<usize> {
    let mut starts = grapheme_starts(text);
    if starts.is_empty() {
        starts.push(0);
    }
    starts
}

/// Every position an Insert bar may take: a grapheme boundary in `[0, len]` (C-6).
pub fn insert_bars(text: &str) -> Vec<usize> {
    let mut bars = grapheme_starts(text);
    bars.push(text.len());
    bars.dedup();
    bars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphemes_keep_clusters_whole() {
        // é (precomposed), e + combining acute, thumbs-up with skin tone, newline.
        let text = "é e\u{301}👍🏽\n";
        let starts = grapheme_starts(text);
        assert_eq!(starts, vec![0, 2, 3, 6, 14]);
        assert_eq!(normal_cursors(text), starts);
        assert_eq!(insert_bars(text), vec![0, 2, 3, 6, 14, 15]);
    }

    /// Walking `next`/`prev` from the ends reproduces `grapheme_starts` plus `len`.
    fn check_neighbours(text: &str) {
        let mut bounds = grapheme_starts(text);
        bounds.push(text.len());
        bounds.dedup();

        let mut forward = vec![0];
        while let Some(n) = next_grapheme_boundary(text, *forward.last().expect("test: seeded")) {
            forward.push(n);
        }
        assert_eq!(forward, bounds, "{text:?}: forward walk");

        let mut backward = vec![text.len()];
        while let Some(p) = prev_grapheme_boundary(text, *backward.last().expect("test: seeded")) {
            backward.push(p);
        }
        backward.reverse();
        assert_eq!(backward, bounds, "{text:?}: backward walk");
    }

    #[test]
    fn neighbour_boundaries_match_the_grapheme_list() {
        for text in [
            "",
            "abc",
            "é e\u{301}👍🏽\n",
            // Two flags: regional indicators pair up from the left.
            "\u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7}x",
            // A ZWJ family and a CRLF cluster.
            "👨\u{200D}👩\u{200D}👧a\r\nb",
            "a\n\nb\n",
        ] {
            check_neighbours(text);
        }
    }

    #[test]
    fn neighbour_boundaries_reject_stray_offsets_without_panicking() {
        let text = "é👍🏽";
        // Mid-character, past the end, and the two ends.
        assert_eq!(next_grapheme_boundary(text, 1), None);
        assert_eq!(prev_grapheme_boundary(text, 1), None);
        assert_eq!(next_grapheme_boundary(text, 99), None);
        assert_eq!(prev_grapheme_boundary(text, 99), None);
        assert_eq!(next_grapheme_boundary(text, text.len()), None);
        assert_eq!(prev_grapheme_boundary(text, 0), None);
        assert_eq!(next_grapheme_boundary("", 0), None);
        assert_eq!(prev_grapheme_boundary("", 0), None);
    }

    #[test]
    fn position_validity_matches_the_cursor_lists() {
        for text in ["", "abc", "é e\u{301}👍🏽\n", "\u{1F1FA}\u{1F1F8}x\n"] {
            let normal = normal_cursors(text);
            let insert = insert_bars(text);
            for o in 0..=text.len() + 2 {
                assert_eq!(
                    is_valid_position(text, o, Mode::Normal),
                    normal.contains(&o),
                    "{text:?} normal {o}"
                );
                assert_eq!(
                    is_valid_position(text, o, Mode::Insert),
                    insert.contains(&o),
                    "{text:?} insert {o}"
                );
            }
        }
    }

    #[test]
    fn snapping_and_normalizing_always_give_valid_positions() {
        for text in [
            "",
            "abc",
            "é e\u{301}👍🏽\n",
            "\u{1F1FA}\u{1F1F8}\u{1F1EC}\u{1F1E7}x\n",
        ] {
            for o in 0..=text.len() + 3 {
                let snapped = snap_to_grapheme_start(text, o);
                assert!(
                    is_grapheme_boundary(text, snapped),
                    "{text:?} {o} -> {snapped}"
                );
                assert!(snapped <= o.min(text.len()), "{text:?} {o} -> {snapped}");
                for mode in [Mode::Normal, Mode::Insert] {
                    let n = normalize_position(text, o, mode);
                    assert!(
                        is_valid_position(text, n, mode),
                        "{text:?} {o} {mode:?} -> {n}"
                    );
                }
            }
        }
        // Inside a cluster snaps to its start; the end of the text is the last grapheme for Normal.
        assert_eq!(snap_to_grapheme_start("e\u{301}x", 1), 0);
        assert_eq!(snap_to_grapheme_start("e\u{301}x", 2), 0);
        assert_eq!(normalize_position("ab\n", 3, Mode::Normal), 2);
        assert_eq!(normalize_position("ab\n", 3, Mode::Insert), 3);
    }

    #[test]
    fn empty_document_has_cursor_zero_and_bar_zero() {
        assert_eq!(normal_cursors(""), vec![0]);
        assert_eq!(insert_bars(""), vec![0]);
    }
}
