//! Document-level helpers over the per-parse `String` snapshot (C-1).

// Consumed by later tasks (motion, geometry); remove once they use every item.
#![allow(dead_code)]

use unicode_segmentation::UnicodeSegmentation;

/// Start offset of every extended grapheme cluster in `text` (`\n` is its own grapheme).
// I-1: offsets are source bytes on grapheme boundaries.
pub fn grapheme_starts(text: &str) -> Vec<usize> {
    text.grapheme_indices(true).map(|(i, _)| i).collect()
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

    #[test]
    fn empty_document_has_cursor_zero_and_bar_zero() {
        assert_eq!(normal_cursors(""), vec![0]);
        assert_eq!(insert_bars(""), vec![0]);
    }
}
