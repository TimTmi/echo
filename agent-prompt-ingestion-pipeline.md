# Task: Build the Extract → Clean → Chunk pipeline module (Rust)

## Context
This module is part of a Rust TUI app (ratatui-based) that lets users do CRUD on a Qdrant vector DB. Users can add content via: plain text, common document files (PDF, DOCX, Markdown), media files (images, audio, video), and web links.

Storage: **Qdrant**
Embeddings: **local model** (ONNX/candle) — embedding + upsert are OUT OF SCOPE for this task; the module should just hand off clean, chunked text + metadata.

## Scope
Build only the **extract → clean → chunk** stage as a standalone, testable module. Do not implement embedding, Qdrant upsert, or the TUI itself. Assume it will be called from an async ingestion job elsewhere in the app and must not block on I/O.

## Requirements

### 1. Extraction
Use existing mature tools/crates per format rather than writing custom parsers. Implement a common trait (e.g. `trait Extractor { async fn extract(&self, input: &Input) -> Result<RawDoc>; }`) with a dispatcher that picks an implementation by MIME type / file extension.

Suggested per-format approach:
- Plain text / Markdown: direct read
- PDF: shell out to `pdftotext` (poppler-utils), or `pdfium-render` if avoiding subprocesses
- DOCX: `docx-rs`, or unzip + parse XML directly
- HTML / links: fetch with `reqwest`, strip boilerplate with a readability-style crate (not raw `scraper` output)
- Images: OCR via `leptess` (Tesseract bindings) or shell to `tesseract`
- Audio/video: `whisper-rs` (whisper.cpp bindings), with `ffmpeg` for format conversion first

Subprocess-based extractors are acceptable — treat the external binary as a normal dependency, but check for its presence and fail with a clear error if missing.

### 2. Cleaning
Implement directly (no dependency needed):
- Unicode normalization (`unicode-normalization`)
- Whitespace/line collapsing
- De-hyphenation for PDF-extracted text (line-break hyphenation artifacts)
- Optional: strip repeated headers/footers when batch-processing similar documents

### 3. Chunking
Evaluate the `text-splitter` crate first (token-aware, markdown/code-aware, supports overlap) before hand-rolling. If custom logic is needed, use `unicode-segmentation` for sentence boundaries. Pair with `tiktoken-rs` if chunk sizing should be by model tokens rather than characters.

Chunking strategy should be configurable (chunk size, overlap, and structure-aware vs. sliding-window mode), since this affects retrieval quality downstream.

### 4. Provenance / metadata
Every output chunk must carry metadata sufficient for later Qdrant payload storage and cascading deletes: source file/URL, extractor used, page/offset or timestamp range (for PDFs/audio/video), and chunk index within the document. Define a clear `Chunk` struct for this.

### 5. Interfaces
Deliver:
- `Extractor` trait + per-format implementations + dispatcher
- `clean(raw: &str) -> String` (or similar) cleaning function
- `Chunker` trait or config struct + implementation using `text-splitter`
- A top-level `pub async fn process(input: Input, config: ChunkConfig) -> Result<Vec<Chunk>>` that runs extract → clean → chunk
- Unit tests per extractor (can use fixture files) and for the cleaning/chunking logic

## Ask before proceeding if unclear
- Preferred crate vs. subprocess trade-off for any specific format (e.g. PDF: `pdftotext` subprocess vs. `pdfium-render`)
- Target chunk size/overlap defaults
- Whether HTML/link fetching needs to handle JS-rendered pages (would require headless browser vs. simple `reqwest`)
- Error-handling convention already used in the rest of the codebase (e.g. `anyhow` vs. `thiserror`)
- Whether subprocess dependencies (poppler, tesseract, whisper.cpp, ffmpeg) are acceptable to require as system dependencies, or must be vendored/bundled
