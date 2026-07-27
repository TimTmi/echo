//! Search screen.
//!
//! Text input for query, generates embedding, searches Qdrant,
//! displays ranked results.

use crate::embedding::EmbeddingClient;
use crate::qdrant::{QdrantClient, SearchResult};
use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListDirection, ListItem, Paragraph};
use tokio::runtime::Handle;

/// State of the search pipeline.
#[derive(Debug, Default, PartialEq)]
enum SearchState {
    #[default]
    Idle,
    GeneratingEmbedding,
    Searching,
    Done,
    Error(String),
}

/// Search screen state.
pub struct SearchScreen {
    query: String,
    cursor: usize,
    input_focused: bool,
    collection: String,
    results: Vec<SearchResult>,
    list_state: ratatui::widgets::ListState,
    search_state: SearchState,
    pending_query: String,
    pending_vector: Vec<f32>,
}

impl Default for SearchScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchScreen {
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor: 0,
            input_focused: true,
            collection: String::new(),
            results: Vec::new(),
            list_state: ratatui::widgets::ListState::default(),
            search_state: SearchState::Idle,
            pending_query: String::new(),
            pending_vector: Vec::new(),
        }
    }

    pub fn set_collection(&mut self, name: &str) {
        self.collection = name.to_string();
    }

    /// Whether the screen is currently consuming text input. The search
    /// input is always focused once `on_enter` has run, so this returns
    /// `true` for the lifetime of the screen.
    /// Used by `App` to decide whether global quit keys (`q`, `Ctrl+C`)
    /// should fire.
    pub fn is_text_editing(&self) -> bool {
        self.input_focused
    }

    /// Read-only view of the current query text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Read-only view of the collection name scoped to the next search.
    /// Empty if no collection has been set (e.g. entered Search from Home
    /// without a configured default).
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Read-only view of the current results list.
    pub fn results(&self) -> &[SearchResult] {
        &self.results
    }

    /// Index of the currently selected result, if any.
    pub fn selected_index(&self) -> Option<usize> {
        self.list_state.selected()
    }

    /// The currently highlighted search result, if any.
    pub fn selected_result(&self) -> Option<&SearchResult> {
        let idx = self.list_state.selected()?;
        self.results.get(idx)
    }

    /// Reset selection state (e.g. after a new search completes).
    fn reset_selection(&mut self) {
        if self.results.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    pub fn on_enter(&mut self) {
        self.results.clear();
        self.search_state = SearchState::Idle;
        self.input_focused = true;
    }

    pub fn tick(
        &mut self,
        client: &QdrantClient,
        embedding_client: &EmbeddingClient,
        handle: &Handle,
    ) {
        match self.search_state {
            SearchState::GeneratingEmbedding => {
                let text = self.pending_query.clone();
                let result = handle.block_on(embedding_client.generate_embedding(&text));
                match result {
                    Ok(vector) => {
                        self.pending_vector = vector;
                        self.search_state = SearchState::Searching;
                    }
                    Err(e) => {
                        self.search_state = SearchState::Error(format!("Embedding failed: {e:#}"));
                    }
                }
            }
            SearchState::Searching if !self.pending_vector.is_empty() => {
                if self.collection.is_empty() {
                    self.search_state = SearchState::Error("no collection selected".to_string());
                    return;
                }
                let vector = self.pending_vector.clone();
                let collection = self.collection.clone();
                let result = handle.block_on(client.search_points(&collection, &vector, 10));
                match result {
                    Ok(results) => {
                        self.results = results;
                        self.search_state = SearchState::Done;
                        self.reset_selection();
                    }
                    Err(e) => {
                        self.search_state = SearchState::Error(format!("Search failed: {e:#}"));
                    }
                }
            }
            _ => {}
        }
    }

    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        if self.search_state != SearchState::Idle && self.search_state != SearchState::Done {
            return true;
        }

        match code {
            KeyCode::Enter => {
                let q = self.query.trim().to_string();
                if q.is_empty() {
                    return true;
                }
                self.pending_query = q;
                self.results.clear();
                self.search_state = SearchState::GeneratingEmbedding;
                self.input_focused = false;
                true
            }
            // Result navigation — before the general Char arm so 'j'/'k' aren't eaten
            KeyCode::Down if !self.input_focused && !self.results.is_empty() => {
                let i = self.list_state.selected().unwrap_or(0);
                let next = (i + 1).min(self.results.len().saturating_sub(1));
                self.list_state.select(Some(next));
                true
            }
            KeyCode::Up if !self.input_focused && !self.results.is_empty() => {
                let i = self.list_state.selected().unwrap_or(0);
                let prev = i.saturating_sub(1);
                self.list_state.select(Some(prev));
                true
            }
            KeyCode::Char('j') if !self.input_focused && !self.results.is_empty() => {
                let i = self.list_state.selected().unwrap_or(0);
                let next = (i + 1).min(self.results.len().saturating_sub(1));
                self.list_state.select(Some(next));
                true
            }
            KeyCode::Char('k') if !self.input_focused && !self.results.is_empty() => {
                let i = self.list_state.selected().unwrap_or(0);
                let prev = i.saturating_sub(1);
                self.list_state.select(Some(prev));
                true
            }
            KeyCode::Char(c) => {
                self.query.insert(self.cursor, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.query.remove(self.cursor);
                }
                true
            }
            KeyCode::Delete => {
                if self.cursor < self.query.len() {
                    self.query.remove(self.cursor);
                }
                true
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                true
            }
            KeyCode::Right => {
                if self.cursor < self.query.len() {
                    self.cursor += 1;
                }
                true
            }
            KeyCode::Home => {
                self.cursor = 0;
                true
            }
            KeyCode::End => {
                self.cursor = self.query.len();
                true
            }
            _ => false,
        }
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

        self.render_input(frame, chunks[0]);
        self.render_status(frame, chunks[1]);
        self.render_results(frame, chunks[2]);
    }

    fn render_input(&self, frame: &mut ratatui::Frame, area: Rect) {
        let input_text = if self.query.is_empty() && self.input_focused {
            Span::styled(
                " Type query and press Enter to search...",
                Style::default().fg(Color::DarkGray),
            )
        } else {
            Span::raw(&self.query)
        };

        let collection_label = if self.collection.is_empty() {
            "no collection".to_string()
        } else {
            self.collection.clone()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(format!(" Search Query [{collection_label}] "))
            .title_alignment(Alignment::Left);

        let paragraph = Paragraph::new(Line::from(input_text)).block(block);
        frame.render_widget(paragraph, area);

        if self.input_focused {
            let cursor_x = area.x + 1 + self.cursor.min(self.query.len()) as u16;
            let cursor_y = area.y + 1;
            frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
        }
    }

    fn render_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        let status = match &self.search_state {
            SearchState::Idle => {
                if self.results.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Found {} results. R to refresh, Esc to go back.",
                        self.results.len()
                    )
                }
            }
            SearchState::GeneratingEmbedding => " Generating embedding... ".to_string(),
            SearchState::Searching => " Searching Qdrant... ".to_string(),
            SearchState::Done => format!(" Search complete. {} results.", self.results.len()),
            SearchState::Error(e) => format!(" Error: {e} "),
        };

        let style = match &self.search_state {
            SearchState::Error(_) => Style::default().fg(Color::White).bg(Color::Red),
            _ => Style::default().fg(Color::Cyan),
        };

        let paragraph = Paragraph::new(Line::from(Span::styled(status, style)));
        frame.render_widget(paragraph, area);
    }

    fn render_results(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        if self.results.is_empty() {
            let msg = if self.search_state == SearchState::Idle {
                " Enter a query above and press Enter to search."
            } else {
                " No results found."
            };
            let paragraph = Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(Color::DarkGray),
            )))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Results ")
                    .title_alignment(Alignment::Left),
            );
            frame.render_widget(paragraph, area);
            return;
        }

        let items: Vec<ListItem> = self
            .results
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let score = r.score.unwrap_or(0.0);
                let id_str = match &r.id {
                    serde_json::Value::String(s) => s.clone(),
                    serde_json::Value::Number(n) => n.to_string(),
                    _ => format!("{:?}", r.id),
                };
                let preview_lines: Vec<Line> = Self::build_preview_lines(i, score, &id_str, &r.payload);

                ListItem::new(preview_lines)
            })
            .collect();

        let title = " Results ";

        let list = List::new(items)
            .direction(ListDirection::TopToBottom)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(title)
                    .title_alignment(Alignment::Left),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );

        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    /// Build the display lines for a single search result.
    /// Priority: (1) "text" field → truncated text content,
    ///           (2) "source_display" / "source" → file path/URL,
    ///           (3) first string field found → key: value,
    ///           (4) fallback → "(no content)".
    fn build_preview_lines(
        i: usize,
        score: f64,
        id_str: &str,
        payload: &Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Vec<Line<'static>> {
        // Priority 1: "text" field — actual chunk content
        if let Some(text) = payload
            .as_ref()
            .and_then(|p| p.get("text"))
            .and_then(|v| v.as_str())
        {
            let max_chars = 200;
            let truncated = if text.len() > max_chars {
                format!("{}...", &text[..max_chars])
            } else {
                text.to_string()
            };
            return vec![
                Line::from(vec![
                    Span::styled(format!("#{}  ", i), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("Score: {:.4}", score),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ID: {id_str}"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Text: ", Style::default().fg(Color::Cyan)),
                    Span::styled(truncated, Style::default().fg(Color::White)),
                ]),
            ];
        }

        // Priority 2: "source_display" or "source" — file path / URL
        if let Some(source_val) = payload
            .as_ref()
            .and_then(|p| p.get("source_display").or_else(|| p.get("source")))
            .and_then(|v| v.as_str())
        {
            let source = source_val.to_string();
            return vec![
                Line::from(vec![
                    Span::styled(format!("#{}  ", i), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("Score: {:.4}", score),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ID: {}", id_str),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Source: ".to_string(), Style::default().fg(Color::Cyan)),
                    Span::styled(source, Style::default().fg(Color::White)),
                ]),
            ];
        }

        // Priority 3: first string field found
        if let Some((k, v)) = payload
            .as_ref()
            .and_then(|p| p.iter().find(|(_, v)| v.is_string()))
        {
            let val = v.as_str().unwrap_or("").to_string();
            return vec![
                Line::from(vec![
                    Span::styled(format!("#{}  ", i), Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("Score: {:.4}", score),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  ID: {}", id_str),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(
                        format!("{k}: "),
                        Style::default().fg(Color::Cyan),
                    ),
                    Span::styled(val, Style::default().fg(Color::White)),
                ]),
            ];
        }

        // Fallback: no recognizable content
        vec![
            Line::from(vec![
                Span::styled(format!("#{}  ", i), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("Score: {:.4}", score),
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("  ID: {}", id_str),
                    Style::default().fg(Color::DarkGray),
                ),
            ]),
            Line::from(Span::styled(
                "(no content)".to_string(),
                Style::default().fg(Color::DarkGray),
            )),
        ]
    }
}
