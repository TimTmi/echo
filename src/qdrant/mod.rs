//! Qdrant client module.
//!
//! REST API client for Qdrant vector database operations.
//! Placeholder — full implementation coming in Phase 2.

/// Collection information retrieved from Qdrant.
pub struct CollectionInfo {
    pub name: String,
    pub vector_size: usize,
    pub distance: String,
    pub points_count: u64,
}
