//! Read-only metadata catalog and stable-id resolution for note citations.
use serde::Serialize;
use crate::{models::PaperMeta, paper};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferencePaper {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub cite_key: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<u32>,
    pub file_type: Option<String>,
}

pub fn cite_key(bibtex: &str) -> Option<String> {
    static HEADER: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = HEADER.get_or_init(|| regex::Regex::new(r"(?i)^@\s*([a-z]+)\s*([\{(])").unwrap());
    let mut rest = bibtex;
    while let Some(at) = rest.find('@') {
        // BibTeX percent comments are not entries.
        let line = rest[..at].rsplit('\n').next().unwrap_or("");
        if line.contains('%') {
            rest = rest[at..].split_once('\n').map(|(_, tail)| tail).unwrap_or(""); continue;
        }
        rest = &rest[at..];
        let Some(header) = re.captures(rest) else { rest = &rest[1..]; continue; };
        let kind = header[1].to_ascii_lowercase();
        let body = &rest[header.get(0)?.end()..];
        if !matches!(kind.as_str(), "string" | "comment" | "preamble") {
            let key = body.split_once(',')?.0.trim();
            if !key.is_empty() && !key.chars().any(|c| c.is_whitespace() || "{}()".contains(c)) {
                return Some(key.into());
            }
        }
        // Skip a complete macro/comment, including nested fake @entry headers.
        let mut stack = vec![if &header[2] == "{" { '}' } else { ')' }];
        let mut escaped = false; let mut quoted = false; let mut end = body.len();
        for (i, c) in body.char_indices() {
            if escaped { escaped = false; continue; }
            if c == '\\' { escaped = true; continue; }
            if c == '"' { quoted = !quoted; continue; }
            if quoted { continue; }
            if c == '{' { stack.push('}'); }
            else if c == '(' && stack.last() == Some(&')') { stack.push(')'); }
            else if stack.last() == Some(&c) {
                stack.pop();
                if stack.is_empty() { end = i + c.len_utf8(); break; }
            }
        }
        rest = &body[end..];
    }
    None
}

pub fn list(root: &str) -> Result<Vec<ReferencePaper>, String> {
    let lock = paper::namespace_lock(root);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut papers = Vec::new();
    // Read the discovered paths directly: resolving every nested slug again
    // would repeatedly walk the collection tree for a large library.
    for (slug, dir) in paper::list_paper_dirs(root)? {
        let path = dir.join("meta.json");
        if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) { continue; }
        let Ok(text) = std::fs::read_to_string(path) else { continue };
        let Ok(meta) = serde_json::from_str::<PaperMeta>(&text) else { continue };
        if meta.file_type.as_deref() == Some("md") { continue; }
        papers.push(ReferencePaper { cite_key: meta.bibtex.as_deref().and_then(cite_key),
            id: meta.id, slug, title: meta.title, authors: meta.authors, year: meta.year, file_type: meta.file_type });
    }
    papers.sort_by(|a, b| a.title.cmp(&b.title).then(a.id.cmp(&b.id)));
    Ok(papers)
}

pub fn resolve(root: &str, id: &str) -> Result<ReferencePaper, String> {
    if id.trim().is_empty() || id.len() > 512 { return Err("Invalid paper reference".into()); }
    let mut matches = list(root)?.into_iter().filter(|p| p.id == id);
    let paper = matches.next().ok_or("The referenced paper is no longer in this library")?;
    if matches.next().is_some() { return Err("Multiple papers have this reference id; resolve the duplicate first".into()); }
    Ok(paper)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_real_bibtex_headers_and_skips_macros() {
        assert_eq!(cite_key("@string{venue = {Journal}}\n@article{Smith2026, title={A paper}}"), Some("Smith2026".into()));
        assert_eq!(cite_key("@Book ( 中文-key:2026, title = {书籍})"), Some("中文-key:2026".into()));
        assert_eq!(cite_key("@preamble{a, b}\n@comment{ignored, body}\n@inproceedings{ConfKey, title={Paper}}"), Some("ConfKey".into()));
        assert_eq!(cite_key("not bibtex"), None);
        assert_eq!(cite_key("@comment{a note (unfinished parenthesis}\n@article{Actual, title={Real}}"), Some("Actual".into()));
        assert_eq!(cite_key("% @article{Wrong, title={Fake}}\n@comment{nested @article{AlsoWrong, title={Fake}}}\n@article{Right, title={Real}}"), Some("Right".into()));
    }
    #[test]
    fn references_survive_moves_and_renames_and_refuse_deleted_or_ambiguous_ids() {
        let lib = crate::library_notes::tests::Library::new();
        let note = lib.note();
        assert!(list(&lib.root).unwrap().is_empty(), "standalone notes are not paper candidates");
        let mut meta = paper::read_meta(&lib.root, &note.slug).unwrap();
        meta.file_type = None; meta.bibtex = Some("@article{Ref2026, title={Test}}".into());
        paper::write_meta(&lib.root, &note.slug, &meta).unwrap();
        let result = resolve(&lib.root, &meta.id).unwrap();
        assert_eq!(result.cite_key.as_deref(), Some("Ref2026"));
        let original = paper::paper_dir(&lib.root, &note.slug);
        let destination = std::path::Path::new(&lib.root).join("papers/another-folder/renamed-paper");
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::rename(&original, &destination).unwrap();
        assert_eq!(resolve(&lib.root, &meta.id).unwrap().slug, "renamed-paper");
        let duplicate = std::path::Path::new(&lib.root).join("papers/duplicate");
        std::fs::create_dir_all(&duplicate).unwrap();
        std::fs::write(duplicate.join("meta.json"), serde_json::to_string(&meta).unwrap()).unwrap();
        assert!(resolve(&lib.root, &meta.id).is_err());
        std::fs::remove_dir_all(duplicate).unwrap();
        std::fs::remove_dir_all(destination).unwrap();
        assert!(resolve(&lib.root, &meta.id).is_err());
        assert!(resolve(&lib.root, "../escape").is_err());
    }
}
