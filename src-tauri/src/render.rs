//! Rasterise a single PDF page to a PNG image via the poppler `pdftoppm` CLI.
//!
//! There is no in-process PDF renderer in the dependency set — `lopdf` parses
//! structure and text only — so page images come from `pdftoppm`, the same
//! binary the OCR path already shells out to. If poppler is not installed the
//! call fails with a message that says so, rather than silently returning
//! nothing.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Default rasterisation resolution. 150 DPI renders figures, diagrams and
/// result curves legibly while keeping a page PNG to a few hundred kilobytes.
pub const DEFAULT_DPI: u32 = 150;

/// Locate `pdftoppm`, checking `PATH` plus the usual Homebrew / system prefixes.
///
/// A GUI app launched from Finder does not inherit a login shell's `PATH`, so
/// the bare name alone is not enough — the same absolute fallbacks the OCR code
/// relies on are checked too.
fn pdftoppm_bin() -> Option<PathBuf> {
    for cand in [
        "pdftoppm",
        "/opt/homebrew/bin/pdftoppm",
        "/usr/local/bin/pdftoppm",
        "/usr/bin/pdftoppm",
    ] {
        // A bare name is resolved by the OS against PATH; an absolute path has
        // to exist on disk to count.
        if !cand.contains('/') || Path::new(cand).is_file() {
            return Some(PathBuf::from(cand));
        }
    }
    None
}

/// Render one **1-based** page of `pdf_path` to PNG bytes at `dpi`.
pub fn render_pdf_page_png(pdf_path: &Path, page: u32, dpi: u32) -> Result<Vec<u8>, String> {
    let bin = pdftoppm_bin().ok_or_else(|| {
        "Cannot render PDF pages: the `pdftoppm` tool (poppler) is not installed. \
         Install poppler (e.g. `brew install poppler`) to use page rendering."
            .to_string()
    })?;

    // `-singlefile` makes pdftoppm write exactly `<prefix>.png` with no page
    // number suffix, so the output path is known without globbing the temp dir.
    let prefix = std::env::temp_dir().join(format!("argus-page-{}", uuid::Uuid::new_v4()));
    let out = prefix.with_extension("png");

    let status = Command::new(&bin)
        .arg("-png")
        .arg("-singlefile")
        .arg("-r")
        .arg(dpi.to_string())
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg(pdf_path)
        .arg(&prefix)
        .status()
        .map_err(|e| format!("Failed to run pdftoppm: {e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(format!(
            "pdftoppm could not render page {page} (is it a valid page number?)."
        ));
    }

    let bytes = std::fs::read(&out)
        .map_err(|e| format!("pdftoppm produced no image for page {page}: {e}"))?;
    let _ = std::fs::remove_file(&out);
    if bytes.is_empty() {
        return Err(format!("pdftoppm produced an empty image for page {page}."));
    }
    Ok(bytes)
}
