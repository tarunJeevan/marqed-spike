//! App state, update and view.

use iced::Element;
use iced::widget::scrollable;

use crate::doc::{is_valid_position, prev_grapheme_boundary};
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

/// Semantic messages published by the widget (C-11). The editing messages
/// (`InsertText`, `Backspace`, `Newline`) arrive with T8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Message {
    MoveLeft,
    MoveRight,
    /// A resolved move (a click, later a vertical motion) and the `goal_x` to keep.
    MoveTo {
        offset: usize,
        goal_x: Option<f32>,
    },
    EnterInsert,
    ExitInsert,
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

pub struct App {
    source: String,
    // Read from T8, where I-8 applies a parse result only if `result.version == version`.
    #[allow(dead_code)]
    version: u64,
    parsed: ParseResult,
    /// I-1: a source byte offset on a grapheme boundary. The Normal cursor, or the Insert bar.
    cursor: usize,
    /// The one piece of view state that is stored (C-1): read by vertical motion in T7.
    #[allow(dead_code)]
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
            source,
            version,
            parsed,
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
            Message::Toggle(toggle) => self.toggle(toggle),
        }
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
        scrollable(DocumentView::new(
            &self.source,
            projections,
            kinds,
            self.cursor,
            self.mode,
            self.flags,
        ))
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
                app.update(*m);
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
        assert_eq!(run(&mut a, &[MoveRight; 5]), [3, 4, 5, 6, 6]);
        assert_eq!(run(&mut a, &[MoveLeft; 5]), [3, 2, 1, 0, 0]);
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
