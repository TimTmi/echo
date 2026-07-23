//! TUI layer module.
//!
//! ratatui-based terminal user interface.
//! Provides the main application loop, event handling, and screen rendering.

pub mod collection_browser;
pub mod point_viewer;
pub mod search_screen;

use crate::embedding::EmbeddingClient;
use crate::qdrant::QdrantClient;
use anyhow::Context;
use collection_browser::CollectionBrowserScreen;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use point_viewer::PointViewerScreen;
use ratatui::Terminal;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use search_screen::SearchScreen;
use std::io::stdout;
use std::time::{Duration, Instant};
use tokio::runtime::Handle;

/// Represents the active screen in the application.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum ActiveScreen {
    #[default]
    Home,
    Collections,
    Search,
    PointViewer,
}

/// Application state for the TUI.
pub struct App {
    /// Whether the application should exit the main loop.
    pub should_quit: bool,
    /// Timestamp when the app started.
    pub started_at: Instant,
    /// Any error message to display (cleared after next render).
    pub error_message: Option<String>,
    /// Current screen being displayed.
    active_screen: ActiveScreen,
    /// Qdrant client for API calls.
    qdrant_client: QdrantClient,
    /// Embedding client for generating vectors.
    embedding_client: EmbeddingClient,
    /// Collection browser screen state.
    collection_browser: CollectionBrowserScreen,
    /// Search screen state.
    search_screen: SearchScreen,
    /// Point viewer screen state.
    point_viewer: PointViewerScreen,
    /// Handle to the Tokio runtime for async operations.
    runtime_handle: Option<Handle>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            should_quit: false,
            started_at: Instant::now(),
            error_message: None,
            active_screen: ActiveScreen::default(),
            qdrant_client: QdrantClient::new("http://localhost:6333"),
            embedding_client: EmbeddingClient::new("http://localhost:8080/v1/embeddings"),
            collection_browser: CollectionBrowserScreen::new(),
            search_screen: SearchScreen::new(),
            point_viewer: PointViewerScreen::new(),
            runtime_handle: None,
        }
    }
}

impl App {
    /// Create a new App with default state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the Qdrant client (used for configuring with a different URL).
    pub fn with_qdrant_client(mut self, client: QdrantClient) -> Self {
        self.qdrant_client = client;
        self
    }

    /// Run the main TUI event loop. This function takes full control of the terminal,
    /// enters alternate screen, and blocks until the user quits.
    pub fn run(&mut self) -> anyhow::Result<()> {
        let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
        self.runtime_handle = Some(rt.handle().clone());

        // Enter raw mode and alternate screen
        enable_raw_mode().context("failed to enable raw mode")?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

        // Create the terminal
        let terminal = Terminal::new(ratatui::prelude::CrosstermBackend::new(stdout))
            .context("failed to create terminal")?;

        // Run the event loop — uses rt.handle() via runtime_handle
        let result = self.event_loop(terminal);

        // Restore terminal — even if event loop errored
        let _ = Self::restore_terminal();

        result
    }

    /// Restore the terminal to normal mode.
    fn restore_terminal() -> anyhow::Result<()> {
        disable_raw_mode().context("failed to disable raw mode")?;
        execute!(stdout(), LeaveAlternateScreen).context("failed to leave alternate screen")?;
        Ok(())
    }

    /// Main event loop — draws and handles events until `should_quit` is true.
    fn event_loop(
        &mut self,
        mut terminal: Terminal<ratatui::prelude::CrosstermBackend<std::io::Stdout>>,
    ) -> anyhow::Result<()> {
        let tick_rate = Duration::from_millis(250);

        // Trigger initial data load for active screen
        self.on_screen_enter();

        loop {
            // Draw the UI
            terminal.draw(|frame| self.render(frame.area(), frame))?;

            // Wait for an event with a timeout so we can redraw on tick
            let has_event = event::poll(tick_rate).context("failed to poll events")?;

            if has_event {
                let event = event::read().context("failed to read event")?;
                self.handle_event(event)?;
            }

            // Tick the active screen for async loading
            self.tick();

            if self.should_quit {
                break;
            }
        }

        Ok(())
    }

    /// Called when entering a screen to trigger initial data loads.
    fn on_screen_enter(&mut self) {
        match self.active_screen {
            ActiveScreen::Collections => {
                self.collection_browser.on_enter();
            }
            ActiveScreen::Search => {
                self.search_screen.on_enter();
            }
            ActiveScreen::PointViewer => {
                self.point_viewer.on_enter();
            }
            ActiveScreen::Home => {}
        }
    }

    /// Periodic tick to progress async operations.
    fn tick(&mut self) {
        match self.active_screen {
            ActiveScreen::Collections => {
                if let Some(handle) = &self.runtime_handle {
                    self.collection_browser.tick(&self.qdrant_client, handle);
                }
            }
            ActiveScreen::Search => {
                if let Some(handle) = &self.runtime_handle {
                    self.search_screen
                        .tick(&self.qdrant_client, &self.embedding_client, handle);
                }
            }
            ActiveScreen::PointViewer => {
                if let Some(handle) = &self.runtime_handle {
                    self.point_viewer.tick(&self.qdrant_client, handle);
                }
            }
            ActiveScreen::Home => {}
        }
    }

    /// Render the current frame.
    fn render(&mut self, area: Rect, frame: &mut ratatui::Frame) {
        let layout = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

        self.render_title(frame, layout[0]);
        match self.active_screen {
            ActiveScreen::Home => self.render_body(frame, layout[1]),
            ActiveScreen::Collections => self.collection_browser.render(frame, layout[1]),
            ActiveScreen::Search => self.search_screen.render(frame, layout[1]),
            ActiveScreen::PointViewer => self.point_viewer.render(frame, layout[1]),
        }
        self.render_status_bar(frame, layout[2]);
    }

    /// Render the title bar at the top of the screen.
    fn render_title(&self, frame: &mut ratatui::Frame, area: Rect) {
        let title = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" Echo ")
            .title_alignment(Alignment::Center);

        let text = Paragraph::new(Line::from(vec![
            Span::raw("Vector Terminal UI — "),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(Color::Cyan),
            ),
        ]))
        .block(title)
        .alignment(Alignment::Center);

        frame.render_widget(text, area);
    }

    /// Render the body area (main content).
    fn render_body(&self, frame: &mut ratatui::Frame, area: Rect) {
        let uptime = self.started_at.elapsed();
        let uptime_secs = uptime.as_secs();
        let uptime_str = format!("{}m {}s", uptime_secs / 60, uptime_secs % 60);

        let content = vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                "⚡ Echo is running",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(""),
            Line::from(vec![
                Span::raw("Uptime: "),
                Span::styled(uptime_str, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "Press 'c' to browse collections",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(Span::styled(
                "Press 'q' or Esc to quit",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(content)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Status ")
                    .title_alignment(Alignment::Left),
            )
            .alignment(Alignment::Center);

        frame.render_widget(paragraph, area);
    }

    /// Render the status bar at the bottom.
    fn render_status_bar(&self, frame: &mut ratatui::Frame, area: Rect) {
        let hints = match self.active_screen {
            ActiveScreen::Home => " [Q]uit | [C]ollections | [S]earch ",
            ActiveScreen::Collections => {
                " [Q]uit | [↑/↓] Navigate | [Enter/R] Refresh detail | [Esc] Back "
            }
            ActiveScreen::Search => " [Q]uit | Type query + Enter to search | [Esc] Back ",
            ActiveScreen::PointViewer => {
                " [Q]uit | [↑/↓] Navigate | [N]ext page | [P]rev | [R]efresh | [Esc] Back "
            }
        };

        let left = Span::styled(hints, Style::default().fg(Color::DarkGray).bg(Color::Reset));

        let error = self.error_message.as_ref().map(|err| {
            Span::styled(
                format!(" Error: {err} "),
                Style::default().fg(Color::White).bg(Color::Red),
            )
        });

        let spans: Vec<Span> = if let Some(ref err_span) = error {
            vec![left, err_span.clone()]
        } else {
            vec![left]
        };

        let paragraph = Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::TOP)
                .border_type(BorderType::Plain),
        );

        frame.render_widget(paragraph, area);
    }

    /// Handle a single terminal event. Returns `true` if the event was handled.
    fn handle_event(&mut self, event: Event) -> anyhow::Result<bool> {
        if let Event::Key(key) = event
            && key.kind == KeyEventKind::Press
            && self.handle_key_press(key.code, key.modifiers)
        {
            return Ok(true);
        }
        Ok(false)
    }

    /// Handle a key press event. Returns `true` if the key was consumed.
    fn handle_key_press(
        &mut self,
        code: KeyCode,
        modifiers: crossterm::event::KeyModifiers,
    ) -> bool {
        // Global quit keys work on every screen
        match code {
            KeyCode::Char('q') | KeyCode::Char('Q') => {
                self.should_quit = true;
                return true;
            }
            KeyCode::Char('c') if modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                self.should_quit = true;
                return true;
            }
            _ => {}
        }

        match self.active_screen {
            ActiveScreen::Home => self.handle_home_key(code),
            ActiveScreen::Collections => {
                let handled = self.collection_browser.handle_key(code);
                if handled {
                    return true;
                }
                // 'P' drills into the selected collection's points.
                if matches!(code, KeyCode::Char('p') | KeyCode::Char('P')) {
                    if let Some(idx) = self.collection_browser.selected_index() {
                        let names = self.collection_browser.collection_names();
                        if let Some(name) = names.get(idx) {
                            self.point_viewer.set_collection(name);
                            self.active_screen = ActiveScreen::PointViewer;
                            self.on_screen_enter();
                            return true;
                        }
                    }
                    return true;
                }
                // Esc on collections screen goes back to home
                if code == KeyCode::Esc {
                    self.active_screen = ActiveScreen::Home;
                    self.on_screen_enter();
                    return true;
                }
                false
            }
            ActiveScreen::Search => {
                let handled = self.search_screen.handle_key(code);
                if handled {
                    return true;
                }
                // Esc on search screen goes back to home
                if code == KeyCode::Esc {
                    self.active_screen = ActiveScreen::Home;
                    self.on_screen_enter();
                    return true;
                }
                false
            }
            ActiveScreen::PointViewer => {
                let handled = self.point_viewer.handle_key(code);
                if handled {
                    return true;
                }
                // Esc on point viewer returns to collections
                if code == KeyCode::Esc {
                    self.active_screen = ActiveScreen::Collections;
                    self.on_screen_enter();
                    return true;
                }
                false
            }
        }
    }

    /// Handle key presses on the home screen.
    fn handle_home_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.active_screen = ActiveScreen::Collections;
                self.on_screen_enter();
                true
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.active_screen = ActiveScreen::Search;
                self.on_screen_enter();
                true
            }
            KeyCode::Esc => {
                self.should_quit = true;
                true
            }
            _ => false,
        }
    }
}
