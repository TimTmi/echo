# Progress: Echo

## Completed
- **Project Kickoff**: Project scoped, tech stack chosen (Rust + ratatui + Qdrant + BGE-M3/llama.cpp)
- **Infrastructure**: Qdrant and BGE-M3 embedding service running via Docker/llama.cpp at D:\embedding\
- **Git Setup**: git init, branch renamed to main, remote configured, first commits pushed
- **Context Initialized**: .context/ folder created with project brief, design, data models, API spec, coding rules, roadmap, decisions
- **Rust Project Scaffold**: Cargo.toml with core deps (ratatui, tokio, reqwest, serde, qdrant-client), module skeleton (config/, qdrant/, embedding/, tui/), config module with TOML load/save and defaults, .gitignore
- **Basic TUI Loop**: Implemented ratatui shell with alternate screen, event polling with 250ms tick rate, quit via q, Esc, or Ctrl+C, title bar, status display with uptime, and status bar
- **Embedding Client Module**: EmbeddingClient with generate_embedding(&str) -> Vec<f32> via llama.cpp /v1/embeddings HTTP endpoint. Includes unit tests with mockito mock HTTP server. mockito added as dev-dependency.
- **Qdrant Client Module**: QdrantClient with `list_collections()` and `get_collection_info(name)` via Qdrant REST API. Uses reqwest, deserializes Qdrant's JSON response format (collections list + collection detail with vector config). 7 unit tests with mockito covering: empty list, multi-collection, HTTP errors, collection info with default/named vectors, 404, and null result.

## In Progress
- No active task

## Next Steps
1. TUI screen: collection browser (list + detail panel)
2. Search screen: input query text, generate embedding, search Qdrant, display results
3. Point viewer: scroll through points in a collection with payload display
