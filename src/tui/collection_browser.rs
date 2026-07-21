//! Collection browser screen.
//!
//! Displays a list of Qdrant collections and detailed information
//! about the currently selected collection.

use crate::qdrant::QdrantClient;
use crossterm::event::KeyCode;
use tokio::runtime::Handle;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListDirection, ListItem, ListState, Paragraph, Wrap,
};
use std::collections::HashMap;

/// State of the collection list data.
#[derive(Debug, Default, PartialEq)]
enum LoadState {
    #[default]
    Idle,
    Loading,
    Loaded,
    Error(String),
}

/// Collection browser screen state.
pub struct CollectionBrowserScreen {
    /// List of collection names fetched from Qdrant.
    collection_names: Vec<String>,
    /// Detailed info for each collection, keyed by name.
    collection_details: HashMap<String, crate::qdrant::CollectionInfo>,
    /// Current selection index in the collection list.
    list_state: ListState,
    /// Loading state for the collection list.
    list_load_state: LoadState,
    /// Name of the collection whose detail is being loaded (if any).
    loading_detail: Option<String>,
    /// Error message for detail loading.
    detail_error: Option<String>,
}

impl Default for CollectionBrowserScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl CollectionBrowserScreen {
    /// Create a new collection browser screen.
    pub fn new() -> Self {
        Self {
            collection_names: Vec::new(),
            collection_details: HashMap::new(),
            list_state: ListState::default(),
            list_load_state: LoadState::Idle,
            loading_detail: None,
            detail_error: None,
        }
    }

    /// Called when entering this screen. Triggers a refresh of the collection list.
    pub fn on_enter(&mut self) {
        if self.list_load_state == LoadState::Idle || self.list_load_state == LoadState::Loaded {
            self.refresh_collections();
        }
    }

    /// Start refreshing the collection list.
    fn refresh_collections(&mut self) {
        self.list_load_state = LoadState::Loading;
        self.collection_names.clear();
        self.collection_details.clear();
        self.list_state.select(Some(0));
        self.loading_detail = None;
        self.detail_error = None;
    }

    /// Periodic tick. Progresses async loading by running the async call
    /// on the provided tokio runtime handle via block_on.
    pub fn tick(&mut self, client: &QdrantClient, handle: &Handle) {
        match self.list_load_state {
            LoadState::Idle => {
                self.list_load_state = LoadState::Loading;
            }
            LoadState::Loading => {
                let result = handle.block_on(client.list_collections());

                match result {
                    Ok(names) => {
                        self.collection_names = names;
                        self.list_load_state = LoadState::Loaded;
                        if !self.collection_names.is_empty() {
                            self.list_state.select(Some(0));
                            self.load_detail();
                        } else {
                            self.list_state.select(None);
                        }
                    }
                    Err(e) => {
                        self.list_load_state =
                            LoadState::Error(format!("Failed to load collections: {e:#}"));
                    }
                }
            }
            LoadState::Loaded | LoadState::Error(_) => {
                if let Some(ref name) = self.loading_detail.clone() {
                    if !self.collection_details.contains_key(name) {
                        let result = handle.block_on(client.get_collection_info(name));

                        match result {
                            Ok(info) => {
                                self.collection_details.insert(name.clone(), info);
                                self.loading_detail = None;
                                self.detail_error = None;
                            }
                            Err(e) => {
                                self.detail_error =
                                    Some(format!("Failed to load detail for '{name}': {e:#}"));
                                self.loading_detail = None;
                            }
                        }
                    } else {
                        self.loading_detail = None;
                    }
                }
            }
        }
    }
    /// Start loading detail for the currently selected collection.
    fn load_detail(&mut self) {
        let selected = self.list_state.selected().unwrap_or(0);
        if selected < self.collection_names.len() {
            let name = self.collection_names[selected].clone();
            if !self.collection_details.contains_key(&name) {
                self.loading_detail = Some(name);
                self.detail_error = None;
            }
        }
    }

    pub fn handle_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        match code {
            KeyCode::Up => {
                if self.collection_names.is_empty() {
                    return true;
                }
                let i = self.list_state.selected().unwrap_or(0);
                let new_i = if i == 0 {
                    self.collection_names.len() - 1
                } else {
                    i - 1
                };
                self.list_state.select(Some(new_i));
                self.detail_error = None;
                let sel_name = &self.collection_names[new_i];
                if !self.collection_details.contains_key(sel_name) {
                    self.loading_detail = Some(sel_name.clone());
                }
                true
            }
            KeyCode::Down => {
                if self.collection_names.is_empty() {
                    return true;
                }
                let i = self.list_state.selected().unwrap_or(0);
                let new_i = (i + 1) % self.collection_names.len();
                self.list_state.select(Some(new_i));
                self.detail_error = None;
                let sel_name = &self.collection_names[new_i];
                if !self.collection_details.contains_key(sel_name) {
                    self.loading_detail = Some(sel_name.clone());
                }
                true
            }
            KeyCode::Enter | KeyCode::Char('r') | KeyCode::Char('R') => {
                let selected = self.list_state.selected();
                if let Some(idx) = selected
                    && idx < self.collection_names.len()
                {
                    let name = &self.collection_names[idx];
                    self.collection_details.remove(name);
                    self.loading_detail = Some(name.clone());
                }
                true
            }
            _ => false,
        }
    }

    /// Render the collection browser screen.
    pub fn render(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let split =
            Layout::horizontal([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)]).split(area);
        self.render_collection_list(frame, split[0]);
        self.render_collection_detail(frame, split[1]);
    }

    fn render_collection_list(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let items: Vec<ListItem> = match self.list_load_state {
            LoadState::Loading => {
                vec![ListItem::new(Line::from(Span::styled(
                    " Loading... ",
                    Style::default().fg(Color::Yellow),
                )))]
            }
            LoadState::Error(ref msg) => {
                vec![ListItem::new(Line::from(vec![
                    Span::styled(" Error: ", Style::default().fg(Color::Red)),
                    Span::styled(msg.clone(), Style::default().fg(Color::White)),
                ]))]
            }
            LoadState::Idle | LoadState::Loaded => {
                if self.collection_names.is_empty() {
                    vec![ListItem::new(Line::from(Span::styled(
                        " No collections found ",
                        Style::default().fg(Color::DarkGray),
                    )))]
                } else {
                    self.collection_names
                        .iter()
                        .map(|name| {
                            ListItem::new(Line::from(Span::styled(
                                format!("  {name}"),
                                Style::default().fg(Color::White),
                            )))
                        })
                        .collect()
                }
            }
        };
        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Collections ")
                    .title_alignment(Alignment::Left),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .direction(ListDirection::TopToBottom);
        frame.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn render_collection_detail(&self, frame: &mut ratatui::Frame, area: Rect) {
        let selected = self.list_state.selected();
        let name = selected.and_then(|i| self.collection_names.get(i));
        let content = if let Some(name) = name {
            if let Some(ref detail_error) = self.detail_error {
                vec![
                    Line::from(Span::styled(
                        format!(" Collection: {name} "),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(detail_error, Style::default().fg(Color::Red))),
                ]
            } else if self.loading_detail.as_deref() == Some(name.as_str()) {
                vec![
                    Line::from(Span::styled(
                        format!(" Collection: {name} "),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Loading detail... ",
                        Style::default().fg(Color::Yellow),
                    )),
                ]
            } else if let Some(info) = self.collection_details.get(name.as_str()) {
                vec![
                    Line::from(Span::styled(
                        format!(" {name} "),
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Vector Size: ", Style::default().fg(Color::Cyan)),
                        Span::styled(
                            format!("{}", info.vector_size),
                            Style::default().fg(Color::White),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled("Distance:    ", Style::default().fg(Color::Cyan)),
                        Span::styled(&info.distance, Style::default().fg(Color::White)),
                    ]),
                    Line::from(vec![
                        Span::styled("Points:      ", Style::default().fg(Color::Cyan)),
                        Span::styled(
                            format!("{}", info.points_count),
                            Style::default().fg(Color::White),
                        ),
                    ]),
                ]
            } else {
                vec![
                    Line::from(Span::styled(
                        format!(" Collection: {name} "),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Queued for loading... ",
                        Style::default().fg(Color::DarkGray),
                    )),
                ]
            }
        } else {
            vec![Line::from(Span::styled(
                " No Collection Selected ",
                Style::default().fg(Color::DarkGray),
            ))]
        };
        let paragraph = Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Details ")
                    .title_alignment(Alignment::Left),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }
}
