use echo::tui::App;

fn main() -> anyhow::Result<()> {
    // Initialize logging (trace level by default, overridable via RUST_LOG)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "echo=info".into()),
        )
        .init();

    // Load config from disk and hand it to the App so Qdrant / embedding
    // clients point at the saved URLs.
    let config = echo::config::Config::load()?;

    // Run the TUI
    let mut app = App::with_config(&config);
    app.run()
}
