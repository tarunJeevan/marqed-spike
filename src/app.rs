//! App state, update and view.

use iced::Element;
use iced::widget::scrollable;

use crate::parse::{ParseResult, parse};
use crate::projection::{check_frame_invariant, project_all};
use crate::reveal::{HeadingPolicy, Mode, NestingPolicy, RevealPolicy, reveal};
use crate::widget::DocumentView;

/// The policies the app starts with. An arbitrary starting state, not a verdict: the
/// developer picks between them with F1/F2 once those toggles exist.
const START_POLICY: RevealPolicy = RevealPolicy {
    heading: HeadingPolicy::G1,
    nesting: NestingPolicy::N1,
};

/// The debug toggles (C-11). Later tasks add F1, F2, F4 and F5.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    F3,
}

/// Semantic messages published by the widget (C-11). Later tasks add the motion,
/// mode and editing messages.
#[derive(Debug, Clone)]
pub enum Message {
    Toggle(Toggle),
}

pub struct App {
    source: String,
    // Read from T8, where I-8 applies a parse result only if `result.version == version`.
    #[allow(dead_code)]
    version: u64,
    parsed: ParseResult,
    /// I-1: a source byte offset on a grapheme boundary.
    cursor: usize,
    mode: Mode,
    policy: RevealPolicy,
    overlay: bool,
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
            mode: Mode::Normal,
            policy: START_POLICY,
            overlay: false,
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Toggle(Toggle::F3) => self.overlay = !self.overlay,
        }
    }

    /// The window title doubles as the status line: mode and policies (C-11).
    pub fn title(&self) -> String {
        let heading = match self.policy.heading {
            HeadingPolicy::G1 => "G1",
            HeadingPolicy::G2 => "G2",
        };
        let nesting = match self.policy.nesting {
            NestingPolicy::N1 => "N1",
            NestingPolicy::N2 => "N2",
        };
        format!(
            "marqed projection spike — {:?} — {heading}/{nesting}",
            self.mode
        )
    }

    pub fn view(&self) -> Element<'_, Message> {
        // Visible offsets and pixels are recomputed every frame (C-1).
        let segments = &self.parsed.segments;
        let set = reveal(segments, self.cursor, self.mode, self.policy);
        let projections = project_all(&self.source, segments, &set);
        // I-6, checked every frame (C-6).
        debug_assert!(
            check_frame_invariant(&projections, self.cursor, self.mode).is_ok(),
            "I-6: cursor concealed under its own projection"
        );
        let kinds = segments.iter().map(|s| s.kind).collect();
        scrollable(DocumentView::new(projections, kinds, self.overlay)).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(Path::new("fixtures").join(name)).expect("test: fixture exists")
    }

    #[test]
    fn f3_toggles_the_overlay() {
        let mut app = App::new("text\n".to_string());
        assert!(!app.overlay);
        app.update(Message::Toggle(Toggle::F3));
        assert!(app.overlay);
        app.update(Message::Toggle(Toggle::F3));
        assert!(!app.overlay);
    }

    #[test]
    fn the_title_shows_mode_and_policies() {
        let app = App::new(String::new());
        assert_eq!(app.title(), "marqed projection spike — Normal — G1/N1");
    }

    #[test]
    fn every_fixture_builds_a_view() {
        for name in ["spike.md", "no_trailing_newline.md", "empty.md"] {
            let app = App::new(fixture(name));
            assert_eq!(app.version, 0);
            assert_eq!(app.parsed.version, 0);
            let _ = app.view();
        }
    }
}
