//! Text chunking module.
//!
//! Uses the `text-splitter` crate for structure-aware and sliding-window
//! chunking. Supports configurable chunk size (in tokens or characters),
//! overlap, and mode selection.

use text_splitter::{ChunkConfig as TextSplitterConfig, TextSplitter};

/// Configuration for the chunking strategy.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkConfig {
    /// Target chunk size in tokens (if `tiktoken-rs` is available) or
    /// characters (fallback).
    pub chunk_size: usize,
    /// Number of tokens/characters to overlap between consecutive chunks.
    pub overlap: usize,
    /// Chunking mode: structure-aware (markdown/code) vs. sliding window.
    pub mode: ChunkMode,
}

/// Chunking strategy selection.
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkMode {
    /// Use `text-splitter`'s markdown-aware, code-aware splitting.
    StructureAware,
    /// Fixed-size sliding window with no structural awareness.
    SlidingWindow,
}

/// Split cleaned text into chunks according to the given configuration.
pub fn chunk(text: &str, config: &ChunkConfig) -> Vec<String> {
    match config.mode {
        ChunkMode::StructureAware => chunk_structure_aware(text, config),
        ChunkMode::SlidingWindow => chunk_sliding_window(text, config),
    }
}

/// Structure-aware chunking using `text-splitter`.
fn chunk_structure_aware(text: &str, config: &ChunkConfig) -> Vec<String> {
    let split_config = TextSplitterConfig::new(config.chunk_size);
    let splitter = TextSplitter::new(split_config);

    let chunks: Vec<String> = splitter.chunks(text).map(|c| c.to_string()).collect();

    if config.overlap > 0 && chunks.len() > 1 {
        apply_overlap(chunks, config.overlap)
    } else {
        chunks
    }
}

/// Fixed-size sliding window chunking (character-based).
fn chunk_sliding_window(text: &str, config: &ChunkConfig) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let chunk_size = config.chunk_size;
    let overlap = config.overlap.min(chunk_size / 2);
    let step = chunk_size.saturating_sub(overlap);

    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    if len <= chunk_size {
        return vec![chars.iter().collect()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < len {
        let end = (start + chunk_size).min(len);
        let chunk: String = chars[start..end].iter().collect();
        chunks.push(chunk);

        if end == len {
            break;
        }

        start += step;
    }

    chunks
}

/// Apply character-level overlap between adjacent chunks.
fn apply_overlap(mut chunks: Vec<String>, overlap: usize) -> Vec<String> {
    if chunks.len() <= 1 || overlap == 0 {
        return chunks;
    }

    for i in (1..chunks.len()).rev() {
        let prev_chars: Vec<char> = chunks[i - 1].chars().collect();
        let overlap_start = prev_chars.len().saturating_sub(overlap);
        let tail: String = prev_chars[overlap_start..].iter().collect();
        chunks[i] = format!("{tail}{}", chunks[i]);
    }

    chunks.dedup();
    chunks
}

/// Token count estimation: ~4 chars per token for English.
pub fn estimate_token_count(text: &str) -> usize {
    (text.len() + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sliding_window_small_text() {
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };
        let text = "Hello world! This text is small.";
        let chunks = chunk(text, &config);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_sliding_window_large_text() {
        let config = ChunkConfig {
            chunk_size: 10,
            overlap: 2,
            mode: ChunkMode::SlidingWindow,
        };
        let text = "This is a longer text that should be split into multiple chunks.";
        let chunks = chunk(text, &config);
        assert!(chunks.len() > 1, "should produce multiple chunks");
        for c in &chunks {
            assert!(
                c.chars().count() <= config.chunk_size + config.overlap,
                "chunk '{}' exceeds max size",
                c
            );
        }
    }

    #[test]
    fn test_structure_aware_basic() {
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 0,
            mode: ChunkMode::StructureAware,
        };
        let text = "# Title\n\nSome paragraph text here.";
        let chunks = chunk(text, &config);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_empty_text() {
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 0,
            mode: ChunkMode::SlidingWindow,
        };
        let chunks = chunk("", &config);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_overlap_produces_expected_windows() {
        let config = ChunkConfig {
            chunk_size: 10,
            overlap: 2,
            mode: ChunkMode::SlidingWindow,
        };
        let text = "AAAAAAAAAABBBBBBBBBBCCCCCC";
        let chunks = chunk(text, &config);
        assert_eq!(
            chunks.len(),
            3,
            "expected 3 chunks with size=10, overlap=2 on 26 chars"
        );
        assert!(
            chunks[1].starts_with(&chunks[0][chunks[0].len() - 2..]),
            "overlap not preserved"
        );
    }

    #[test]
    fn test_overlap_exceeds_chunk_size() {
        let config = ChunkConfig {
            chunk_size: 10,
            overlap: 20,
            mode: ChunkMode::SlidingWindow,
        };
        let text = "This is a test of overlap clamping behavior.";
        let chunks = chunk(text, &config);
        assert!(!chunks.is_empty());
    }

    #[test]
    fn test_estimate_token_count() {
        assert_eq!(estimate_token_count("a"), 1);
        assert_eq!(estimate_token_count(""), 0);
    }

    #[test]
    fn test_structure_aware_markdown() {
        let config = ChunkConfig {
            chunk_size: 10,
            overlap: 0,
            mode: ChunkMode::StructureAware,
        };
        let text = "# Header\n\nBody text here.\n\n## Subheader\n\nMore text.";
        let chunks = chunk(text, &config);
        assert!(chunks.len() >= 2);
    }
}