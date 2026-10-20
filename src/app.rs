//! App state, update and view.

use std::time::{Duration, Instant};

use iced::Element;
use iced::widget::scrollable;
use ropey::Rope;

use crate::doc::{is_valid_position, prev_grapheme_boundary, snap_to_grapheme_end};
use crate::motion::{Direction, move_horizontal};
use crate::parse::{ParseResult, parse};
use crate::projection::{Projection, check_frame_invariant, project_all};
use crate::reveal::{HeadingPolicy, Mode, NestingPolicy, RevealPolicy, reveal};
use crate::widget::DocumentView;

/// The policies the app starts with. An arbitrary starting state, not a verdict: the
/// developer picks between them with F1/F2.
const START_POLICY: RevealPolicy = RevealPolicy {
    heading: HeadingPolicy::G1,
    nesting: NestingPolicy::N1,
};

/// The debug toggles (C-11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// Heading policy G1 ↔ G2.
    F1,
    /// Nesting policy N1 ↔ N2.
    F2,
    /// Debug overlay.
    F3,
    /// Click enabled.
    F4,
    /// Identity strategy L ↔ C.
    F5,
}

/// Semantic messages published by the widget (C-11). Not `Copy`: `InsertText` owns its text.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    MoveLeft,
    MoveRight,
    /// A move the widget resolved against its layout (a click or a vertical motion) and the
    /// `goal_x` to keep: `None` for a click, the sticky column for `j`/`k`/`Up`/`Down`.
    MoveTo {
        offset: usize,
        goal_x: Option<f32>,
    },
    EnterInsert,
    ExitInsert,
    /// Typed text, inserted at the Insert bar (C-9).
    InsertText(String),
    /// Deletes the source grapheme before the bar (C-9).
    Backspace,
    /// Inserts `"\n"` at the bar (C-9).
    Newline,
    Toggle(Toggle),
}

/// The debug flags the widget reads (C-1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DebugFlags {
    pub overlay: bool,
    pub click: bool,
}

/// The block-identity strategy F5 toggles (C-10). The strategies themselves arrive in T9.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityStrategy {
    /// Position, then kind, then content hash.
    L,
    /// Content first.
    C,
}

/// The timings of one edit's reparse loop (C-12). `identity` is `None` until the identity
/// match exists (T9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditPerf {
    pub version: u64,
    pub snapshot: Duration,
    pub parse: Duration,
    pub identity: Option<Duration>,
    pub reveal_project: Duration,
    pub total: Duration,
}

impl EditPerf {
    /// The one line written to stderr per edit, and shown in the overlay.
    pub fn line(&self) -> String {
        let ms = |d: Duration| format!("{:.3}", d.as_secs_f64() * 1000.0);
        let identity = self
            .identity
            .map_or_else(|| "-".to_string(), |d| format!("{} ms", ms(d)));
        format!(
            "edit v{}: snapshot {} ms, parse {} ms, identity {}, reveal+project {} ms, total {} ms",
            self.version,
            ms(self.snapshot),
            ms(self.parse),
            identity,
            ms(self.reveal_project),
            ms(self.total),
        )
    }
}

pub struct App {
    /// C-1: the text. Edits go here; `source` is re-snapshotted from it after each one.
    rope: Rope,
    /// The `String` snapshot of `rope` that `parsed` was computed from (C-1): grapheme
    /// boundaries and slicing use it, never the rope.
    source: String,
    /// Bumped by every edit (C-9). I-8: a parse result is applied only if it carries this.
    version: u64,
    parsed: ParseResult,
    /// The latest edit's timings (C-12).
    perf: Option<EditPerf>,
    /// I-1: a source byte offset on a grapheme boundary. The Normal cursor, or the Insert bar.
    cursor: usize,
    /// The one piece of view state that is stored (C-1): the sticky column of vertical motion.
    goal_x: Option<f32>,
    mode: Mode,
    policy: RevealPolicy,
    flags: DebugFlags,
    identity: IdentityStrategy,
}

impl App {
    pub fn new(source: String) -> Self {
        let version = 0;
        let parsed = parse(&source, version);
        Self {
            rope: Rope::from_str(&source),
            source,
            version,
            parsed,
            perf: None,
            cursor: 0,
            goal_x: None,
            mode: Mode::Normal,
            policy: START_POLICY,
            flags: DebugFlags::default(),
            identity: IdentityStrategy::L,
        }
    }

    /// Projections for the current cursor, mode and policies: recomputed on demand, never
    /// stored (C-1).
    fn projections(&self) -> Vec<Projection> {
        let segments = &self.parsed.segments;
        let set = reveal(segments, self.cursor, self.mode, self.policy);
        project_all(&self.source, segments, &set)
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::MoveLeft => self.move_horizontal(Direction::Left),
            Message::MoveRight => self.move_horizontal(Direction::Right),
            Message::MoveTo { offset, goal_x } => {
                // I-1: the widget resolves clicks and vertical moves to grapheme positions.
                let offset = is_valid_position(&self.source, offset, self.mode)
                    .then_some(offset)
                    .expect("I-1: MoveTo offset is a valid grapheme position");
                self.cursor = offset;
                self.goal_x = goal_x;
            }
            // Normal `i` -> Insert, with the bar at the cursor offset.
            Message::EnterInsert if self.mode == Mode::Normal => self.mode = Mode::Insert,
            // Insert `Esc` -> Normal, with the cursor on the grapheme starting at the bar if
            // there is one, else the last grapheme, else 0.
            Message::ExitInsert if self.mode == Mode::Insert => {
                let bar = self.cursor;
                let cursor = if bar < self.source.len() {
                    bar
                } else {
                    prev_grapheme_boundary(&self.source, bar).unwrap_or(0)
                };
                self.mode = Mode::Normal;
                self.cursor = cursor;
                if cursor != bar {
                    self.goal_x = None; // the position moved, so the old column is stale
                }
            }
            Message::EnterInsert | Message::ExitInsert => {}
            // C-9: typed text goes in at the bar and the bar advances past it.
            Message::InsertText(text) if self.mode == Mode::Insert => {
                if !text.is_empty() {
                    let bar = self.cursor;
                    self.edit(bar, bar, &text);
                }
            }
            // C-9: Backspace deletes the source grapheme before the bar, concealed or not.
            Message::Backspace if self.mode == Mode::Insert => {
                if let Some(start) = prev_grapheme_boundary(&self.source, self.cursor) {
                    self.edit(start, self.cursor, "");
                }
            }
            Message::Newline if self.mode == Mode::Insert => {
                let bar = self.cursor;
                self.edit(bar, bar, "\n");
            }
            // Editing messages in Normal mode do nothing (C-9: everything else is ignored).
            Message::InsertText(_) | Message::Backspace | Message::Newline => {}
            Message::Toggle(toggle) => self.toggle(toggle),
        }
    }

    /// Replaces source bytes `start..end` with `text` and runs the reparse loop (C-9). The bar
    /// ends after the inserted text (or at `start` for a deletion). Every edit clears `goal_x`
    /// (C-7), bumps the version and writes one timing line to stderr (C-12).
    fn edit(&mut self, start: usize, end: usize, text: &str) {
        let began = Instant::now();
        // I-1: positions are source bytes; ropey does the byte -> char conversion.
        let (from, to) = (self.rope.byte_to_char(start), self.rope.byte_to_char(end));
        self.rope.remove(from..to);
        self.rope.insert(from, text);
        self.goal_x = None;

        // 1. version += 1.
        self.version += 1;
        // 2. Snapshot the rope to a String.
        let at = Instant::now();
        let snapshot = self.rope.to_string();
        let snapshot_time = at.elapsed();
        // 3. Parse it.
        let at = Instant::now();
        let result = parse(&snapshot, self.version);
        let parse_time = at.elapsed();
        // 4. Apply it only if it is current (I-8). The loop is synchronous, so it always is.
        let applied = self.apply_parse(snapshot, result);
        debug_assert!(applied, "I-8: a synchronous parse is never stale");
        // 5. The identity match (C-10) arrives with T9.

        // Typed text can merge with its neighbours into one grapheme cluster (a combining mark
        // typed before another, say), so the literal `bar + text.len()` can fall inside a
        // cluster; the bar then goes to the end of that cluster (C-6, I-1; see F-37).
        self.cursor = snap_to_grapheme_end(&self.source, start + text.len());

        // The reveal and projection that the next frame will compute, timed once here.
        let at = Instant::now();
        let projections = self.projections();
        std::hint::black_box(&projections);
        let reveal_project = at.elapsed();

        let perf = EditPerf {
            version: self.version,
            snapshot: snapshot_time,
            parse: parse_time,
            identity: None,
            reveal_project,
            total: began.elapsed(),
        };
        eprintln!("{}", perf.line());
        self.perf = Some(perf);
    }

    /// Applies a parse result computed from `snapshot` only if it carries the current
    /// version; a stale one is discarded and nothing changes. Returns whether it was applied.
    // I-8
    fn apply_parse(&mut self, snapshot: String, result: ParseResult) -> bool {
        if result.version != self.version {
            return false;
        }
        self.source = snapshot;
        self.parsed = result;
        true
    }

    /// Every horizontal motion clears `goal_x` (C-7).
    fn move_horizontal(&mut self, dir: Direction) {
        // The snapping contract: resolved against the projection before the move (C-6).
        let projections = self.projections();
        self.cursor = move_horizontal(&self.source, &projections, self.cursor, self.mode, dir);
        self.goal_x = None;
    }

    fn toggle(&mut self, toggle: Toggle) {
        match toggle {
            Toggle::F1 => {
                self.policy.heading = match self.policy.heading {
                    HeadingPolicy::G1 => HeadingPolicy::G2,
                    HeadingPolicy::G2 => HeadingPolicy::G1,
                };
            }
            Toggle::F2 => {
                self.policy.nesting = match self.policy.nesting {
                    NestingPolicy::N1 => NestingPolicy::N2,
                    NestingPolicy::N2 => NestingPolicy::N1,
                };
            }
            Toggle::F3 => self.flags.overlay = !self.flags.overlay,
            Toggle::F4 => self.flags.click = !self.flags.click,
            Toggle::F5 => {
                self.identity = match self.identity {
                    IdentityStrategy::L => IdentityStrategy::C,
                    IdentityStrategy::C => IdentityStrategy::L,
                };
            }
        }
    }

    /// The window title doubles as the status line: mode, policies, identity strategy and
    /// whether click is on (C-11).
    pub fn title(&self) -> String {
        let heading = match self.policy.heading {
            HeadingPolicy::G1 => "G1",
            HeadingPolicy::G2 => "G2",
        };
        let nesting = match self.policy.nesting {
            NestingPolicy::N1 => "N1",
            NestingPolicy::N2 => "N2",
        };
        let identity = match self.identity {
            IdentityStrategy::L => "L",
            IdentityStrategy::C => "C",
        };
        let click = if self.flags.click { "on" } else { "off" };
        format!(
            "marqed projection spike — {:?} — {heading}/{nesting} — ids {identity} — click {click}",
            self.mode
        )
    }

    pub fn view(&self) -> Element<'_, Message> {
        // Visible offsets and pixels are recomputed every frame (C-1).
        let projections = self.projections();
        // I-6, checked every frame (C-6).
        debug_assert!(
            check_frame_invariant(&projections, self.cursor, self.mode).is_ok(),
            "I-6: cursor concealed under its own projection"
        );
        let kinds = self.parsed.segments.iter().map(|s| s.kind).collect();
        scrollable(
            DocumentView::new(
                &self.source,
                projections,
                kinds,
                self.cursor,
                self.mode,
                self.goal_x,
                self.flags,
            )
            .perf(self.perf.map(|p| p.line())),
        )
        .into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doc::{insert_bars, normal_cursors};
    use proptest::prelude::*;
    use std::path::Path;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new("fixtures").join(name)).expect("test: fixture exists")
    }

    fn app(text: &str) -> App {
        App::new(text.to_string())
    }

    /// Applies each message and records the cursor after it.
    fn run(app: &mut App, messages: &[Message]) -> Vec<usize> {
        messages
            .iter()
            .map(|m| {
                app.update(m.clone());
                // The per-frame I-6 `debug_assert!` runs inside `view`.
                let _ = app.view();
                app.cursor
            })
            .collect()
    }

    use Message::{EnterInsert, ExitInsert, MoveLeft, MoveRight};

    #[test]
    fn toggles_flip_their_state_and_back() {
        let mut a = app("# h\n\n**b**\n");
        assert_eq!(
            a.title(),
            "marqed projection spike — Normal — G1/N1 — ids L — click off"
        );
        for (toggle, expect) in [
            (Toggle::F1, "G2/N1 — ids L — click off"),
            (Toggle::F2, "G2/N2 — ids L — click off"),
            (Toggle::F4, "G2/N2 — ids L — click on"),
            (Toggle::F5, "G2/N2 — ids C — click on"),
        ] {
            a.update(Message::Toggle(toggle));
            assert!(a.title().ends_with(expect), "{toggle:?}: {}", a.title());
            let _ = a.view(); // I-6 under the new policy at the same cursor
        }
        assert!(!a.flags.overlay);
        a.update(Message::Toggle(Toggle::F3));
        assert!(a.flags.overlay);
        for t in [Toggle::F1, Toggle::F2, Toggle::F3, Toggle::F4, Toggle::F5] {
            a.update(Message::Toggle(t));
        }
        assert_eq!(
            a.title(),
            "marqed projection spike — Normal — G1/N1 — ids L — click off"
        );
        assert_eq!(a.flags, DebugFlags::default());
    }

    #[test]
    fn normal_h_and_l_follow_the_projection() {
        // `a**b**c`: `l` skips the concealed `**`, then walks the revealed markers (T4).
        let mut a = app("a**b**c");
        assert_eq!(run(&mut a, &vec![MoveRight; 5]), [3, 4, 5, 6, 6]);
        assert_eq!(run(&mut a, &vec![MoveLeft; 5]), [3, 2, 1, 0, 0]);
    }

    #[test]
    fn i_enters_insert_at_the_cursor_and_esc_returns() {
        let mut a = app("abc\n");
        run(&mut a, &[MoveRight]);
        assert_eq!((a.mode, a.cursor), (Mode::Normal, 1));
        assert_eq!(run(&mut a, &[EnterInsert]), [1]);
        assert_eq!(a.mode, Mode::Insert);
        // Insert Right moves the bar over the next grapheme.
        assert_eq!(run(&mut a, &[MoveRight, MoveRight]), [2, 3]);
        // Esc with the bar before the final `\n`: the cursor is the grapheme starting there.
        assert_eq!(run(&mut a, &[ExitInsert]), [3]);
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn esc_with_the_bar_at_the_end_lands_on_the_last_grapheme() {
        let mut a = app("ab\n");
        run(&mut a, &[EnterInsert, MoveRight, MoveRight, MoveRight]);
        assert_eq!((a.mode, a.cursor), (Mode::Insert, 3)); // the bar after the final `\n`
        assert_eq!(run(&mut a, &[ExitInsert]), [2]); // the `\n` is the last grapheme
        // No newline at the end: the last visible byte.
        let mut a = app("ab");
        run(&mut a, &[EnterInsert, MoveRight, MoveRight]);
        assert_eq!(a.cursor, 2);
        assert_eq!(run(&mut a, &[ExitInsert]), [1]);
        // An empty document: 0.
        let mut a = app("");
        assert_eq!(run(&mut a, &[EnterInsert, ExitInsert]), [0, 0]);
    }

    #[test]
    fn mode_messages_are_ignored_in_the_wrong_mode() {
        let mut a = app("abc\n");
        run(&mut a, &[ExitInsert]);
        assert_eq!(a.mode, Mode::Normal);
        run(&mut a, &[EnterInsert, EnterInsert]);
        assert_eq!(a.mode, Mode::Insert);
    }

    #[test]
    fn horizontal_motion_clears_goal_x_and_move_to_sets_it() {
        let mut a = app("abc\n");
        a.update(Message::MoveTo {
            offset: 1,
            goal_x: Some(12.5),
        });
        assert_eq!((a.cursor, a.goal_x), (1, Some(12.5)));
        a.update(MoveRight);
        assert_eq!((a.cursor, a.goal_x), (2, None));
        // Even a motion that does not move (the end of the document) clears it.
        a.update(Message::MoveTo {
            offset: 3,
            goal_x: Some(3.0),
        });
        a.update(MoveRight);
        assert_eq!((a.cursor, a.goal_x), (3, None));
        // A click publishes `goal_x: None`.
        a.update(Message::MoveTo {
            offset: 0,
            goal_x: None,
        });
        assert_eq!(a.goal_x, None);
    }

    #[test]
    fn esc_clears_goal_x_only_when_the_cursor_moves() {
        let mut a = app("ab\n");
        a.update(EnterInsert);
        a.update(Message::MoveTo {
            offset: 1,
            goal_x: Some(7.0),
        });
        a.update(ExitInsert); // the bar is before a grapheme: the cursor stays
        assert_eq!((a.cursor, a.goal_x), (1, Some(7.0)));
        a.update(EnterInsert);
        a.update(Message::MoveTo {
            offset: 3,
            goal_x: Some(7.0),
        });
        a.update(ExitInsert); // the cursor moves to the last grapheme
        assert_eq!((a.cursor, a.goal_x), (2, None));
    }

    #[test]
    #[should_panic(expected = "I-1")]
    fn move_to_an_invalid_offset_violates_the_invariant() {
        let mut a = app("e\u{301}x\n");
        a.update(Message::MoveTo {
            offset: 1, // inside the grapheme `e` + combining acute
            goal_x: None,
        });
    }

    #[test]
    #[should_panic(expected = "I-1")]
    fn move_to_the_end_is_not_a_normal_cursor() {
        let mut a = app("ab");
        a.update(Message::MoveTo {
            offset: 2,
            goal_x: None,
        });
    }

    // ---- vertical motion and goal_x (C-8) ----------------------------------------------------

    use crate::widget::{
        Caret, ParagraphCache, Vertical, layout_document, space_width, vertical_target,
    };

    /// What the widget does for a vertical key, then what App does with the message: resolve
    /// against this frame's layout, publish `MoveTo { offset, goal_x }`, apply it. Returns
    /// whether anything moved (a no-op at the first or last row publishes nothing).
    fn vertical(a: &mut App, cache: &mut ParagraphCache, dir: Vertical) -> bool {
        let projections = a.projections();
        let layout = layout_document(cache, &projections, 400.0);
        let caret = Caret {
            offset: a.cursor,
            mode: a.mode,
            goal_x: a.goal_x,
        };
        let target = vertical_target(&layout, &projections, &a.source, caret, dir, &space_width);
        let moved = target.is_some();
        if let Some((offset, goal_x)) = target {
            a.update(Message::MoveTo {
                offset,
                goal_x: Some(goal_x),
            });
        }
        let _ = a.view(); // the per-frame I-6 assert
        moved
    }

    fn move_to(a: &mut App, offset: usize) {
        a.update(Message::MoveTo {
            offset,
            goal_x: None,
        });
    }

    // Three hard lines: `abcdefgh` (0..8), `ab` (9..11), `abcdefgh` (12..20); the `\n`s are
    // at 8, 11 and 20.
    const COLUMNS: &str = "abcdefgh\nab\nabcdefgh\n";

    #[test]
    fn goal_x_is_set_by_the_first_vertical_move_and_kept_through_a_short_line() {
        let mut a = app(COLUMNS);
        let mut cache = ParagraphCache::default();
        move_to(&mut a, 6);
        assert_eq!(a.goal_x, None);
        // Down to the short line: its EOL cell. The column is now stored.
        assert!(vertical(&mut a, &mut cache, Vertical::Down));
        assert_eq!(a.cursor, 11);
        let goal = a.goal_x.expect("test: the first move stores the column");
        assert!(goal > 0.0);
        // Down again: the stored column, not the short line's end, so back at the 7th column.
        assert!(vertical(&mut a, &mut cache, Vertical::Down));
        assert_eq!((a.cursor, a.goal_x), (18, Some(goal)));
        // And up through the short line again.
        assert!(vertical(&mut a, &mut cache, Vertical::Up));
        assert_eq!((a.cursor, a.goal_x), (11, Some(goal)));
        assert!(vertical(&mut a, &mut cache, Vertical::Up));
        assert_eq!((a.cursor, a.goal_x), (6, Some(goal)));
    }

    #[test]
    fn a_horizontal_motion_clears_the_column_and_the_next_vertical_move_rereads_it() {
        let mut a = app(COLUMNS);
        let mut cache = ParagraphCache::default();
        move_to(&mut a, 6);
        assert!(vertical(&mut a, &mut cache, Vertical::Down));
        let stale = a.goal_x.expect("test: a column");
        a.update(MoveRight); // 11 -> 12, the start of the last line
        assert_eq!((a.cursor, a.goal_x), (12, None));
        assert!(vertical(&mut a, &mut cache, Vertical::Up));
        // Column 0 of the short line, not the stale column.
        assert_eq!((a.cursor, a.goal_x), (9, Some(0.0)));
        assert_ne!(a.goal_x, Some(stale));
        // A click publishes `goal_x: None` and does the same.
        assert!(vertical(&mut a, &mut cache, Vertical::Down));
        move_to(&mut a, 3);
        assert_eq!(a.goal_x, None);
    }

    #[test]
    fn a_no_op_at_either_end_changes_neither_the_cursor_nor_the_column() {
        let mut a = app("ab\ncd\n");
        let mut cache = ParagraphCache::default();
        // At the first row, with no column stored and with one.
        assert!(!vertical(&mut a, &mut cache, Vertical::Up));
        assert_eq!((a.cursor, a.goal_x), (0, None));
        a.update(Message::MoveTo {
            offset: 0,
            goal_x: Some(5.0),
        });
        assert!(!vertical(&mut a, &mut cache, Vertical::Up));
        assert_eq!((a.cursor, a.goal_x), (0, Some(5.0)));
        // At the last row.
        a.update(Message::MoveTo {
            offset: 3,
            goal_x: Some(5.0),
        });
        assert!(!vertical(&mut a, &mut cache, Vertical::Down));
        assert_eq!((a.cursor, a.goal_x), (3, Some(5.0)));
        // An empty document has no rows at all.
        let mut a = app("");
        for mode_insert in [false, true] {
            if mode_insert {
                a.update(EnterInsert);
            }
            for dir in [Vertical::Up, Vertical::Down] {
                assert!(!vertical(&mut a, &mut cache, dir));
                assert_eq!((a.cursor, a.goal_x), (0, None));
            }
        }
    }

    #[test]
    fn the_column_survives_a_mode_change() {
        let mut a = app(COLUMNS);
        let mut cache = ParagraphCache::default();
        move_to(&mut a, 6);
        assert!(vertical(&mut a, &mut cache, Vertical::Down));
        let goal = a.goal_x.expect("test: a column");
        // `i` keeps it; the bar goes down to the nearest boundary of the stored column.
        a.update(EnterInsert);
        assert_eq!((a.cursor, a.goal_x), (11, Some(goal)));
        assert!(vertical(&mut a, &mut cache, Vertical::Down));
        assert_eq!((a.cursor, a.goal_x), (18, Some(goal)));
        // Esc with the bar before a grapheme leaves the cursor, so it keeps the column too.
        a.update(ExitInsert);
        assert_eq!((a.mode, a.cursor, a.goal_x), (Mode::Normal, 18, Some(goal)));
        assert!(vertical(&mut a, &mut cache, Vertical::Up));
        assert_eq!((a.cursor, a.goal_x), (11, Some(goal)));
    }

    #[derive(Debug, Clone)]
    enum VStep {
        Up,
        Down,
        Left,
        Right,
        Insert,
        Esc,
        Click(prop::sample::Index),
        Toggle(Toggle),
    }

    fn vstep() -> impl Strategy<Value = VStep> {
        prop_oneof![
            4 => Just(VStep::Up),
            4 => Just(VStep::Down),
            2 => Just(VStep::Left),
            2 => Just(VStep::Right),
            1 => Just(VStep::Insert),
            1 => Just(VStep::Esc),
            1 => any::<prop::sample::Index>().prop_map(VStep::Click),
            1 => prop::sample::select(vec![Toggle::F1, Toggle::F2, Toggle::F3, Toggle::F4, Toggle::F5])
                .prop_map(VStep::Toggle),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(300))]

        /// `goal_x` follows the C-7/C-8 rules through any key sequence, the cursor stays a
        /// valid position, and the per-frame I-6 assert (in `view`) never fires. The rules:
        /// a vertical move stores the column on its first move and keeps it after; a no-op
        /// changes nothing; every horizontal motion and every click clears it; Esc clears it
        /// only if the cursor moved; `i` and the toggles leave it alone.
        #[test]
        fn goal_x_follows_the_rules_through_random_key_sequences(
            atoms in prop::collection::vec(
                prop::sample::select(vec![
                    "a", "é", "e\u{301}", "👍🏽", " ", "word ", "*", "_", "#", "\n", "\n\n", "**", "# ", "*a*", "-",
                ]),
                0..30,
            ),
            steps in prop::collection::vec(vstep(), 1..40),
        ) {
            let text = atoms.concat();
            let mut a = App::new(text.clone());
            let mut cache = ParagraphCache::default();
            for step in steps {
                let (before_cursor, before_goal) = (a.cursor, a.goal_x);
                match &step {
                    VStep::Up | VStep::Down => {
                        let dir = if matches!(step, VStep::Up) { Vertical::Up } else { Vertical::Down };
                        let moved = vertical(&mut a, &mut cache, dir);
                        match (moved, before_goal) {
                            (false, _) => prop_assert_eq!((a.cursor, a.goal_x), (before_cursor, before_goal)),
                            (true, Some(_)) => prop_assert_eq!(a.goal_x, before_goal, "the column is kept"),
                            (true, None) => prop_assert!(a.goal_x.is_some_and(f32::is_finite)),
                        }
                    }
                    VStep::Left | VStep::Right => {
                        a.update(if matches!(step, VStep::Left) { MoveLeft } else { MoveRight });
                        prop_assert_eq!(a.goal_x, None);
                    }
                    VStep::Insert => {
                        a.update(EnterInsert);
                        prop_assert_eq!(a.goal_x, before_goal);
                    }
                    VStep::Esc => {
                        a.update(ExitInsert);
                        if a.cursor == before_cursor {
                            prop_assert_eq!(a.goal_x, before_goal);
                        } else {
                            prop_assert_eq!(a.goal_x, None);
                        }
                    }
                    VStep::Click(i) => {
                        let valid = if a.mode == Mode::Insert { insert_bars(&text) } else { normal_cursors(&text) };
                        move_to(&mut a, valid[i.index(valid.len())]);
                        prop_assert_eq!(a.goal_x, None);
                    }
                    VStep::Toggle(t) => {
                        a.update(Message::Toggle(*t));
                        prop_assert_eq!(a.goal_x, before_goal);
                    }
                }
                let _ = a.view();
                prop_assert!(is_valid_position(&text, a.cursor, a.mode), "{:?}: {} in {:?}", text, a.cursor, a.mode);
            }
        }
    }

    // ---- editing and the reparse loop (C-9, C-12) -----------------------------------------------

    use crate::parse::{ConstructKind, SegmentKind};
    use unicode_segmentation::UnicodeSegmentation;

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

    /// An app in Insert mode on `text`, with the bar at `bar`, under `policy`.
    fn inserting(text: &str, bar: usize, policy: RevealPolicy) -> App {
        let mut a = app(text);
        a.policy = policy;
        a.update(EnterInsert);
        a.update(Message::MoveTo {
            offset: bar,
            goal_x: None,
        });
        a
    }

    fn type_text(a: &mut App, text: &str) {
        a.update(Message::InsertText(text.to_string()));
        let _ = a.view(); // the per-frame I-6 assert
    }

    /// Segment `i`'s visible text under the current reveal state.
    fn visible(a: &App, i: usize) -> String {
        a.projections()[i].visible.clone()
    }

    #[test]
    fn typing_a_heading_follows_c4_at_every_key_under_every_policy() {
        // `#`, `#`, space, `Roadmap`, one key at a time into an empty document.
        for policy in POLICIES {
            let mut a = inserting("", 0, policy);
            let mut source = String::new();
            for (step, typed) in ["#", "#", " ", "R", "o", "a", "d", "m", "a", "p"]
                .into_iter()
                .enumerate()
            {
                type_text(&mut a, typed);
                source.push_str(typed);
                let at = format!("{policy:?} after {source:?}");
                assert_eq!(
                    (a.source.as_str(), a.cursor),
                    (source.as_str(), source.len()),
                    "{at}"
                );
                // `#` alone is already an (empty) ATX heading, of the level typed so far.
                let level = if step == 0 { 1 } else { 2 };
                assert_eq!(a.parsed.segments.len(), 1, "{at}");
                assert_eq!(
                    a.parsed.segments[0].kind,
                    SegmentKind::Heading { level },
                    "{at}"
                );
                // C-4: G1 keeps the heading revealed while the bar is on its line. G2 reveals
                // it while the bar is at or before the end of the `#` run, and conceals it
                // once the space is typed (the bar is then past the run and at the end of the
                // opening marker, not strictly inside it).
                let expected = match policy.heading {
                    HeadingPolicy::G1 => source.clone(),
                    HeadingPolicy::G2 if step < 2 => source.clone(),
                    HeadingPolicy::G2 => source[3..].to_string(),
                };
                assert_eq!(visible(&a, 0), expected, "{at}");
            }
        }
    }

    #[test]
    fn typing_bold_follows_c4_at_every_key_and_reveals_when_the_bar_enters() {
        for policy in POLICIES {
            let mut a = inserting("", 0, policy);
            let text = "**important**";
            for (k, typed) in text.char_indices() {
                type_text(&mut a, &typed.to_string());
                let prefix = &text[..=k];
                let at = format!("{policy:?} after {prefix:?}");
                assert_eq!(
                    (a.source.as_str(), a.cursor),
                    (prefix, prefix.len()),
                    "{at}"
                );
                let constructs = &a.parsed.segments[0].constructs;
                match k + 1 {
                    // Unclosed or just opened: plain text, nothing to conceal.
                    1..=11 => {
                        assert!(constructs.is_empty(), "{at}: {constructs:?}");
                        assert_eq!(visible(&a, 0), prefix, "{at}");
                    }
                    // `**important*`: a literal `*` and an emphasis `*important*`. The bar is
                    // at the emphasis end, not strictly inside, so its markers are concealed.
                    12 => {
                        let kinds: Vec<_> = constructs
                            .iter()
                            .map(|c| (&c.kind, c.range.clone()))
                            .collect();
                        assert_eq!(kinds, [(&ConstructKind::Emphasis, 1..12)], "{at}");
                        assert_eq!(visible(&a, 0), "*important", "{at}");
                    }
                    // `**important**`: a strong, concealed with the bar at its end.
                    _ => {
                        let kinds: Vec<_> = constructs
                            .iter()
                            .map(|c| (&c.kind, c.range.clone()))
                            .collect();
                        assert_eq!(kinds, [(&ConstructKind::Strong, 0..13)], "{at}");
                        assert_eq!(visible(&a, 0), "important", "{at}");
                    }
                }
            }
            // The bar moving back inside the strong reveals its markers.
            a.update(MoveLeft);
            assert_eq!(visible(&a, 0), "**important**", "{policy:?}: bar 12");
            a.update(MoveLeft);
            assert_eq!(visible(&a, 0), "**important**", "{policy:?}: bar 11");
        }
    }

    #[test]
    fn enter_twice_in_the_middle_of_a_paragraph_splits_it() {
        let mut a = inserting("abcdef\n", 3, POLICIES[0]);
        assert_eq!(a.parsed.segments.len(), 1);
        a.update(Message::Newline);
        // One `\n` is a hard line inside the same paragraph.
        assert_eq!(
            (a.source.as_str(), a.cursor, a.version),
            ("abc\ndef\n", 4, 1)
        );
        assert_eq!(a.parsed.segments.len(), 1);
        a.update(Message::Newline);
        assert_eq!(
            (a.source.as_str(), a.cursor, a.version),
            ("abc\n\ndef\n", 5, 2)
        );
        let kinds: Vec<_> = a
            .parsed
            .segments
            .iter()
            .map(|s| (s.kind, s.range.clone()))
            .collect();
        assert_eq!(
            kinds,
            [
                (SegmentKind::Paragraph, 0..4),
                (SegmentKind::Gap, 4..5),
                (SegmentKind::Paragraph, 5..9),
            ]
        );
    }

    #[test]
    fn backspace_after_a_concealed_closing_marker_degrades_the_construct_to_source() {
        let mut a = inserting("**b**", 5, POLICIES[0]);
        // The bar is after the closing `**`: not inside the strong, so it is concealed.
        assert_eq!(visible(&a, 0), "b");
        assert_eq!(a.parsed.segments[0].constructs.len(), 1);
        // Backspace deletes the source grapheme before the bar, a concealed marker byte: the
        // strong is gone (a literal `*` and an emphasis `*b*` remain).
        a.update(Message::Backspace);
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("**b*", 4, 1));
        let kinds: Vec<_> = a.parsed.segments[0]
            .constructs
            .iter()
            .map(|c| c.kind.clone())
            .collect();
        assert_eq!(kinds, [ConstructKind::Emphasis]);
        assert_eq!(visible(&a, 0), "*b");
        // Once more: no construct left, and every marker byte renders as plain source.
        a.update(Message::Backspace);
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("**b", 3, 2));
        assert!(a.parsed.segments[0].constructs.is_empty());
        assert_eq!(visible(&a, 0), "**b");
    }

    #[test]
    fn backspace_deletes_a_whole_grapheme_and_is_a_no_op_at_the_start() {
        // `e` + combining acute, then a thumbs-up with a skin tone: one grapheme each.
        let mut a = inserting("xe\u{301}👍🏽", 12, POLICIES[0]);
        assert_eq!(a.source.len(), 12); // 1 + (1 + 2) + (4 + 4)
        a.update(Message::Backspace);
        assert_eq!((a.source.as_str(), a.cursor), ("xe\u{301}", 4));
        a.update(Message::Backspace);
        assert_eq!((a.source.as_str(), a.cursor), ("x", 1));
        a.update(Message::Backspace);
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("", 0, 3));
        // Nothing before the bar: no edit, no version bump.
        a.update(Message::Backspace);
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("", 0, 3));
    }

    #[test]
    fn the_version_bumps_once_per_effective_edit_and_the_parse_carries_it() {
        let mut a = inserting("ab\n", 1, POLICIES[0]);
        assert_eq!((a.version, a.parsed.version), (0, 0));
        type_text(&mut a, "x");
        assert_eq!((a.version, a.parsed.version), (1, 1));
        a.update(Message::Newline);
        assert_eq!((a.version, a.parsed.version), (2, 2));
        a.update(Message::Backspace);
        assert_eq!((a.version, a.parsed.version), (3, 3));
        // Motions, mode changes and toggles are not edits.
        for m in [
            MoveLeft,
            MoveRight,
            ExitInsert,
            EnterInsert,
            Message::Toggle(Toggle::F1),
        ] {
            a.update(m);
            assert_eq!(a.version, 3);
        }
        a.update(Message::MoveTo {
            offset: 0,
            goal_x: Some(1.0),
        });
        // Empty text and a Backspace at the start change nothing.
        a.update(Message::InsertText(String::new()));
        a.update(Message::Backspace);
        assert_eq!((a.version, a.cursor, a.source.as_str()), (3, 0, "axb\n"));
        // Editing messages are ignored in Normal mode, including a Backspace that would have
        // something before the cursor to delete.
        a.update(Message::MoveTo {
            offset: 2,
            goal_x: None,
        });
        a.update(ExitInsert);
        assert_eq!((a.mode, a.cursor), (Mode::Normal, 2));
        for m in [
            Message::InsertText("z".to_string()),
            Message::Backspace,
            Message::Newline,
        ] {
            a.update(m);
        }
        assert_eq!((a.version, a.source.as_str()), (3, "axb\n"));
    }

    #[test]
    fn an_edit_clears_goal_x() {
        let mut a = inserting("ab\ncd\n", 1, POLICIES[0]);
        a.update(Message::MoveTo {
            offset: 1,
            goal_x: Some(9.0),
        });
        assert_eq!(a.goal_x, Some(9.0));
        type_text(&mut a, "x");
        assert_eq!(a.goal_x, None);
        a.goal_x = Some(9.0);
        a.update(Message::Backspace);
        assert_eq!(a.goal_x, None);
        a.goal_x = Some(9.0);
        a.update(Message::Newline);
        assert_eq!(a.goal_x, None);
    }

    #[test]
    fn a_stale_parse_result_is_discarded() {
        let mut a = inserting("ab\n", 2, POLICIES[0]);
        type_text(&mut a, "c");
        type_text(&mut a, "d");
        assert_eq!((a.version, a.source.as_str()), (2, "abcd\n"));
        let before = a.parsed.clone();

        // Computed from an older version, and from a version that does not exist yet.
        for version in [0, 1, 3, u64::MAX] {
            let applied = a.apply_parse("zzz\n\nzzz\n".to_string(), parse("zzz\n\nzzz\n", version));
            assert!(!applied, "version {version} is not current");
            assert_eq!(
                (a.source.as_str(), &a.parsed),
                ("abcd\n", &before),
                "version {version}"
            );
        }
        // The current one applies.
        let fresh = parse("abcd\n", 2);
        assert!(a.apply_parse("abcd\n".to_string(), fresh.clone()));
        assert_eq!(a.parsed, fresh);
    }

    #[test]
    fn typed_text_that_merges_into_a_cluster_leaves_the_bar_on_a_boundary() {
        // F-37: C-9's `bar + t.len()` can land inside a grapheme cluster when the typed text
        // merges with what follows; the bar goes to the end of that cluster instead.
        // A combining acute after the bar: `e` joins it into one grapheme.
        let mut a = inserting("\u{301}b\n", 0, POLICIES[0]);
        type_text(&mut a, "e");
        assert_eq!((a.source.as_str(), a.cursor), ("e\u{301}b\n", 3));
        // Two regional indicators form one flag; typing the first before the second.
        let mut a = inserting("\u{1F1F8}x", 0, POLICIES[0]);
        type_text(&mut a, "\u{1F1FA}");
        assert_eq!((a.source.as_str(), a.cursor), ("\u{1F1FA}\u{1F1F8}x", 8));
        // Multi-character text (an IME commit, say) is one edit.
        let mut a = inserting("", 0, POLICIES[0]);
        type_text(&mut a, "é👍🏽");
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("é👍🏽", 10, 1));
    }

    #[test]
    fn each_edit_is_timed_and_the_line_names_every_stage() {
        let mut a = inserting("ab\n", 1, POLICIES[0]);
        assert_eq!(a.perf, None);
        type_text(&mut a, "x");
        let perf = a.perf.expect("test: an edit records its timing");
        assert_eq!(perf.version, 1);
        // The identity match arrives in T9.
        assert_eq!(perf.identity, None);
        assert!(perf.total >= perf.snapshot + perf.parse + perf.reveal_project);
        let line = perf.line();
        for part in [
            "edit v1:",
            "snapshot",
            "parse",
            "identity -",
            "reveal+project",
            "total",
        ] {
            assert!(line.contains(part), "{line}");
        }
        // A non-edit leaves the last line alone.
        a.update(MoveLeft);
        assert_eq!(a.perf, Some(perf));

        let fixed = EditPerf {
            version: 7,
            snapshot: Duration::from_micros(1500),
            parse: Duration::from_micros(250),
            identity: Some(Duration::from_micros(3)),
            reveal_project: Duration::from_millis(2),
            total: Duration::from_micros(4000),
        };
        assert_eq!(
            fixed.line(),
            "edit v7: snapshot 1.500 ms, parse 0.250 ms, identity 0.003 ms, reveal+project 2.000 ms, total 4.000 ms"
        );
    }

    /// Feeds the real widget's messages for one batch of key events back into the app.
    fn through_the_widget(a: &mut App, drive: impl FnOnce(&mut iced_test::Simulator<'_, Message>)) {
        let mut ui = iced_test::simulator(a.view());
        drive(&mut ui);
        let messages: Vec<Message> = ui.into_messages().collect();
        for m in messages {
            a.update(m);
            let _ = a.view();
        }
    }

    #[test]
    fn keys_through_the_real_widget_edit_the_document() {
        use iced::keyboard::Key;
        use iced::keyboard::key::Named;
        let mut a = app("ab\n");
        a.update(EnterInsert);
        through_the_widget(&mut a, |ui| {
            ui.typewrite("hi");
        });
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("hiab\n", 2, 2));
        through_the_widget(&mut a, |ui| {
            ui.tap_key(Key::Named(Named::Enter));
        });
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("hi\nab\n", 3, 3));
        through_the_widget(&mut a, |ui| {
            ui.tap_key(Key::Named(Named::Backspace));
        });
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("hiab\n", 2, 4));
        through_the_widget(&mut a, |ui| {
            ui.tap_key(Key::Named(Named::ArrowRight));
            ui.typewrite("!");
        });
        assert_eq!((a.source.as_str(), a.cursor, a.version), ("hia!b\n", 4, 5));
        // Esc: back to Normal on the grapheme starting at the bar. Typing does nothing there.
        through_the_widget(&mut a, |ui| {
            ui.tap_key(Key::Named(Named::Escape));
        });
        assert_eq!((a.mode, a.cursor), (Mode::Normal, 4));
        through_the_widget(&mut a, |ui| {
            ui.typewrite("xy");
            ui.tap_key(Key::Named(Named::Backspace));
            ui.tap_key(Key::Named(Named::Enter));
        });
        assert_eq!((a.source.as_str(), a.version), ("hia!b\n", 5));
        // The rope was edited, not just the snapshot.
        assert_eq!(a.rope.to_string(), a.source);
    }

    /// A plain-`String` model of an Insert session: the same edits, with the cluster
    /// boundaries taken straight from `unicode-segmentation`.
    struct Model {
        text: String,
        bar: usize,
        edits: u64,
    }

    impl Model {
        fn boundaries(&self) -> Vec<usize> {
            let mut b: Vec<usize> = self.text.grapheme_indices(true).map(|(i, _)| i).collect();
            b.push(self.text.len());
            b
        }

        /// The first boundary at or after `o`.
        fn snap_forward(&self, o: usize) -> usize {
            self.boundaries()
                .into_iter()
                .find(|&b| b >= o)
                .expect("test: the end is a boundary")
        }

        fn insert(&mut self, text: &str) {
            self.text.insert_str(self.bar, text);
            self.bar = self.snap_forward(self.bar + text.len());
            self.edits += 1;
        }

        fn backspace(&mut self) {
            let start = self
                .boundaries()
                .into_iter()
                .filter(|&b| b < self.bar)
                .max();
            if let Some(start) = start {
                self.text.replace_range(start..self.bar, "");
                self.bar = self.snap_forward(start);
                self.edits += 1;
            }
        }
    }

    #[derive(Debug, Clone)]
    enum Key {
        Type(&'static str),
        Backspace,
        Enter,
        Left,
        Right,
        EscThenI,
    }

    fn key() -> impl Strategy<Value = Key> {
        prop_oneof![
            6 => prop::sample::select(vec![
                "a", "é", "e\u{301}", "\u{301}", "👍🏽", " ", "*", "_", "#", "\\", "n", "**", "# ",
                "*a*", "-", "word ", "\u{1F1FA}", "\u{1F1F8}", "ab cd",
            ]).prop_map(Key::Type),
            3 => Just(Key::Backspace),
            2 => Just(Key::Enter),
            2 => Just(Key::Left),
            2 => Just(Key::Right),
            1 => Just(Key::EscThenI),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// Random Insert sessions: the rope, the snapshot and a plain `String` model agree after
        /// every key; the parse is the parse of that text at the current version; the version
        /// counts effective edits; the bar is a valid position and the per-frame I-6 assert (in
        /// `view`) never fires, under all four policies.
        #[test]
        fn insert_sessions_match_a_string_model(
            atoms in prop::collection::vec(
                prop::sample::select(vec![
                    "a", "é", "e\u{301}", "👍🏽", " ", "*", "_", "#", "\n", "\\", "n", "**", "# ", "*a*", "-",
                ]),
                0..20,
            ),
            policy in prop::sample::select(POLICIES.to_vec()),
            start in any::<prop::sample::Index>(),
            keys in prop::collection::vec(key(), 1..40),
        ) {
            let text = atoms.concat();
            let bars = insert_bars(&text);
            let bar = bars[start.index(bars.len())];
            let mut a = inserting(&text, bar, policy);
            let mut model = Model { text: text.clone(), bar, edits: 0 };
            for k in keys {
                let moved = match k {
                    Key::Type(t) => { type_text(&mut a, t); model.insert(t); false }
                    Key::Backspace => { a.update(Message::Backspace); model.backspace(); false }
                    Key::Enter => { a.update(Message::Newline); model.insert("\n"); false }
                    Key::Left => { a.update(MoveLeft); true }
                    Key::Right => { a.update(MoveRight); true }
                    Key::EscThenI => { a.update(ExitInsert); a.update(EnterInsert); true }
                };
                let _ = a.view();
                if moved {
                    // Motions are T4's; take where the app put the bar.
                    model.bar = a.cursor;
                }
                let at = format!("{:?} {:?} {:?}", text, policy, model.text);
                prop_assert_eq!(&a.source, &model.text, "{}", at);
                prop_assert_eq!(a.rope.to_string(), model.text.clone(), "{}", at);
                prop_assert_eq!(a.cursor, model.bar, "{}", at);
                prop_assert!(is_valid_position(&a.source, a.cursor, Mode::Insert), "{}", at);
                prop_assert_eq!(a.version, model.edits, "{}", at);
                prop_assert_eq!(&a.parsed, &parse(&a.source, a.version), "{}", at);
            }
        }
    }

    #[test]
    fn every_fixture_builds_a_view() {
        for name in ["spike.md", "no_trailing_newline.md", "empty.md"] {
            let a = app(&fixture(name));
            assert_eq!(a.version, 0);
            assert_eq!(a.parsed.version, 0);
            let _ = a.view();
        }
    }

    #[derive(Debug, Clone)]
    enum Step {
        Left,
        Right,
        Insert,
        Esc,
        To(prop::sample::Index),
        Toggle(Toggle),
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            3 => Just(Step::Left),
            3 => Just(Step::Right),
            1 => Just(Step::Insert),
            1 => Just(Step::Esc),
            1 => any::<prop::sample::Index>().prop_map(Step::To),
            1 => prop::sample::select(vec![Toggle::F1, Toggle::F2, Toggle::F3, Toggle::F4, Toggle::F5])
                .prop_map(Step::Toggle),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// Any sequence of key messages keeps the cursor valid for its mode and the frame
        /// invariant (checked by `view`'s `debug_assert!`), and never panics.
        #[test]
        fn random_message_sequences_keep_the_invariants(
            atoms in prop::collection::vec(
                prop::sample::select(vec![
                    "a", "é", "e\u{301}", "👍🏽", " ", "*", "_", "#", "\n", "\\", "n", "**", "# ", "*a*",
                ]),
                0..30,
            ),
            steps in prop::collection::vec(step(), 1..40),
        ) {
            let text = atoms.concat();
            let mut a = App::new(text.clone());
            for s in steps {
                let message = match s {
                    Step::Left => MoveLeft,
                    Step::Right => MoveRight,
                    Step::Insert => EnterInsert,
                    Step::Esc => ExitInsert,
                    Step::Toggle(t) => Message::Toggle(t),
                    Step::To(i) => {
                        let valid = if a.mode == Mode::Insert {
                            insert_bars(&text)
                        } else {
                            normal_cursors(&text)
                        };
                        Message::MoveTo { offset: valid[i.index(valid.len())], goal_x: None }
                    }
                };
                a.update(message);
                let _ = a.view();
                prop_assert!(is_valid_position(&text, a.cursor, a.mode), "{:?}: {} in {:?}", text, a.cursor, a.mode);
            }
        }
    }
}
