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
- **TUI Collection Browser**: Collection browser screen (`src/tui/collection_browser.rs`) with list + detail panels, Up/Down navigation, Enter/R refresh, Esc back, async loading on tick. Screen-based architecture (`ActiveScreen` enum) for future screens. QdrantClient wired into App state.
- **Collection Browser Cleanup (2026-07-21)**: Fixed tool-detour artifacts from commits ee9a75/2673a70:
  - Removed redundant `std::thread::scope` wrapping `tokio::runtime::Handle::block_on`
  - Changed `tick()` to take `&QdrantClient` instead of by-value (was cloning every 250ms tick)
  - Removed unused `client` parameters from `on_enter()`, `refresh_collections()`, `load_detail()`
  - Fixed `ListState` being cloned every frame (now passed `&mut self.list_state` directly)
  - Changed `render()` to `&mut self`
  - Removed manual "? " selection prefix (List widget `highlight_style` handles it)
  - Added `Default` impl and collapsed nested `if` per clippy

## Completed (continued)
- **Search Screen**: Full search screen at `src/tui/search_screen.rs`. Text input with cursor, Enter triggers embedding gen + Qdrant search, results list with score/ID/payload preview. Wiring in `mod.rs`: `ActiveScreen::Search`, `EmbeddingClient` in App state, tick/key/render routing. Unused import and deprecation warnings fixed.
- **Qdrant Search Endpoint**: `QdrantClient::search_points(collection, vector, limit)` → `POST /collections/{name}/points/search`. `SearchResult` struct with id, score, payload, vector fields.
- **Point Viewer (2026-07-23)**: Cursor-paginated point browser.
  - `QdrantClient::scroll_points(collection, limit, offset) -> ScrollPage` → `POST /collections/{name}/points/scroll`. Public types `PointRecord { id, payload }` and `ScrollPage { points, next_offset }`.
  - New screen `src/tui/point_viewer.rs`. List (ID + first string payload preview) | Payload detail (pretty JSON) split. Keys: Up/Down navigate, `n` next page, `p` prev page, `r` refresh, Esc back.
  - Page size = 20. Pagination via `next_page_offset` cursor with `prev_offsets` history stack for back navigation.
  - Wired into `tui/mod.rs`: `ActiveScreen::PointViewer` variant, dispatch in on_screen_enter/tick/render, drill-in key `[P]` on Collections screen, Esc returns to Collections.
  - Added `selected_index()` + `collection_names()` accessors to `CollectionBrowserScreen`.
  - 4 new qdrant unit tests (basic, with_offset, empty, http_error); total 14 passing.

## In Progress
- No active task

## Next Steps
1. ~~TUI screen: collection browser (list + detail panel)~~ ✅ Done
2. ~~Search screen: input query text, generate embedding, search Qdrant, display results~~ ✅ Done
3. ~~Point viewer: scroll through points in a collection with payload display~~ ✅ Done
4. (Phase 3) Create / delete collections from the TUI
5. (Phase 3) Upsert points (paste or load from file)
6. (Phase 3) Delete points by filter or ID
7. (Phase 3) Configuration screen (edit Qdrant URL, embedding URL from TUI)
