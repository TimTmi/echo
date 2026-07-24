//! Extractor trait, per-format implementations, and MIME-based dispatcher.
//!
//! Each format gets its own extractor that implements the [`Extractor`] trait.
//! The [`dispatcher`] function selects the right extractor by MIME type / file
//! extension.

use std::path::PathBuf;

use anyhow::Context;
use async_trait::async_trait;

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
pub trait Extractor: Send + Sync + std::fmt::Debug {
    /// Return the extractor name (e.g. `"pdf"`, `"plaintext"`).
    fn name(&self) -> &'static str;

    /// Extract plain text from the given input.
    ///
    /// # Errors
    ///
    /// Returns an error if the input cannot be parsed, if a required system
    /// dependency (e.g. `pdftotext`) is not found, or if the input is empty
    /// and the extractor requires non-empty data.
    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc>;
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

    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc> {
        let text = String::from_utf8_lossy(&input.data).to_string();
        Ok(RawDoc {
            text,
            page: None,
            timestamp_range: None,
        })
    }
}

// ---------------------------------------------------------------------------
// PDF extractor (pdftotext subprocess)
// ---------------------------------------------------------------------------

/// PDF extractor — shells out to `pdftotext` (poppler-utils).
#[derive(Debug)]
struct PdfExtractor;

impl PdfExtractor {
    fn check_pdftotext() -> anyhow::Result<()> {
        let output = std::process::Command::new("pdftotext")
            .arg("--version")
            .output()
            .context(
                "pdftotext not found. Install poppler-utils (e.g. `apt install poppler-utils` \
                 on Debian/Ubuntu, `brew install poppler` on macOS, or download from \
                 https://poppler.freedesktop.org/).",
            )?;
        if !output.status.success() {
            anyhow::bail!("pdftotext --version returned non-zero status");
        }
        Ok(())
    }
}

#[async_trait]
impl Extractor for PdfExtractor {
    fn name(&self) -> &'static str {
        "pdf"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc> {
        Self::check_pdftotext()?;

        let mut temp_path = std::env::temp_dir();
        temp_path.push(format!("echo_pdf_{}.pdf", std::process::id()));
        std::fs::write(&temp_path, &input.data).context("failed to write temp PDF")?;

        let output_path = temp_path.with_extension("txt");

        let status = std::process::Command::new("pdftotext")
            .arg(&temp_path)
            .arg(&output_path)
            .output()
            .context("failed to run pdftotext")?;

        let _ = std::fs::remove_file(&temp_path);

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            let _ = std::fs::remove_file(&output_path);
            anyhow::bail!("pdftotext failed: {stderr}");
        }

        let text = std::fs::read_to_string(&output_path)
            .context("failed to read pdftotext output")?;
        let _ = std::fs::remove_file(&output_path);

        let page_count = text.matches('\x0C').count() as u32;

        Ok(RawDoc {
            text,
            page: if page_count > 0 { Some(page_count - 1) } else { None },
            timestamp_range: None,
        })
    }
}

// ---------------------------------------------------------------------------
// DOCX extractor (docx-rs)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// HTML extractor (reqwest + html2text)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct HtmlExtractor;

impl HtmlExtractor {
    async fn ensure_data(input: &Input) -> anyhow::Result<Vec<u8>> {
        if !input.data.is_empty() {
            return Ok(input.data.clone());
        }
        match &input.source {
            Source::Url(url) => {
                let client = reqwest::Client::new();
                let response = client
                    .get(url)
                    .send()
                    .await
                    .context(format!("failed to fetch URL: {url}"))?;
                if !response.status().is_success() {
                    anyhow::bail!("HTTP {} when fetching {url}", response.status());
                }
                let bytes = response.bytes().await?;
                Ok(bytes.to_vec())
            }
            Source::File(p) => {
                std::fs::read(p).context(format!("failed to read file: {}", p.display()))
            }
            Source::Text(_) => anyhow::bail!("HTML extractor requires a file or URL source"),
        }
    }
}

#[async_trait]
impl Extractor for HtmlExtractor {
    fn name(&self) -> &'static str {
        "html"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc> {
        let data = Self::ensure_data(input).await?;
        let html = String::from_utf8_lossy(&data);
        let text = html2text::from_read(html.as_bytes(), 80)
            .context("html2text conversion failed")?;
        Ok(RawDoc {
            text,
            page: None,
            timestamp_range: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Image OCR extractor (tesseract subprocess)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ImageExtractor;

impl ImageExtractor {
    fn check_tesseract() -> anyhow::Result<()> {
        let output = std::process::Command::new("tesseract")
            .arg("--version")
            .output()
            .context(
                "tesseract not found. Install Tesseract OCR (e.g. `apt install tesseract-ocr` \
                 on Debian/Ubuntu, `brew install tesseract` on macOS, or download from \
                 https://github.com/tesseract-ocr/tesseract).",
            )?;
        if !output.status.success() {
            anyhow::bail!("tesseract --version returned non-zero status");
        }
        Ok(())
    }
}

#[async_trait]
impl Extractor for ImageExtractor {
    fn name(&self) -> &'static str {
        "image_ocr"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc> {
        Self::check_tesseract()?;

        let mut temp_path = std::env::temp_dir();
        temp_path.push(format!("echo_ocr_{}", std::process::id()));

        // Detect extension from source
        let ext = match &input.source {
            Source::File(p) => p.extension().and_then(|e| e.to_str()).unwrap_or("png"),
            Source::Url(u) => {
                let path = u.split('?').next().unwrap_or(u);
                std::path::Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("png")
            }
            Source::Text(_) => "png",
        };

        let img_path = temp_path.with_extension(ext);
        std::fs::write(&img_path, &input.data).context("failed to write temp image")?;

        // tesseract outputs to a file with _out suffix; specify stdout mode
        let output = std::process::Command::new("tesseract")
            .arg(&img_path)
            .arg("stdout")
            .arg("-l")
            .arg("eng")
            .output()
            .context("failed to run tesseract")?;

        let _ = std::fs::remove_file(&img_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("tesseract failed: {stderr}");
        }

        let text = String::from_utf8_lossy(&output.stdout).to_string();

        Ok(RawDoc {
            text,
            page: None,
            timestamp_range: None,
        })
    }
}

// ---------------------------------------------------------------------------
// Audio/Video extractor (ffmpeg + whisper.cpp subprocess)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AudioVideoExtractor;

impl AudioVideoExtractor {
    fn check_ffmpeg() -> anyhow::Result<()> {
        let output = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .context(
                "ffmpeg not found. Install ffmpeg (e.g. `apt install ffmpeg` on Debian/Ubuntu, \
                 `brew install ffmpeg` on macOS, or download from https://ffmpeg.org/).",
            )?;
        if !output.status.success() {
            anyhow::bail!("ffmpeg -version returned non-zero status");
        }
        Ok(())
    }

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

    fn convert_to_wav(data: &[u8], extension: &str) -> anyhow::Result<Vec<u8>> {
        Self::check_ffmpeg()?;

        let mut input_path = std::env::temp_dir();
        input_path.push(format!("echo_media_in_{}.{}", std::process::id(), extension));
        let mut output_path = std::env::temp_dir();
        output_path.push(format!("echo_media_out_{}.wav", std::process::id()));

        std::fs::write(&input_path, data).context("failed to write temp media file")?;

        let status = std::process::Command::new("ffmpeg")
            .arg("-y")
            .arg("-i")
            .arg(&input_path)
            .arg("-ar")
            .arg("16000")
            .arg("-ac")
            .arg("1")
            .arg("-sample_fmt")
            .arg("s16")
            .arg(&output_path)
            .output()
            .context("failed to run ffmpeg conversion")?;

        let _ = std::fs::remove_file(&input_path);

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            let _ = std::fs::remove_file(&output_path);
            anyhow::bail!("ffmpeg conversion failed: {stderr}");
        }

        let wav_data = std::fs::read(&output_path).context("failed to read WAV output")?;
        let _ = std::fs::remove_file(&output_path);
        Ok(wav_data)
    }
}

#[derive(Debug)]
struct DocxExtractor;

#[async_trait]
impl Extractor for DocxExtractor {
    fn name(&self) -> &'static str {
        "docx"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc> {
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

        Ok(RawDoc {
            text,
            page: None,
            timestamp_range: None,
        })
    }
}

#[async_trait]
impl Extractor for AudioVideoExtractor {
    fn name(&self) -> &'static str {
        "audio_video"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<RawDoc> {
        Self::check_whisper()?;

        let extension = match &input.source {
            Source::File(p) => p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("wav")
                .to_lowercase(),
            Source::Url(u) => {
                let path = u.split('?').next().unwrap_or(u);
                std::path::Path::new(path)
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("wav")
                    .to_lowercase()
            }
            Source::Text(_) => {
                anyhow::bail!("audio/video extractor requires a file or URL source")
            }
        };

        // Convert to WAV if needed, write to temp file
        let wav_data = if extension == "wav" {
            input.data.clone()
        } else {
            Self::convert_to_wav(&input.data, &extension)?
        };

        let mut wav_path = std::env::temp_dir();
        wav_path.push(format!("echo_whisper_in_{}.wav", std::process::id()));
        std::fs::write(&wav_path, &wav_data).context("failed to write temp WAV")?;

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
            .arg(&wav_path.with_extension("")) // output file prefix (wav_path without .wav)
            .output()
            .context(format!("failed to run whisper CLI '{cli}'"))?;

        let _ = std::fs::remove_file(&wav_path);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Try reading the .txt output file as fallback
            let txt_path = wav_path.with_extension("txt");
            if let Ok(text) = std::fs::read_to_string(&txt_path) {
                let _ = std::fs::remove_file(&txt_path);
                return Ok(RawDoc {
                    text: text.trim().to_string(),
                    page: None,
                    timestamp_range: None,
                });
            }
            let _ = std::fs::remove_file(&txt_path);
            anyhow::bail!("whisper transcription failed: {stderr}");
        }

        // whisper.cpp writes to a .txt file alongside the input
        let txt_path = wav_path.with_extension("txt");
        let text = std::fs::read_to_string(&txt_path)
            .context("whisper completed but output .txt not found")?;
        let _ = std::fs::remove_file(&txt_path);

        Ok(RawDoc {
            text: text.trim().to_string(),
            page: None,
            timestamp_range: None,
        })
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
        let doc = extractor.extract(&input).await.unwrap();
        assert_eq!(doc.text, "Hello, world!");
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
        let doc = extractor.extract(&input).await.unwrap();
        assert!(doc.text.contains("Title"));
        assert!(doc.text.contains("bold"));
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
        let err = dispatcher("application/x-foobar").unwrap_err();
        assert!(err.to_string().contains("unsupported"));
    }

    #[test]
    fn test_dispatcher_case_insensitive() {
        let ext = dispatcher("APPLICATION/PDF").unwrap();
        assert_eq!(ext.name(), "pdf");
    }
}