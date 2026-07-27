use anyhow::Context;
use echo::tui::App;

fn main() -> anyhow::Result<()> {
    // Initialize logging to a file (not stderr) so TUI alternate screen stays clean.
    // Log path: echo.log in CWD (same directory as echo.toml).
    // RUST_LOG env var overrides the default filter ("echo=info").
    let log_path = std::path::PathBuf::from("echo.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open log file: {}", log_path.display()))?;

    tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(log_file))
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "echo=info".into()),
        )
        .with_ansi(false) // disable colors in log file
        .init();

    // Load config from disk and hand it to the App so Qdrant / embedding
    // clients point at the saved URLs.
    let config = echo::config::Config::load()?;

    // Run the TUI
    let mut app = App::with_config(&config);
    app.run()
}
