# Progress: Echo

## Completed
- **Project Kickoff**: Project scoped, tech stack chosen (Rust + ratatui + Qdrant + BGE-M3/llama.cpp)
- **Infrastructure**: Qdrant and BGE-M3 embedding service running via Docker/llama.cpp at D:\embeddings (renamed from D:\embedding on 2026-07-23). Caddy reverse proxy routes `qdrant.localhost` and `embeddings.localhost` vhosts. Browser auto-resolves `.localhost` per RFC 6761; Rust clients require hosts file entries or custom resolver.
- **`GET /collections` parse fix (2026-07-23)**: Qdrant returns `{"result":{"collections":[…]}}` but client deserializer previously expected `{"result":[…]}` (array directly). Added `CollectionsListResult` wrapper struct in `src/qdrant/mod.rs`; updated `list_collections()` extraction. Two list-collections tests updated to use nested `result.collections` fixture. Hosts file entries added for `qdrant.localhost` and `embeddings.localhost` → `127.0.0.1` so Caddy vhost Host header survives.
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
- **Configuration Screen (2026-07-23)**: new screen `src/tui/config_screen.rs` for editing the four config fields (qdrant_url, embedding_url, default_collection, embedding_model) with per-field edit buffer, dirty marker, save/discard semantics, and reload-from-disk on entry.
  - `ConfigKeyOutcome { Handled, Ignore, Back }` returned from `handle_key` so the App can wire Esc -> discard + return home.
  - Config gains `PartialEq` so dirty tracking can compare the working copy against the last-saved snapshot.
  - App gains `ActiveScreen::Config` variant + key wiring; Home key `g` opens config; status bar hints updated.
  - 12 unit tests covering navigation, edit/commit/cancel, dirty round-trip, default_collection None/Some, save/discard.
- **Config -> Clients wiring (2026-07-23)**: `App::with_config(&Config)` constructor builds `QdrantClient` and `EmbeddingClient` from `cfg.qdrant_url` / `cfg.embedding_url`. `main.rs` now calls it with the loaded config instead of discarding into `_`. Edits saved through the config screen take effect on next launch.
- **Config input bug fix (2026-07-23)**: typing `q` or `Ctrl+C` while editing a config field used to quit the app because the global quit handler ran before per-screen dispatch. `App::handle_key_press` now skips the `q`/`Ctrl+C` block when the active screen has `is_text_editing()` true. Generalized to `SearchScreen` (query input is always focused). 7 new tests (4 original Config + 3 Search regression).

## In Progress
- No active task

## Next Steps
- (Phase 3) Create / delete collections from the TUI
- (Phase 3) Upsert points (paste or load from file)
- (Phase 3) Delete points by filter or ID

See `roadmap.md` for the full build order.