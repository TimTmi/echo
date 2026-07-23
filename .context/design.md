# Echo Technical Design

## Architecture
Single Rust binary with a ratatui-based terminal UI. The app is a standalone client that communicates with external services (Qdrant and llama.cpp) over HTTP/REST. No server-side component — Echo is purely a client-side TUI tool.

## Components

| Component | Responsibility |
|---|---|
| **TUI Layer** (ratatui) | Render terminal UI, handle keyboard input, manage screen state (collection list, search results, settings) |
| **Qdrant Client** | REST API client for Qdrant — create/delete collections, upsert/search points, view collection info |
| **Embedding Client** | HTTP client for llama.cpp embedding server (BGE-M3) — generate embeddings from text |
| **Config Module** | Load/save local config (connection URLs, defaults) from a TOML/YAML file |
| **App State / Controller** | Orchestrate between UI events and service clients, manage application state machine |

## Communication
- **Echo ↔ Qdrant:** HTTP REST API (or optionally gRPC) on localhost:6333 / qdrant.localhost
- **Echo ↔ llama.cpp (BGE-M3):** HTTP POST to `/v1/embeddings` on localhost:8080 / embeddings.localhost
- **Echo ↔ Config File:** Local file read/write (TOML), no network involved

### Hostname DNS for `.localhost` vhosts
`*.localhost` hostnames (e.g. `qdrant.localhost`, `embeddings.localhost`) resolve to `127.0.0.1` only inside browsers (RFC 6761 magic). The Rust `reqwest` client uses OS DNS, which does **not** apply that rule — connections would fail DNS lookup.

**Required**: hosts file entry mapping `qdrant.localhost` and `embeddings.localhost` to `127.0.0.1`. This preserves the Caddy vhost `Host` header so per-vhost routing still works (URL-level rewrites drop the header and would break vhost selection).

Hosts file: `C:\Windows\System32\drivers\etc\hosts` (edit requires admin).

## Key Reliability Patterns
- Graceful error handling for service disconnections (Qdrant / llama.cpp unreachable)
- Non-blocking HTTP calls (async Rust) to keep TUI responsive
- Config validation with sensible defaults on first launch
- Connection retry with exponential backoff where appropriate

## Important Tradeoffs
- Async Rust (tokio) adds complexity but is necessary for a responsive TUI that makes network calls
- ratatui gives full control over rendering but requires more boilerplate than higher-level TUI frameworks

## Cross-References
- `data_models.md`: Qdrant collection schema, embedding config
- `api_spec.md`: Qdrant and llama.cpp endpoint contracts
- `decisions.md`: dated architecture decisions and rationale
