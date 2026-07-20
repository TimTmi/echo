# Roadmap: Echo

## Phase 1: Foundation
- [ ] Scaffold Rust project with ratatui, tokio, reqwest, qdrant-client crate
- [ ] Implement config module (TOML file read/write)
- [ ] Basic TUI shell (ratatui app loop, empty screen, quit key)

## Phase 2: Core Features
- [ ] Qdrant client module — list collections, view collection info
- [ ] TUI screen: collection browser (list + detail panel)
- [ ] Embedding client module — generate embeddings via llama.cpp
- [ ] Search screen: input query text, generate embedding, search Qdrant, display results
- [ ] Point viewer: scroll through points in a collection with payload display

## Phase 3: Enhanced Interaction
- [ ] Create / delete collections from the TUI
- [ ] Upsert points (paste or load from file)
- [ ] Delete points by filter or ID
- [ ] Configuration screen (edit Qdrant URL, embedding URL from TUI)

## Phase 4: Polish & Extensibility
- [ ] Support multiple embedding providers (Ollama, OpenAI API-compatible)
- [ ] History / recent searches
- [ ] Payload filtering UI
- [ ] Performance optimizations for large collections
