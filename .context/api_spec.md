# API Spec: Echo

Status: **Defined externally** (standard APIs documented for reference)

Echo is a client that consumes existing APIs from Qdrant and llama.cpp. The contracts below document how Echo interacts with them.

> **Note**: response shapes below reflect observed live behavior on this stack (Caddy reverse proxy on port 80). Where code disagrees with the live shape, **live wins** — update code, not docs.

---

## Qdrant REST API

Base URL: `http://qdrant.localhost` (Caddy vhost, port 80) or `http://localhost:6333` (Qdrant direct)

### Key Endpoints Used

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/collections` | List all collections |
| `GET` | `/collections/{name}` | Get collection info (vector size, distance, points count) |
| `PUT` | `/collections/{name}` | Create a new collection |
| `DELETE` | `/collections/{name}` | Delete a collection |
| `PUT` | `/collections/{name}/points` | Upsert points (vectors + payload) |
| `POST` | `/collections/{name}/points/search` | Search points by vector |
| `POST` | `/collections/{name}/points/scroll` | Scroll / list points in a collection |
| `POST` | `/collections/{name}/points/delete` | Delete points by filter or ID |

### List Collections — Response
```json
{
  "result": {
    "collections": [
      { "name": "documents" },
      { "name": "images" }
    ]
  },
  "status": "ok",
  "time": 0.001
}
```
**Gotcha**: `result` is an **object** with a `collections` array key, NOT an array directly. Earlier code deserialized it as an array — silent break against real Qdrant; tests used a fabricated array shape.

### Collection Info — Response
```json
{
  "result": {
    "status": "green",
    "optimizer_status": "ok",
    "points_count": 42,
    "segments_count": 2,
    "config": {
      "params": {
        "vectors": {
          "": { "size": 1024, "distance": "Cosine" }
        }
      }
    }
  },
  "status": "ok",
  "time": 0.005
}
```
The empty string key `""` under `vectors` denotes the unnamed (default) vector. Named vectors use their label as the key.

### Collection Creation Payload
```json
{
  "vectors": {
    "size": 1024,
    "distance": "Cosine"
  }
}
```

### Search Payload
```json
{
  "vector": [0.1, 0.2, ...],
  "limit": 10,
  "with_payload": true
}
```

### Scroll Payload
```json
{
  "limit": 20,
  "offset": null,
  "with_payload": true,
  "with_vector": false
}
```

`offset` is the cursor returned by the previous response (`next_page_offset`). Omit or use `null` for the first page. When `next_page_offset` is `null` or absent, there are no more pages.

---

## llama.cpp Embedding API

Base URL: `http://embeddings.localhost` (Caddy vhost) or `http://localhost:8080` (llama-server direct). Endpoint path: `/v1/embeddings`.

### Generate Embeddings

**Endpoint:** `POST /v1/embeddings`

**Request (OpenAI-compat, code and live agree):**
```json
{ "input": "Your text here", "model": "BGE-M3" }
```

**URL config requirement**: `embedding_url` in `echo.toml` must include the path `/v1/embeddings`. POSTing to the bare hostname (e.g. `http://embeddings.localhost`) returns 404 because there's no handler at the root.

**Response (OpenAI-compat, code and live agree):**
```json
{
  "model": "BGE-M3",
  "object": "list",
  "usage": { "prompt_tokens": 3, "total_tokens": 3 },
  "data": [
    { "embedding": [0.0123, 0.0456, ...] }
  ]
}
```
The embedding vector lives at `data[0].embedding`, not at top-level.

Notes:
- BGE-M3 returns a **1024-dimensional** float vector
- llama.cpp is running in `--embeddings` mode (no text generation)
- The `/v1/embeddings` endpoint is OpenAI-API-compatible
- `model` in the request carries through to llama-server for routing (BGE-M3 expected)

---

## Collection Lifecycle

Echo manages a single "default" collection tied to `Config.default_collection`. The vector config is locked to BGE-M3 (1024-D Cosine).

### Endpoints used

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/collections/{name}` | Existence check before any ensure/rename |
| `PUT` | `/collections/{name}` | Create with `{"vectors": {"size": 1024, "distance": "Cosine"}}` |
| `DELETE` | `/collections/{name}` | Destroy during rename. 404 treated as success. |

### Behaviour

| Trigger | Outcome |
|---|---|
| App startup with `default_collection = Some(name)` | `ensure_default_collection(name)` runs. Creates if missing. |
| `Config` save renames `X` -> `Y` where `Y` does not exist | Delete `X`, create `Y`. Flash: "Renamed default collection: 'X' -> 'Y'." |
| `Config` save renames `X` -> `Y` where `Y` **does** exist | No destructive op. Both kept. Flash: "Default set to 'Y', which already exists; kept existing." |
| `Config` save sets `None` -> `Some(Y)` where `Y` doesn't exist | Create `Y`. Flash: "Created default collection 'Y'." |
| `Config` save clears `Some(X)` -> `None` | Qdrant untouched. Flash: "Default cleared from config..." |
| `Config` save leaves `Some(X)` unchanged | No-op. Flash: "Default collection unchanged." |

---

## Auth
- Neither service uses authentication in the current local setup
- Future: support Qdrant API key if needed
