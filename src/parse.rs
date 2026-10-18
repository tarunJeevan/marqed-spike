//! Parse: source text to identity-free segments and constructs (C-2, C-3).

// Consumed by later tasks (reveal, projection); remove once T2/T3 use every item.
#![allow(dead_code)]

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentKind {
    Paragraph,
    /// ATX only.
    Heading {
        level: u8,
    },
    Raw,
    Gap,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstructKind {
    Strong,
    Emphasis,
    /// `hash_run` is the contiguous `#` run inside the opening marker.
    Heading {
        hash_run: Range<usize>,
    },
}

/// A construct with its two marker ranges. `parent` indexes into the owning
/// segment's `constructs`.
// I-1: every range here is a source byte offset into the parsed text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Construct {
    pub kind: ConstructKind,
    pub range: Range<usize>,
    pub parent: Option<usize>,
    pub open: Range<usize>,
    pub close: Range<usize>,
}

/// There is deliberately no id field: parse output is identity-free (C-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub range: Range<usize>,
    pub kind: SegmentKind,
    pub content_hash: u64,
    pub constructs: Vec<Construct>,
}

impl Segment {
    /// The EOL byte offset: the segment's final byte, if it is `\n`.
    pub fn eol(&self, text: &str) -> Option<usize> {
        let last = self.range.end.checked_sub(1)?;
        (last >= self.range.start && text.as_bytes().get(last) == Some(&b'\n')).then_some(last)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult {
    /// I-8: the version this result was computed from.
    pub version: u64,
    pub segments: Vec<Segment>,
}

/// A top-level block as pulldown reports it, before line normalization.
struct RawBlock {
    range: Range<usize>,
    shape: Shape,
}

enum Shape {
    Paragraph {
        inline: Vec<Construct>,
    },
    Heading {
        level: u8,
        first_child_start: Option<usize>,
        last_child_end: Option<usize>,
        inline: Vec<Construct>,
    },
    Other,
}

pub fn parse(text: &str, version: u64) -> ParseResult {
    let blocks = top_level_blocks(text);
    let segments = build_segments(text, blocks);
    ParseResult { version, segments }
}

pub fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
}

fn level_number(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Walks the depth-0 events. Each `Start`..`End` pair is one block; a depth-0
/// event that is not a `Start` (a thematic break) is a one-event block.
fn top_level_blocks(text: &str) -> Vec<RawBlock> {
    let mut blocks = Vec::new();
    let mut depth = 0usize;
    let mut current: Option<RawBlock> = None;
    // Nearest-kept-ancestor bookkeeping for inline constructs of the current block.
    let mut stack: Vec<Option<usize>> = Vec::new();

    for (event, range) in Parser::new_ext(text, options()).into_offset_iter() {
        match event {
            Event::Start(tag) => {
                if depth == 0 {
                    let shape = match tag {
                        Tag::Paragraph => Shape::Paragraph { inline: Vec::new() },
                        Tag::Heading { level, .. } => Shape::Heading {
                            level: level_number(level),
                            first_child_start: None,
                            last_child_end: None,
                            inline: Vec::new(),
                        },
                        _ => Shape::Other,
                    };
                    current = Some(RawBlock { range, shape });
                    stack.clear();
                } else if let Some(block) = current.as_mut() {
                    note_child(&mut block.shape, &range);
                    if let Tag::Strong | Tag::Emphasis = tag {
                        let kind = if matches!(tag, Tag::Strong) {
                            ConstructKind::Strong
                        } else {
                            ConstructKind::Emphasis
                        };
                        open_inline(text, &mut block.shape, &mut stack, kind, range);
                    }
                }
                depth += 1;
            }
            Event::End(end) => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    blocks.extend(current.take());
                } else if let Some(block) = current.as_mut() {
                    note_child(&mut block.shape, &range);
                    if matches!(end, TagEnd::Strong | TagEnd::Emphasis) {
                        stack.pop();
                    }
                }
            }
            _ if depth == 0 => blocks.push(RawBlock {
                range,
                shape: Shape::Other,
            }),
            _ => {
                if let Some(block) = current.as_mut() {
                    note_child(&mut block.shape, &range);
                }
            }
        }
    }
    blocks
}

/// Records the extent of the events inside a heading (for C-3 marker ranges).
fn note_child(shape: &mut Shape, range: &Range<usize>) {
    if let Shape::Heading {
        first_child_start,
        last_child_end,
        ..
    } = shape
    {
        first_child_start.get_or_insert(range.start);
        *last_child_end = Some(range.end);
    }
}

/// Pushes a Strong/Emphasis construct, or drops it (C-3) if its marker bytes
/// are not all `*`/`_`. A dropped construct's children attach to the nearest
/// kept ancestor.
fn open_inline(
    text: &str,
    shape: &mut Shape,
    stack: &mut Vec<Option<usize>>,
    kind: ConstructKind,
    range: Range<usize>,
) {
    let (Shape::Paragraph { inline } | Shape::Heading { inline, .. }) = shape else {
        stack.push(None);
        return;
    };
    let width = if kind == ConstructKind::Strong { 2 } else { 1 };
    let bytes = text.as_bytes();
    let (open, close) = (
        range.start..range.start + width,
        range.end.saturating_sub(width)..range.end,
    );
    let is_marker = |r: &Range<usize>| {
        bytes
            .get(r.clone())
            .is_some_and(|b| b.iter().all(|&c| c == b'*' || c == b'_'))
    };
    if range.len() < 2 * width || !is_marker(&open) || !is_marker(&close) {
        stack.push(None);
        return;
    }
    let parent = stack.iter().rev().find_map(|p| *p);
    inline.push(Construct {
        kind,
        range,
        parent,
        open,
        close,
    });
    stack.push(Some(inline.len() - 1));
}

fn line_start(bytes: &[u8], at: usize) -> usize {
    bytes[..at.min(bytes.len())]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1)
}

/// Just past the `\n` ending the line that holds byte `last`, or EOF.
fn line_end_after(bytes: &[u8], last: usize) -> usize {
    bytes[last.min(bytes.len())..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(bytes.len(), |i| last + i + 1)
}

fn build_segments(text: &str, blocks: Vec<RawBlock>) -> Vec<Segment> {
    let bytes = text.as_bytes();

    // Normalize to whole lines and drop anything empty.
    let mut normalized: Vec<(Range<usize>, Shape)> = blocks
        .into_iter()
        .filter_map(|b| {
            let start = line_start(bytes, b.range.start);
            let last = b.range.end.saturating_sub(1).max(b.range.start);
            let end = line_end_after(bytes, last);
            (start < end).then_some((start..end, b.shape))
        })
        .collect();
    normalized.sort_by_key(|(r, _)| r.start);

    // Blocks that share a line merge into one Raw segment.
    let mut merged: Vec<(Range<usize>, Option<Shape>)> = Vec::new();
    for (range, shape) in normalized {
        match merged.last_mut() {
            Some((prev, prev_shape)) if range.start < prev.end => {
                prev.end = prev.end.max(range.end);
                *prev_shape = None;
            }
            _ => merged.push((range, Some(shape))),
        }
    }

    let mut segments = Vec::new();
    let mut cursor = 0;
    for (range, shape) in merged {
        if cursor < range.start {
            segments.push(gap(bytes, cursor..range.start));
        }
        cursor = range.end;
        segments.push(block_segment(text, range, shape));
    }
    if cursor < bytes.len() {
        segments.push(gap(bytes, cursor..bytes.len()));
    }
    segments
}

fn gap(bytes: &[u8], range: Range<usize>) -> Segment {
    Segment {
        content_hash: content_hash(&bytes[range.clone()]),
        range,
        kind: SegmentKind::Gap,
        constructs: Vec::new(),
    }
}

fn block_segment(text: &str, range: Range<usize>, shape: Option<Shape>) -> Segment {
    let bytes = text.as_bytes();
    let content_hash = content_hash(&bytes[range.clone()]);
    let raw = |range| Segment {
        content_hash,
        range,
        kind: SegmentKind::Raw,
        constructs: Vec::new(),
    };
    let ends_in_eol = bytes[range.end - 1] == b'\n';
    let line_end = if ends_in_eol {
        range.end - 1
    } else {
        range.end
    };
    match shape {
        Some(Shape::Paragraph { inline }) => Segment {
            content_hash,
            range,
            kind: SegmentKind::Paragraph,
            constructs: inline,
        },
        // ATX headings are one line. A setext heading always spans at least two
        // lines (text plus underline), even when its text starts with `#`.
        Some(Shape::Heading {
            level,
            first_child_start,
            last_child_end,
            inline,
        }) if !bytes[range.start..line_end].contains(&b'\n') => {
            let (open_end, close_start) = match (first_child_start, last_child_end) {
                (Some(first), Some(last)) => (first, last),
                // Empty heading: the opening marker is the whole line content.
                _ => (line_end, line_end),
            };
            // Keep the markers inside the line whatever offsets pulldown reported.
            let open_end = open_end.clamp(range.start, line_end);
            let close_start = close_start.clamp(open_end, line_end);
            let hash_run = hash_run(bytes, range.start, open_end);
            let mut constructs = vec![Construct {
                kind: ConstructKind::Heading { hash_run },
                range: range.start..line_end,
                parent: None,
                open: range.start..open_end,
                close: close_start..line_end,
            }];
            constructs.extend(inline.into_iter().map(|mut c| {
                c.parent = Some(c.parent.map_or(0, |p| p + 1));
                c
            }));
            Segment {
                content_hash,
                range,
                kind: SegmentKind::Heading { level },
                constructs,
            }
        }
        _ => raw(range),
    }
}

/// The first contiguous `#` run inside `from..to`, after any indentation.
fn hash_run(bytes: &[u8], from: usize, to: usize) -> Range<usize> {
    let start = bytes[from..to]
        .iter()
        .position(|&b| b == b'#')
        .map_or(to, |i| from + i);
    let len = bytes[start..to].iter().take_while(|&&b| b == b'#').count();
    start..start + len
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn segs(text: &str) -> Vec<Segment> {
        parse(text, 0).segments
    }

    /// The range of the `n`th (0-based) occurrence of `needle` in `text`.
    fn nth(text: &str, needle: &str, n: usize) -> Range<usize> {
        let (start, _) = text
            .match_indices(needle)
            .nth(n)
            .expect("test: needle occurs");
        start..start + needle.len()
    }

    fn at(text: &str, needle: &str) -> Range<usize> {
        nth(text, needle, 0)
    }

    fn only(text: &str) -> Segment {
        let mut s = segs(text);
        assert_eq!(s.len(), 1, "{text:?}: {s:?}");
        s.remove(0)
    }

    fn kinds(text: &str) -> Vec<SegmentKind> {
        segs(text).iter().map(|s| s.kind).collect()
    }

    fn assert_partition(text: &str, segments: &[Segment]) {
        let mut cursor = 0;
        for s in segments {
            assert_eq!(s.range.start, cursor, "{text:?}: gap in partition");
            assert!(s.range.end > s.range.start, "{text:?}: empty segment");
            cursor = s.range.end;
        }
        assert_eq!(cursor, text.len(), "{text:?}: partition does not reach EOF");
        if text.is_empty() {
            assert!(segments.is_empty());
        }
    }

    fn assert_construct_shape(text: &str, seg: &Segment) {
        let bytes = text.as_bytes();
        for (i, c) in seg.constructs.iter().enumerate() {
            assert!(
                seg.range.start <= c.range.start && c.range.end <= seg.range.end,
                "{text:?}: construct outside its segment"
            );
            for marker in [&c.open, &c.close] {
                assert!(
                    c.range.start <= marker.start && marker.end <= c.range.end,
                    "{text:?}: marker outside its construct"
                );
                assert!(
                    !bytes[marker.clone()].contains(&b'\n'),
                    "{text:?}: marker contains a newline"
                );
                assert!(text.is_char_boundary(marker.start) && text.is_char_boundary(marker.end));
            }
            assert!(c.open.end <= c.close.start, "{text:?}: markers overlap");
            if let Some(p) = c.parent {
                assert!(p < i, "{text:?}: parent not before child");
                let parent = &seg.constructs[p];
                assert!(
                    parent.range.start <= c.range.start && c.range.end <= parent.range.end,
                    "{text:?}: parent does not contain child"
                );
            }
            match &c.kind {
                ConstructKind::Strong | ConstructKind::Emphasis => {
                    let width = if c.kind == ConstructKind::Strong {
                        2
                    } else {
                        1
                    };
                    assert_eq!(c.open.len(), width);
                    assert_eq!(c.close.len(), width);
                    assert_eq!(c.open.start, c.range.start);
                    assert_eq!(c.close.end, c.range.end);
                    assert!(
                        bytes[c.open.clone()]
                            .iter()
                            .chain(&bytes[c.close.clone()])
                            .all(|&b| b == b'*' || b == b'_'),
                        "{text:?}: non-marker byte in a marker"
                    );
                }
                ConstructKind::Heading { hash_run } => {
                    assert_eq!(i, 0, "{text:?}: heading construct is not the root");
                    assert!(c.open.start <= hash_run.start && hash_run.end <= c.open.end);
                    assert!(bytes[hash_run.clone()].iter().all(|&b| b == b'#'));
                }
            }
        }
    }

    // ---- partition -------------------------------------------------------

    #[test]
    fn empty_document_has_no_segments() {
        assert!(segs("").is_empty());
    }

    #[test]
    fn version_is_carried() {
        assert_eq!(parse("a", 7).version, 7);
    }

    #[test]
    fn gaps_and_blocks_partition_the_text() {
        let text = "a\n\n\n   \nb\n";
        let s = segs(text);
        assert_partition(text, &s);
        assert_eq!(
            s.iter().map(|s| s.range.clone()).collect::<Vec<_>>(),
            vec![0..2, 2..8, 8..10]
        );
        assert_eq!(s[1].kind, SegmentKind::Gap);
    }

    #[test]
    fn no_trailing_newline_and_eol_byte() {
        let text = "a\nb";
        let s = only(text);
        assert_eq!(s.range, 0..3);
        assert_eq!(s.eol(text), None);
        let text = "a\n";
        assert_eq!(only(text).eol(text), Some(1));
    }

    #[test]
    fn blocks_sharing_a_line_merge_into_raw() {
        // Synthetic: two normalized ranges that share a line merge into one Raw.
        let text = "x\n";
        let blocks = vec![
            RawBlock {
                range: 0..1,
                shape: Shape::Paragraph { inline: Vec::new() },
            },
            RawBlock {
                range: 1..2,
                shape: Shape::Other,
            },
        ];
        let s = build_segments(text, blocks);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, SegmentKind::Raw);
        assert!(s[0].constructs.is_empty());
        assert_eq!(s[0].range, 0..2);
    }

    #[test]
    fn real_overlapping_blocks_merge_into_raw() {
        // pulldown reports a list and a paragraph whose normalized ranges share lines here.
        for text in ["-\n aaaa", "*\n aaaa", "- \n aaa", "\n*\n aaa"] {
            let s = segs(text);
            assert_partition(text, &s);
            let blocks: Vec<_> = s.iter().filter(|s| s.kind != SegmentKind::Gap).collect();
            assert_eq!(blocks.len(), 1, "{text:?}: {s:?}");
            assert_eq!(blocks[0].kind, SegmentKind::Raw, "{text:?}");
            assert!(blocks[0].constructs.is_empty());
        }
    }

    #[test]
    fn link_reference_definition_becomes_a_gap() {
        // Characterization: pulldown emits no events for a definition, so by C-2 its
        // lines are a Gap even though they are not blank.
        let text = "[a]: /url\n\ntext\n";
        let s = segs(text);
        assert_partition(text, &s);
        assert_eq!(s[0].kind, SegmentKind::Gap);
        assert_eq!(&text[s[0].range.clone()], "[a]: /url\n\n");
        assert_eq!(s[1].kind, SegmentKind::Paragraph);
    }

    #[test]
    fn whitespace_only_lines_are_gaps() {
        assert_eq!(
            kinds("a\n   \nb"),
            vec![
                SegmentKind::Paragraph,
                SegmentKind::Gap,
                SegmentKind::Paragraph
            ]
        );
    }

    #[test]
    fn spike_fixture_partitions() {
        let text = include_str!("../fixtures/spike.md");
        let s = segs(text);
        assert_partition(text, &s);
        for seg in &s {
            assert_construct_shape(text, seg);
        }
    }

    #[test]
    fn other_fixtures_partition() {
        for text in [
            include_str!("../fixtures/no_trailing_newline.md"),
            include_str!("../fixtures/empty.md"),
        ] {
            let s = segs(text);
            assert_partition(text, &s);
            for seg in &s {
                assert_construct_shape(text, seg);
            }
        }
    }

    // ---- kinds -------------------------------------------------------------

    #[test]
    fn raw_kinds() {
        for text in [
            "Title\n-----\n",
            "Title\n=====\n",
            "- a\n- b\n",
            "1. a\n2. b\n",
            "```\ncode\n```\n",
            "    indented code\n",
            "> quote\n",
            "| a | b |\n|---|---|\n| 1 | 2 |\n",
            "<div>\nhi\n</div>\n",
            "***\n",
            "---\n",
        ] {
            assert_eq!(only(text).kind, SegmentKind::Raw, "{text:?}");
            assert!(only(text).constructs.is_empty(), "{text:?}");
        }
    }

    #[test]
    fn setext_heading_starting_with_hash_is_raw() {
        // The contract's "first non-space byte is `#`" test alone would call this ATX.
        for text in ["#tag\n---\n", "#tag\nmore\n===\n"] {
            assert_eq!(only(text).kind, SegmentKind::Raw, "{text:?}");
        }
    }

    #[test]
    fn atx_heading_levels() {
        for level in 1..=6u8 {
            let text = format!("{} h\n", "#".repeat(level.into()));
            assert_eq!(only(&text).kind, SegmentKind::Heading { level });
        }
        // Seven hashes is not a heading.
        assert_eq!(only("####### h\n").kind, SegmentKind::Paragraph);
    }

    #[test]
    fn hash_without_space_is_a_paragraph() {
        assert_eq!(only("#tag\n").kind, SegmentKind::Paragraph);
    }

    #[test]
    fn paragraph_without_constructs() {
        let s = only("plain text\n");
        assert_eq!(s.kind, SegmentKind::Paragraph);
        assert!(s.constructs.is_empty());
    }

    // ---- inline constructs -------------------------------------------------

    #[test]
    fn strong_and_emphasis_markers() {
        let text = "x **bold** and *it* and __b__ and _i_\n";
        let s = only(text);
        let got: Vec<_> = s
            .constructs
            .iter()
            .map(|c| {
                (
                    c.kind.clone(),
                    c.range.clone(),
                    c.open.clone(),
                    c.close.clone(),
                )
            })
            .collect();
        let r = |n: &str| at(text, n);
        assert_eq!(
            got,
            vec![
                (ConstructKind::Strong, r("**bold**"), 2..4, 8..10),
                (ConstructKind::Emphasis, r("*it*"), 15..16, 18..19),
                (ConstructKind::Strong, r("__b__"), 24..26, 27..29),
                (ConstructKind::Emphasis, r("_i_"), 34..35, 36..37),
            ]
        );
        assert!(s.constructs.iter().all(|c| c.parent.is_none()));
    }

    #[test]
    fn adjacent_strongs_are_siblings() {
        let text = "**a** **b**\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 2);
        assert_eq!(s.constructs[0].range, 0..5);
        assert_eq!(s.constructs[1].range, 6..11);
        assert!(s.constructs.iter().all(|c| c.parent.is_none()));
    }

    #[test]
    fn both_is_a_two_level_tree() {
        let text = "***both***\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 2, "{:?}", s.constructs);
        let (outer, inner) = (&s.constructs[0], &s.constructs[1]);
        assert_eq!(outer.parent, None);
        assert_eq!(inner.parent, Some(0));
        assert!(outer.range.start <= inner.range.start && inner.range.end <= outer.range.end);
        // Whatever order pulldown nests them in, together they own all six markers.
        let mut owned: Vec<usize> = s
            .constructs
            .iter()
            .flat_map(|c| c.open.clone().chain(c.close.clone()))
            .collect();
        owned.sort_unstable();
        assert_eq!(owned, vec![0, 1, 2, 7, 8, 9]);
        let kinds: Vec<_> = s.constructs.iter().map(|c| c.kind.clone()).collect();
        assert!(kinds.contains(&ConstructKind::Strong));
        assert!(kinds.contains(&ConstructKind::Emphasis));
        // Record pulldown's actual nesting for the verdict.
        eprintln!(
            "***both*** nesting: outer={:?} inner={:?}",
            kinds[0], kinds[1]
        );
    }

    #[test]
    fn nested_emphasis_in_strong() {
        let text = "**a *b* c**\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 2);
        assert_eq!(s.constructs[0].kind, ConstructKind::Strong);
        assert_eq!(s.constructs[1].kind, ConstructKind::Emphasis);
        assert_eq!(s.constructs[1].parent, Some(0));
        assert_eq!(s.constructs[1].open, 4..5);
        assert_eq!(s.constructs[1].close, 6..7);
    }

    #[test]
    fn strong_spanning_a_line_break_with_indentation() {
        // C-3: markers are never "range minus children"; continuation indentation is plain text.
        let text = "a **b\n   c** d\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 1);
        let c = &s.constructs[0];
        assert_eq!(c.range, at(text, "**b\n   c**"));
        assert_eq!(c.open, 2..4);
        assert_eq!(c.close, 10..12);
    }

    #[test]
    fn unclosed_escaped_and_literal_backslash_n_make_no_construct() {
        for text in [
            "an unclosed **x marker\n",
            "an unclosed *x marker\n",
            "the escape \\*not bold\\*\n",
            "\\*\\*not strong\\*\\*\n",
            "literal \\n here\n",
            "a path C:\\new\\path\n",
            "snake_case_word\n",
            "2 * 3 * 4\n",
        ] {
            let s = only(text);
            assert_eq!(s.kind, SegmentKind::Paragraph, "{text:?}");
            assert!(s.constructs.is_empty(), "{text:?}: {:?}", s.constructs);
        }
    }

    #[test]
    fn code_spans_links_and_strikethrough_make_no_construct() {
        for text in [
            "`**not bold**`\n",
            "[link](http://x)\n",
            "~~gone~~\n",
            "<b>html</b>\n",
        ] {
            let s = only(text);
            assert!(s.constructs.is_empty(), "{text:?}: {:?}", s.constructs);
        }
    }

    #[test]
    fn constructs_inside_a_link_or_strikethrough_are_found() {
        let text = "[**a**](u) ~~*b*~~\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 2);
        assert_eq!(s.constructs[0].range, at(text, "**a**"));
        assert_eq!(s.constructs[1].range, at(text, "*b*"));
    }

    #[test]
    fn unicode_does_not_shift_marker_offsets() {
        let text = "é **👍🏽 e\u{301}** 中\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 1);
        let c = &s.constructs[0];
        assert_eq!(c.open, at(text, "**"));
        assert_eq!(c.close, nth(text, "**", 1));
        assert_eq!(c.range, c.open.start..c.close.end);
    }

    // ---- heading construct ------------------------------------------------

    fn heading(text: &str) -> Construct {
        let s = only(text);
        assert!(matches!(s.kind, SegmentKind::Heading { .. }), "{text:?}");
        let c = s.constructs[0].clone();
        assert!(matches!(c.kind, ConstructKind::Heading { .. }));
        assert_eq!(c.parent, None);
        c
    }

    fn hash_run_of(c: &Construct) -> Range<usize> {
        match &c.kind {
            ConstructKind::Heading { hash_run } => hash_run.clone(),
            other => panic!("test: not a heading: {other:?}"),
        }
    }

    #[test]
    fn plain_heading_markers() {
        let text = "## Title\n";
        let c = heading(text);
        assert_eq!(c.range, 0..8);
        assert_eq!(c.open, 0..3);
        assert_eq!(c.close, 8..8);
        assert_eq!(hash_run_of(&c), 0..2);
        assert!(matches!(only(text).kind, SegmentKind::Heading { level: 2 }));
    }

    #[test]
    fn heading_without_trailing_newline() {
        let c = heading("# Title");
        assert_eq!(c.range, 0..7);
        assert_eq!(c.open, 0..2);
        assert_eq!(c.close, 7..7);
    }

    #[test]
    fn closing_sequence_is_the_closing_marker() {
        let text = "### Title ###\n";
        let c = heading(text);
        assert_eq!(c.range, 0..13);
        assert_eq!(c.open, 0..4);
        assert_eq!(c.close, 9..13);
        assert_eq!(hash_run_of(&c), 0..3);
    }

    #[test]
    fn closing_sequence_includes_trailing_whitespace() {
        let c = heading("## Title ##   \n");
        assert_eq!(c.open, 0..3);
        assert_eq!(c.close, 8..14);
        let c = heading("## Title   \n");
        assert_eq!(c.close, 8..11);
    }

    #[test]
    fn empty_heading() {
        for (text, run) in [
            ("##\n", 0..2),
            ("##", 0..2),
            ("#\n", 0..1),
            ("## \n", 0..2),
            ("###   \n", 0..3),
        ] {
            let c = heading(text);
            let line_end = text.trim_end_matches('\n').len();
            assert_eq!(c.range, 0..line_end, "{text:?}");
            assert_eq!(c.open, 0..line_end, "{text:?}");
            assert_eq!(c.close, line_end..line_end, "{text:?}");
            assert_eq!(hash_run_of(&c), run, "{text:?}");
        }
    }

    #[test]
    fn empty_heading_with_closing_sequence_only() {
        // `### ###` is a valid empty heading: the whole line is the opening marker.
        let c = heading("### ###\n");
        assert_eq!(c.open, 0..7);
        assert_eq!(c.close, 7..7);
        assert_eq!(hash_run_of(&c), 0..3);
    }

    #[test]
    fn indented_heading_markers() {
        let text = "   ### Indented heading\n";
        let s = only(text);
        assert_eq!(s.kind, SegmentKind::Heading { level: 3 });
        let c = &s.constructs[0];
        assert_eq!(c.range, 0..text.len() - 1);
        assert_eq!(c.open, 0..7);
        assert_eq!(hash_run_of(c), 3..6);
        assert_eq!(c.close, text.len() - 1..text.len() - 1);
    }

    #[test]
    fn inline_constructs_inside_a_heading_are_children_of_the_heading() {
        let text = "## Roadmap with **bold** inside\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 2);
        assert!(matches!(
            s.constructs[0].kind,
            ConstructKind::Heading { .. }
        ));
        let strong = &s.constructs[1];
        assert_eq!(strong.kind, ConstructKind::Strong);
        assert_eq!(strong.parent, Some(0));
        assert_eq!(strong.range, at(text, "**bold**"));
        assert_eq!(s.constructs[0].open, 0..3);
        assert_eq!(s.constructs[0].close, text.len() - 1..text.len() - 1);
    }

    #[test]
    fn heading_that_starts_and_ends_with_inline_constructs() {
        let text = "# **a** b *c* ##\n";
        let s = only(text);
        let h = &s.constructs[0];
        assert_eq!(h.open, 0..2);
        assert_eq!(h.close, at(text, " ##"));
        assert_eq!(s.constructs.len(), 3);
        assert!(s.constructs[1..].iter().all(|c| c.parent == Some(0)));
    }

    #[test]
    fn nested_inline_in_heading_keeps_the_tree() {
        let text = "# ***x***\n";
        let s = only(text);
        assert_eq!(s.constructs.len(), 3);
        assert_eq!(s.constructs[1].parent, Some(0));
        assert_eq!(s.constructs[2].parent, Some(1));
        assert_eq!(s.constructs[0].close, 9..9);
    }

    #[test]
    fn heading_with_non_inline_first_child() {
        let text = "# `code` tail\n";
        let c = heading(text);
        assert_eq!(c.open, 0..2);
        assert_eq!(c.close, 13..13);
    }

    #[test]
    fn heading_with_escape_first_conceals_the_backslash() {
        // Characterization (do not fix): C-3 puts content_start at the first child event.
        // pulldown reports the escaped Text event from the escaped character, so the
        // backslash falls inside the opening marker.
        let c = heading("# \\*a\n");
        assert_eq!(c.open, 0..3);
    }

    // ---- fixture -----------------------------------------------------------

    #[test]
    fn spike_fixture_constructs() {
        let text = include_str!("../fixtures/spike.md");
        let s = segs(text);
        let find = |needle: &str| {
            let r = at(text, needle);
            s.iter()
                .find(|seg| seg.range.start <= r.start && r.end <= seg.range.end)
                .unwrap_or_else(|| panic!("test: no segment holds {needle:?}"))
        };

        // H1 and H2-with-bold.
        let h1 = find("# Projection spike");
        assert_eq!(h1.kind, SegmentKind::Heading { level: 1 });
        let h2 = find("## Roadmap with **bold** inside");
        assert_eq!(h2.kind, SegmentKind::Heading { level: 2 });
        assert_eq!(h2.constructs.len(), 2);
        assert_eq!(h2.constructs[0].open.len(), 3);
        assert_eq!(h2.constructs[1].range, at(text, "**bold**"));

        // Closing sequence, empty heading, indented heading.
        let h3 = find("### Title ###");
        assert_eq!(
            h3.constructs[0].close,
            at(text, " ###\n").start..at(text, " ###\n").end - 1
        );
        let empty_start = at(text, "\n##\n").start + 1;
        let empty = s
            .iter()
            .find(|seg| seg.range.start == empty_start)
            .expect("test: segment at the empty heading");
        assert_eq!(empty.kind, SegmentKind::Heading { level: 2 });
        assert_eq!(empty.constructs[0].open, empty_start..empty_start + 2);
        assert_eq!(empty.constructs[0].close, empty_start + 2..empty_start + 2);
        let indented = find("   ### Indented heading");
        assert_eq!(indented.kind, SegmentKind::Heading { level: 3 });
        assert_eq!(indented.constructs[0].open.len(), 7);

        // The mixed-inline paragraph: bold, italic, both (2), a, b; the unclosed one is not a construct.
        let mixed = find("A paragraph with **bold**");
        assert_eq!(mixed.kind, SegmentKind::Paragraph);
        assert_eq!(mixed.constructs.len(), 6, "{:?}", mixed.constructs);
        let first = &mixed.constructs[0];
        assert_eq!(first.range, nth(text, "**bold**", 1));
        assert_eq!(first.open.len(), 2);
        assert!(
            mixed
                .constructs
                .iter()
                .all(|c| !text[c.range.clone()].contains("x marker"))
        );

        // Strong spanning a line break inside a three-line paragraph.
        let spans = find("**bold that\nspans a line break**");
        assert_eq!(spans.constructs.len(), 1);
        assert_eq!(
            spans.constructs[0].range,
            at(text, "**bold that\nspans a line break**")
        );

        // Unicode paragraph: Strong and the *éé* emphasis.
        let uni = find("Unicode: precomposed");
        assert_eq!(uni.constructs.len(), 3);
        assert_eq!(uni.constructs[1].range, at(text, "**日本語**"));

        // Literal `\n`, path and escapes: no constructs.
        let lit = find("Literal backslash-n:");
        assert_eq!(lit.kind, SegmentKind::Paragraph);
        assert!(lit.constructs.is_empty());

        // Gaps: blank runs and the whitespace-only line.
        let gap_before = |needle: &str| {
            let r = at(text, needle);
            let i = s
                .iter()
                .position(|seg| seg.range.start <= r.start && r.end <= seg.range.end)
                .expect("test: segment holds needle");
            let g = &s[i - 1];
            assert_eq!(g.kind, SegmentKind::Gap, "{needle:?}");
            &text[g.range.clone()]
        };
        assert_eq!(gap_before("Two blank lines"), "\n\n");
        assert_eq!(gap_before("Three blank lines"), "\n\n\n");
        assert_eq!(gap_before("After a whitespace"), "   \n");

        // List, fence and setext are Raw.
        assert_eq!(find("- list item one").kind, SegmentKind::Raw);
        assert_eq!(find("```rust").kind, SegmentKind::Raw);
        assert_eq!(find("Setext heading").kind, SegmentKind::Raw);
        assert_eq!(find("Last paragraph.").kind, SegmentKind::Paragraph);
    }

    // ---- hash --------------------------------------------------------------

    #[test]
    fn content_hash_is_deterministic_and_position_independent() {
        let a = segs("same text\n");
        let b = segs("same text\n");
        assert_eq!(a[0].content_hash, b[0].content_hash);
        // Same bytes at a different position and in a different kind.
        let moved = segs("other\n\nsame text\n");
        let p = moved
            .iter()
            .find(|s| s.range.start > 0 && s.kind == SegmentKind::Paragraph)
            .expect("test: second paragraph");
        assert_eq!(p.content_hash, a[0].content_hash);
        let h = segs("# x\n");
        let q = segs("x\n");
        assert_ne!(h[0].content_hash, q[0].content_hash);
        // Kind does not enter the hash: Raw and Paragraph with the same bytes agree.
        assert_eq!(segs("- a\n")[0].content_hash, content_hash(b"- a\n"));
        assert_eq!(segs("- a\n")[0].kind, SegmentKind::Raw);
    }

    #[test]
    fn content_hash_changes_with_content() {
        assert_ne!(segs("abc\n")[0].content_hash, segs("abd\n")[0].content_hash);
        assert_ne!(segs("abc\n")[0].content_hash, segs("abc")[0].content_hash);
    }

    // ---- properties --------------------------------------------------------

    fn atom() -> impl Strategy<Value = &'static str> {
        prop::sample::select(vec![
            "a", "é", "e\u{301}", "👍🏽", " ", "*", "_", "#", "\n", "\\", "n",
        ])
    }

    fn document() -> impl Strategy<Value = String> {
        prop::collection::vec(atom(), 0..60).prop_map(|atoms| atoms.concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        #[test]
        fn partition_invariant(text in document()) {
            let result = parse(&text, 3);
            prop_assert_eq!(result.version, 3);
            assert_partition(&text, &result.segments);
        }

        #[test]
        fn segments_are_whole_lines_on_char_boundaries(text in document()) {
            for s in parse(&text, 0).segments {
                prop_assert!(text.is_char_boundary(s.range.start));
                prop_assert!(text.is_char_boundary(s.range.end));
                prop_assert!(s.range.start == 0 || text.as_bytes()[s.range.start - 1] == b'\n');
                prop_assert!(s.range.end == text.len() || text.as_bytes()[s.range.end - 1] == b'\n');
            }
        }

        #[test]
        fn construct_shape(text in document()) {
            for s in parse(&text, 0).segments {
                match s.kind {
                    SegmentKind::Paragraph | SegmentKind::Heading { .. } => {}
                    _ => prop_assert!(s.constructs.is_empty()),
                }
                if matches!(s.kind, SegmentKind::Heading { .. }) {
                    let is_heading = matches!(s.constructs[0].kind, ConstructKind::Heading { .. });
                    prop_assert!(is_heading);
                    let eol = s.eol(&text).unwrap_or(s.range.end);
                    prop_assert_eq!(s.constructs[0].range.clone(), s.range.start..eol);
                    prop_assert!(!text[s.range.clone()].trim_end_matches('\n').contains('\n'));
                }
                assert_construct_shape(&text, &s);
            }
        }

        #[test]
        fn hash_matches_the_segment_bytes(text in document()) {
            for s in parse(&text, 0).segments {
                prop_assert_eq!(s.content_hash, content_hash(&text.as_bytes()[s.range.clone()]));
            }
        }

        #[test]
        fn parse_is_deterministic(text in document()) {
            prop_assert_eq!(parse(&text, 1), parse(&text, 1));
        }
    }
}
