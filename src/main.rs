use echo::tui::App;

fn main() -> anyhow::Result<()> {
    // Initialize logging (trace level by default, overridable via RUST_LOG)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "echo=info".into()),
        )
        .init();

    // Load config (not yet used in the basic loop, will be wired in later tasks)
    let _config = echo::config::Config::load()?;

    // Run the TUI
    let mut app = App::new();
    app.run()
}
