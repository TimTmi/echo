# Progress: Echo

## Completed
- **Project Kickoff**: Project scoped, tech stack chosen (Rust + ratatui + Qdrant + BGE-M3/llama.cpp)
- **Infrastructure**: Qdrant and BGE-M3 embedding service running via Docker/llama.cpp at D:\embedding\
- **Git Setup**: git init, branch renamed to main, remote configured, first commits pushed
- **Context Initialized**: .context/ folder created with project brief, design, data models, API spec, coding rules, roadmap, decisions
- **Rust Project Scaffold**: Cargo.toml with core deps, module skeleton (config/, qdrant/, embedding/, tui/), config module with TOML load/save and defaults, .gitignore
- **Basic TUI Loop**: ratatui shell with alternate screen, 250 ms tick, quit via q/Esc/Ctrl+C, title bar, status bar
- **Embedding Client Module**: `EmbeddingClient::generate_embedding(&str) -> Vec<f32>` via llama.cpp `/v1/embeddings`. Mockito unit tests.
- **Qdrant Client Module**: `list_collections()`, `get_collection_info(name)`, `search_points(coll, vec, limit)`, `scroll_points(coll, limit, offset)`. 11 mockito tests on the Qdrant side.
- **TUI Screen Architecture**: `ActiveScreen { Home, Collections, Search, PointViewer }` with on_enter / tick / render / key dispatch.
- **Collection Browser Screen** (`src/tui/collection_browser.rs`): list + detail split, Up/Down nav, Enter/R refresh detail. Cleanup 2026-07-21 removed redundant thread-scope, by-value client, unused params, manual selection prefix.
- **Search Screen** (`src/tui/search_screen.rs`): text input with cursor, Enter triggers embedding gen + Qdrant search, results with score/ID/payload preview.
- **Point Viewer (2026-07-23)**: cursor-paginated point browser.
  - `scroll_points` -> `POST /collections/{name}/points/scroll`. Public types `PointRecord { id, payload }` and `ScrollPage { points, next_offset }`.
  - New screen `src/tui/point_viewer.rs`. List (ID + first string payload preview) | Payload detail (pretty JSON). Keys: Up/Down nav, `n` next page, `p` prev page, `r` refresh, Esc back.
  - Page size = 20. Pagination via `next_page_offset` cursor with `prev_offsets` history stack.
  - Drill-in `[P]` from Collections screen; Esc returns to Collections.
  - Added `selected_index()` + `collection_names()` accessors on `CollectionBrowserScreen`.

## In Progress
- No active task

## Next Steps
- (Phase 3) Create / delete collections from the TUI
- (Phase 3) Upsert points (paste or load from file)
- (Phase 3) Delete points by filter or ID
- (Phase 3) Configuration screen (edit Qdrant URL, embedding URL from TUI)

See `roadmap.md` for the full build order.