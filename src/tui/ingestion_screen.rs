//! Ingestion screen.
//!
//! Extracts text from files/URLs/paste, chunks it, generates embeddings,
//! and upserts the resulting points into a Qdrant collection.

use crate::embedding::EmbeddingClient;
use crate::ingestion::chunker::{ChunkConfig, ChunkMode};
use crate::ingestion::extractor::{Input, Source};
use crate::ingestion::{process, Chunk};
use crate::qdrant::{QdrantClient, UpsertPoint};
use crossterm::event::KeyCode;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListDirection, ListItem, Paragraph, Wrap};
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

const FLASH_DURATION: Duration = Duration::from_secs(4);

#[derive(Debug, Default, PartialEq)]
enum ScreenState {
    #[default]
    Idle,
    InputText { buffer: String, cursor: usize },
    InputFilePath { buffer: String, cursor: usize },
    InputUrl { buffer: String, cursor: usize },
    ProcessingExtraction,
    Reviewing,
    GeneratingEmbeddings { index: usize, total: usize },
    Upserting { index: usize, total: usize },
    Done,
    Error(String),
}

pub struct IngestionScreen {
    state: ScreenState,
    chunks: Vec<Chunk>,
    target_collection: String,
    chunk_mode: ChunkMode,
    chunk_size: usize,
    chunk_overlap: usize,
    pending_input: Option<Input>,
    pending_vector: Vec<f32>,
    flash: Option<(String, Instant)>,
}

impl IngestionScreen {
    pub fn new() -> Self {
        Self {
            state: ScreenState::Idle,
            chunks: Vec::new(),
            target_collection: String::new(),
            chunk_mode: ChunkMode::StructureAware,
            chunk_size: 512,
            chunk_overlap: 64,
            pending_input: None,
            pending_vector: Vec::new(),
            flash: None,
        }
    }

    pub fn set_default_collection(&mut self, name: &str) {
        if self.target_collection.is_empty() {
            self.target_collection = name.to_string();
        }
    }

    pub fn is_text_editing(&self) -> bool {
        matches!(
            self.state,
            ScreenState::InputText { .. }
                | ScreenState::InputFilePath { .. }
                | ScreenState::InputUrl { .. }
        )
    }

    pub fn on_enter(&mut self) {
        self.state = ScreenState::Idle;
        self.chunks.clear();
        self.flash = None;
    }

    #[allow(dead_code)]
    fn buffer(&self) -> &str {
        match &self.state {
            ScreenState::InputText { buffer, .. } => buffer,
            ScreenState::InputFilePath { buffer, .. } => buffer,
            ScreenState::InputUrl { buffer, .. } => buffer,
            _ => "",
        }
    }

    #[allow(dead_code)]
    fn buffer_mut(&mut self) -> Option<&mut String> {
        match &mut self.state {
            ScreenState::InputText { buffer, .. } => Some(buffer),
            ScreenState::InputFilePath { buffer, .. } => Some(buffer),
            ScreenState::InputUrl { buffer, .. } => Some(buffer),
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn cursor_mut(&mut self) -> Option<&mut usize> {
        match &mut self.state {
            ScreenState::InputText { cursor, .. } => Some(cursor),
            ScreenState::InputFilePath { cursor, .. } => Some(cursor),
            ScreenState::InputUrl { cursor, .. } => Some(cursor),
            _ => None,
        }
    }

    fn insert_char(&mut self, c: char) {
        if let ScreenState::InputText { buffer, cursor } | ScreenState::InputFilePath { buffer, cursor } | ScreenState::InputUrl { buffer, cursor } = &mut self.state {
            buffer.insert(*cursor, c);
            *cursor += 1;
        }
    }

    fn backspace(&mut self) {
        if let ScreenState::InputText { buffer, cursor } | ScreenState::InputFilePath { buffer, cursor } | ScreenState::InputUrl { buffer, cursor } = &mut self.state
            && *cursor > 0 { *cursor -= 1; buffer.remove(*cursor); }
    }

    fn delete(&mut self) {
        if let ScreenState::InputText { buffer, cursor } | ScreenState::InputFilePath { buffer, cursor } | ScreenState::InputUrl { buffer, cursor } = &mut self.state
            && *cursor < buffer.len() { buffer.remove(*cursor); }
    }

    fn cursor_left(&mut self) {
        if let ScreenState::InputText { buffer, cursor } | ScreenState::InputFilePath { buffer, cursor } | ScreenState::InputUrl { buffer, cursor } = &mut self.state {
            if *cursor > 0 { *cursor -= 1; } else { *cursor = buffer.len(); }
        }
    }

    fn cursor_right(&mut self) {
        if let ScreenState::InputText { buffer, cursor } | ScreenState::InputFilePath { buffer, cursor } | ScreenState::InputUrl { buffer, cursor } = &mut self.state {
            if *cursor < buffer.len() { *cursor += 1; } else { *cursor = 0; }
        }
    }

    fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), Instant::now()));
    }

    pub fn handle_key(&mut self, code: KeyCode) -> bool {
        match &self.state {
            ScreenState::Idle => self.handle_idle_key(code),
            ScreenState::InputText { .. }
            | ScreenState::InputFilePath { .. }
            | ScreenState::InputUrl { .. } => self.handle_input_key(code),
            ScreenState::Reviewing => match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    if self.chunks.is_empty() {
                        self.set_flash("No chunks to upsert.");
                        return true;
                    }
                    let total = self.chunks.len();
                    self.state = ScreenState::GeneratingEmbeddings { index: 0, total };
                    true
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.chunks.clear();
                    self.state = ScreenState::Idle;
                    self.set_flash("Discarded chunks.");
                    true
                }
                _ => false,
            },
            ScreenState::ProcessingExtraction
            | ScreenState::GeneratingEmbeddings { .. }
            | ScreenState::Upserting { .. } => false,
            ScreenState::Done | ScreenState::Error(_) => {
                self.state = ScreenState::Idle;
                self.chunks.clear();
                true
            }
        }
    }

    fn handle_idle_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('t' | 'T') => {
                self.state = ScreenState::InputText { buffer: String::new(), cursor: 0 };
                true
            }
            KeyCode::Char('f' | 'F') => {
                self.state = ScreenState::InputFilePath { buffer: String::new(), cursor: 0 };
                true
            }
            KeyCode::Char('u' | 'U') => {
                self.state = ScreenState::InputUrl { buffer: String::new(), cursor: 0 };
                true
            }
            KeyCode::Char('c' | 'C') => {
                self.chunk_mode = match self.chunk_mode {
                    ChunkMode::StructureAware => ChunkMode::SlidingWindow,
                    ChunkMode::SlidingWindow => ChunkMode::StructureAware,
                };
                true
            }
            KeyCode::Char('+' | '=') => {
                self.chunk_size = (self.chunk_size + 128).min(4096);
                true
            }
            KeyCode::Char('-' | '_') => {
                self.chunk_size = self.chunk_size.saturating_sub(128).max(64);
                true
            }
            KeyCode::Delete if !self.target_collection.is_empty() => {
                self.target_collection.clear();
                true
            }
            KeyCode::Esc => false,
            _ => false,
        }
    }

    fn handle_input_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Enter => {
                let buf = self.buffer().to_string();
                if buf.trim().is_empty() {
                    self.set_flash("Input cannot be empty.");
                    return true;
                }
                let input = match &self.state {
                    ScreenState::InputText { .. } => Input {
                        source: Source::Text(buf.clone()),
                        content_type: "text/plain".to_string(),
                        data: buf.into_bytes(),
                    },
                    ScreenState::InputFilePath { .. } => {
                        let path = std::path::PathBuf::from(&buf);
                        let data = match std::fs::read(&path) {
                            Ok(d) => d,
                            Err(e) => { self.set_flash(format!("Cannot read file: {e}")); return true; }
                        };
                        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
                        let ct: String = match ext.as_str() {
                            "md" | "markdown" => "text/markdown",
                            "pdf" => "application/pdf",
                            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                            "html" | "htm" => "text/html",
                            "png" | "jpg" | "jpeg" | "gif" => "image/*",
                            "mp3" | "wav" | "flac" | "ogg" => "audio/*",
                            "mp4" | "avi" | "mkv" | "mov" => "video/*",
                            _ => "text/plain",
                        }.to_string();
                        Input { source: Source::File(path), content_type: ct.to_string(), data }
                    }
                    ScreenState::InputUrl { .. } => Input {
                        source: Source::Url(buf),
                        content_type: "text/html".to_string(),
                        data: Vec::new(),
                    },
                    _ => return false,
                };
                self.pending_input = Some(input);
                self.state = ScreenState::ProcessingExtraction;
                true
            }
            KeyCode::Char(c) => { self.insert_char(c); true }
            KeyCode::Backspace => { self.backspace(); true }
            KeyCode::Delete => { self.delete(); true }
            KeyCode::Left => { self.cursor_left(); true }
            KeyCode::Right => { self.cursor_right(); true }
            KeyCode::Home => { if let ScreenState::InputText { cursor, .. } | ScreenState::InputFilePath { cursor, .. } | ScreenState::InputUrl { cursor, .. } = &mut self.state { *cursor = 0; } true }
            KeyCode::End => { if let ScreenState::InputText { buffer, cursor } | ScreenState::InputFilePath { buffer, cursor } | ScreenState::InputUrl { buffer, cursor } = &mut self.state { *cursor = buffer.len(); } true }
            KeyCode::Esc => { self.state = ScreenState::Idle; true }
            _ => false,
        }
    }

    pub fn tick(
        &mut self,
        client: &QdrantClient,
        embedding_client: &EmbeddingClient,
        handle: &Handle,
    ) {
        match &self.state {
            ScreenState::ProcessingExtraction => {
                let input = match self.pending_input.take() {
                    Some(i) => i,
                    None => return,
                };
                let config = ChunkConfig {
                    chunk_size: self.chunk_size,
                    overlap: self.chunk_overlap,
                    mode: self.chunk_mode.clone(),
                };
                let result = handle.block_on(async { process(input, config).await });
                match result {
                    Ok(chunks) => {
                        self.chunks = chunks;
                        if self.chunks.is_empty() {
                            self.set_flash("Extraction produced no chunks.");
                            self.state = ScreenState::Idle;
                        } else {
                            self.state = ScreenState::Reviewing;
                            self.set_flash(format!(
                                "Extracted {} chunk(s). Press [y] to upsert or [n] to discard.",
                                self.chunks.len()
                            ));
                        }
                    }
                    Err(e) => self.state = ScreenState::Error(format!("Extraction failed: {e:#}")),
                }
            }
            ScreenState::GeneratingEmbeddings { index, total } => {
                let i = *index;
                if i >= self.chunks.len() {
                    self.state = ScreenState::Upserting { index: 0, total: self.chunks.len() };
                    self.pending_vector.clear();
                    return;
                }
                let text = self.chunks[i].text.clone();
                match handle.block_on(embedding_client.generate_embedding(&text)) {
                    Ok(vector) => {
                        self.pending_vector = vector;
                        self.state = ScreenState::Upserting { index: i, total: *total };
                    }
                    Err(e) => self.state = ScreenState::Error(format!(
                        "Embedding failed for chunk {}/{}: {e:#}", i + 1, total
                    )),
                }
            }
            ScreenState::Upserting { index, total } => {
                let i = *index;
                let vector = std::mem::take(&mut self.pending_vector);
                if vector.is_empty() && i < self.chunks.len() {
                    self.state = ScreenState::GeneratingEmbeddings { index: i, total: *total };
                    return;
                }
                if i >= self.chunks.len() || vector.is_empty() {
                    let msg = format!("Ingested {} point(s) into collection '{}'.", total, self.target_collection);
                    self.set_flash(msg);
                    self.state = ScreenState::Done;
                    self.chunks.clear();
                    return;
                }
                let chunk = &self.chunks[i];
                let mut payload = serde_json::Map::new();
                payload.insert("text".to_string(), serde_json::Value::String(chunk.text.clone()));
                payload.insert("source".to_string(), serde_json::Value::String(
                    match &chunk.metadata.source {
                        Source::File(p) => p.to_string_lossy().to_string(),
                        Source::Url(u) => u.clone(),
                        Source::Text(_) => "<text input>".to_string(),
                    }));
                payload.insert("source_display".to_string(), serde_json::Value::String(chunk.metadata.source_display.clone()));
                payload.insert("extractor".to_string(), serde_json::Value::String(chunk.metadata.extractor.clone()));
                payload.insert("chunk_index".to_string(), serde_json::Value::Number(serde_json::Number::from(chunk.metadata.chunk_index as u64)));
                payload.insert("total_chunks".to_string(), serde_json::Value::Number(serde_json::Number::from(chunk.metadata.total_chunks as u64)));

                let id = serde_json::Value::String(uuid::Uuid::new_v4().to_string());
                let point = UpsertPoint { id, vector, payload: Some(payload) };
                let collection = if self.target_collection.is_empty() { "default" } else { &self.target_collection };

                match handle.block_on(client.upsert_points(collection, &[point])) {
                    Ok(()) => {
                        let next = i + 1;
                        if next >= self.chunks.len() {
                            let msg = format!("Ingested {} point(s) into collection '{}'.", total, collection);
                            self.set_flash(msg);
                            self.state = ScreenState::Done;
                            self.chunks.clear();
                        } else {
                            self.state = ScreenState::GeneratingEmbeddings { index: next, total: *total };
                        }
                    }
                    Err(e) => self.state = ScreenState::Error(format!("Upsert failed for chunk {}/{}: {e:#}", i + 1, total)),
                }
            }
            _ => {}
        }
    }

    pub fn render(&self, frame: &mut ratatui::Frame, area: Rect) {
        let layout = Layout::vertical([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);
        self.render_top(frame, layout[0]);
        self.render_status(frame, layout[1]);
        self.render_main(frame, layout[2]);
    }

    fn render_top(&self, frame: &mut ratatui::Frame, area: Rect) {
        let input_mode_label = match &self.state {
            ScreenState::InputText { .. } => "[Text mode]",
            ScreenState::InputFilePath { .. } => "[File mode]",
            ScreenState::InputUrl { .. } => "[URL mode]",
            _ => "[Idle]",
        };
        let mode_hint = match &self.state {
            ScreenState::Idle =>
                " [t]ext [f]ile [u]rl | [c]ycle mode | [+/-] chunk size | [Del] clear collection | Esc back",
            ScreenState::Reviewing => " [y] upsert | [n] discard",
            ScreenState::Done | ScreenState::Error(_) => " Press any key to continue",
            _ => "",
        };
        let chunk_mode_label = match self.chunk_mode {
            ChunkMode::StructureAware => "StructureAware",
            ChunkMode::SlidingWindow => "SlidingWindow",
        };
        let collection_str = if self.target_collection.is_empty() {
            "<no target>".to_string()
        } else {
            self.target_collection.clone()
        };

        let lines = vec![
            Line::from(vec![
                Span::styled(" Mode: ", Style::default().fg(Color::Cyan)),
                Span::styled(input_mode_label, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("Collection: ", Style::default().fg(Color::Cyan)),
                Span::styled(collection_str, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled(" Chunk: ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("{}  size:{}  overlap:{}", chunk_mode_label, self.chunk_size, self.chunk_overlap),
                    Style::default().fg(Color::Green),
                ),
            ]),
            Line::from(Span::styled(mode_hint, Style::default().fg(Color::DarkGray))),
        ];
        let paragraph = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                .title(" Ingest ").title_alignment(Alignment::Left));
        frame.render_widget(paragraph, area);
    }

    fn render_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        let msg = match &self.state {
            ScreenState::ProcessingExtraction => " Processing extraction...".to_string(),
            ScreenState::GeneratingEmbeddings { index, total } =>
                format!(" Generating embedding ({}/{})...", index + 1, total),
            ScreenState::Upserting { index, total } =>
                format!(" Upserting point ({}/{})...", index + 1, total),
            ScreenState::Done => " Done.".to_string(),
            ScreenState::Error(e) => format!(" Error: {e}"),
            _ => {
                if let Some((msg, start)) = &self.flash {
                    if start.elapsed() < FLASH_DURATION { format!(" {}", msg) } else { String::new() }
                } else { String::new() }
            }
        };
        let style = match &self.state {
            ScreenState::Error(_) => Style::default().fg(Color::White).bg(Color::Red),
            ScreenState::Done => Style::default().fg(Color::Green).bg(Color::Black),
            ScreenState::ProcessingExtraction | ScreenState::GeneratingEmbeddings { .. } | ScreenState::Upserting { .. }
                => Style::default().fg(Color::Cyan),
            _ => Style::default().fg(Color::DarkGray),
        };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(msg, style))), area);
    }

    fn render_main(&self, frame: &mut ratatui::Frame, area: Rect) {
        match &self.state {
            ScreenState::Idle | ScreenState::InputText { .. }
            | ScreenState::InputFilePath { .. } | ScreenState::InputUrl { .. } => {
                self.render_input_preview(frame, area);
            }
            ScreenState::Reviewing => self.render_chunk_list(frame, area),
            ScreenState::ProcessingExtraction | ScreenState::GeneratingEmbeddings { .. } | ScreenState::Upserting { .. } => {
                let text = Paragraph::new(Line::from(Span::styled(" Working...", Style::default().fg(Color::Cyan))))
                    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                        .title(" Progress ").title_alignment(Alignment::Left));
                frame.render_widget(text, area);
            }
            ScreenState::Done => {
                let paragraph = Paragraph::new(vec![Line::from(Span::styled(
                    " Ingestion complete! Press any key to return.", Style::default().fg(Color::Green),
                ))]).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                    .title(" Result ").title_alignment(Alignment::Left));
                frame.render_widget(paragraph, area);
            }
            ScreenState::Error(e) => {
                let paragraph = Paragraph::new(vec![Line::from(Span::styled(
                    format!(" {e}"), Style::default().fg(Color::Red),
                ))]).block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                    .title(" Error ").title_alignment(Alignment::Left));
                frame.render_widget(paragraph, area);
            }
        }
    }

    fn render_input_preview(&self, frame: &mut ratatui::Frame, area: Rect) {
        let (input_title, preview, cursor) = match &self.state {
            ScreenState::InputText { buffer, cursor } => {
                let txt = if buffer.is_empty() { " <type or paste text here>".to_string() } else { buffer[..buffer.len().min(500)].to_string() };
                (" Text Input ", txt, *cursor)
            }
            ScreenState::InputFilePath { buffer, cursor } => {
                if buffer.is_empty() { (" File Path ", " <enter file path>".to_string(), *cursor) } else { (" File Path ", format!(" {}", buffer), *cursor) }
            }
            ScreenState::InputUrl { buffer, cursor } => {
                if buffer.is_empty() { (" URL ", " <enter URL>".to_string(), *cursor) } else { (" URL ", format!(" {}", buffer), *cursor) }
            }
            _ => (" Ready ", " Press [t]ext [f]ile [u]rl to start".to_string(), 0),
        };

        let cursor_visible = matches!(
            self.state,
            ScreenState::InputText { .. } | ScreenState::InputFilePath { .. } | ScreenState::InputUrl { .. }
        );

        let paragraph = Paragraph::new(Line::from(Span::styled(&preview, Style::default().fg(Color::White))))
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                .title(input_title).title_alignment(Alignment::Left))
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);

        if cursor_visible {
            let cursor_x = area.x + 1 + cursor.min(preview.len()) as u16;
            let cursor_y = area.y + 1;
            frame.set_cursor_position(ratatui::layout::Position::new(cursor_x, cursor_y));
        }
    }

    fn render_chunk_list(&self, frame: &mut ratatui::Frame, area: Rect) {
        if self.chunks.is_empty() {
            let paragraph = Paragraph::new(Line::from(Span::styled(" No chunks to review.", Style::default().fg(Color::DarkGray))))
                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                    .title(" Chunks ").title_alignment(Alignment::Left));
            frame.render_widget(paragraph, area);
            return;
        }
        let items: Vec<ListItem> = self.chunks.iter().map(|chunk| {
            let preview = if chunk.text.len() > 200 { format!("{}...", &chunk.text[..200]) } else { chunk.text.clone() };
            ListItem::new(vec![
                Line::from(Span::styled(
                    format!("  [{}:{}] {}", chunk.metadata.extractor, chunk.metadata.chunk_index, chunk.metadata.source_display),
                    Style::default().fg(Color::DarkGray),
                )),
                Line::from(Span::styled(format!("  {}", preview), Style::default().fg(Color::White))),
            ])
        }).collect();
        let list = List::new(items).direction(ListDirection::TopToBottom)
            .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded)
                .title(format!(" Chunks ({}) ", self.chunks.len())).title_alignment(Alignment::Left));
        frame.render_widget(list, area);
    }
}

impl Default for IngestionScreen {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::extractor::Source;
    use crate::ingestion::ChunkMetadata;

    fn make_screen() -> IngestionScreen {
        let mut s = IngestionScreen::new();
        s.target_collection = "general".to_string();
        s
    }

    #[test]
    fn idle_t_switches_to_input_text() {
        let mut s = make_screen();
        assert!(s.handle_idle_key(KeyCode::Char('t')));
        assert!(matches!(s.state, ScreenState::InputText { .. }));
    }

    #[test]
    fn idle_f_switches_to_input_file() {
        let mut s = make_screen();
        assert!(s.handle_idle_key(KeyCode::Char('f')));
        assert!(matches!(s.state, ScreenState::InputFilePath { .. }));
    }

    #[test]
    fn idle_u_switches_to_input_url() {
        let mut s = make_screen();
        assert!(s.handle_idle_key(KeyCode::Char('u')));
        assert!(matches!(s.state, ScreenState::InputUrl { .. }));
    }

    #[test]
    fn idle_c_cycles_chunk_mode() {
        let mut s = make_screen();
        assert_eq!(s.chunk_mode, ChunkMode::StructureAware);
        s.handle_idle_key(KeyCode::Char('c'));
        assert_eq!(s.chunk_mode, ChunkMode::SlidingWindow);
        s.handle_idle_key(KeyCode::Char('c'));
        assert_eq!(s.chunk_mode, ChunkMode::StructureAware);
    }

    #[test]
    fn idle_plus_minus_adjusts_chunk_size() {
        let mut s = make_screen();
        let original = s.chunk_size;
        s.handle_idle_key(KeyCode::Char('+'));
        assert_eq!(s.chunk_size, original + 128);
        s.handle_idle_key(KeyCode::Char('-'));
        assert_eq!(s.chunk_size, original);
    }

    #[test]
    fn idle_delete_clears_collection() {
        let mut s = make_screen();
        assert_eq!(s.target_collection, "general");
        assert!(s.handle_idle_key(KeyCode::Delete));
        assert!(s.target_collection.is_empty());
    }

    #[test]
    fn input_text_insert_and_backspace() {
        let mut s = make_screen();
        s.state = ScreenState::InputText { buffer: String::new(), cursor: 0 };
        assert!(s.handle_input_key(KeyCode::Char('h')));
        assert!(s.handle_input_key(KeyCode::Char('i')));
        assert_eq!(s.buffer(), "hi");
        s.handle_input_key(KeyCode::Backspace);
        assert_eq!(s.buffer(), "h");
    }

    #[test]
    fn input_text_empty_enter_shows_flash() {
        let mut s = make_screen();
        s.state = ScreenState::InputText { buffer: String::new(), cursor: 0 };
        assert!(s.handle_input_key(KeyCode::Enter));
        assert!(matches!(s.state, ScreenState::InputText { .. }));
        assert!(s.flash.is_some());
    }

    #[test]
    fn input_text_esc_returns_to_idle() {
        let mut s = make_screen();
        s.state = ScreenState::InputText { buffer: "hello".to_string(), cursor: 5 };
        assert!(s.handle_input_key(KeyCode::Esc));
        assert_eq!(s.state, ScreenState::Idle);
    }

    #[test]
    fn reviewing_y_starts_embedding() {
        let mut s = make_screen();
        s.chunks = vec![
            Chunk { text: "chunk one".to_string(), metadata: ChunkMetadata {
                source: Source::Text("test".to_string()), source_display: "<text input>".to_string(),
                extractor: "plaintext".to_string(), page: None, timestamp_range: None,
                chunk_index: 0, total_chunks: 2,
            }},
            Chunk { text: "chunk two".to_string(), metadata: ChunkMetadata {
                source: Source::Text("test".to_string()), source_display: "<text input>".to_string(),
                extractor: "plaintext".to_string(), page: None, timestamp_range: None,
                chunk_index: 1, total_chunks: 2,
            }},
        ];
        s.state = ScreenState::Reviewing;
        assert!(s.handle_key(KeyCode::Char('y')));
        assert!(matches!(s.state, ScreenState::GeneratingEmbeddings { index: 0, total: 2 }));
    }

    #[test]
    fn reviewing_n_discards() {
        let mut s = make_screen();
        s.chunks = vec![Chunk { text: "test".to_string(), metadata: ChunkMetadata {
            source: Source::Text("test".to_string()), source_display: "<text input>".to_string(),
            extractor: "plaintext".to_string(), page: None, timestamp_range: None,
            chunk_index: 0, total_chunks: 1,
        }}];
        s.state = ScreenState::Reviewing;
        assert!(s.handle_key(KeyCode::Char('n')));
        assert_eq!(s.state, ScreenState::Idle);
        assert!(s.chunks.is_empty());
    }

    #[test]
    fn is_text_editing_true_for_input_states() {
        let mut s = make_screen();
        assert!(!s.is_text_editing());
        s.state = ScreenState::InputText { buffer: String::new(), cursor: 0 };
        assert!(s.is_text_editing());
        s.state = ScreenState::InputFilePath { buffer: String::new(), cursor: 0 };
        assert!(s.is_text_editing());
        s.state = ScreenState::InputUrl { buffer: String::new(), cursor: 0 };
        assert!(s.is_text_editing());
        s.state = ScreenState::Reviewing;
        assert!(!s.is_text_editing());
        s.state = ScreenState::ProcessingExtraction;
        assert!(!s.is_text_editing());
    }

    #[test]
    fn done_state_any_key_returns_to_idle() {
        let mut s = make_screen();
        s.state = ScreenState::Done;
        assert!(s.handle_key(KeyCode::Char(' ')));
        assert_eq!(s.state, ScreenState::Idle);
    }

    #[test]
    fn error_state_any_key_returns_to_idle() {
        let mut s = make_screen();
        s.state = ScreenState::Error("something broke".to_string());
        assert!(s.handle_key(KeyCode::Char(' ')));
        assert_eq!(s.state, ScreenState::Idle);
    }
}
