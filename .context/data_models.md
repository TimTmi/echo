# Data Models: Echo

**Qdrant** is the primary data store. **Local config file** (TOML) stores connection settings.

## Qdrant Collections

| Entity | Description | Key Fields |
|---|---|---|
| **Collection** | A named vector collection in Qdrant | `name`, `vector_size` (1024 for BGE-M3), `distance` (Cosine), `points_count` |
| **Point** | A single vector point with optional payload | `id` (UUID or integer), `vector` (f32[1024]), `payload` (JSON object) |
| **Payload** | Metadata attached to a point | Arbitrary JSON key-value pairs (text chunk, source URL, timestamp, etc.) |

## Local Config

| Field | Type | Description |
|---|---|---|
| `qdrant_url` | string | Qdrant REST API base URL (default: `http://localhost:6333`) |
| `embedding_url` | string | llama.cpp embedding endpoint (default: `http://localhost:8080/v1/embeddings`) |
| `default_collection` | Option<string> | Last-used collection name (None = no default) |
| `embedding_model` | string | Model name for display (default: `BGE-M3`) |

## Relationships
- A **Collection** contains many **Points**
- A **Point** has one **vector** (1024D) and optional **payload** metadata
- **Local config** is standalone — not stored in Qdrant

## Constraints and Indexes
- BGE-M3 produces **1024-dimensional** vectors with **Cosine** distance
- Qdrant supports payload indexing for filtered searches
- Config file is local-only, no multi-user concurrency considerations
