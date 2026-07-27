//! Extractor trait, per-format implementations, and MIME-based dispatcher.
//!
//! Each format gets its own extractor that implements the [`Extractor`] trait.
//! The [`dispatcher`] function selects the right extractor by MIME type / file
//! extension.

use std::path::{Path, PathBuf};

use anyhow::Context;
use async_trait::async_trait;
use tempfile::TempDir;
// ---------------------------------------------------------------------------
// CommandRunner trait — injectable for testing subprocess-based extractors
// ---------------------------------------------------------------------------

/// Abstraction over running external commands.
///
/// The real implementation calls [`std::process::Command`]. A test/mock impl
/// can return canned results without needing real binaries installed.
pub trait CommandRunner: Send + Sync + std::fmt::Debug {
    /// Run `binary` with `args`, return its stdout on success.
    fn run_to_stdout(&self, binary: &str, args: &[&str]) -> anyhow::Result<Vec<u8>>;

    /// Run `binary` with `args`, then read the file at `output_path`.
    fn run_to_file(
        &self,
        binary: &str,
        args: &[&str],
        output_path: &Path,
    ) -> anyhow::Result<Vec<u8>>;

    /// Run `binary` with `args`, read the file at `output_path`, return
    /// `(exit_code, file_content, stderr)` regardless of exit status.
    ///
    /// Unlike [`run_to_file`], this does **not** bail on non-zero exit — it
    /// returns the exit code plus whatever was written to disk. Useful for
    /// tools that may exit non-zero but still produce usable output (e.g.
    /// whisper.cpp with early audio end).
    fn run_to_file_salvage(
        &self,
        binary: &str,
        args: &[&str],
        output_path: &Path,
    ) -> anyhow::Result<(i32, Vec<u8>, Vec<u8>)>;

    /// Check whether `binary` exists and can be invoked.
    fn check_binary(&self, binary: &str, install_hint: &str) -> anyhow::Result<()>;
}

/// Real [`CommandRunner`] that shells out to system binaries.
#[derive(Debug, Default)]
pub struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run_to_stdout(&self, binary: &str, args: &[&str]) -> anyhow::Result<Vec<u8>> {
        tracing::debug!("running {} with {} arg(s)", binary, args.len());
        let output = std::process::Command::new(binary)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {binary}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("{} exited with non-zero status; stderr: {}", binary, stderr.trim());
            anyhow::bail!("{binary} failed: {stderr}");
        }
        tracing::debug!("{} returned {} bytes on stdout", binary, output.stdout.len());
        Ok(output.stdout)
    }

    fn run_to_file(
        &self,
        binary: &str,
        args: &[&str],
        output_path: &Path,
    ) -> anyhow::Result<Vec<u8>> {
        tracing::debug!("running {} {} arg(s) -> {:?}", binary, args.len(), output_path);
        let output = std::process::Command::new(binary)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {binary}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!("{} exited with non-zero status; stderr: {}", binary, stderr.trim());
            anyhow::bail!("{binary} failed: {stderr}");
        }
        let content = std::fs::read(output_path)
            .with_context(|| format!("failed to read {binary} output file"))?;
        tracing::debug!("{} wrote {} bytes to {:?}", binary, content.len(), output_path);
        Ok(content)
    }

    fn run_to_file_salvage(
        &self,
        binary: &str,
        args: &[&str],
        output_path: &Path,
    ) -> anyhow::Result<(i32, Vec<u8>, Vec<u8>)> {
        tracing::debug!(
            "running {} {} arg(s) -> {:?} (salvage mode)",
            binary,
            args.len(),
            output_path
        );
        let output = std::process::Command::new(binary)
            .args(args)
            .output()
            .with_context(|| format!("failed to run {binary}"))?;
        let exit_code = output.status.code().unwrap_or(-1);
        let stderr = output.stderr;
        let file_content = std::fs::read(output_path).unwrap_or_default();
        tracing::debug!(
            "{} exited with {}; {} bytes on disk, {} bytes stderr",
            binary,
            exit_code,
            file_content.len(),
            stderr.len()
        );
        Ok((exit_code, file_content, stderr))
    }

    fn check_binary(&self, binary: &str, install_hint: &str) -> anyhow::Result<()> {
        let output = std::process::Command::new(binary)
            .arg("--version")
            .output()
            .with_context(|| format!("{binary} not found. {install_hint}"))?;
        if !output.status.success() {
            tracing::warn!("{} --version returned non-zero; stderr: {}", binary, String::from_utf8_lossy(&output.stderr));
            anyhow::bail!("{binary} --version returned non-zero status");
        }
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        tracing::info!("{} found: {}", binary, version);
        Ok(())
    }
}

/// Mock [`CommandRunner`] for use in tests.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct MockCommandRunner {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: i32,
    /// Override exit code for `run_to_file` / `run_to_file_salvage` only.
    /// When set, `run_to_file` and `run_to_file_salvage` use this value
    /// instead of `exit_code` (which is used by `run_to_stdout` /
    /// `check_binary`). This lets tests have `check_whisper` pass (via
    /// `exit_code=0`) while the transcription call fails (via
    /// `exit_code_salvage=Some(1)`).
    pub(crate) exit_code_salvage: Option<i32>,
    pub(crate) binary_missing: bool,
    pub(crate) output_file_content: Option<Vec<u8>>,
    pub(crate) output_file_missing: bool,
}

#[cfg(test)]
impl MockCommandRunner {
    pub(crate) fn new() -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: 0,
            exit_code_salvage: None,
            binary_missing: false,
            output_file_content: None,
            output_file_missing: false,
        }
    }

    pub(crate) fn with_exit_code(mut self, code: i32) -> Self {
        self.exit_code = code;
        self
    }

    pub(crate) fn with_exit_code_salvage(mut self, code: i32) -> Self {
        self.exit_code_salvage = Some(code);
        self
    }

    pub(crate) fn with_stderr(mut self, err: &[u8]) -> Self {
        self.stderr = err.to_vec();
        self
    }

    pub(crate) fn with_stdout(mut self, out: &[u8]) -> Self {
        self.stdout = out.to_vec();
        self
    }

    pub(crate) fn with_binary_missing(mut self) -> Self {
        self.binary_missing = true;
        self
    }

    pub(crate) fn with_output_file(mut self, content: &[u8]) -> Self {
        self.output_file_content = Some(content.to_vec());
        self
    }

    pub(crate) fn with_output_file_missing(mut self) -> Self {
        self.output_file_missing = true;
        self
    }
}

#[cfg(test)]
impl CommandRunner for MockCommandRunner {
    fn run_to_stdout(&self, _binary: &str, _args: &[&str]) -> anyhow::Result<Vec<u8>> {
        if self.binary_missing {
            anyhow::bail!("binary not found (mock)")
        }
        if self.exit_code != 0 {
            let stderr = String::from_utf8_lossy(&self.stderr);
            anyhow::bail!("mock binary failed: {stderr}");
        }
        Ok(self.stdout.clone())
    }

    fn run_to_file(
        &self,
        binary: &str,
        _args: &[&str],
        output_path: &Path,
    ) -> anyhow::Result<Vec<u8>> {
        if self.binary_missing {
            anyhow::bail!("binary not found (mock)")
        }
        let ec = self.exit_code_salvage.unwrap_or(self.exit_code);
        if ec != 0 {
            let stderr = String::from_utf8_lossy(&self.stderr);
            anyhow::bail!("{binary} failed: {stderr}");
        }
        if self.output_file_missing {
            anyhow::bail!("failed to read {binary} output file")
        }
        // Write the mock content to the real temp file so the extractor
        // can read it back with std::fs::read_to_string.
        if let Some(content) = &self.output_file_content {
            std::fs::write(output_path, content)
                .with_context(|| format!("mock failed to write {binary} output"))?;
            Ok(content.clone())
        } else {
            Ok(Vec::new())
        }
    }

    fn check_binary(&self, _binary: &str, install_hint: &str) -> anyhow::Result<()> {
        if self.binary_missing {
            anyhow::bail!("binary not found. {install_hint}");
        }
        Ok(())
    }

    fn run_to_file_salvage(
        &self,
        binary: &str,
        _args: &[&str],
        output_path: &Path,
    ) -> anyhow::Result<(i32, Vec<u8>, Vec<u8>)> {
        if self.binary_missing {
            anyhow::bail!("binary not found (mock)")
        }
        let ec = self.exit_code_salvage.unwrap_or(self.exit_code);
        let stderr = self.stderr.clone();
        if self.output_file_missing && ec != 0 {
            // No output file + non-zero exit means total failure.
            return Ok((ec, Vec::new(), stderr));
        }
        // Write mock content to the real temp file.
        if let Some(content) = &self.output_file_content {
            std::fs::write(output_path, content)
                .with_context(|| format!("mock failed to write {binary} output"))?;
            Ok((ec, content.clone(), stderr))
        } else {
            Ok((ec, Vec::new(), stderr))
        }
    }
}

/// Maximum bytes for file-based inputs (DOCX, PDF, images, audio/video).
/// Files exceeding this limit are rejected before any extraction work.
/// Shares value with [`MAX_DOWNLOAD_SIZE`] to keep a consistent pipeline-wide cap.
pub(crate) const MAX_FILE_SIZE: usize = crate::ingestion::MAX_DOWNLOAD_SIZE;

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
    /// `true` when text was salvaged from a failed subprocess (e.g. whisper
    /// exited non-zero but wrote partial output). Downstream code can use this
    /// to flag the content as potentially incomplete or garbage.
    pub salvaged: bool,
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
            salvaged: false,
        }])
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Create a temporary directory with the standard prefix.
fn make_temp_dir() -> anyhow::Result<TempDir> {
    TempDir::new().context("failed to create temp dir")
}

/// Write `data` to `dir / filename`, returning the full path.
fn write_temp_file(dir: &Path, filename: &str, data: &[u8]) -> anyhow::Result<()> {
    std::fs::write(dir.join(filename), data)
        .with_context(|| format!("failed to write temp file '{filename}'"))
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
struct PdfExtractor {
    runner: Box<dyn CommandRunner>,
}

impl PdfExtractor {
    fn new() -> Self {
        Self {
            runner: Box::new(RealCommandRunner),
        }
    }

    fn with_runner(runner: Box<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Extractor for PdfExtractor {
    fn name(&self) -> &'static str {
        "pdf"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        if input.data.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "PDF file size ({} bytes) exceeds maximum allowed ({} bytes)",
                input.data.len(),
                MAX_FILE_SIZE,
            );
        }
        self.runner.check_binary(
            "pdftotext",
            "Install poppler-utils (e.g. `apt install poppler-utils` on Debian/Ubuntu, \
             `brew install poppler` on macOS, or download from https://poppler.freedesktop.org/).",
        )?;

        let tmp = make_temp_dir()?;
        let input_path = tmp.path().join("input.pdf");
        let output_path = tmp.path().join("output.txt");
        write_temp_file(tmp.path(), "input.pdf", &input.data)?;

        self.runner.run_to_file(
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
            tracing::info!("pdftotext extracted text but no pages found (no form-feed separators)");
            return Ok(vec![RawDoc {
                text: text.clone(),
                page: None,
                timestamp_range: None,
                salvaged: false,
            }]);
        }

        tracing::info!("pdftotext extracted {} pages, {} total chars", pages.len(), text.len());
        Ok(pages.into_iter().enumerate().map(|(i, page_text)| {
            RawDoc {
                text: page_text.trim().to_string(),
                page: Some(i as u32),
                timestamp_range: None,
                salvaged: false,
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
        if input.data.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "DOCX file size ({} bytes) exceeds maximum allowed ({} bytes)",
                input.data.len(),
                MAX_FILE_SIZE,
            );
        }
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
            salvaged: false,
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
            salvaged: false,
        }])
    }
}

// ---------------------------------------------------------------------------
// Image OCR extractor (tesseract subprocess)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct ImageExtractor {
    runner: Box<dyn CommandRunner>,
}

impl ImageExtractor {
    fn new() -> Self {
        Self {
            runner: Box::new(RealCommandRunner),
        }
    }

    fn with_runner(runner: Box<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Extractor for ImageExtractor {
    fn name(&self) -> &'static str {
        "image_ocr"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        if input.data.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "Image file size ({} bytes) exceeds maximum allowed ({} bytes)",
                input.data.len(),
                MAX_FILE_SIZE,
            );
        }
        self.runner.check_binary(
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

        let stdout = self.runner.run_to_stdout(
            "tesseract",
            &[
                img_path.to_str().unwrap(),
                "stdout",
                "-l",
                "eng",
            ],
        )?;

        let text = String::from_utf8_lossy(&stdout).to_string();
        let trimmed = text.trim();
        let src = match &input.source {
            Source::File(p) => p.to_string_lossy().to_string(),
            Source::Url(u) => u.clone(),
            Source::Text(_) => "<text input>".to_string(),
        };
        tracing::info!("tesseract OCR extracted {} chars from {}", trimmed.len(), src);

        Ok(vec![RawDoc {
            text,
            page: None,
            timestamp_range: None,
            salvaged: false,
        }])
    }
}

// ---------------------------------------------------------------------------
// Audio/Video extractor (ffmpeg + whisper.cpp subprocess)
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct AudioVideoExtractor {
    runner: Box<dyn CommandRunner>,
}

impl AudioVideoExtractor {
    fn new() -> Self {
        Self {
            runner: Box::new(RealCommandRunner),
        }
    }

    fn with_runner(runner: Box<dyn CommandRunner>) -> Self {
        Self { runner }
    }

    fn check_whisper(&self) -> anyhow::Result<()> {
        // Check for whisper.cpp CLI (the `main` binary or `whisper-cli`)
        let candidates = &["whisper-cli", "whisper", "main"];
        for cmd in candidates {
            if self.runner.run_to_stdout(cmd, &["--help"]).is_ok() {
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

    /// Resolve the path to the whisper model file.
    ///
    /// Priority (first wins):
    /// 1. `WHISPER_MODEL_PATH` env var
    /// 2. Platform-specific data directory:
    ///    - Windows: `%APPDATA%/echo/models/ggml-base.en.bin`
    ///    - Unix:    `$XDG_DATA_HOME/echo/models/ggml-base.en.bin`
    ///               (defaults to `~/.local/share/echo/models/ggml-base.en.bin`)
    /// 3. CWD fallback: `models/ggml-base.en.bin` (with a warning since it's fragile)
    fn resolve_whisper_model() -> String {
        // Priority 1: explicit env var
        if let Ok(path) = std::env::var("WHISPER_MODEL_PATH") {
            if !path.is_empty() {
                return path;
            }
        }

        // Priority 2: platform data directory
        let data_dir = if cfg!(target_os = "windows") {
            std::env::var("APPDATA")
                .ok()
                .map(|a| PathBuf::from(a).join("echo"))
        } else {
            std::env::var("XDG_DATA_HOME")
                .ok()
                .map(|x| PathBuf::from(x).join("echo"))
                .or_else(|| {
                    std::env::var("HOME")
                        .ok()
                        .map(|h| PathBuf::from(h).join(".local").join("share").join("echo"))
                })
        };

        if let Some(dir) = data_dir {
            let model = dir.join("models").join("ggml-base.en.bin");
            if model.exists() {
                return model.to_string_lossy().to_string();
            }
        }

        // Priority 3: CWD fallback (fragile but preserves current behavior)
        let cwd_fallback = PathBuf::from("models").join("ggml-base.en.bin");
        tracing::warn!(
            "WHISPER_MODEL_PATH not set; falling back to CWD-relative path {:?}. \
             Set WHISPER_MODEL_PATH to a model file, or place the model at the \
             platform data directory.",
            cwd_fallback
        );
        cwd_fallback.to_string_lossy().to_string()
    }
}

#[async_trait]
impl Extractor for AudioVideoExtractor {
    fn name(&self) -> &'static str {
        "audio_video"
    }

    async fn extract(&self, input: &Input) -> anyhow::Result<Vec<RawDoc>> {
        if input.data.len() > MAX_FILE_SIZE {
            anyhow::bail!(
                "Audio/video file size ({} bytes) exceeds maximum allowed ({} bytes)",
                input.data.len(),
                MAX_FILE_SIZE,
            );
        }
        self.check_whisper()?;

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
            tracing::info!("audio is already WAV format, skipping ffmpeg conversion");
            input.data.clone()
        } else {
            tracing::info!("converting audio/video (.{}) to WAV via ffmpeg", extension);
            self.runner.check_binary(
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

            let wav = self.runner.run_to_file(
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
            )?;
            tracing::info!("ffmpeg produced {} bytes of WAV audio", wav.len());
            wav
        };
        let tmp = make_temp_dir()?;
        let wav_name = "input.wav".to_string();
        let wav_path = tmp.path().join(&wav_name);
        write_temp_file(tmp.path(), &wav_name, &wav_data)?;

        let model_path = Self::resolve_whisper_model();

        tracing::info!("transcribing via {} with model {}", Self::whisper_cli(), model_path);
        let cli = Self::whisper_cli();
        let txt_path = tmp.path().join("output.txt");
        let (exit_code, file_bytes, stderr) = self.runner.run_to_file_salvage(
            &cli,
            &[
                "-m",
                &model_path,
                "-f",
                wav_path.to_str().unwrap(),
                "-otxt",
                "-of",
                tmp.path().join("output").to_str().unwrap(),
            ],
            &txt_path,
        )?;

        if exit_code != 0 {
            let stderr_str = String::from_utf8_lossy(&stderr);
            if !file_bytes.is_empty() {
                tracing::warn!(
                    "whisper exited with {exit_code} but partial output found ({} bytes); \
                     result flagged as salvaged",
                    file_bytes.len()
                );
                let text = String::from_utf8_lossy(&file_bytes);
                return Ok(vec![RawDoc {
                    text: text.trim().to_string(),
                    page: None,
                    timestamp_range: None,
                    salvaged: true,
                }]);
            }
            tracing::warn!("whisper transcription failed: {}", stderr_str.trim());
            anyhow::bail!("whisper transcription failed: {stderr_str}");
        }

        let text = String::from_utf8_lossy(&file_bytes);
        let trimmed = text.trim();
        tracing::info!("whisper transcribed {} chars", trimmed.len());

        Ok(vec![RawDoc {
            text: trimmed.to_string(),
            page: None,
            timestamp_range: None,
            salvaged: false,
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
        return Ok(Box::new(PdfExtractor::new()));
    }
    if ct == "application/vnd.openxmlformats-officedocument.wordprocessingml.document" {
        return Ok(Box::new(DocxExtractor));
    }
    if ct == "text/html" {
        return Ok(Box::new(HtmlExtractor));
    }
    if ct.starts_with("image/") {
        return Ok(Box::new(ImageExtractor::new()));
    }
    if ct.starts_with("audio/") || ct.starts_with("video/") {
        return Ok(Box::new(AudioVideoExtractor::new()));
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

    // ---------------------------------------------------------------------------
    // File size guard tests
    // ---------------------------------------------------------------------------

    /// Build an input with data exceeding MAX_FILE_SIZE.
    fn oversized_input(content_type: &str) -> Input {
        Input {
            source: Source::File(PathBuf::from("oversized.bin")),
            content_type: content_type.to_string(),
            data: vec![0u8; super::MAX_FILE_SIZE + 1],
        }
    }

    #[tokio::test]
    async fn test_docx_rejects_oversized() {
        let input = oversized_input(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        );
        let ext = DocxExtractor;
        let err = ext.extract(&input).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed"));
    }

    #[tokio::test]
    async fn test_docx_accepts_normal_size() {
        // Without a guard, a normal DOCX mock would need real DOCX bytes.
        // Just verify that data at MAX_FILE_SIZE passes the guard for docx.
        // (docx_rs will then fail with a parse error, which is fine.)
        let input = Input {
            source: Source::File(PathBuf::from("normal.docx")),
            content_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                .to_string(),
            data: vec![0u8; 1], // well under limit
        };
        let ext = DocxExtractor;
        let err = ext.extract(&input).await.unwrap_err();
        assert!(!err.to_string().contains("exceeds maximum allowed"));
    }

    #[tokio::test]
    async fn test_pdf_rejects_oversized() {
        let input = oversized_input("application/pdf");
        let ext = PdfExtractor::new();
        let err = ext.extract(&input).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed"));
    }

    #[tokio::test]
    async fn test_image_rejects_oversized() {
        let input = oversized_input("image/png");
        let ext = ImageExtractor::new();
        let err = ext.extract(&input).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed"));
    }

    #[tokio::test]
    async fn test_audio_rejects_oversized() {
        let input = oversized_input("audio/mp3");
        let ext = AudioVideoExtractor::new();
        let err = ext.extract(&input).await.unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed"));
    }

    // -----------------------------------------------------------------------
    // Mock-based subprocess extractor tests
    // -----------------------------------------------------------------------

    fn pdf_input() -> Input {
        Input {
            source: Source::File(PathBuf::from("test.pdf")),
            content_type: "application/pdf".to_string(),
            data: b"fake pdf bytes".to_vec(),
        }
    }

    fn image_input() -> Input {
        Input {
            source: Source::File(PathBuf::from("test.png")),
            content_type: "image/png".to_string(),
            data: b"fake png bytes".to_vec(),
        }
    }

    fn audio_input() -> Input {
        Input {
            source: Source::File(PathBuf::from("test.mp3")),
            content_type: "audio/mp3".to_string(),
            data: b"fake audio bytes".to_vec(),
        }
    }

    fn wav_input() -> Input {
        Input {
            source: Source::File(PathBuf::from("test.wav")),
            content_type: "audio/wav".to_string(),
            data: b"fake wav".to_vec(),
        }
    }

    #[tokio::test]
    async fn test_pdf_binary_missing() {
        let ext = PdfExtractor::with_runner(Box::new(MockCommandRunner::new().with_binary_missing()));
        let err = ext.extract(&pdf_input()).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_pdf_subprocess_fails() {
        let ext = PdfExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_exit_code(1)
                .with_stderr(b"permission denied"),
        ));
        let err = ext.extract(&pdf_input()).await.unwrap_err();
        assert!(err.to_string().contains("failed"));
    }

    #[tokio::test]
    async fn test_pdf_output_file_missing() {
        let ext = PdfExtractor::with_runner(Box::new(
            MockCommandRunner::new().with_output_file_missing(),
        ));
        let err = ext.extract(&pdf_input()).await.unwrap_err();
        assert!(err.to_string().contains("failed to read"));
    }

    #[tokio::test]
    async fn test_pdf_single_page() {
        let ext = PdfExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_output_file(b"Single page content.\x0C"),
        ));
        let docs = ext.extract(&pdf_input()).await.unwrap();
        assert_eq!(docs.len(), 1, "single form-feed => one page");
        assert_eq!(docs[0].page, Some(0));
        assert!(docs[0].text.contains("Single page content"));
    }

    #[tokio::test]
    async fn test_pdf_multi_page() {
        let ext = PdfExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_output_file(b"Page one.\x0CPage two.\x0CPage three.\x0C"),
        ));
        let docs = ext.extract(&pdf_input()).await.unwrap();
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0].page, Some(0));
        assert_eq!(docs[1].page, Some(1));
        assert_eq!(docs[2].page, Some(2));
    }

    #[tokio::test]
    async fn test_image_binary_missing() {
        let ext = ImageExtractor::with_runner(Box::new(
            MockCommandRunner::new().with_binary_missing(),
        ));
        let err = ext.extract(&image_input()).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_image_subprocess_fails() {
        let ext = ImageExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_exit_code(1)
                .with_stderr(b"ocr error"),
        ));
        let err = ext.extract(&image_input()).await.unwrap_err();
        assert!(err.to_string().contains("failed"));
    }

    #[tokio::test]
    async fn test_image_extracts_text() {
        let ext = ImageExtractor::with_runner(Box::new(
            MockCommandRunner::new().with_stdout(b"Extracted OCR text."),
        ));
        let docs = ext.extract(&image_input()).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].text.contains("OCR text"));
    }

    #[tokio::test]
    async fn test_audio_binary_missing() {
        let ext = AudioVideoExtractor::with_runner(Box::new(
            MockCommandRunner::new().with_binary_missing(),
        ));
        let err = ext.extract(&audio_input()).await.unwrap_err();
        assert!(err.to_string().contains("Install Tesseract")  // ffmpeg check_binary hint
            || err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_audio_wav_passthrough() {
        // WAV input does not trigger ffmpeg, only whisper check
        // whisper check will fail because mock binary is missing
        let ext = AudioVideoExtractor::with_runner(Box::new(
            MockCommandRunner::new().with_binary_missing(),
        ));
        let err = ext.extract(&wav_input()).await.unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_audio_transcription_success() {
        // WAV input → skip ffmpeg → whisper via run_to_file_salvage
        // exit_code=0 passes check_whisper; exit_code_salvage=None → uses exit_code=0 → success
        let ext = AudioVideoExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_stdout(b"whisper help text") // for check_whisper
                .with_output_file(b"Transcribed text."),
        ));
        let docs = ext.extract(&wav_input()).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].text, "Transcribed text.");
        assert!(!docs[0].salvaged);
    }

    #[tokio::test]
    async fn test_audio_transcription_salvaged() {
        // Whisper exits non-zero but wrote partial output
        let ext = AudioVideoExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_stdout(b"whisper help text") // passes check_whisper
                .with_exit_code_salvage(1)         // run_to_file_salvage returns non-zero
                .with_stderr(b"early audio end")
                .with_output_file(b"Partial transcribed text."),
        ));
        let docs = ext.extract(&wav_input()).await.unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].text, "Partial transcribed text.");
        assert!(docs[0].salvaged);
    }

    #[tokio::test]
    async fn test_audio_transcription_failed() {
        // Whisper exits non-zero AND no output file → total failure
        let ext = AudioVideoExtractor::with_runner(Box::new(
            MockCommandRunner::new()
                .with_stdout(b"whisper help text") // passes check_whisper
                .with_exit_code_salvage(1)         // run_to_file_salvage returns non-zero
                .with_stderr(b"model file not found")
                .with_output_file_missing(),
        ));
        let err = ext.extract(&wav_input()).await.unwrap_err();
        assert!(err.to_string().contains("whisper transcription failed"));
        assert!(err.to_string().contains("model file not found"));
    }
}