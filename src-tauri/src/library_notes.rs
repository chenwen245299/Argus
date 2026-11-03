//! Independent Markdown documents in the ordinary collection tree. The primary
//! body is not a paper attachment or extracted fulltext: AI reads it only via
//! the dedicated approval-gated tool.
use crate::{collections, fsutil, models::PaperMeta, paper};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

const BODY: &str = "notes/note.md";
static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
fn lock() -> std::sync::MutexGuard<'static, ()> { LOCK.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner()) }

fn collection_file(root: &str) -> Result<crate::models::CollectionsFile, String> {
    let path = std::path::Path::new(root).join(".argus/collections.json");
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("Invalid collections file: {e}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Default::default()),
        Err(error) => Err(error.to_string()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub slug: String,
    pub id: String,
    pub title: String,
    pub content: String,
    pub collection_id: Option<String>,
    pub collection_name: String,
    pub created_at: String,
}

pub fn collection_name(root: &str, id: Option<&str>) -> Result<String, String> {
    let Some(id) = id else { return Ok("文献库 / Library".into()); };
    let file = collection_file(root)?;
    let mut names = Vec::new(); let mut current = Some(id); let mut seen = std::collections::HashSet::new();
    while let Some(id) = current {
        if !seen.insert(id) { return Err("Invalid collection hierarchy".into()); }
        let item = file.collections.iter().find(|c| c.id == id).ok_or("Collection no longer exists")?;
        names.push(item.name.clone()); current = item.parent_id.as_deref();
    }
    names.reverse(); Ok(names.join(" / "))
}

pub fn validate(title: &str, content: &str) -> Result<(), String> {
    if title.trim().is_empty() || title.chars().count() > 120 || title.contains(['\n', '\r', '\0']) {
        return Err("Use a one-line note title of 1–120 characters".into());
    }
    if content.len() > 2 * 1024 * 1024 { return Err("Note exceeds 2 MiB".into()); }
    Ok(())
}

fn read_unlocked(root: &str, slug: &str) -> Result<Snapshot, String> {
    crate::path_guard::validate_segment("note slug", slug)?;
    let meta = paper::read_meta(root, slug)?;
    if meta.file_type.as_deref() != Some("md") { return Err("This entry is not an independent Markdown note".into()); }
    let dir = paper::paper_dir(root, slug);
    let path = dir.join(BODY);
    for p in [dir.join("notes"), path.clone()] {
        if std::fs::symlink_metadata(p).map_err(|e| e.to_string())?.file_type().is_symlink() {
            return Err("Note body and directory must not be symlinks".into());
        }
    }
    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 { return Err("Note exceeds 2 MiB".into()); }
    let content = std::fs::read_to_string(path).map_err(|e| format!("Read note: {e}"))?;
    let file = collection_file(root)?;
    let collection_id = file.assignments.iter().find(|a| a.paper_id == meta.id).map(|a| a.collection_id.clone());
    Ok(Snapshot { slug: slug.into(), id: meta.id, title: meta.title, content,
        collection_name: collection_name(root, collection_id.as_deref())?, collection_id, created_at: meta.added_at })
}

pub fn read(root: &str, slug: &str) -> Result<Snapshot, String> {
    let _guard = lock();
    let namespace = paper::namespace_lock(root);
    let _namespace = namespace.lock().unwrap_or_else(|e| e.into_inner());
    read_unlocked(root, slug)
}

pub fn create(root: &str, collection_id: Option<&str>, title: &str, content: &str) -> Result<Snapshot, String> {
    validate(title, content)?; collection_name(root, collection_id)?;
    if let Some(id) = collection_id { collections::ensure_collection_can_receive_papers(root, id)?; }
    let _guard = lock();
    let id = uuid::Uuid::new_v4().to_string();
    let dir = paper::reserve_paper_dir(root, &format!("note_{}", id.replace('-', "")))?;
    let slug = dir.file_name().unwrap().to_str().ok_or("Invalid note folder")?.to_string();
    let result = (|| {
        let meta: PaperMeta = serde_json::from_value(serde_json::json!({
            "id":id,"title":title.trim(),"authors":[],"tags":[],"added_at":chrono::Utc::now().to_rfc3339(),
            "original_filename":"note.md","file_type":"md","import_source":"file"
        })).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir.join("notes")).map_err(|e| e.to_string())?;
        fsutil::atomic_write_str(&dir.join(BODY), content)?;
        paper::write_meta(root, &slug, &meta)?;
        paper::ensure_paper_files(root, &slug);
        if let Some(cid) = collection_id { collections::move_paper_to_collection(root, &id, cid)?; }
        read_unlocked(root, &slug)
    })();
    if result.is_err() {
        let _ = collections::purge_paper(root, &id);
        let owned = paper::find_paper_dir(root, &slug).unwrap_or(dir);
        if !owned.join("meta.json").exists() || paper::read_meta(root, &slug).is_ok_and(|m| m.id == id) {
            let _ = std::fs::remove_dir_all(owned);
        }
    }
    result
}

pub fn update(root: &str, expected: &Snapshot, title: &str, content: &str) -> Result<Snapshot, String> {
    validate(title, content)?;
    let _guard = lock();
    let namespace = paper::namespace_lock(root);
    let _namespace = namespace.lock().unwrap_or_else(|e| e.into_inner());
    let current = read_unlocked(root, &expected.slug)?;
    if &current != expected { return Err("Note changed since it was read. Your draft was retained; reload before saving.".into()); }
    // Collection moves also use the namespace lock; resolve the current path now.
    let dir = paper::paper_dir(root, &expected.slug);
    let mut meta = paper::read_meta(root, &expected.slug)?;
    if meta.id != expected.id { return Err("Note identity changed".into()); }
    fsutil::atomic_write_str(&dir.join(BODY), content)?;
    meta.title = title.trim().into();
    if let Err(error) = paper::write_meta(root, &expected.slug, &meta) {
        let _ = fsutil::atomic_write_str(&dir.join(BODY), &current.content);
        return Err(error);
    }
    Ok(Snapshot { title: meta.title, content: content.into(), ..current })
}

pub fn delete(root: &str, expected: &Snapshot) -> Result<(), String> {
    let _guard = lock();
    let original; let backup;
    {
        let namespace = paper::namespace_lock(root);
        let _namespace = namespace.lock().unwrap_or_else(|e| e.into_inner());
        if read_unlocked(root, &expected.slug)? != *expected { return Err("Note changed since the deletion preview".into()); }
        let backup_root = std::path::Path::new(root).join(".argus/library-note-trash");
        std::fs::create_dir_all(&backup_root).map_err(|e| e.to_string())?;
        if std::fs::symlink_metadata(&backup_root).map_err(|e| e.to_string())?.file_type().is_symlink() {
            return Err("Invalid note backup directory".into());
        }
        backup = backup_root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir(&backup).map_err(|e| e.to_string())?;
        let record = serde_json::json!({"note":expected,"deletedAt":chrono::Utc::now().to_rfc3339()});
        fsutil::atomic_write_str(&backup.join("deleted.json"), &serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?)?;
        original = paper::paper_dir(root, &expected.slug);
        std::fs::rename(&original, backup.join("entry")).map_err(|e| format!("Back up deleted note: {e}"))?;
    }
    if let Err(error) = collections::purge_paper(root, &expected.id) {
        std::fs::rename(backup.join("entry"), original).map_err(|e| format!("{error}; restore note backup: {e}"))?;
        let _ = std::fs::remove_dir_all(backup); return Err(error);
    }
    let _ = crate::search::remove_paper(root, &expected.slug);
    Ok(())
}

pub fn notify(app: &tauri::AppHandle, root: &str, slug: &str, action: &str) {
    use tauri::Emitter;
    let _ = app.emit("library-notes-changed", serde_json::json!({"root":root,"slug":slug,"action":action}));
}

pub fn write_asset(root: &str, slug: &str, ext: &str, bytes: &[u8]) -> Result<String, String> {
    let _guard = lock();
    let namespace = paper::namespace_lock(root);
    let _namespace = namespace.lock().unwrap_or_else(|e| e.into_inner());
    read_unlocked(root, slug)?;
    paper::write_note_asset(root, slug, ext, bytes)
}

pub fn append_marks(root: &str, expected: &Snapshot, marks: Vec<crate::models::Highlight>) -> Result<Vec<crate::models::Highlight>, String> {
    let _guard = lock();
    let namespace = paper::namespace_lock(root);
    let _namespace = namespace.lock().unwrap_or_else(|e| e.into_inner());
    if read_unlocked(root, &expected.slug)? != *expected { return Err("Markdown document changed since the annotation preview; inspect it again".into()); }
    paper::append_highlights(root, &expected.slug, marks)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) struct Library { pub root: String, pub collection: String }
    impl Library {
        pub fn new() -> Self {
            let root = std::env::temp_dir().join(format!("argus-library-notes-{}", uuid::Uuid::new_v4())).to_string_lossy().to_string();
            std::fs::create_dir_all(std::path::Path::new(&root).join(".argus")).unwrap();
            let parent = collections::create_collection(&root, "图书".into(), None).unwrap();
            let child = collections::create_collection(&root, "读书笔记".into(), Some(parent.id)).unwrap();
            Self { root, collection: child.id }
        }
        pub fn note(&self) -> Snapshot { create(&self.root, Some(&self.collection), "阅读记录", "# 原始笔记\n\n保留正文与公式 $x^2$。").unwrap() }
    }
    impl Drop for Library { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.root); } }

    #[test]
    fn new_markdown_document_is_indexed_in_its_collection_and_saved_as_plain_md() {
        let lib = Library::new(); let note = lib.note();
        let path = paper::paper_dir(&lib.root, &note.slug);
        assert_eq!(std::fs::read_to_string(path.join(BODY)).unwrap(), note.content);
        assert!(path.to_string_lossy().contains("读书笔记"));
        let papers = crate::library::scan_library(&lib.root).unwrap();
        let entry = papers.iter().find(|p| p.slug == note.slug).unwrap();
        assert_eq!(entry.file_type.as_deref(), Some("md"));
        assert_eq!(collections::list_papers_in_collection(&lib.root, &lib.collection).unwrap().len(), 1);
        assert!(paper::list_notes(&lib.root, &note.slug).is_empty());
        assert!(crate::extraction::read_fulltext(&lib.root, &note.slug).is_empty());
    }
    #[test]
    fn autosave_preserves_identity_and_rejects_stale_edits() {
        let lib = Library::new(); let before = lib.note();
        let saved = update(&lib.root, &before, "新标题", "追加后的完整正文").unwrap();
        assert_eq!(saved.id, before.id); assert_eq!(saved.created_at, before.created_at);
        assert!(update(&lib.root, &before, "旧窗口", "不能覆盖").is_err());
        assert_eq!(read(&lib.root, &saved.slug).unwrap(), saved);
        assert!(delete(&lib.root, &before).is_err());
        assert!(!std::path::Path::new(&lib.root).join(".argus/library-note-trash").exists());
    }
    #[test]
    fn approved_delete_retains_body_and_assets_in_a_backup_and_removes_the_live_entry() {
        let lib = Library::new(); let note = lib.note();
        let dir = paper::paper_dir(&lib.root, &note.slug);
        std::fs::create_dir_all(dir.join("notes/assets")).unwrap();
        std::fs::write(dir.join("notes/assets/figure.png"), b"image fixture").unwrap();
        delete(&lib.root, &note).unwrap();
        assert!(read(&lib.root, &note.slug).is_err());
        assert!(collections::read_collections(&lib.root).assignments.iter().all(|a| a.paper_id != note.id));
        let backup = std::fs::read_dir(std::path::Path::new(&lib.root).join(".argus/library-note-trash")).unwrap().next().unwrap().unwrap().path();
        assert_eq!(std::fs::read_to_string(backup.join("entry").join(BODY)).unwrap(), note.content);
        assert_eq!(std::fs::read(backup.join("entry/notes/assets/figure.png")).unwrap(), b"image fixture");
    }
    #[test]
    fn invalid_locations_and_paths_never_create_or_overwrite_notes() {
        let lib = Library::new();
        assert!(create(&lib.root, Some("missing"), "标题", "正文").is_err());
        assert!(create(&lib.root, None, "", "正文").is_err());
        assert!(read(&lib.root, "../escape").is_err());
        let first = lib.note(); let second = lib.note();
        assert_ne!(first.id, second.id); assert_ne!(first.slug, second.slug);
    }
    #[test]
    fn a_collection_accepts_many_independent_notes_even_with_the_same_title() {
        let lib = Library::new();
        let notes: Vec<_> = (0..10).map(|i| create(&lib.root, Some(&lib.collection), "新建笔记", &format!("第 {i} 篇正文")).unwrap()).collect();
        assert_eq!(notes.iter().map(|n| &n.id).collect::<std::collections::HashSet<_>>().len(), 10);
        assert_eq!(notes.iter().map(|n| &n.slug).collect::<std::collections::HashSet<_>>().len(), 10);
        assert_eq!(collections::list_papers_in_collection(&lib.root, &lib.collection).unwrap().len(), 10);
        for (i, note) in notes.iter().enumerate() {
            assert_eq!(read(&lib.root, &note.slug).unwrap().content, format!("第 {i} 篇正文"));
        }
        update(&lib.root, &notes[0], "第一篇", "修改第一篇").unwrap();
        assert_eq!(read(&lib.root, &notes[9].slug).unwrap().content, "第 9 篇正文");
    }
    #[test]
    fn concurrent_writers_cannot_both_commit_the_same_old_snapshot() {
        let lib = Library::new(); let note = lib.note();
        let handles: Vec<_> = ["A", "B"].into_iter().map(|body| {
            let root = lib.root.clone(); let note = note.clone();
            std::thread::spawn(move || update(&root, &note, &note.title, body))
        }).collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        let current = read(&lib.root, &note.slug).unwrap();
        assert!(current.content == "A" || current.content == "B");
    }
    #[test]
    fn collection_moves_preserve_body_and_invalidate_an_old_location_preview() {
        let lib = Library::new(); let note = lib.note();
        collections::rename_collection(&lib.root, &lib.collection, "新的分类".into()).unwrap();
        let current = read(&lib.root, &note.slug).unwrap();
        assert_eq!(current.content, note.content); assert_eq!(current.id, note.id);
        assert!(update(&lib.root, &note, &note.title, "旧预览不能写").is_err());
        assert!(update(&lib.root, &current, &current.title, "核对后的正文").is_ok());
    }
    #[test]
    fn malformed_collection_data_is_preserved_and_never_treated_as_an_empty_tree() {
        let lib = Library::new(); let note = lib.note();
        let path = std::path::Path::new(&lib.root).join(".argus/collections.json");
        std::fs::write(&path, "{broken").unwrap();
        assert!(read(&lib.root, &note.slug).is_err());
        assert!(update(&lib.root, &note, "新标题", "正文").is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{broken");
    }
}
