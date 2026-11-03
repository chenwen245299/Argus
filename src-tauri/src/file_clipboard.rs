//! Copy actual PDF files in one native clipboard operation, rather than paths
//! as text or repeated writes that leave only the final file.
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;

fn pdf_paths(root: &str, slugs: &[String]) -> Result<Vec<PathBuf>, String> {
    if slugs.is_empty() {
        return Err("No PDF files selected".into());
    }
    for slug in slugs {
        crate::path_guard::validate_segment("paper slug", slug)?;
    }
    let canonical_root =
        std::fs::canonicalize(root).map_err(|e| format!("Resolve library: {e}"))?;
    // Resolve the library once, including papers inside nested collections.
    let dirs: HashMap<_, _> = crate::paper::list_paper_dirs(root)?.into_iter().collect();
    let mut paths = Vec::new();
    let mut seen = HashSet::new();
    for slug in slugs {
        let dir = dirs
            .get(slug)
            .ok_or_else(|| format!("Paper directory not found: {slug}"))?;
        let mut candidates: Vec<_> = std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
                    && path.is_file()
            })
            .collect();
        candidates.sort();
        let path = candidates
            .first()
            .ok_or_else(|| format!("PDF file not found: {slug}"))?
            .canonicalize()
            .map_err(|e| format!("Resolve PDF: {e}"))?;
        if !path.starts_with(&canonical_root) {
            return Err("Refused: PDF is outside the library".into());
        }
        if seen.insert(path.clone()) {
            paths.push(path);
        }
    }
    Ok(paths)
}

// Paths arrive as argv, never interpolated into executable script text.
#[cfg(target_os = "macos")]
const MAC_SCRIPT: &str = r#"ObjC.import('AppKit');
function copyFiles(paths, pasteboard) {
    var urls = $.NSMutableArray.alloc.init;
    paths.forEach(function(path) { urls.addObject($.NSURL.fileURLWithPath($(path))); });
    pasteboard.clearContents;
    if (!pasteboard.writeObjects(urls)) throw new Error('Failed to copy PDF files');
}
function run(argv) { copyFiles(argv, $.NSPasteboard.generalPasteboard); }
"#;

fn clipboard_command(paths: &[PathBuf]) -> Result<Command, String> {
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("osascript");
        command
            .args(["-l", "JavaScript", "-e", MAC_SCRIPT])
            .args(paths);
        Ok(command)
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let paths: Vec<_> = paths
            .iter()
            .map(|p| p.to_str().ok_or("Invalid PDF filename"))
            .collect::<Result<_, _>>()?;
        let mut command = Command::new("powershell");
        command.args(["-NoProfile", "-NonInteractive", "-STA", "-Command",
            "$ErrorActionPreference = 'Stop'; $paths = [string[]]@(ConvertFrom-Json -InputObject $env:ARGUS_CLIP_PATHS); Set-Clipboard -LiteralPath $paths"])
            .env("ARGUS_CLIP_PATHS", serde_json::to_string(&paths).map_err(|e| e.to_string())?)
            .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        Ok(command)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = paths;
        Err("Copying files is supported on macOS and Windows".into())
    }
}

pub fn copy_pdfs(root: &str, slugs: &[String]) -> Result<usize, String> {
    // Validate every file before changing the clipboard: a missing file must
    // not silently turn an eight-file selection into a partial copy.
    let paths = pdf_paths(root, slugs)?;
    let output = clipboard_command(&paths)?
        .output()
        .map_err(|e| format!("Copy PDF files: {e}"))?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if error.is_empty() {
            "Failed to copy PDF files".into()
        } else {
            error
        });
    }
    Ok(paths.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Library(PathBuf);
    impl Library {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("argus-file-clipboard-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
        fn add(&self, slug: &str, name: &str) -> PathBuf {
            let dir = self.0.join("papers/分类/子分类").join(slug);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("meta.json"), "{}").unwrap();
            let path = dir.join(name);
            std::fs::write(&path, b"%PDF fixture").unwrap();
            path.canonicalize().unwrap()
        }
        fn root(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }
    impl Drop for Library {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn resolves_all_eight_pdfs_in_selection_order_and_deduplicates() {
        let lib = Library::new();
        let slugs: Vec<_> = (0..8).map(|i| format!("paper-{i}")).collect();
        let expected: Vec<_> = slugs
            .iter()
            .map(|slug| lib.add(slug, "中文 [1] 'quoted'.PDF"))
            .collect();
        let mut repeated = slugs.clone();
        repeated.push(slugs[0].clone());
        assert_eq!(pdf_paths(lib.root(), &repeated).unwrap(), expected);
    }
    #[test]
    fn rejects_missing_files_empty_selections_and_invalid_slugs_before_copying() {
        let lib = Library::new();
        lib.add("valid", "paper.pdf");
        lib.add("ebook", "book.epub");
        for slugs in [
            vec![],
            vec!["valid".into(), "missing".into()],
            vec!["ebook".into()],
            vec!["../escape".into()],
        ] {
            assert!(pdf_paths(lib.root(), &slugs).is_err());
        }
    }
    #[cfg(unix)]
    #[test]
    fn rejects_pdf_symlinks_leading_outside_the_library() {
        let lib = Library::new();
        let outside = Library::new();
        let external = outside.add("external", "paper.pdf");
        let path = lib.add("linked", "remove.txt");
        std::os::unix::fs::symlink(external, path.parent().unwrap().join("linked.pdf")).unwrap();
        assert!(pdf_paths(lib.root(), &["linked".into()])
            .unwrap_err()
            .contains("outside"));
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn mac_command_passes_every_filename_as_an_argument() {
        let paths = vec![
            PathBuf::from("/tmp/中文 \"quoted\".pdf"),
            PathBuf::from("/tmp/a\nb.pdf"),
        ];
        let command = clipboard_command(&paths).unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            &args[4..],
            paths.iter().map(|p| p.as_os_str()).collect::<Vec<_>>()
        );
        assert!(!MAC_SCRIPT.contains("quoted"));
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_pasteboard_contains_eight_independent_file_items() {
        // A private named pasteboard verifies actual macOS file representations
        // without reading or changing the user's system clipboard.
        let lib = Library::new();
        let paths: Vec<_> = (0..8)
            .map(|i| lib.add(&format!("paper-{i}"), "中文 [x] \"quoted\".pdf"))
            .collect();
        let script = format!(
            r#"{MAC_SCRIPT}
function run(argv) {{
    var pb = $.NSPasteboard.pasteboardWithUniqueName;
    try {{
        copyFiles(argv, pb);
        var result = [], items = pb.pasteboardItems;
        for (var i = 0; i < items.count; i++) {{
            var url = items.objectAtIndex(i).stringForType('public.file-url');
            result.push(ObjC.unwrap($.NSURL.URLWithString(url).path));
        }}
        return JSON.stringify(result);
    }} finally {{ pb.releaseGlobally; }}
}}"#
        );
        let output = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .args(&paths)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let copied: Vec<PathBuf> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(copied, paths);
    }
}
