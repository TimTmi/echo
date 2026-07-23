# Roadmap: Echo

## Phase 1: Foundation
- [x] Scaffold Rust project with ratatui, tokio, reqwest
- [x] Implement config module (TOML file read/write)
- [x] Basic TUI shell (ratatui app loop, empty screen, quit key)

## Phase 2: Core Features
- [x] Qdrant client module — list collections, view collection info
- [x] TUI screen: collection browser (list + detail panel)
- [x] Embedding client module — generate embeddings via llama.cpp
- [x] Search screen: input query text, generate embedding, search Qdrant, display results
- [x] Point viewer: scroll through points in a collection with payload display

## Phase 3: Enhanced Interaction
- [x] Create / delete collections from the TUI
- [ ] Upsert points (paste or load from file)
- [ ] Delete points by filter or ID
- [x] Configuration screen (edit Qdrant URL, embedding URL from TUI)

## Phase 4: Polish & Extensibility
- [ ] Support multiple embedding providers (Ollama, OpenAI API-compatible)
- [ ] History / recent searches
- [ ] Payload filtering UI
- [ ] Performance optimizations for large collections
- [x] **Cross-collection search** — auto-fallback inside `SearchScreen` when no collection is selected: fan out `search_points` to compatible collections (1024-Cosine), merge by score, label each result with its source. Option A from `decisions.md`.
