use std::path::Path;

use crate::models::AppSettings;

// Minimum average characters per page to consider a PDF digitally native.
const MIN_CHARS_PER_PAGE: usize = 80;

// ── lopdf full-text extraction ────────────────────────────────────────────────

/// Extract text from ALL pages of a PDF using lopdf.
/// Returns (text, page_count).
fn extract_all_pages(pdf_path: &Path) -> Result<(String, usize), String> {
    let bytes = std::fs::read(pdf_path).map_err(|e| format!("Read PDF: {e}"))?;
    let doc = lopdf::Document::load_mem(&bytes).map_err(|e| format!("lopdf load: {e}"))?;
    let pages = doc.get_pages();
    let page_count = pages.len();
    if page_count == 0 {
        return Ok((String::new(), 0));
    }
    let page_nums: Vec<u32> = pages.keys().copied().collect();
    let text = doc.extract_text(&page_nums).unwrap_or_default();
    Ok((text, page_count))
}

/// Decide whether the extracted text is sufficient (not a scanned-only PDF).
fn is_sufficient(text: &str, page_count: usize) -> bool {
    if page_count == 0 {
        return false;
    }
    let chars = text.chars().filter(|c| !c.is_whitespace()).count();
    chars / page_count >= MIN_CHARS_PER_PAGE
}

// ── pdftotext fallback ────────────────────────────────────────────────────────

/// Try to extract text via the system `pdftotext` (poppler-utils).
/// Returns None if the binary is not installed or extraction fails.
fn try_pdftotext(pdf_path: &Path) -> Option<String> {
    let output = std::process::Command::new("pdftotext")
        .arg("-enc")
        .arg("UTF-8")
        .arg(pdf_path.as_os_str())
        .arg("-") // write to stdout
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Extraction outcome for a single paper.
pub enum ExtractionResult {
    /// Text extracted via lopdf (digital PDF).
    Text,
    /// No usable embedded text could be extracted.
    NoText,
    /// Error during extraction.
    Failed(String),
}

/// Extract digital PDF text.
/// Tries lopdf first; falls back to pdftotext if lopdf yields insufficient text.
/// Writes `fulltext.txt` and updates `.status.json` on success.
pub fn extract_and_write(root: &str, slug: &str, _settings: &AppSettings) -> ExtractionResult {
    let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);

    if !pdf_path.exists() {
        return ExtractionResult::Failed("PDF file not found".to_string());
    }

    // Stage 1: lopdf
    let (lopdf_text, page_count) = match extract_all_pages(&pdf_path) {
        Ok(r) => r,
        Err(e) => return ExtractionResult::Failed(e),
    };

    if is_sufficient(&lopdf_text, page_count) {
        if let Err(e) = write_fulltext_and_status(root, slug, &lopdf_text) {
            return ExtractionResult::Failed(e);
        }
        return ExtractionResult::Text;
    }

    // Stage 2: pdftotext fallback (handles encodings lopdf can't decode)
    if let Some(text) = try_pdftotext(&pdf_path) {
        if is_sufficient(&text, page_count.max(1)) {
            if let Err(e) = write_fulltext_and_status(root, slug, &text) {
                return ExtractionResult::Failed(e);
            }
            return ExtractionResult::Text;
        }
    }

    let mut status = crate::paper::read_status_for(root, slug);
    status.text_extracted = false;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    let _ = crate::paper::write_status(root, slug, &status);
    ExtractionResult::NoText
}

fn write_fulltext_and_status(root: &str, slug: &str, text: &str) -> Result<(), String> {
    let dir = crate::paper::paper_dir(root, slug);
    let fulltext_path = dir.join("fulltext.txt");
    std::fs::write(&fulltext_path, text).map_err(|e| format!("Write fulltext.txt: {e}"))?;

    let mut status = crate::paper::read_status_for(root, slug);
    status.text_extracted = true;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    crate::paper::write_status(root, slug, &status)
}

/// Read the fulltext.txt for a given slug (returns empty string if absent).
pub fn read_fulltext(root: &str, slug: &str) -> String {
    let path = crate::paper::paper_dir(root, slug).join("fulltext.txt");
    std::fs::read_to_string(&path).unwrap_or_default()
}
