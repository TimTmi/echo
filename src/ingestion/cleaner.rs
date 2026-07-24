//! Text cleaning pipeline.
//!
//! Transforms raw extracted text into clean, normalized text suitable for
//! chunking and embedding. All operations are heuristic-based and stateless.

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

/// Clean raw extracted text by applying a series of normalization passes.
///
/// 1. Unicode normalization (NFC)
/// 2. De-hyphenation: remove `-\n` line-break artifacts from PDF extraction
/// 3. Collapse multiple whitespace characters into a single space
/// 4. Collapse multiple newlines into a single newline
/// 5. Trim leading/trailing whitespace
///
/// # Optional: Header/Footer stripping
///
/// If `repeated_lines` is provided, lines that appear at the same position
/// on every page (detected by frequency) are removed. Pass the raw text
/// through [`detect_repeated_lines`] first, then pass the resulting set
/// to [`clean_with_footer_strip`].
pub fn clean(raw: &str) -> String {
    let text = raw.nfc().collect::<String>();
    let text = dehyphenate(&text);
    let text = collapse_whitespace(&text);
    let text = collapse_newlines(&text);
    text.trim().to_string()
}

/// Remove PDF de-hyphenation artifacts: a letter, hyphen, newline, letter
/// — the hyphen is the line-break artifact.
fn dehyphenate(text: &str) -> String {
    let re = Regex::new(r"(\p{L})-\n(\p{L})").unwrap();
    re.replace_all(text, |caps: &regex::Captures| {
        format!("{}{}", &caps[1], &caps[2])
    })
    .to_string()
}

/// Collapse runs of 2+ whitespace chars (spaces, tabs) into single space.
fn collapse_whitespace(text: &str) -> String {
    let re = Regex::new(r"[ \t]{2,}").unwrap();
    re.replace_all(text, " ").to_string()
}

/// Collapse runs of 2+ consecutive newlines into a single newline.
fn collapse_newlines(text: &str) -> String {
    let re = Regex::new(r"\n{2,}").unwrap();
    re.replace_all(text, "\n").to_string()
}

/// Detect lines that appear repeatedly at the same position across pages.
///
/// Returns a set of (trimmed_line_text) pairs that appear on more than one
/// page. Useful for stripping headers and footers when batch-processing PDFs.
pub fn detect_repeated_lines(text: &str) -> std::collections::HashSet<String> {
    let pages: Vec<&str> = text.split('\x0C').collect();
    if pages.len() < 3 {
        return std::collections::HashSet::new();
    }

    let mut line_freq: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for page in &pages {
        let trimmed = page.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lines: Vec<&str> = trimmed.lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() < 3 {
            continue;
        }
        if let Some(first) = lines.first() {
            *line_freq.entry(first.to_string()).or_insert(0) += 1;
        }
        if let Some(last) = lines.last() {
            *line_freq.entry(last.to_string()).or_insert(0) += 1;
        }
    }

    line_freq
        .into_iter()
        .filter(|(_, count)| *count > pages.len() / 2)
        .map(|(line, _)| line)
        .collect()
}

/// Clean with optional header/footer stripping.
///
/// Lines matching any entry in `repeated_lines` are removed from the text.
/// The check is a simple substring match on each trimmed line.
pub fn clean_with_footer_strip(
    raw: &str,
    repeated_lines: &std::collections::HashSet<String>,
) -> String {
    if repeated_lines.is_empty() {
        return clean(raw);
    }

    let mut result = String::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if !repeated_lines.contains(trimmed) {
            result.push_str(line);
            result.push('\n');
        }
    }

    clean(&result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dehyphenation() {
        let input = "This is a hyphen-\nated word in a PDF.";
        let result = dehyphenate(input);
        assert_eq!(result, "This is a hyphenated word in a PDF.");
    }

    #[test]
    fn test_dehyphenation_non_breaking_hyphen() {
        let input = "well-known fact";
        let result = dehyphenate(input);
        assert_eq!(result, "well-known fact");
    }

    #[test]
    fn test_collapse_whitespace() {
        let input = "This   has   multiple    spaces.";
        let result = collapse_whitespace(input);
        assert_eq!(result, "This has multiple spaces.");
    }

    #[test]
    fn test_collapse_newlines() {
        let input = "Line 1\n\n\nLine 2\n\nLine 3";
        let result = collapse_newlines(input);
        assert_eq!(result, "Line 1\nLine 2\nLine 3");
    }

    #[test]
    fn test_clean_full_pipeline() {
        // 'This-\nis' → dehyphenated to 'Thisis' (the hyphen was a line-break artifact)
        let input = "  Hello   world!  This-\nis a test.  \n\n\nWith extra spacing.\n  ";
        let result = clean(input);
        assert_eq!(
            result,
            "Hello world! Thisis a test. \nWith extra spacing.",
            "dehyphenation joins 'This-' + 'is' → 'Thisis'; see test_dehyphenation for isolated behavior"
        );
    }

    #[test]
    fn test_detect_repeated_lines_single_page() {
        let text = "Header\nContent\nFooter";
        let repeated = detect_repeated_lines(text);
        assert!(repeated.is_empty(), "single page should have no repeats");
    }

    #[test]
    fn test_clean_with_footer_strip() {
        let input = "Header\nBody text\nFooter\nHeader\nMore body\nFooter";
        let mut repeated = std::collections::HashSet::new();
        repeated.insert("Header".to_string());
        repeated.insert("Footer".to_string());
        let result = clean_with_footer_strip(input, &repeated);
        assert!(!result.contains("Header"), "header should be stripped");
        assert!(result.contains("Body text"), "body should be kept");
    }

    #[test]
    fn test_unicode_normalization() {
        let decomposed = "cafe\u{0301}"; // café in NFD
        let result = clean(decomposed);
        assert_eq!(result.chars().count(), 4, "NFC should combine accent");
    }

    #[test]
    fn test_empty_input() {
        assert_eq!(clean(""), "");
    }

    #[test]
    fn test_only_whitespace() {
        assert_eq!(clean("   \n\n  \t  "), "");
    }
}