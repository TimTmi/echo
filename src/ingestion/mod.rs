//! Ingestion pipeline: Extract → Clean → Chunk.
//!
//! Top-level entry point for processing documents into chunks suitable for
//! embedding and Qdrant upsert. Supports plain text, Markdown, PDF, DOCX,
//! HTML/URLs, images (OCR via Tesseract), and audio/video (transcription via
//! Whisper).
//!
//! # Architecture
//!
//! ```text
//! Input ──► Extractor (dispatched by MIME/extension) ──► RawDoc
//!                │
//!                ▼
//!          Cleaner (normalize, collapse, de-hyphenate)
//!                │
//!                ▼
//!          Chunker (text-splitter, configurable size/overlap/mode)
//!                │
//!                ▼
//!          Vec<Chunk>  ← each carries provenance metadata
//! ```

pub mod extractor;
pub mod cleaner;
pub mod chunker;

use anyhow::Context;
use extractor::{Input, Source};
use chunker::ChunkConfig;

/// A single processed chunk ready for embedding.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub text: String,
    pub metadata: ChunkMetadata,
}

/// Provenance metadata carried with each chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkMetadata {
    /// Original source (file, URL, or direct text).
    pub source: Source,
    /// Human-readable display name (filename or URL).
    pub source_display: String,
    /// Name of the extractor used (e.g. "plaintext", "pdf", "docx").
    pub extractor: String,
    /// Page number for PDFs.
    pub page: Option<u32>,
    /// Timestamp range in seconds for audio/video.
    pub timestamp_range: Option<(f64, f64)>,
    /// Zero-based index of this chunk within the document.
    pub chunk_index: usize,
    /// Total number of chunks for this document.
    pub total_chunks: usize,
}

impl ChunkMetadata {
    fn new(source: &Source, extractor: &str, chunk_index: usize, total_chunks: usize) -> Self {
        let source_display = match source {
            Source::File(p) => p.to_string_lossy().to_string(),
            Source::Url(u) => u.clone(),
            Source::Text(_) => "<text input>".to_string(),
        };
        Self {
            source: source.clone(),
            source_display,
            extractor: extractor.to_string(),
            page: None,
            timestamp_range: None,
            chunk_index,
            total_chunks,
        }
    }
}

/// Run the full extract → clean → chunk pipeline.
///
/// 1. If `input` is a URL, fetches the content via HTTP.
/// 2. Dispatches to the correct extractor based on content type.
/// 3. Cleans the extracted text.
/// 4. Splits into chunks with provenance metadata.
///
/// # Errors
///
/// Returns an error if the extractor for the given content type is not found,
/// the extraction itself fails, the HTTP fetch fails, or any required system
/// dependency is missing.
pub async fn process(input: Input, config: ChunkConfig) -> anyhow::Result<Vec<Chunk>> {
    // Resolve URLs by fetching content
    let resolved_input = match &input.source {
        Source::Url(url) => {
            let client = reqwest::Client::new();
            let response = client
                .get(url)
                .send()
                .await
                .context(format!("failed to fetch URL: {url}"))?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "HTTP {} when fetching {}",
                    response.status(),
                    url
                );
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/plain")
                .to_string();
            let bytes = response.bytes().await.context("failed to read response body")?;
            Input {
                source: input.source.clone(),
                content_type,
                data: bytes.to_vec(),
            }
        }
        _ => input,
    };

    // Detect content type from extension if MIME is generic or absent
    let detected_type = refine_content_type(&resolved_input);

    // Dispatch to the right extractor
    let extractor = extractor::dispatcher(&detected_type)?;
    let raw_doc = extractor.extract(&resolved_input).await?;

    // Clean the raw text
    let cleaned = cleaner::clean(&raw_doc.text);

    // Chunk
    let chunks = chunker::chunk(&cleaned, &config);

    // Attach metadata
    let total = chunks.len();
    let mut result: Vec<Chunk> = chunks
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            let mut meta = ChunkMetadata::new(&resolved_input.source, extractor.name(), i, total);
            if let Some(page) = raw_doc.page {
                meta.page = Some(page);
            }
            if let Some(ts) = raw_doc.timestamp_range {
                meta.timestamp_range = Some(ts);
            }
            Chunk { text, metadata: meta }
        })
        .collect();

    // De-duplicate: skip empty or duplicate chunks
    result.dedup_by(|a, b| a.text == b.text);
    result.retain(|c| !c.text.trim().is_empty());

    Ok(result)
}

/// If the content-type is generic (`text/plain`, `application/octet-stream`,
/// or missing), refine it from the file extension in the source path/URL.
fn refine_content_type(input: &Input) -> String {
    let generic = matches!(
        input.content_type.as_str(),
        "text/plain" | "application/octet-stream" | "" | "text/html"
    );

    if !generic {
        return input.content_type.clone();
    }

    let path_str = match &input.source {
        Source::File(p) => p.to_string_lossy().to_string(),
        Source::Url(u) => u.split('?').next().unwrap_or(u).to_string(),
        Source::Text(_) => return input.content_type.clone(),
    };

    let ext = std::path::Path::new(&path_str)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "md" | "markdown" => "text/markdown".to_string(),
        "pdf" => "application/pdf".to_string(),
        "docx" | "doc" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        "html" | "htm" => "text/html".to_string(),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tiff" | "webp" => "image/*".to_string(),
        "mp3" | "wav" | "ogg" | "flac" | "m4a" | "wma" => "audio/*".to_string(),
        "mp4" | "avi" | "mkv" | "mov" | "webm" => "video/*".to_string(),
        "txt" | "text" | "csv" | "json" | "yaml" | "yml" | "xml" | "toml" => "text/plain".to_string(),
        _ => input.content_type.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::chunker::ChunkMode;
    use crate::ingestion::extractor::Source;

    #[tokio::test]
    async fn test_process_plain_text() {
        let input = Input {
            source: Source::Text("Hello world! This is a test.".to_string()),
            content_type: "text/plain".to_string(),
            data: b"Hello world! This is a test.".to_vec(),
        };
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };
        let chunks = process(input, config).await.unwrap();
        assert!(!chunks.is_empty(), "should produce at least one chunk");
        assert!(
            chunks[0].text.contains("Hello world"),
            "should contain original text"
        );
        assert_eq!(chunks[0].metadata.extractor, "plaintext");
        assert_eq!(chunks[0].metadata.chunk_index, 0);
    }

    #[tokio::test]
    async fn test_process_markdown() {
        let input = Input {
            source: Source::File(std::path::PathBuf::from("test.md")),
            content_type: "text/markdown".to_string(),
            data: b"# Heading\n\nThis is a **markdown** document.".to_vec(),
        };
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };
        let chunks = process(input, config).await.unwrap();
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].metadata.extractor, "plaintext");
    }

    #[tokio::test]
    async fn test_process_empty_input() {
        let input = Input {
            source: Source::Text(String::new()),
            content_type: "text/plain".to_string(),
            data: Vec::new(),
        };
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };
        let chunks = process(input, config).await.unwrap();
        assert!(chunks.is_empty(), "empty input should produce zero chunks");
    }

    #[tokio::test]
    async fn test_refine_content_type_by_extension() {
        let input = Input {
            source: Source::File(std::path::PathBuf::from("report.pdf")),
            content_type: "application/octet-stream".to_string(),
            data: Vec::new(),
        };
        let refined = refine_content_type(&input);
        assert_eq!(refined, "application/pdf");

        let input2 = Input {
            source: Source::File(std::path::PathBuf::from("readme.md")),
            content_type: "text/plain".to_string(),
            data: Vec::new(),
        };
        let refined2 = refine_content_type(&input2);
        assert_eq!(refined2, "text/markdown");
    }

    #[tokio::test]
    async fn test_refine_content_type_unknown_extension() {
        let input = Input {
            source: Source::File(std::path::PathBuf::from("data.bin")),
            content_type: "application/octet-stream".to_string(),
            data: Vec::new(),
        };
        let refined = refine_content_type(&input);
        assert_eq!(refined, "application/octet-stream");
    }
}