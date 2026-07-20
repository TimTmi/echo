# Decisions: Echo

## TUI Framework

### 2026-07-20: Use ratatui for terminal UI
- **Decision**: Build the TUI with ratatui (Rust)
- **Reason**: ratatui is the most mature and actively maintained TUI framework in the Rust ecosystem. It provides full control over terminal rendering and has strong community support.
- **Consequences**: More boilerplate compared to higher-level frameworks, but better flexibility and no magic. The team (or AI agents) must understand ratatui's immediate-mode rendering model and widget system.

## External Services

### 2026-07-20: Use HTTP REST for both Qdrant and llama.cpp communication
- **Decision**: Communicate with Qdrant via its REST API and with llama.cpp via HTTP
- **Reason**: REST is simple, well-documented, and easy to debug. Qdrant's REST API is feature-complete. Both services are local, so performance overhead is negligible.
- **Consequences**: No dependency on Qdrant's gRPC client crate (optional later). Easy to test with mock HTTP servers.

## Async Runtime

### 2026-07-20: Use tokio for async I/O
- **Decision**: Use tokio as the async runtime
- **Reason**: reqwest (HTTP client) and qdrant-client both integrate well with tokio. ratatui can run its event loop on a tokio task. tokio is the de-facto standard async runtime in Rust.
- **Consequences**: Increases binary size slightly. Must manage cancellation and task lifetimes carefully to avoid TUI freezes.

## Configuration

### 2026-07-20: Use TOML for local config file
- **Decision**: Store connection settings in a TOML file
- **Reason**: TOML is the standard Rust ecosystem config format (serde support via `toml` crate), human-readable, and easy to edit by hand.
- **Consequences**: Users can pre-configure before launching the app. Config is simple enough that no schema migration complexity is expected.
