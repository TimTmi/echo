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

**Request (OpenAI-compat, verified live):**
```json
{ "input": "Your text here", "model": "BGE-M3" }
```

**Code currently sends:**
```json
{ "content": "Your text here" }
```
**Untested**: the `content` field is not standard OpenAI-compat. The server may accept it as a non-standard alias, or reject it. Needs live verification before assuming Search works. If it rejects, change `EmbeddingRequest` struct in `src/embedding/mod.rs` to use `input`.

**Response (observed live):**
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
**Gotcha**: embedding vector lives inside `data[0].embedding`, not at top-level `embedding`. Same blind-spot as the collections-list bug — code's `EmbeddingResponse` struct expects top-level `embedding`. Will fail to parse on real server.

Notes:
- BGE-M3 returns a **1024-dimensional** float vector
- llama.cpp is running in `--embeddings` mode (no text generation)
- The `/v1/embeddings` endpoint is OpenAI-API-compatible

---

## Auth
- Neither service uses authentication in the current local setup
- Future: support Qdrant API key if needed
