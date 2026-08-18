//! Rasterise a single PDF page to a PNG image.
//!
//! Rendering runs in-process via **PDFium**, bound to a dynamic library that
//! ships inside the app (fetched at build time by `scripts/fetch-pdfium.mjs`),
//! so the user needs no poppler / `pdftoppm` install. If the PDFium library
//! cannot be located for any reason, the old poppler CLI is tried as a fallback
//! so a machine that already has it still works.

use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Default rasterisation resolution. 150 DPI renders figures, diagrams and
/// result curves legibly while keeping a page PNG to a few hundred kilobytes.
pub const DEFAULT_DPI: u32 = 150;

/// Render one **1-based** page of `pdf_path` to PNG bytes at `dpi`.
pub fn render_pdf_page_png(pdf_path: &Path, page: u32, dpi: u32) -> Result<Vec<u8>, String> {
    match render_with_pdfium(pdf_path, page, dpi) {
        Ok(bytes) => Ok(bytes),
        // PDFium missing or failed: fall back to poppler so a machine that has
        // it installed keeps working. Surface both errors if that fails too.
        Err(pdfium_err) => render_with_pdftoppm(pdf_path, page, dpi)
            .map_err(|poppler_err| format!("{pdfium_err} · poppler 兜底也失败：{poppler_err}")),
    }
}

// ── PDFium (bundled, the normal path) ─────────────────────────────────────────

/// Candidate locations for the bundled PDFium dynamic library, most specific
/// first. Resolved from the running executable so it works from both the
/// packaged app and the MCP subprocess (same binary), with a dev copy under
/// `src-tauri/lib` as the last resort.
fn pdfium_lib_candidates() -> Vec<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut cands = Vec::new();
    // Explicit override, mostly for development.
    if let Ok(explicit) = std::env::var("ARGUS_PDFIUM_LIB") {
        cands.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Windows / Linux: Tauri drops bundled resources beside the exe.
            cands.push(dir.join(&name));
            cands.push(dir.join("lib").join(&name));
            // macOS .app: the exe is in Contents/MacOS, resources in Resources.
            cands.push(dir.join("../Resources").join(&name));
            cands.push(dir.join("../Resources/lib").join(&name));
        }
    }
    // Dev builds run from target/…; the fetch script drops the lib here.
    cands.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("lib")
            .join(&name),
    );
    cands
}

fn bind_pdfium() -> Result<Pdfium, String> {
    for cand in pdfium_lib_candidates() {
        if cand.is_file() {
            if let Ok(bindings) = Pdfium::bind_to_library(&cand) {
                return Ok(Pdfium::new(bindings));
            }
        }
    }
    // Last resort: a system-wide install, if the user happens to have one.
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .map_err(|e| {
            format!("找不到内置的 PDFium 渲染库（也未在系统中安装）：{e}")
        })
}

fn render_with_pdfium(pdf_path: &Path, page: u32, dpi: u32) -> Result<Vec<u8>, String> {
    if page == 0 {
        return Err("页码从 1 开始".to_string());
    }
    let pdfium = bind_pdfium()?;
    let doc = pdfium
        .load_pdf_from_file(pdf_path, None)
        .map_err(|e| format!("无法打开 PDF：{e}"))?;
    let page_obj = doc
        .pages()
        .get((page - 1) as u16)
        .map_err(|e| format!("这份 PDF 没有第 {page} 页：{e}"))?;

    // PDF user space is 72 DPI; scale up to the requested resolution.
    let config = PdfRenderConfig::new().scale_page_by_factor(dpi as f32 / 72.0);
    let bitmap = page_obj
        .render_with_config(&config)
        .map_err(|e| format!("PDFium 渲染第 {page} 页失败：{e}"))?;

    let mut buf = std::io::Cursor::new(Vec::new());
    bitmap
        .as_image()
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| format!("第 {page} 页编码 PNG 失败：{e}"))?;
    let bytes = buf.into_inner();
    if bytes.is_empty() {
        return Err(format!("PDFium 渲染第 {page} 页得到空图。"));
    }
    Ok(bytes)
}

// ── Poppler `pdftoppm` (fallback for machines that already have it) ────────────

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
        if !cand.contains('/') || Path::new(cand).is_file() {
            return Some(PathBuf::from(cand));
        }
    }
    None
}

fn render_with_pdftoppm(pdf_path: &Path, page: u32, dpi: u32) -> Result<Vec<u8>, String> {
    let bin = pdftoppm_bin().ok_or_else(|| "系统未安装 pdftoppm（poppler）".to_string())?;

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
        .map_err(|e| format!("运行 pdftoppm 失败：{e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(format!("pdftoppm 无法渲染第 {page} 页（页码是否有效？）。"));
    }

    let bytes = std::fs::read(&out)
        .map_err(|e| format!("pdftoppm 没有为第 {page} 页产出图片：{e}"))?;
    let _ = std::fs::remove_file(&out);
    if bytes.is_empty() {
        return Err(format!("pdftoppm 为第 {page} 页产出了空图片。"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    /// The bundled PDFium library (src-tauri/lib, dropped by fetch-pdfium.mjs)
    /// must actually load at runtime — catching a wrong platform lib name or a
    /// broken download before it turns into a silent page-render failure.
    #[test]
    fn pdfium_library_binds() {
        super::bind_pdfium().expect("PDFium should bind to the bundled library");
    }
}
