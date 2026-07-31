//! Point viewer screen.
//!
//! Paginates through points in a Qdrant collection using cursor-based scroll
//! (`QdrantClient::scroll_points`), displays point IDs and full payload as
//! formatted JSON.

use crate::qdrant::{PointRecord, QdrantClient};
use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, List, ListDirection, ListItem, ListState, Paragraph, Wrap,
};
use serde_json::Value;
use tokio::runtime::Handle;

/// Page size per scroll request to Qdrant.
const PAGE_SIZE: usize = 20;

/// State of the current scroll request.
#[derive(Debug, Default, PartialEq)]
enum LoadState {
    #[default]
    Idle,
    Loading,
    Loaded,
    Error(String),
}

/// Sub-mode for the point viewer screen.
///
/// `Browsing` is the default list + detail view. `ConfirmDelete` shows a y/n
/// prompt. `PendingDelete` waits for the delete request to land.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Mode {
    #[default]
    Browsing,
    ConfirmDelete,
    PendingDelete,
}

/// View mode for the detail panel.
#[derive(Debug, Clone, Copy, PartialEq)]
enum ViewMode {
    Raw,
    Clean,
}

impl ViewMode {
    fn label(self) -> &'static str {
        match self {
            ViewMode::Raw => " raw ",
            ViewMode::Clean => " clean ",
        }
    }

    fn toggle(self) -> Self {
        match self {
            ViewMode::Raw => ViewMode::Clean,
            ViewMode::Clean => ViewMode::Raw,
        }
    }
}

/// Point viewer screen state.
pub struct PointViewerScreen {
    collection: String,
    points: Vec<PointRecord>,
    page_offset: Option<Value>,
    next_offset: Option<Value>,
    prev_offsets: Vec<Option<Value>>,
    list_state: ListState,
    load_state: LoadState,
    view_mode: ViewMode,
    mode: Mode,
}

impl Default for PointViewerScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl PointViewerScreen {
    pub fn new() -> Self {
        Self {
            collection: String::new(),
            points: Vec::new(),
            page_offset: None,
            next_offset: None,
            prev_offsets: Vec::new(),
            list_state: ListState::default(),
            load_state: LoadState::Idle,
            view_mode: ViewMode::Raw,
            mode: Mode::Browsing,
        }
    }

    /// Set collection and immediately load a concrete set of points (from
    /// search results, for example). Disables cursor-based pagination since
    /// these points come from a search, not a scroll.
    pub fn set_collection_and_points(&mut self, name: &str, points: Vec<PointRecord>) {
        self.collection = name.to_string();
        self.points = points;
        self.page_offset = None;
        self.next_offset = None;
        self.prev_offsets.clear();
        self.load_state = LoadState::Loaded;
        if self.points.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    pub fn set_collection(&mut self, name: &str) {
        self.collection = name.to_string();
        self.reset_to_first_page();
    }

    fn reset_to_first_page(&mut self) {
        self.points.clear();
        self.page_offset = None;
        self.next_offset = None;
        self.prev_offsets.clear();
        self.list_state.select(None);
        self.load_state = LoadState::Loading;
    }

    pub fn on_enter(&mut self) {
        if !self.collection.is_empty() {
            self.reset_to_first_page();
        } else {
            self.load_state = LoadState::Error("no collection selected".to_string());
        }
    }

    pub fn tick(&mut self, client: &QdrantClient, handle: &Handle) {
        if self.load_state != LoadState::Loading || self.collection.is_empty() {
            return;
        }
        let result = handle.block_on(client.scroll_points(
            &self.collection,
            PAGE_SIZE,
            self.page_offset.as_ref(),
        ));
        match result {
            Ok(page) => {
                self.points = page.points;
                self.next_offset = page.next_offset;
                self.load_state = LoadState::Loaded;
                if self.points.is_empty() {
                    self.list_state.select(None);
                } else {
                    self.list_state.select(Some(0));
                }
            }
            Err(e) => {
                self.load_state = LoadState::Error(format!("scroll failed: {e:#}"));
            }
        }
    }

    fn selected_point(&self) -> Option<&PointRecord> {
        let idx = self.list_state.selected()?;
        self.points.get(idx)
    }

    /// Get the ID of the currently selected point, if any.
    pub fn selected_point_id(&self) -> Option<serde_json::Value> {
        let idx = self.list_state.selected()?;
        self.points.get(idx).map(|p| p.id.clone())
    }

    /// Read-only view of the target collection name.
    pub fn collection(&self) -> &str {
        &self.collection
    }

    /// Force-refresh the point list (used by App after a delete).
    pub fn _trigger_refresh(&mut self) {
        if !self.collection.is_empty() {
            self.reset_to_first_page();
        }
    }

    /// Seed test points (test helper).
    #[cfg(test)]
    pub fn _test_seed(&mut self, points: Vec<PointRecord>) {
        self.collection = "test".to_string();
        self.points = points;
        self.load_state = LoadState::Loaded;
        if self.points.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    /// Read the current mode (test helper).
    #[cfg(test)]
    pub fn _test_mode(&self) -> &Mode {
        &self.mode
    }

    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        // Mode-aware dispatch. PendingDelete swallows every keypress so the
        // user can't fire another mutation before the in-flight one lands.
        match self.mode {
            Mode::Browsing => self.handle_browse_key(code),
            Mode::ConfirmDelete => self.handle_confirm_key(code),
            Mode::PendingDelete => true,
        }
    }

    /// Key handler for the default `Browsing` mode: navigation, page, refresh,
    /// view toggle, and the entry point into the delete confirm flow.
    fn handle_browse_key(&mut self, code: KeyCode) -> bool {
        if self.load_state == LoadState::Loading {
            return !matches!(code, KeyCode::Esc);
        }

        match code {
            KeyCode::Up => {
                if self.points.is_empty() {
                    return true;
                }
                let i = self.list_state.selected().unwrap_or(0);
                let new_i = if i == 0 { self.points.len() - 1 } else { i - 1 };
                self.list_state.select(Some(new_i));
                true
            }
            KeyCode::Down => {
                if self.points.is_empty() {
                    return true;
                }
                let i = self.list_state.selected().unwrap_or(0);
                let new_i = (i + 1) % self.points.len();
                self.list_state.select(Some(new_i));
                true
            }
            KeyCode::Char('j') | KeyCode::Char('J') => {
                if self.points.is_empty() {
                    return true;
                }
                let i = self.list_state.selected().unwrap_or(0);
                let new_i = (i + 1).min(self.points.len().saturating_sub(1));
                self.list_state.select(Some(new_i));
                true
            }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                if self.points.is_empty() {
                    return true;
                }
                let i = self.list_state.selected().unwrap_or(0);
                let new_i = i.saturating_sub(1);
                self.list_state.select(Some(new_i));
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                if self.load_state == LoadState::Loaded && self.next_offset.is_some() {
                    self.prev_offsets.push(self.page_offset.clone());
                    self.page_offset = self.next_offset.clone();
                    self.points.clear();
                    self.list_state.select(None);
                    self.next_offset = None;
                    self.load_state = LoadState::Loading;
                }
                true
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                if let Some(prev) = self.prev_offsets.pop() {
                    self.page_offset = prev;
                    self.points.clear();
                    self.list_state.select(None);
                    self.next_offset = None;
                    self.load_state = LoadState::Loading;
                }
                true
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.view_mode = self.view_mode.toggle();
                true
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                if self.load_state == LoadState::Loaded {
                    self.points.clear();
                    self.list_state.select(None);
                    self.next_offset = None;
                    self.load_state = LoadState::Loading;
                }
                true
            }
            KeyCode::Char('d') | KeyCode::Char('D') => {
                if self.selected_point().is_some() {
                    self.mode = Mode::ConfirmDelete;
                }
                true
            }
            _ => false,
        }
    }

    /// Key handler for the delete confirm. `y` arms PendingDelete, `n`/Esc cancels.
    fn handle_confirm_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.mode = Mode::PendingDelete;
                true
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                self.mode = Mode::Browsing;
                true
            }
            _ => true,
        }
    }

    /// Check whether a delete is pending and return the collection + point ID
    /// to delete. Resets the mode to `Browsing` after extracting the ID.
    /// Returns `None` if no delete is pending.
    pub fn drain_pending_delete(&mut self) -> Option<(String, serde_json::Value)> {
        if self.mode != Mode::PendingDelete {
            return None;
        }
        let point_id = self.selected_point_id()?;
        let collection = self.collection.clone();
        self.mode = Mode::Browsing;
        Some((collection, point_id))
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let chunks =
            Layout::horizontal([Constraint::Ratio(1, 3), Constraint::Ratio(2, 3)]).split(area);
        self.render_list(frame, chunks[0]);
        self.render_detail(frame, chunks[1]);
    }

    fn render_list(&mut self, frame: &mut ratatui::Frame, area: Rect) {
        let page_info = match self.load_state {
            LoadState::Idle => " (idle) ".to_string(),
            LoadState::Loading => " (loading) ".to_string(),
            LoadState::Loaded => {
                let suffix = match self.next_offset {
                    Some(_) => ", more available",
                    None => ", end of collection",
                };
                format!(" ({} points{}) ", self.points.len(), suffix)
            }
            LoadState::Error(_) => " (error) ".to_string(),
        };

        let items: Vec<ListItem> = match self.load_state {
            LoadState::Loading => vec![ListItem::new(Line::from(Span::styled(
                " Loading... ",
                Style::default().fg(Color::Yellow),
            )))],
            LoadState::Error(ref msg) => vec![ListItem::new(Line::from(vec![
                Span::styled(" Error: ", Style::default().fg(Color::Red)),
                Span::styled(msg.clone(), Style::default().fg(Color::White)),
            ]))],
            _ if self.points.is_empty() => vec![ListItem::new(Line::from(Span::styled(
                " No points on this page ",
                Style::default().fg(Color::DarkGray),
            )))],
            _ => self
                .points
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let id_str = id_to_string(&p.id);
                    let preview = p
                        .payload
                        .as_ref()
                        .and_then(|pl| pl.iter().find(|(_, v)| v.is_string()))
                        .map(|(_, v)| v.as_str().unwrap_or("").to_string())
                        .unwrap_or_default();
                    let preview_truncated = if preview.chars().count() > 60 {
                        let cut: String = preview.chars().take(60).collect();
                        format!("{cut}…")
                    } else {
                        preview
                    };
                    ListItem::new(vec![
                        Line::from(vec![
                            Span::styled(format!("#{i} "), Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                id_str,
                                Style::default()
                                    .fg(Color::Yellow)
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]),
                        Line::from(Span::styled(
                            preview_truncated,
                            Style::default().fg(Color::White),
                        )),
                    ])
                })
                .collect(),
        };

        let list = List::new(items)
            .direction(ListDirection::TopToBottom)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(format!(" Points - {} {} ", self.collection, page_info))
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

    fn render_detail(&self, frame: &mut ratatui::Frame, area: Rect) {
        let content: Vec<Line> = match self.selected_point() {
            Some(p) => {
                let id_str = id_to_string(&p.id);

                let mut lines = vec![
                    Line::from(Span::styled(
                        format!(" ID: {id_str} "),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                ];

                match self.view_mode {
                    ViewMode::Raw => {
                        let json_pretty = match p.payload.as_ref() {
                            Some(payload) => serde_json::to_string_pretty(payload)
                                .unwrap_or_else(|_| "{}".to_string()),
                            None => "{}".to_string(),
                        };
                        for json_line in json_pretty.lines() {
                            lines.push(Line::from(Span::styled(
                                format!(" {} ", json_line),
                                Style::default().fg(Color::White),
                            )));
                        }
                    }
                    ViewMode::Clean => {
                        if let Some(payload) = p.payload.as_ref() {
                            flatten_clean(&Value::Object(payload.clone()), &mut lines, 1);
                        }
                    }
                }

                lines
            }
            None => vec![Line::from(Span::styled(
                " Select a point on the left ",
                Style::default().fg(Color::DarkGray),
            ))],
        };

        let title = format!(" Payload{}", self.view_mode.label());
        let paragraph = Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(title)
                    .title_alignment(Alignment::Left),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
    }

    /// Status bar hints varying by sub-mode.
    pub fn status_bar_hints(&self) -> &'static str {
        match self.mode {
            Mode::Browsing => {
                " [Q]uit | [↑/↓] [j/k] Navigate | [N] Next page | [P] Prev | [R] Refresh | [D]elete | [T] View | [Esc] Back "
            }
            Mode::ConfirmDelete => {
                " Delete point? [Y]es | [N]o | [Esc] Cancel "
            }
            Mode::PendingDelete => {
                " Deleting point... "
            }
        }
    }
}

fn id_to_string(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Format a JSON value as clean key-value lines (no JSON syntax noise).
///
/// Nested objects and arrays are abbreviated as `{…}` / `[n items]` at
/// depth >= 2 to keep the view scannable.
fn flatten_clean(value: &Value, lines: &mut Vec<Line>, depth: usize) {
    match value {
        Value::Object(map) => {
            for (key, val) in map {
                let indent = "  ".repeat(depth);
                match val {
                    Value::String(s) => {
                        let display = s.replace('\n', "↵");
                        lines.push(Line::from(Span::styled(
                            format!(" {indent}{key}: {display}"),
                            Style::default().fg(Color::White),
                        )));
                    }
                    Value::Number(n) => {
                        lines.push(Line::from(Span::styled(
                            format!(" {indent}{key}: {n}"),
                            Style::default().fg(Color::Cyan),
                        )));
                    }
                    Value::Bool(b) => {
                        lines.push(Line::from(Span::styled(
                            format!(" {indent}{key}: {b}"),
                            Style::default().fg(Color::Magenta),
                        )));
                    }
                    Value::Null => {
                        lines.push(Line::from(Span::styled(
                            format!(" {indent}{key}: null"),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                    Value::Object(_) => {
                        if depth < 2 {
                            let prefix = format!(" {indent}{key}: ");
                            lines.push(Line::from(Span::styled(
                                prefix,
                                Style::default().fg(Color::White),
                            )));
                            flatten_clean(val, lines, depth + 1);
                        } else {
                            lines.push(Line::from(Span::styled(
                                format!(" {indent}{key}: {{…}}"),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    }
                    Value::Array(arr) => {
                        let label = format!(" {} {key}: [{} item(s)]", indent, arr.len());
                        lines.push(Line::from(Span::styled(
                            label,
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }
        }
        other => {
            lines.push(Line::from(Span::styled(
                format!(" {}", other),
                Style::default().fg(Color::White),
            )));
        }
    }
}
