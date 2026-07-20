//! Embedding client module.
//!
//! HTTP client for llama.cpp embedding server (BGE-M3).
//! Placeholder — full implementation coming in Phase 2.

/// Response from the llama.cpp `/v1/embeddings` endpoint.
pub struct EmbeddingResponse {
    pub embedding: Vec<f32>,
}
