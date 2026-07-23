//! Configuration screen.
//!
//! Edit connection settings (Qdrant URL, embedding URL, default collection,
//! embedding model) and persist to the local TOML config file.

use crate::config::Config;
use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListDirection, ListItem, ListState, Paragraph, Wrap,
};
use std::time::{Duration, Instant};

/// Labels for each editable config field, in display order.
const FIELDS: [&str; 4] = [
    "Qdrant URL",
    "Embedding URL",
    "Default Collection",
    "Embedding Model",
];

/// How long a flash message stays visible.
const FLASH_DURATION: Duration = Duration::from_millis(2500);

/// Outcome of a key press on the config screen.
#[derive(Debug, PartialEq)]
pub enum ConfigKeyOutcome {
    /// Key was consumed by the screen.
    Handled,
    /// Key is not relevant to this screen.
    Ignore,
    /// Caller should leave the configuration screen.
    Back,
}

/// Configuration screen state.
pub struct ConfigScreen {
    config: Config,
    original: Config,
    selected: usize,
    editing: bool,
    edit_buffer: String,
    edit_cursor: usize,
    dirty: bool,
    flash: Option<(String, Instant)>,
}

impl ConfigScreen {
    pub fn new(initial: Config) -> Self {
        Self {
            config: initial.clone(),
            original: initial,
            selected: 0,
            editing: false,
            edit_buffer: String::new(),
            edit_cursor: 0,
            dirty: false,
            flash: None,
        }
    }

    pub fn on_enter(&mut self) {
        self.selected = 0;
        self.editing = false;
        self.edit_buffer.clear();
        self.edit_cursor = 0;
        self.refresh_from_disk();
    }

    pub fn refresh_from_disk(&mut self) {
        match Config::load() {
            Ok(cfg) => {
                self.config = cfg.clone();
                self.original = cfg;
                self.dirty = false;
                self.flash = Some(("Reloaded from disk.".to_string(), Instant::now()));
            }
            Err(e) => {
                self.flash = Some((format!("Reload failed: {e:#}"), Instant::now()));
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Read-only access to the working config (latest edits, not yet saved).
    /// Used by `App` for read-only fallbacks like picking the default
    /// collection to open the Search screen with.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Whether the screen is currently consuming text input. Used by `App` to
    /// decide whether global quit keys (`q`, `Ctrl+C`) should fire while a
    /// user is typing in a field.
    pub fn is_text_editing(&self) -> bool {
        self.editing
    }

    pub fn current_config(&self) -> &Config {
        &self.config
    }

    /// Read-only view of the in-progress edit buffer (empty when not editing).
    pub fn edit_buffer(&self) -> &str {
        &self.edit_buffer
    }

    /// String value of the currently selected field.
    fn selected_value(&self) -> String {
        match self.selected {
            0 => self.config.qdrant_url.clone(),
            1 => self.config.embedding_url.clone(),
            2 => self.config.default_collection.clone().unwrap_or_default(),
            3 => self.config.embedding_model.clone(),
            _ => String::new(),
        }
    }

    fn commit_edit(&mut self) {
        let new_value = self.edit_buffer.trim().to_string();
        match self.selected {
            0 => self.config.qdrant_url = new_value,
            1 => self.config.embedding_url = new_value,
            2 => {
                self.config.default_collection = if new_value.is_empty() {
                    None
                } else {
                    Some(new_value)
                };
            }
            3 => self.config.embedding_model = new_value,
            _ => return,
        }
        self.dirty = self.config != self.original;
        self.editing = false;
        self.edit_buffer.clear();
        self.edit_cursor = 0;
    }

    fn cancel_edit(&mut self) {
        self.editing = false;
        self.edit_buffer.clear();
        self.edit_cursor = 0;
    }

    fn begin_edit(&mut self) {
        self.edit_buffer = self.selected_value();
        self.edit_cursor = self.edit_buffer.len();
        self.editing = true;
    }

    fn save_to_disk(&mut self) {
        match self.config.save() {
            Ok(()) => {
                self.original = self.config.clone();
                self.dirty = false;
                self.flash = Some(("Saved.".to_string(), Instant::now()));
            }
            Err(e) => {
                self.flash = Some((format!("Save failed: {e:#}"), Instant::now()));
            }
        }
    }

    pub fn discard(&mut self) {
        self.config = self.original.clone();
        self.editing = false;
        self.dirty = false;
        self.edit_buffer.clear();
        self.edit_cursor = 0;
        self.flash = Some(("Discarded changes.".to_string(), Instant::now()));
    }
    /// Handle a key press event.
    pub fn handle_key(&mut self, code: KeyCode) -> ConfigKeyOutcome {
        if self.editing {
            return self.handle_edit_key(code);
        }

        match code {
            KeyCode::Esc => ConfigKeyOutcome::Back,
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected == 0 {
                    self.selected = FIELDS.len() - 1;
                } else {
                    self.selected -= 1;
                }
                ConfigKeyOutcome::Handled
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1) % FIELDS.len();
                ConfigKeyOutcome::Handled
            }
            KeyCode::Enter => {
                self.begin_edit();
                ConfigKeyOutcome::Handled
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.save_to_disk();
                ConfigKeyOutcome::Handled
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                self.discard();
                ConfigKeyOutcome::Handled
            }
            _ => ConfigKeyOutcome::Ignore,
        }
    }

    fn handle_edit_key(&mut self, code: KeyCode) -> ConfigKeyOutcome {
        match code {
            KeyCode::Esc => {
                self.cancel_edit();
                ConfigKeyOutcome::Handled
            }
            KeyCode::Enter => {
                self.commit_edit();
                ConfigKeyOutcome::Handled
            }
            KeyCode::Backspace => {
                if self.edit_cursor > 0 {
                    self.edit_buffer.remove(self.edit_cursor - 1);
                    self.edit_cursor -= 1;
                }
                ConfigKeyOutcome::Handled
            }
            KeyCode::Delete => {
                if self.edit_cursor < self.edit_buffer.len() {
                    self.edit_buffer.remove(self.edit_cursor);
                }
                ConfigKeyOutcome::Handled
            }
            KeyCode::Left => {
                if self.edit_cursor > 0 {
                    self.edit_cursor -= 1;
                }
                ConfigKeyOutcome::Handled
            }
            KeyCode::Right => {
                if self.edit_cursor < self.edit_buffer.len() {
                    self.edit_cursor += 1;
                }
                ConfigKeyOutcome::Handled
            }
            KeyCode::Home => {
                self.edit_cursor = 0;
                ConfigKeyOutcome::Handled
            }
            KeyCode::End => {
                self.edit_cursor = self.edit_buffer.len();
                ConfigKeyOutcome::Handled
            }
            KeyCode::Char(c) => {
                self.edit_buffer.insert(self.edit_cursor, c);
                self.edit_cursor += 1;
                ConfigKeyOutcome::Handled
            }
            _ => ConfigKeyOutcome::Handled,
        }
    }

    pub fn tick(&mut self) {
        if let Some((_, shown_at)) = &self.flash
            && shown_at.elapsed() >= FLASH_DURATION
        {
            self.flash = None;
        }
    }

    pub fn render(&self, frame: &mut ratatui::Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(area);

        self.render_header(frame, chunks[0]);
        self.render_field_list(frame, chunks[1]);
        self.render_footer(frame, chunks[2]);
    }

    fn render_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        let dirty_marker = if self.dirty { " (unsaved)" } else { "" };
        let text = Paragraph::new(Line::from(Span::styled(
            format!(" Edit configuration{dirty_marker} "),
            Style::default().fg(Color::Cyan),
        )))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Echo - Config "),
        )
        .alignment(Alignment::Center);
        frame.render_widget(text, area);
    }

    fn render_field_list(&self, frame: &mut ratatui::Frame, area: Rect) {
        let items: Vec<ListItem> = FIELDS
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let selected = i == self.selected;
                let label_style = if selected {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
                };
                let label_span = Span::styled(format!(" {:<18} ", label), label_style);

                let value_spans: Vec<Span<'static>> = if selected && self.editing {
                    self.editing_value_spans()
                } else {
                    let raw = match i {
                        0 => self.config.qdrant_url.clone(),
                        1 => self.config.embedding_url.clone(),
                        2 => self
                            .config
                            .default_collection
                            .clone()
                            .unwrap_or_else(|| "(none)".to_string()),
                        3 => self.config.embedding_model.clone(),
                        _ => String::new(),
                    };
                    vec![Span::styled(raw, Style::default().fg(Color::White))]
                };

                let mut spans = Vec::with_capacity(1 + value_spans.len());
                spans.push(label_span);
                spans.extend(value_spans);
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .direction(ListDirection::TopToBottom)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Settings ")
                    .title_alignment(Alignment::Left),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            );

        let mut state = ListState::default();
        state.select(Some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn editing_value_spans(&self) -> Vec<Span<'static>> {
        let before = self.edit_buffer[..self.edit_cursor].to_string();
        let after = self.edit_buffer[self.edit_cursor..].to_string();
        vec![
            Span::styled(before, Style::default().fg(Color::White)),
            Span::styled(
                "|".to_string(),
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::styled(after, Style::default().fg(Color::White)),
        ]
    }

    fn render_footer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut lines: Vec<Line> = Vec::new();
        if self.editing {
            lines.push(Line::from(Span::styled(
                " Editing: type to insert, Backspace delete, Enter commit, Esc cancel ",
                Style::default().fg(Color::Yellow),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                " [Up/Down] select | [Enter] edit | [s] save | [d] discard | [Esc] back ",
                Style::default().fg(Color::DarkGray),
            )));
        }
        if let Some((msg, _)) = &self.flash {
            lines.push(Line::from(Span::styled(
                format!(" {msg} "),
                Style::default().fg(Color::Green),
            )));
        }
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make() -> ConfigScreen {
        ConfigScreen::new(Config::default())
    }

    #[test]
    fn fresh_screen_is_not_dirty_and_field_zero_selected() {
        let s = make();
        assert!(!s.is_dirty());
        assert_eq!(s.selected, 0);
        assert!(!s.editing);
    }

    #[test]
    fn down_advances_field() {
        let mut s = make();
        assert_eq!(s.handle_key(KeyCode::Down), ConfigKeyOutcome::Handled);
        assert_eq!(s.selected, 1);
    }

    #[test]
    fn up_wraps_to_last() {
        let mut s = make();
        assert_eq!(s.handle_key(KeyCode::Up), ConfigKeyOutcome::Handled);
        assert_eq!(s.selected, FIELDS.len() - 1);
    }

    #[test]
    fn down_wraps_at_end() {
        let mut s = make();
        s.selected = FIELDS.len() - 1;
        assert_eq!(s.handle_key(KeyCode::Down), ConfigKeyOutcome::Handled);
        assert_eq!(s.selected, 0);
    }

    #[test]
    fn enter_starts_editing_with_current_value_in_buffer() {
        let mut s = make();
        assert_eq!(s.handle_key(KeyCode::Enter), ConfigKeyOutcome::Handled);
        assert!(s.editing);
        assert_eq!(s.edit_buffer, "http://localhost:6333");
        assert_eq!(s.edit_cursor, s.edit_buffer.len());
    }

    #[test]
    fn enter_commits_edit_and_marks_dirty() {
        let mut s = make();
        s.handle_key(KeyCode::Enter);
        s.edit_buffer = "http://qdrant.localhost:80".to_string();
        s.edit_cursor = s.edit_buffer.len();
        assert_eq!(s.handle_key(KeyCode::Enter), ConfigKeyOutcome::Handled);
        assert!(!s.editing);
        assert!(s.is_dirty());
        assert_eq!(s.config.qdrant_url, "http://qdrant.localhost:80");
    }

    #[test]
    fn esc_during_edit_cancels_and_keeps_original_value() {
        let mut s = make();
        let original = s.config.qdrant_url.clone();
        s.handle_key(KeyCode::Enter);
        s.edit_buffer = "http://mutated".to_string();
        s.edit_cursor = s.edit_buffer.len();
        assert_eq!(s.handle_key(KeyCode::Esc), ConfigKeyOutcome::Handled);
        assert!(!s.editing);
        assert_eq!(s.config.qdrant_url, original);
        assert!(!s.is_dirty());
    }

    #[test]
    fn empty_default_collection_commits_as_none_and_marks_dirty() {
        let mut s = make();
        // first set to Some("docs") so clearing later is a real change.
        s.selected = 2;
        s.handle_key(KeyCode::Enter);
        s.edit_buffer = "docs".to_string();
        s.edit_cursor = s.edit_buffer.len();
        s.handle_key(KeyCode::Enter);
        assert_eq!(s.config.default_collection.as_deref(), Some("docs"));
        assert!(s.is_dirty());

        // now clear back to None.
        s.handle_key(KeyCode::Enter);
        s.edit_buffer.clear();
        s.edit_cursor = 0;
        s.handle_key(KeyCode::Enter);
        assert!(s.config.default_collection.is_none());
        assert!(!s.is_dirty());
    }

    #[test]
    fn non_empty_default_collection_commits_as_some() {
        let mut s = make();
        s.selected = 2;
        s.handle_key(KeyCode::Enter);
        s.edit_buffer = "docs".to_string();
        s.edit_cursor = s.edit_buffer.len();
        s.handle_key(KeyCode::Enter);
        assert_eq!(s.config.default_collection.as_deref(), Some("docs"));
        assert!(s.is_dirty());
    }

    #[test]
    fn discard_key_reverts_all_changes() {
        let mut s = make();
        let original_url = s.config.qdrant_url.clone();
        s.selected = 0;
        s.handle_key(KeyCode::Enter);
        s.edit_buffer = "http://mutated.example".to_string();
        s.edit_cursor = s.edit_buffer.len();
        s.handle_key(KeyCode::Enter);
        assert!(s.is_dirty());
        assert_eq!(s.config.qdrant_url, "http://mutated.example");
        assert_eq!(s.handle_key(KeyCode::Char('d')), ConfigKeyOutcome::Handled);
        assert_eq!(s.config.qdrant_url, original_url);
        assert!(!s.is_dirty());
    }

    #[test]
    fn esc_when_not_editing_signals_back() {
        let mut s = make();
        assert_eq!(s.handle_key(KeyCode::Esc), ConfigKeyOutcome::Back);
    }

    #[test]
    fn unrecognized_keys_are_ignored() {
        let mut s = make();
        assert_eq!(s.handle_key(KeyCode::F(1)), ConfigKeyOutcome::Ignore);
    }

    #[test]
    fn q_char_inside_edit_is_consumed_as_input() {
        let mut s = make();
        s.handle_key(KeyCode::Enter);
        s.edit_cursor = 0;
        assert_eq!(s.handle_key(KeyCode::Char('q')), ConfigKeyOutcome::Handled);
        // 'q' was inserted at the start of the buffer.
        assert!(s.edit_buffer.starts_with('q'));
        assert!(s.editing);
    }
}
