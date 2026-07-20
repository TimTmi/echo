# Project Brief: Echo

## Goal
Echo is a terminal UI (TUI) application for interacting with embeddings and vector databases. The main use case is connecting to Qdrant (vector DB) and embedding models like BGE-M3 (served via llama.cpp) for semantic search, embedding management, and vector collection operations — all from the terminal.

## Stack
- **Language:** Rust
- **TUI Framework:** ratatui
- **Vector Database:** Qdrant (via REST/gRPC client)
- **Embedding Model:** BGE-M3 (served via llama.cpp with `--embeddings` flag)
- **Infrastructure**: Docker Compose (Qdrant + Caddy reverse proxy), llama.cpp standalone
- **Tooling:** cargo test, clippy, Rustfmt

## Core Features
- Connect and browse Qdrant collections from the TUI
- Run semantic search queries against collections using BGE-M3 embeddings
- View collection details: vector count, payload schema, configuration
- Manage embedding generation via llama.cpp HTTP API
- Local config file for storing connection settings (Qdrant URL, embedding endpoint)
