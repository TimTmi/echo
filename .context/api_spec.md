# API Spec: Echo

Status: **Defined externally** (standard APIs documented for reference)

Echo is a client that consumes existing APIs from Qdrant and llama.cpp. The contracts below document how Echo interacts with them.

---

## Qdrant REST API

Base URL: `http://qdrant.localhost:80` or `http://localhost:6333`

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

Base URL: `http://embeddings.localhost:80` or `http://localhost:8080`

### Generate Embeddings

**Endpoint:** `POST /v1/embeddings`

**Request:**
```json
{
  "content": "Your text here"
}
```

**Response:**
```json
{
  "embedding": [0.0123, 0.0456, ...]
}
```

Notes:
- BGE-M3 returns a **1024-dimensional** float vector
- llama.cpp is running in `--embeddings` mode (no text generation)
- The `/v1/embeddings` endpoint is OpenAI-API-compatible

---

## Auth
- Neither service uses authentication in the current local setup
- Future: support Qdrant API key if needed
