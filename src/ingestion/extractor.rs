//! Extractor trait, per-format implementations, and MIME-based dispatcher.
//!
//! Each format gets its own extractor that implements the [`Extractor`] trait.
//! The [`dispatcher`] function selects the right extractor by MIME type / file
//! extension.

use std::path::{Path, PathBuf};

use anyhow::Context;
use async_trait::async_trait;
use tempfile::TempDir;

/// Input to the ingestion pipeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    /// Where this content came from (file, URL, or direct text).
    pub source: Source,
    /// MIME type or format hint (e.g. `"text/plain"`, `"application/pdf"`).
    pub content_type: String,
    /// Raw bytes of the content (for files and fetched URLs).
    pub data: Vec<u8>,
}

/// Origin of the content being processed.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    File(PathBuf),
    Url(String),
    Text(String),
}

/// Raw extracted document before cleaning and chunking.
#[derive(Debug, Clone, Default)]
pub struct RawDoc {
    /// Extracted plain text.
    pub text: String,
    /// Page number for PDFs (0-based). `None` for non-paginated formats.
    pub page: Option<u32>,
    /// Timestamp range in seconds for audio/video.
    pub timestamp_range: Option<(f64, f64)>,
}

/// Common trait for all format extractors.
///
/// Each extractor is responsible for converting its input format into
/// plain text. The trait is `async` so extractors can fetch remote content,
/// run subprocesses, or call native library functions without blocking the
/// caller.
#[async_trait]
pub trait Extractor: Send + Sync {
    /// Return the extractor name (e.g. `"pdf"`, `"plaintext"`).
    fn name(&self) -> &'static str;

    /// Extract plain text from the given input.
    ///
    /// Returns one or more [`RawDoc`] items. Most extractors return a single
    /// item; the PDF extractor returns one item per page so each chunk can
    /// carry an accurate page number.
    ///
    /// # Errors
    ///
    /// Returns an error if the input cannot be parsed, if a required system
    /// dependency (e.g. `pdftotext`) is not found, or if the input is empty
    /// and the extractor requires non-empty data.
    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>>;
}

// ---------------------------------------------------------------------------
// Plain text / Markdown extractor
// ---------------------------------------------------------------------------

/// Plain text / Markdown extractor — reads `data` directly as UTF-8.
#[derive(Debug)]
struct PlainTextExtractor;

#[async_trait]
impl Extractor for PlainTextExtractor {
    fn name(&self) -> &'static str {
        "plaintext"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        let text = String::from_utf8_lossy(&input.data).to_string();
        Ok(vec![RawDoc {
            text,
            page: None,
            timestamp_range: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// Shared subprocess helpers
// ---------------------------------------------------------------------------

/// Check that `binary` exists and can be run (passing `--version`).
/// Returns a clear error with `install_hint` if not found.
fn check_binary(binary: &str, install_hint: &str) -> anyhow::Result<()> {
    let output = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("{binary} not found. {install_hint}"))?;
    if !output.status.success() {
        anyhow::bail!("{binary} --version returned non-zero status");
    }
    Ok(())
}

/// Create a temporary directory with the standard prefix.
fn make_temp_dir() -> anyhow::Result<TempDir> {
    TempDir::new().context("failed to create temp dir")
}

/// Write `data` to `dir / filename`, returning the full path.
fn write_temp_file(dir: &Path, filename: &str, data: &[u8]) -> anyhow::Result<()> {
    std::fs::write(dir.join(filename), data)
        .with_context(|| format!("failed to write temp file '{filename}'"))
}

/// Run a subprocess that writes its output to a file, then read that file.
///
/// Temp files are created inside a [`TempDir`] and automatically cleaned
/// up when it drops.
fn run_subprocess_to_file(
    bin: &str,
    args: &[&str],
    output_file: &Path,
) -> anyhow::Result<Vec<u8>> {
    let status = std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {bin}"))?;

    if !status.status.success() {
        let stderr = String::from_utf8_lossy(&status.stderr);
        anyhow::bail!("{bin} failed: {stderr}");
    }

    std::fs::read(output_file)
        .with_context(|| format!("failed to read {bin} output file"))
}

/// Run a subprocess that emits output on stdout, capture it.
fn run_subprocess_to_stdout(bin: &str, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let output = std::process::Command::new(bin)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {bin}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{bin} failed: {stderr}");
    }

    Ok(output.stdout)
}

/// Extract the file extension from an `Input` source (file path or URL).
fn input_extension(input: &Input, fallback: &str) -> String {
    match &input.source {
        Source::File(p) => p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or(fallback)
            .to_lowercase(),
        Source::Url(u) => {
            let path = u.split('?').next().unwrap_or(u);
            Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or(fallback)
                .to_lowercase()
        }
        Source::Text(_) => fallback.to_lowercase(),
    }
}

// ---------------------------------------------------------------------------
// PDF extractor (pdftotext subprocess)
// ---------------------------------------------------------------------------

/// PDF extractor — shells out to `pdftotext` (poppler-utils).
#[derive(Debug)]
struct PdfExtractor;

#[async_trait]
impl Extractor for PdfExtractor {
    fn name(&self) -> &'static str {
        "pdf"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        check_binary(
            "pdftotext",
            "Install poppler-utils (e.g. `apt install poppler-utils` on Debian/Ubuntu, \
             `brew install poppler` on macOS, or download from https://poppler.freedesktop.org/).",
        )?;

        let tmp = make_temp_dir()?;
        let input_path = tmp.path().join("input.pdf");
        let output_path = tmp.path().join("output.txt");
        write_temp_file(tmp.path(), "input.pdf", &input.data)?;

        run_subprocess_to_file(
            "pdftotext",
            &[
                input_path.to_str().unwrap(),
                output_path.to_str().unwrap(),
            ],
            &output_path,
        )?;

        let text = std::fs::read_to_string(&output_path)
            .context("failed to read pdftotext output")?;

        // Split on form-feed characters to get per-page text.
        // pdftotext separates pages with \x0C. Split into page-sized chunks.
        let pages: Vec<&str> = text.split('\x0C').collect();
        // `pdftotext` ends the last page with a \x0C, so the final split
        // element is an empty string. Drop it.
        let pages: Vec<&str> = pages.into_iter()
            .filter(|p| !p.is_empty())
            .collect();

        if pages.is_empty() {
            return Ok(vec![RawDoc {
                text: text.clone(),
                page: None,
                timestamp_range: None,
            }]);
        }

        Ok(pages.into_iter().enumerate().map(|(i, page_text)| {
            RawDoc {
                text: page_text.trim().to_string(),
                page: Some(i as u32),
                timestamp_range: None,
            }
        }).collect())
    }
}

// ---------------------------------------------------------------------------
// DOCX extractor (docx-rs)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct DocxExtractor;

#[async_trait]
impl Extractor for DocxExtractor {
    fn name(&self) -> &'static str {
        "docx"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        let doc = docx_rs::read_docx(&input.data)
            .map_err(|e| anyhow::anyhow!("failed to parse DOCX: {e}"))?;

        let mut text = String::new();
        for child in &doc.document.children {
            if let docx_rs::DocumentChild::Paragraph(p) = child {
                for p_child in &p.children {
                    if let docx_rs::ParagraphChild::Run(r) = p_child {
                        for r_child in &r.children {
                            if let docx_rs::RunChild::Text(t) = r_child {
                                text.push_str(&t.text);
                            }
                        }
                        text.push(' ');
                    }
                }
                text.push('\n');
            }
        }

        Ok(vec![RawDoc {
            text,
            page: None,
            timestamp_range: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// HTML extractor (reqwest + html2text)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct HtmlExtractor;

#[async_trait]
impl Extractor for HtmlExtractor {
    fn name(&self) -> &'static str {
        "html"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        if input.data.is_empty() {
            // process() guarantees data for URLs/files; this is a safety net
            // for direct extract() calls without the pipeline.
            return Ok(vec![RawDoc::default()]);
        }
        let html = String::from_utf8_lossy(&input.data);
        let text = html2text::from_read(html.as_bytes(), 80)
            .context("html2text conversion failed")?;
        Ok(vec![RawDoc {
            text,
            page: None,
            timestamp_range: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// Image OCR extractor (tesseract subprocess)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ImageExtractor;

#[async_trait]
impl Extractor for ImageExtractor {
    fn name(&self) -> &'static str {
        "image_ocr"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        check_binary(
            "tesseract",
            "Install Tesseract OCR (e.g. `apt install tesseract-ocr` on Debian/Ubuntu, \
             `brew install tesseract` on macOS, or download from \
             https://github.com/tesseract-ocr/tesseract).",
        )?;

        let tmp = make_temp_dir()?;
        let ext = input_extension(input, "png");
        let filename = format!("input.{ext}");
        let img_path = tmp.path().join(&filename);
        write_temp_file(tmp.path(), &filename, &input.data)?;

        let stdout = run_subprocess_to_stdout(
            "tesseract",
            &[
                img_path.to_str().unwrap(),
                "stdout",
                "-l",
                "eng",
            ],
        )?;

        let text = String::from_utf8_lossy(&stdout).to_string();

        Ok(vec![RawDoc {
            text,
            page: None,
            timestamp_range: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// Audio/Video extractor (ffmpeg + whisper.cpp subprocess)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AudioVideoExtractor;

impl AudioVideoExtractor {
    fn check_whisper() -> anyhow::Result<()> {
        // Check for whisper.cpp CLI (the `main` binary or `whisper-cli`)
        let candidates = &["whisper-cli", "whisper", "main"];
        for cmd in candidates {
            if std::process::Command::new(cmd)
                .arg("--help")
                .output()
                .is_ok()
            {
                return Ok(());
            }
        }
        anyhow::bail!(
            "whisper.cpp CLI not found. Install whisper.cpp (https://github.com/ggerganov/whisper.cpp) \
             and ensure the binary is in PATH, or set WHISPER_CLI env var."
        );
    }

    fn whisper_cli() -> String {
        std::env::var("WHISPER_CLI").unwrap_or_else(|_| "whisper-cli".to_string())
    }
}

#[async_trait]
impl Extractor for AudioVideoExtractor {
    fn name(&self) -> &'static str {
        "audio_video"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        Self::check_whisper()?;

        let extension = match &input.source {
            Source::File(p) => p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("wav")
                .to_lowercase(),
            Source::Url(u) => {
                let path = u.split('?').next().unwrap_or(u);
                Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("wav")
                    .to_lowercase()
            }
            Source::Text(_) => {
                anyhow::bail!("audio/video extractor requires a file or URL source")
            }
        };

        // Convert to WAV if needed
        let wav_data = if extension == "wav" {
            input.data.clone()
        } else {
            check_binary(
                "ffmpeg",
                "Install ffmpeg (e.g. `apt install ffmpeg` on Debian/Ubuntu, \
                 `brew install ffmpeg` on macOS, or download from https://ffmpeg.org/).",
            )?;

            let tmp = make_temp_dir()?;
            let in_name = format!("input.{extension}");
            let out_name = "output.wav".to_string();
            let in_path = tmp.path().join(&in_name);
            let out_path = tmp.path().join(&out_name);
            write_temp_file(tmp.path(), &in_name, &input.data)?;

            run_subprocess_to_file(
                "ffmpeg",
                &[
                    "-y",
                    "-i",
                    in_path.to_str().unwrap(),
                    "-ar",
                    "16000",
                    "-ac",
                    "1",
                    "-sample_fmt",
                    "s16",
                    out_path.to_str().unwrap(),
                ],
                &out_path,
            )?
        };

        // Write WAV for whisper and transcribe
        let tmp = make_temp_dir()?;
        let wav_name = "input.wav".to_string();
        let wav_path = tmp.path().join(&wav_name);
        write_temp_file(tmp.path(), &wav_name, &wav_data)?;

        let model_path = std::env::var("WHISPER_MODEL_PATH")
            .unwrap_or_else(|_| "models/ggml-base.en.bin".to_string());

        let cli = Self::whisper_cli();
        let output = std::process::Command::new(&cli)
            .arg("-m")
            .arg(&model_path)
            .arg("-f")
            .arg(&wav_path)
            .arg("-otxt")   // output as plain text
            .arg("-of")
            .arg(tmp.path().join("output")) // output file prefix
            .output()
            .with_context(|| format!("failed to run whisper CLI '{cli}'"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Try reading the .txt output file as fallback
            let txt_path = tmp.path().join("output.txt");
            if let Ok(text) = std::fs::read_to_string(&txt_path) {
                return Ok(vec![RawDoc {
                    text: text.trim().to_string(),
                    page: None,
                    timestamp_range: None,
                }]);
            }
            anyhow::bail!("whisper transcription failed: {stderr}");
        }

        // whisper.cpp writes to output.txt alongside the prefix
        let txt_path = tmp.path().join("output.txt");
        let text = std::fs::read_to_string(&txt_path)
            .context("whisper completed but output .txt not found")?;

        Ok(vec![RawDoc {
            text: text.trim().to_string(),
            page: None,
            timestamp_range: None,
        }])
    }
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Select the appropriate extractor based on MIME type or file extension.
pub fn dispatcher(content_type: &str) -> anyhow::Result<Box<dyn Extractor>> {
    let ct = content_type.to_lowercase();

    if ct == "text/plain" || ct == "text/markdown" {
        return Ok(Box::new(PlainTextExtractor));
    }
    if ct == "application/pdf" {
        return Ok(Box::new(PdfExtractor));
    }
    if ct == "application/vnd.openxmlformats-officedocument.wordprocessingml.document" {
        return Ok(Box::new(DocxExtractor));
    }
    if ct == "text/html" {
        return Ok(Box::new(HtmlExtractor));
    }
    if ct.starts_with("image/") {
        return Ok(Box::new(ImageExtractor));
    }
    if ct.starts_with("audio/") || ct.starts_with("video/") {
        return Ok(Box::new(AudioVideoExtractor));
    }

    anyhow::bail!("unsupported content type: {content_type}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_plain_text_extractor() {
        let input = Input {
            source: Source::Text("hello".to_string()),
            content_type: "text/plain".to_string(),
            data: b"Hello, world!".to_vec(),
        };
        let extractor = PlainTextExtractor;
        let docs = extractor.extract(&input).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].text, "Hello, world!");
        assert_eq!(extractor.name(), "plaintext");
    }

    #[tokio::test]
    async fn test_markdown_extractor() {
        let input = Input {
            source: Source::File(std::path::PathBuf::from("doc.md")),
            content_type: "text/markdown".to_string(),
            data: b"# Title\n\nSome **bold** text.".to_vec(),
        };
        let extractor = PlainTextExtractor;
        let docs = extractor.extract(&input).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].text.contains("Title"));
        assert!(docs[0].text.contains("bold"));
    }

    #[test]
    fn test_dispatcher_plain_text() {
        let ext = dispatcher("text/plain").unwrap();
        assert_eq!(ext.name(), "plaintext");
    }

    #[test]
    fn test_dispatcher_pdf() {
        let ext = dispatcher("application/pdf").unwrap();
        assert_eq!(ext.name(), "pdf");
    }

    #[test]
    fn test_dispatcher_docx() {
        let ext = dispatcher(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
        .unwrap();
        assert_eq!(ext.name(), "docx");
    }

    #[test]
    fn test_dispatcher_html() {
        let ext = dispatcher("text/html").unwrap();
        assert_eq!(ext.name(), "html");
    }

    #[test]
    fn test_dispatcher_image() {
        let ext = dispatcher("image/png").unwrap();
        assert_eq!(ext.name(), "image_ocr");
    }

    #[test]
    fn test_dispatcher_audio() {
        let ext = dispatcher("audio/mp3").unwrap();
        assert_eq!(ext.name(), "audio_video");
    }

    #[test]
    fn test_dispatcher_video() {
        let ext = dispatcher("video/mp4").unwrap();
        assert_eq!(ext.name(), "audio_video");
    }

    #[test]
    fn test_dispatcher_unsupported() {
        let err = match dispatcher("application/x-foobar") {
            Err(e) => e,
            Ok(_) => panic!("expected error"),
        };
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn test_dispatcher_case_insensitive() {
        let ext = dispatcher("APPLICATION/PDF").unwrap();
        assert_eq!(ext.name(), "pdf");
    }

    /// Test the form-feed split logic used for per-page PDF text.
    /// This validates that the algorithm correctly assigns page numbers
    /// when text is separated by \x0C characters.
    #[test]
    fn test_pdf_formfeed_split_pages() {
        // Simulate pdftotext output with form-feed page separators.
        // pdftotext places \x0C after each page, including the last.
        let text = "Page one content.\x0CPage two content.\x0CPage three content.\x0C";

        let pages: Vec<&str> = text.split('\x0C').collect();
        let pages: Vec<&str> = pages.into_iter().filter(|p| !p.is_empty()).collect();

        assert_eq!(pages.len(), 3, "should split into 3 non-empty pages");
        assert!(pages[0].contains("Page one"), "page 0 should contain 'Page one'");
        assert!(pages[1].contains("Page two"), "page 1 should contain 'Page two'");
        assert!(pages[2].contains("Page three"), "page 2 should contain 'Page three'");
    }

    #[test]
    fn test_pdf_formfeed_single_page() {
        // Single page PDF — no form-feed
        let text = "Just one page of text.";
        let pages: Vec<&str> = text.split('\x0C').collect();
        let pages: Vec<&str> = pages.into_iter().filter(|p| !p.is_empty()).collect();

        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0], "Just one page of text.");
    }

    #[test]
    fn test_pdf_formfeed_empty_text() {
        // Empty text (e.g. blank PDF)
        let text = "";
        let pages: Vec<&str> = text.split('\x0C').collect();
        let pages: Vec<&str> = pages.into_iter().filter(|p| !p.is_empty()).collect();

        assert_eq!(pages.len(), 0, "empty text should produce zero pages");
    }
}