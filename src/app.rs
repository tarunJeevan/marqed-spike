//! App state, update and view.

use iced::Element;
use iced::widget::{scrollable, text};

/// No messages yet; later tasks add the semantic key messages (C-11).
#[derive(Debug, Clone)]
pub enum Message {}

pub struct App {
    source: String,
}

impl App {
    pub fn new(source: String) -> Self {
        Self { source }
    }

    pub fn update(&mut self, message: Message) {
        match message {}
    }

    pub fn view(&self) -> Element<'_, Message> {
        scrollable(text(&self.source)).into()
    }
}
