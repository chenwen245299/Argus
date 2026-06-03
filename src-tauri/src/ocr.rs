use std::path::Path;

/// OCR a single JPEG image (raw bytes).
/// On macOS, tries the Vision framework first (via a lazily compiled Swift helper).
/// Falls back to the external `tesseract` binary on all platforms.
pub fn ocr_jpeg_bytes(jpeg_bytes: &[u8]) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    if let Ok(text) = ocr_via_vision(jpeg_bytes) {
        if !text.trim().is_empty() {
            return Ok(text);
        }
    }
    ocr_via_tesseract(jpeg_bytes)
}

/// OCR an entire PDF using `pdftoppm` (poppler) to render pages then `tesseract` to recognize.
/// Returns `None` if either tool is unavailable or yields no text.
pub fn ocr_pdf_file(pdf_path: &Path) -> Option<String> {
    let uid = uuid::Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("argus_ocr_{uid}"));
    std::fs::create_dir_all(&temp_dir).ok()?;

    let ok = std::process::Command::new("pdftoppm")
        .arg("-jpeg")
        .arg("-r")
        .arg("150")
        .arg(pdf_path.as_os_str())
        .arg(temp_dir.join("page").as_os_str())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if !ok {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return None;
    }

    let mut images: Vec<std::path::PathBuf> = std::fs::read_dir(&temp_dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("jpg") | Some("jpeg")
            )
        })
        .collect();
    images.sort();

    if images.is_empty() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return None;
    }

    let mut page_texts: Vec<String> = Vec::new();
    let mut any_ok = false;
    for img_path in &images {
        if let Ok(bytes) = std::fs::read(img_path) {
            match ocr_jpeg_bytes(&bytes) {
                Ok(t) => {
                    page_texts.push(t);
                    any_ok = true;
                }
                Err(_) => page_texts.push(String::new()),
            }
        }
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
    if !any_ok {
        return None;
    }

    let combined = page_texts.join("\n\n");
    if combined.trim().is_empty() {
        None
    } else {
        Some(combined)
    }
}

// ── tesseract OCR ─────────────────────────────────────────────────────────────

fn ocr_via_tesseract(jpeg_bytes: &[u8]) -> Result<String, String> {
    let uid = uuid::Uuid::new_v4().to_string();
    let tmp = std::env::temp_dir().join(format!("argus_ocr_{uid}.jpg"));
    std::fs::write(&tmp, jpeg_bytes).map_err(|e| format!("write tmp: {e}"))?;

    let result = std::process::Command::new("tesseract")
        .arg(tmp.as_os_str())
        .arg("stdout")
        .arg("--oem")
        .arg("3") // LSTM engine
        .arg("--psm")
        .arg("3") // fully automatic page segmentation
        .output();

    let _ = std::fs::remove_file(&tmp);

    match result {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(
            "tesseract not installed (macOS: brew install tesseract)".into(),
        ),
        Err(e) => Err(format!("tesseract: {e}")),
    }
}

// ── macOS Vision framework via lazily compiled Swift helper ───────────────────
// The helper is compiled once with `swiftc` and cached in the temp directory.
// Requires Xcode Command Line Tools (`xcode-select --install`).

#[cfg(target_os = "macos")]
static VISION_HELPER: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

#[cfg(target_os = "macos")]
fn ocr_via_vision(jpeg_bytes: &[u8]) -> Result<String, String> {
    use std::io::Write as _;
    let helper = VISION_HELPER
        .get_or_init(compile_vision_helper)
        .as_ref()
        .ok_or_else(|| "vision helper unavailable".to_string())?;

    let mut child = std::process::Command::new(helper)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn vision helper: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(jpeg_bytes);
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("vision helper wait: {e}"))?;

    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err("vision helper returned non-zero".into())
    }
}

#[cfg(target_os = "macos")]
fn compile_vision_helper() -> Option<std::path::PathBuf> {
    // One-file Swift program: reads JPEG from stdin, writes OCR text to stdout.
    const SRC: &str = r#"
import Vision
import Foundation
import AppKit

let data = FileHandle.standardInput.readDataToEndOfFile()
guard let img = NSImage(data: data),
      let cg = img.cgImage(forProposedRect: nil, context: nil, hints: nil) else {
    exit(0)
}
let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate
req.usesLanguageCorrection = true
try? VNImageRequestHandler(cgImage: cg, options: [:]).perform([req])
let lines = (req.results as? [VNRecognizedTextObservation])?.compactMap {
    $0.topCandidates(1).first?.string
} ?? []
print(lines.joined(separator: "\n"), terminator: "")
"#;

    let dir = std::env::temp_dir().join("argus_ocr_helper");
    let bin_path = dir.join("vision_ocr");

    // Return cached binary if already compiled.
    if bin_path.exists() {
        return Some(bin_path);
    }

    let src_path = dir.join("vision_ocr.swift");
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&src_path, SRC).ok()?;

    let ok = std::process::Command::new("swiftc")
        .arg(&src_path)
        .arg("-o")
        .arg(&bin_path)
        .arg("-O")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ok {
        Some(bin_path)
    } else {
        None
    }
}
