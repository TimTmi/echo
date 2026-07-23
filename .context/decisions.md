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

## Testing

### 2026-07-23: Deserialization fixtures must mirror live API responses
- **Decision**: When adding or updating serde-based response structs, capture a live response from the real service and use that as the test fixture. Do not author fixtures from the struct shape alone.
- **Reason**: Mocks validated from struct-instead-of-from-API pass tests but break against the live service. Concretely hit on 2026-07-23: `CollectionsListResponse.result` was modeled as `Vec<…>` while real Qdrant returns an object `{ collections: Vec<…> }`. Tests fabricated with `{"result": […]}` passed; live response failed to parse. Same risk applies to `EmbeddingResponse` (suspected live shape: `data[0].embedding`, code expects top-level `embedding`).
- **Consequences**: One-time cost to capture a live response per endpoint. Future drift between docs and live service surfaces only at the live test layer, not from author-written fixtures.

### 2026-07-23: Required dependencies must be set before the screen that uses them
- **Decision**: Screens whose transitions depend on external state must obtain that state explicitly before being entered. Means: enter-Search must pull a collection name from something (selected list, default config, or user prompt) and refuse to run with an empty target. Same pattern applies to any future endpoint that names a resource.
- **Reason**: `SearchScreen.collection` was never wired by either path that entered it (Home `s`, Collections `S`). Effect: `/points/search` URL became `/collections//points/search` → opaque 404 that masked the actual problem (empty string in URL). A guard in the screen plus an explicit setter on entry keeps the contract local and testable.
- **Consequences**: Each new screen needs an explicit setter for its preconditions, plus a test pair (with-precondition passes through; without-precondition shows clean error).

### 2026-07-23: Cross-collection search (accepted, not yet implemented)
- **Status**: ADR pending. Awaiting user's choice between alternatives below.
- **Problem**: `SearchScreen.collection` is empty when the user has `default_collection = None` and hasn't first drilled into Collections. Today this hard-errors with "no collection selected." But Qdrant has no native cross-collection query endpoint; the natural fix is client-side fanout (one `search_points` per collection, merged by score). Auto-running that fanout when no collection is selected removes the dead-end UX for users who don't configure a default.
- **Alternatives under consideration**:
  - **A. Auto-fallback in `SearchScreen` (recommended)**: when `collection.is_empty()`, fan out to all collections in `CollectionBrowserScreen.collection_names`. Each result row carries its source collection name. Bare-bones, sequential HTTP. Tightest UX, smallest diff. Trade-off: latency grows linearly with collection count; score-comparison across collections is only meaningful if all collections share the same vector config (currently true: 1024-D Cosine for BGE-M3).
  - **B. New `ActiveScreen::CrossSearch` variant**: parallel screen with explicit cross-collection input. Cleaner separation, but doubles screen surface area for a feature that's mostly indistinguishable from Search to the user.
  - **C. Explicit toggle on `SearchScreen` (e.g. `[Tab]` mode)**: user opts in to cross-search, separate from single-search. Less implicit, more discoverable, but adds a keybinding to remember.
- **Recommended**: A. Source-label rendering is the only must-have change beyond the fanout call. Avoid B unless a future workflow really needs cross-search framed as a distinct mode.
- **Risks** (for any alternative):
  - Vector-shape drift: if any collection uses non-BGE-M3 config (different size or distance), fanout returns errors for those and silently misses results. Mitigation: at fanout time, fetch each collection's info first and skip those whose vector config doesn't match `BGE-M3` shape, surface a warning.
  - Score semantics: cosine scores from different collections are still numerically comparable (same distance), but ordering with payload-filtered collections is not. Mitigation: warn in flash if any collection in the fanout uses a non-Cosine distance.
  - Latency: N round-trips. Mitigation later (out of scope here): parallel fanout via `tokio::join!`.

**Decision**: Option A (source-label fanout inside `SearchScreen`). When `collection` is empty, iterate all collections returned by `list_collections`, run `search_points` on each, merge results sorted by score, prefix each row with source collection name.

**Status**: Accepted. Implementation deferred until after Phase 3 (upsert/delete) ships.

### 2026-07-23: Collection CRUD lives inline in the Collections screen, not in a new `ActiveScreen`
- **Decision**: Implement Create and Delete for collections as additional `Mode` variants on `CollectionBrowserScreen` rather than introducing a second screen (e.g. `ActiveScreen::CollectionManage`).
- **Reason**: The Collections screen's responsibility is "manage collections" (browse them, view detail, mutate them). Mounting a separate modal screen for each write operation would split one responsibility across two screens and create an awkward entry/exit dance. An inline form rendered into the existing detail panel keeps the screen count down and the user's mental model simple: "I am on Collections; I pressed [N]; I see a name field." The form overlays the browsing content via state, not via a separate screen. Both `[N]` (create) and `[D]` (confirm delete) operate on the same selection the user is already on, so the context loss of a screen transition would be a real downside.
- **Consequences**:
  - `CollectionBrowserScreen` gains a `Mode` enum with five variants. The handle key path now branches on `Mode` rather than collapsing keys into one match.
  - Pending states (`PendingCreate(name)` / `PendingDelete(name)`) swallow every keypress so the user cannot fire a second mutation before the first lands.
  - The state-machine methods (`begin_create`, `begin_delete`, `complete_create_success`, `complete_delete_success`, `complete_op_error`) are unit-testable without driving `tick()` end-to-end. The HTTP round-trip itself remains in `qdrant::tests`.
  - On error, we surface the failure as a flash banner and return to `Browsing` without forcing a list reload -- Qdrant state is unchanged, so showing cached state is the right thing.
  - The form has a `Pending<X>` waiting state with no visible feedback during the HTTP call. Acceptable because the typical call is sub-100 ms locally; if that ever changes, render "Working..." lines inside the detail panel -- the placeholder already exists for this.
  - "Vector config" remains hidden from users. Collections are always created with BGE-M3's (1024, Cosine) shape. This matches `data_models.md` and avoids dragging in vector-config UI for a use case we do not support.
