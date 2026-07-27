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

use std::time::Duration;
use anyhow::Context;
use extractor::{Input, Source};
use chunker::ChunkConfig;

/// Maximum number of bytes to download when fetching a URL.
/// Content exceeding this limit is rejected to prevent runaway memory use.
pub(crate) const MAX_DOWNLOAD_SIZE: usize = 100 * 1024 * 1024; // 100 MiB

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

/// Format the input source for log messages.
fn source_display(input: &Input) -> String {
    match &input.source {
        Source::File(p) => p.to_string_lossy().to_string(),
        Source::Url(u) => u.clone(),
        Source::Text(_) => "<text input>".to_string(),
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
    let src = source_display(&input);
    tracing::info!("ingestion pipeline starting: source={src}, content_type={}, chunk_size={}, overlap={}, mode={:?}",
        input.content_type, config.chunk_size, config.overlap, config.mode);

    // Resolve URLs by fetching content
    let resolved_input = match &input.source {
        Source::Url(url) => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .context("failed to build HTTP client")?;
            let response = client
                .get(url)
                .send()
                .await
                .with_context(|| format!("failed to fetch URL: {url}"))?;
            if !response.status().is_success() {
                anyhow::bail!(
                    "HTTP {} when fetching {}",
                    response.status(),
                    url
                );
            }
            // Reject oversized downloads before reading the body
            if let Some(cl) = response
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok())
            {
                if cl > MAX_DOWNLOAD_SIZE {
                    anyhow::bail!(
                        "Content-Length {cl} exceeds maximum download size of {MAX_DOWNLOAD_SIZE} bytes"
                    );
                }
            }
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/plain")
                .to_string();
            let bytes = response.bytes().await.context("failed to read response body")?;
            if bytes.len() > MAX_DOWNLOAD_SIZE {
                // Use byte length vs alloc length after reading to catch chunked / no-CL cases
                anyhow::bail!(
                    "Downloaded {} bytes exceeds maximum download size of {MAX_DOWNLOAD_SIZE} bytes",
                    bytes.len()
                );
            }
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
    let raw_docs = extractor.extract(&resolved_input).await?;

    // Process each raw doc (one per page for PDFs, one overall for others)
    // through clean → chunk → metadata attach, then flatten.
    //
    // chunk_index and total_chunks are global across all pages so that
    // multi-page PDFs produce a contiguous index range.
    let mut result: Vec<Chunk> = Vec::new();
    let mut global_index: usize = 0;
    for raw_doc in &raw_docs {
        let cleaned = cleaner::clean(&raw_doc.text);
        let chunks = chunker::chunk(&cleaned, &config);

        let mut page_chunks: Vec<Chunk> = chunks
            .into_iter()
            .map(|text| {
                let meta = ChunkMetadata::new(
                    &resolved_input.source,
                    extractor.name(),
                    global_index,
                    // total_chunks is set below after we know the full count
                    0,
                );
                let chunk = Chunk {
                    text,
                    metadata: ChunkMetadata {
                        page: raw_doc.page,
                        timestamp_range: raw_doc.timestamp_range,
                        ..meta
                    },
                };
                global_index += 1;
                chunk
            })
            .collect();

        result.append(&mut page_chunks);
    }

    // Fix total_chunks now that we know the global count
    let total = result.len();
    for chunk in &mut result {
        chunk.metadata.total_chunks = total;
    }

    // De-duplicate: skip empty or duplicate chunks
    result.dedup_by(|a, b| a.text == b.text);
    let before_dedup = result.len();
    result.retain(|c| !c.text.trim().is_empty());
    let after_dedup = result.len();

    tracing::info!("ingestion pipeline finished: {src} -> {} chunks ({} empty/duplicate removed)", after_dedup, before_dedup - after_dedup);

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
        // Non-PDF inputs should have no page number
        assert_eq!(chunks[0].metadata.page, None);
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

    // ---------------------------------------------------------------------------
    // Download size cap tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn test_download_rejects_oversized_content_length() {
        let mut server = mockito::Server::new_async().await;
        // Body exactly 1 byte over the cap with matching Content-Length header.
        // hyper validates header-body agreement, so the body must match the header.
        let big_body = vec![b'x'; MAX_DOWNLOAD_SIZE + 1];
        let mock = server
            .mock("GET", "/big-file")
            .with_status(200)
            .with_header("Content-Type", "text/plain")
            .with_header("Content-Length", &format!("{}", MAX_DOWNLOAD_SIZE + 1))
            .with_body(&big_body)
            .create_async()
            .await;

        let url = format!("{}/big-file", server.url());
        let input = Input {
            source: Source::Url(url),
            content_type: "text/plain".to_string(),
            data: Vec::new(),
        };
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };

        let result = process(input, config).await;
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds maximum download size"),
            "error should mention the cap, got: {msg}"
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn test_download_rejects_oversized_body_no_content_length() {
        let mut server = mockito::Server::new_async().await;
        // Build a body larger than MAX_DOWNLOAD_SIZE
        let big_body = vec![b'x'; MAX_DOWNLOAD_SIZE + 1];
        let mock = server
            .mock("GET", "/big-body")
            .with_status(200)
            .with_header("Content-Type", "text/plain")
            // Omit Content-Length to simulate chunked transfer
            .with_body(&big_body)
            .create_async()
            .await;

        let url = format!("{}/big-body", server.url());
        let input = Input {
            source: Source::Url(url),
            content_type: "text/plain".to_string(),
            data: Vec::new(),
        };
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };

        let result = process(input, config).await;
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exceeds maximum download size"),
            "error should mention the cap, got: {msg}"
        );
        mock.assert_async().await;
    }

    // -----------------------------------------------------------------------
    // refine_content_type with extensionless URL
    // -----------------------------------------------------------------------

    #[test]
    fn test_refine_content_type_extensionless_url() {
        let input = Input {
            source: Source::Url("https://example.com/data".to_string()),
            content_type: "application/octet-stream".to_string(),
            data: Vec::new(),
        };
        let refined = refine_content_type(&input);
        assert_eq!(refined, "application/octet-stream");
    }

    // -----------------------------------------------------------------------
    // detect_repeated_lines / clean_with_footer_strip substring false-positive
    // -----------------------------------------------------------------------

    #[test]
    fn test_clean_with_footer_strip_substring_no_false_positive() {
        let text = "Page\nImportant: Page settings should not be removed.\nFooter\n\
                     Page\nMore body content about page layout.\nFooter";
        let mut repeated = std::collections::HashSet::new();
        repeated.insert("Page".to_string());
        repeated.insert("Footer".to_string());
        let result = cleaner::clean_with_footer_strip(text, &repeated);
        assert!(!result.contains("Page\nImportant"), "header 'Page' should be stripped");
        assert!(
            result.contains("page settings") || result.contains("page layout"),
            "body lines containing 'page' as substring must survive"
        );
    }

    // -----------------------------------------------------------------------
    // Concurrent process() calls
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_process_concurrent_text_inputs() {
        let text_a = "Concurrent chunk A. ".repeat(100);
        let text_b = "Concurrent chunk B. ".repeat(100);
        let input_a = Input {
            source: Source::Text(text_a.clone()),
            content_type: "text/plain".to_string(),
            data: text_a.as_bytes().to_vec(),
        };
        let input_b = Input {
            source: Source::Text(text_b.clone()),
            content_type: "text/plain".to_string(),
            data: text_b.as_bytes().to_vec(),
        };
        let config = ChunkConfig {
            chunk_size: 256,
            overlap: 32,
            mode: ChunkMode::SlidingWindow,
        };

        let (res_a, res_b) = tokio::join!(
            process(input_a, config.clone()),
            process(input_b, config),
        );
        let chunks_a = res_a.unwrap();
        let chunks_b = res_b.unwrap();

        assert!(!chunks_a.is_empty());
        assert!(!chunks_b.is_empty());
        assert!(chunks_a[0].text.contains("Concurrent chunk A"));
        assert!(chunks_b[0].text.contains("Concurrent chunk B"));
    }
}