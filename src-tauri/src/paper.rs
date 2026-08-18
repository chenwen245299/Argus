use std::path::{Path, PathBuf};

use crate::models::{Highlight, Note, PaperMeta, PaperStatus, ReadingState};

/// Write `content` to `path` atomically (temp file + rename), preventing
/// partial-write data loss if the process crashes mid-write.
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    crate::fsutil::atomic_write_str(path, content)
}

pub fn papers_dir(root: &str) -> PathBuf {
    Path::new(root).join("papers")
}

fn invalid_paper_dir(root: &str) -> PathBuf {
    papers_dir(root).join("__argus_invalid_paper_slug__")
}

fn validate_slug(slug: &str) -> Result<(), String> {
    crate::path_guard::validate_segment("paper slug", slug)
}

fn validate_note_id(note_id: &str) -> Result<(), String> {
    crate::path_guard::validate_segment("note id", note_id)
}

fn note_file_path(root: &str, slug: &str, note_id: &str) -> Result<PathBuf, String> {
    validate_slug(slug)?;
    validate_note_id(note_id)?;
    Ok(notes_dir(root, slug).join(format!("{note_id}.md")))
}

pub fn find_paper_dir(root: &str, slug: &str) -> Option<PathBuf> {
    validate_slug(slug).ok()?;
    let direct = papers_dir(root).join(slug);
    let direct_is_real_dir = std::fs::symlink_metadata(&direct)
        .ok()
        .is_some_and(|m| m.is_dir() && !m.file_type().is_symlink());
    if direct_is_real_dir && direct.join("meta.json").exists() {
        return Some(direct);
    }

    list_paper_dirs(root)
        .ok()?
        .into_iter()
        .find_map(|(s, path)| if s == slug { Some(path) } else { None })
}

pub fn paper_dir(root: &str, slug: &str) -> PathBuf {
    if validate_slug(slug).is_err() {
        return invalid_paper_dir(root);
    }
    find_paper_dir(root, slug).unwrap_or_else(|| papers_dir(root).join(slug))
}

pub fn list_paper_dirs(root: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    let base = papers_dir(root);
    if !base.exists() {
        return Ok(out);
    }
    collect_paper_dirs(&base, &mut out)?;
    Ok(out)
}

fn collect_paper_dirs(dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("Read {}: {e}", dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }

        if path.join("meta.json").exists() {
            if let Some(slug) = path.file_name().and_then(|n| n.to_str()) {
                out.push((slug.to_string(), path));
            }
        } else {
            collect_paper_dirs(&path, out)?;
        }
    }

    Ok(())
}

// ── PaperMeta ─────────────────────────────────────────────────────────────────

pub fn read_meta(root: &str, slug: &str) -> Result<PaperMeta, String> {
    validate_slug(slug)?;
    let path = paper_dir(root, slug).join("meta.json");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read meta.json for {slug}: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse meta.json for {slug}: {e}"))
}

pub fn write_meta(root: &str, slug: &str, meta: &PaperMeta) -> Result<(), String> {
    validate_slug(slug)?;
    let path = paper_dir(root, slug).join("meta.json");
    let mut meta = meta.clone();
    meta.import_source = Some(crate::models::normalize_import_source(
        meta.import_source.as_deref(),
        meta.arxiv_id.as_deref(),
    ));
    let content = serde_json::to_string_pretty(&meta)
        .map_err(|e| format!("Failed to serialize meta: {e}"))?;
    atomic_write(&path, &content)
}

// ── Notes ─────────────────────────────────────────────────────────────────────

pub fn read_notes(root: &str, slug: &str) -> String {
    if validate_slug(slug).is_err() {
        return String::new();
    }
    let path = paper_dir(root, slug).join("notes.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

pub fn write_notes(root: &str, slug: &str, content: &str) -> Result<(), String> {
    validate_slug(slug)?;
    let path = paper_dir(root, slug).join("notes.md");
    atomic_write(&path, content).map_err(|e| format!("Failed to write notes.md: {e}"))
}

// ── Multi-Notes ───────────────────────────────────────────────────────────────

fn notes_dir(root: &str, slug: &str) -> PathBuf {
    paper_dir(root, slug).join("notes")
}

fn read_notes_index(root: &str, slug: &str) -> Vec<Note> {
    if validate_slug(slug).is_err() {
        return Vec::new();
    }
    let path = notes_dir(root, slug).join("index.json");
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_notes_index(root: &str, slug: &str, notes: &[Note]) -> Result<(), String> {
    validate_slug(slug)?;
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;
    let path = dir.join("index.json");
    let content = serde_json::to_string_pretty(notes)
        .map_err(|e| format!("Failed to serialize notes index: {e}"))?;
    atomic_write(&path, &content)
}

/// One-time migration: if notes/ doesn't exist yet but notes.md does, import it as the first note.
fn maybe_migrate_legacy_note(root: &str, slug: &str) {
    if validate_slug(slug).is_err() {
        return;
    }
    let dir = notes_dir(root, slug);
    if dir.exists() {
        return;
    }
    let legacy = paper_dir(root, slug).join("notes.md");
    let content = legacy
        .exists()
        .then(|| std::fs::read_to_string(&legacy).ok())
        .flatten()
        .unwrap_or_default();
    let _ = std::fs::create_dir_all(&dir);
    if content.trim().is_empty() {
        let _ = write_notes_index(root, slug, &[]);
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let note = Note {
        id: id.clone(),
        title: "笔记".to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    let _ = std::fs::write(dir.join(format!("{id}.md")), &content);
    let _ = write_notes_index(root, slug, &[note]);
}

pub fn list_notes(root: &str, slug: &str) -> Vec<Note> {
    maybe_migrate_legacy_note(root, slug);
    read_notes_index(root, slug)
}

pub fn get_note(root: &str, slug: &str, note_id: &str) -> String {
    let Ok(path) = note_file_path(root, slug, note_id) else {
        return String::new();
    };
    std::fs::read_to_string(&path).unwrap_or_default()
}

pub fn get_note_by_title(root: &str, slug: &str, title: &str) -> Option<String> {
    maybe_migrate_legacy_note(root, slug);
    read_notes_index(root, slug)
        .into_iter()
        .find(|n| n.title == title)
        .map(|n| get_note(root, slug, &n.id))
}

pub fn create_note(root: &str, slug: &str) -> Result<Note, String> {
    validate_slug(slug)?;
    maybe_migrate_legacy_note(root, slug);
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let note = Note {
        id: id.clone(),
        title: "新建笔记".to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;
    std::fs::write(dir.join(format!("{id}.md")), "")
        .map_err(|e| format!("Failed to create note file: {e}"))?;
    let mut notes = read_notes_index(root, slug);
    notes.push(note.clone());
    write_notes_index(root, slug, &notes)?;
    Ok(note)
}

/// Create a brand-new note carrying `title` and `content`.
///
/// Always a *new* note. Unlike [`upsert_note_by_title`] it never opens an
/// existing one, even when the title is identical — a duplicate title is
/// allowed, since notes are addressed by id. That is the property that makes
/// this safe to reach from the agent's write tool: the worst outcome is a note
/// the user deletes, never a paragraph of theirs that is gone.
///
/// The title only ever lands in `index.json`; the file on disk is named after a
/// fresh uuid, so nothing here can be steered at another path.
pub fn create_note_with(root: &str, slug: &str, title: &str, content: &str) -> Result<Note, String> {
    validate_slug(slug)?;
    maybe_migrate_legacy_note(root, slug);
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;

    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().to_rfc3339();
    let note = Note {
        id: id.clone(),
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    atomic_write(&dir.join(format!("{id}.md")), content)
        .map_err(|e| format!("Failed to create note file: {e}"))?;
    let mut notes = read_notes_index(root, slug);
    notes.push(note.clone());
    write_notes_index(root, slug, &notes)?;
    Ok(note)
}

pub fn upsert_note_by_title(
    root: &str,
    slug: &str,
    title: &str,
    content: &str,
) -> Result<Note, String> {
    validate_slug(slug)?;
    maybe_migrate_legacy_note(root, slug);
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;

    let mut notes = read_notes_index(root, slug);
    let now = chrono::Utc::now().to_rfc3339();

    if let Some(note) = notes.iter_mut().find(|n| n.title == title) {
        note.updated_at = now;
        atomic_write(&dir.join(format!("{}.md", note.id)), content)
            .map_err(|e| format!("Failed to write note: {e}"))?;
        let updated = note.clone();
        write_notes_index(root, slug, &notes)?;
        return Ok(updated);
    }

    let id = uuid::Uuid::new_v4().to_string();
    let note = Note {
        id: id.clone(),
        title: title.to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    atomic_write(&dir.join(format!("{id}.md")), content)
        .map_err(|e| format!("Failed to create note file: {e}"))?;
    notes.push(note.clone());
    write_notes_index(root, slug, &notes)?;
    Ok(note)
}

pub fn save_note(root: &str, slug: &str, note_id: &str, content: &str) -> Result<(), String> {
    validate_slug(slug)?;
    validate_note_id(note_id)?;
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;
    atomic_write(&note_file_path(root, slug, note_id)?, content)
        .map_err(|e| format!("Failed to write note: {e}"))?;
    let mut notes = read_notes_index(root, slug);
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(n) = notes.iter_mut().find(|n| n.id == note_id) {
        n.updated_at = now;
    }
    write_notes_index(root, slug, &notes)
}

pub fn rename_note(root: &str, slug: &str, note_id: &str, title: &str) -> Result<(), String> {
    validate_slug(slug)?;
    validate_note_id(note_id)?;
    let mut notes = read_notes_index(root, slug);
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(n) = notes.iter_mut().find(|n| n.id == note_id) {
        n.title = title.to_string();
        n.updated_at = now;
    }
    write_notes_index(root, slug, &notes)
}

pub fn delete_note(root: &str, slug: &str, note_id: &str) -> Result<(), String> {
    validate_slug(slug)?;
    validate_note_id(note_id)?;
    let note_file = note_file_path(root, slug, note_id)?;
    if note_file.exists() {
        std::fs::remove_file(&note_file).map_err(|e| format!("Failed to delete note file: {e}"))?;
    }
    let mut notes = read_notes_index(root, slug);
    let was_ai_summary = notes.iter().any(|n| n.id == note_id && n.title == "AI总结");
    notes.retain(|n| n.id != note_id);
    write_notes_index(root, slug, &notes)?;
    if was_ai_summary {
        let mut status = read_status_for(root, slug);
        status.ai_summary_done = false;
        status.last_updated = chrono::Utc::now().to_rfc3339();
        write_status(root, slug, &status)?;
    }
    Ok(())
}

// ── Note assets (pasted images) ───────────────────────────────────────────────
// Images pasted into a note are written next to it, under `notes/assets/`, and
// referenced from the markdown by the relative path `assets/<name>`. Keeping the
// link relative is what makes a paper folder self-contained: copy it elsewhere
// and the notes still resolve. Absolute paths (or an inlined data: URI) would
// not survive the move.

/// Extensions we're willing to write and hand back to a webview `<img>`. SVG is
/// deliberately absent — it can carry script, and these bytes come from whatever
/// happened to be on the clipboard.
const NOTE_ASSET_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// Cap on a single pasted image. Large enough for a retina screenshot, small
/// enough that a stray paste can't wedge the editor or bloat the library.
const NOTE_ASSET_MAX_BYTES: usize = 20 * 1024 * 1024;

fn note_assets_dir(root: &str, slug: &str) -> PathBuf {
    notes_dir(root, slug).join("assets")
}

fn note_asset_path(root: &str, slug: &str, name: &str) -> Result<PathBuf, String> {
    validate_slug(slug)?;
    crate::path_guard::validate_segment("asset name", name)?;
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !NOTE_ASSET_EXTS.contains(&ext.as_str()) {
        return Err(format!("Unsupported image type: {ext}"));
    }
    Ok(note_assets_dir(root, slug).join(name))
}

/// Write a pasted image and return its note-relative path (`assets/<name>`).
pub fn write_note_asset(root: &str, slug: &str, ext: &str, bytes: &[u8]) -> Result<String, String> {
    if bytes.is_empty() {
        return Err("Image is empty".to_string());
    }
    if bytes.len() > NOTE_ASSET_MAX_BYTES {
        return Err(format!(
            "Image is too large ({} MB); the limit is {} MB",
            bytes.len() / (1024 * 1024),
            NOTE_ASSET_MAX_BYTES / (1024 * 1024)
        ));
    }
    let ext = ext.trim().trim_start_matches('.').to_ascii_lowercase();
    if !NOTE_ASSET_EXTS.contains(&ext.as_str()) {
        return Err(format!("Unsupported image type: {ext}"));
    }
    let dir = note_assets_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create assets dir: {e}"))?;

    // Named by local wall-clock time so the folder reads chronologically and a
    // file can be matched to when it was pasted. Two pastes inside one second
    // are entirely possible, hence the `-2`, `-3` … fallback.
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let (name, path) = (1..=999)
        .find_map(|n| {
            let name = if n == 1 {
                format!("{stamp}.{ext}")
            } else {
                format!("{stamp}-{n}.{ext}")
            };
            let path = note_asset_path(root, slug, &name).ok()?;
            (!path.exists()).then_some((name, path))
        })
        .ok_or_else(|| "Could not find a free filename for the image".to_string())?;

    std::fs::write(&path, bytes).map_err(|e| format!("Failed to write image: {e}"))?;
    Ok(format!("assets/{name}"))
}

/// Read a note asset back for display. Returns the raw bytes; the caller turns
/// them into a blob URL rather than embedding them in the markdown.
pub fn read_note_asset(root: &str, slug: &str, name: &str) -> Result<Vec<u8>, String> {
    let path = note_asset_path(root, slug, name)?;
    // `note_asset_path` already rejects separators and `..`, so the join cannot
    // escape the assets dir; this is belt-and-braces against symlinked entries.
    let meta = std::fs::symlink_metadata(&path)
        .map_err(|e| format!("Failed to read image: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err("Not a regular file".to_string());
    }
    if meta.len() as usize > NOTE_ASSET_MAX_BYTES {
        return Err("Image is too large".to_string());
    }
    std::fs::read(&path).map_err(|e| format!("Failed to read image: {e}"))
}

// ── Highlights ────────────────────────────────────────────────────────────────

pub fn read_highlights(root: &str, slug: &str) -> Vec<Highlight> {
    if validate_slug(slug).is_err() {
        return Vec::new();
    }
    let path = paper_dir(root, slug).join("highlights.json");
    if !path.exists() {
        return Vec::new();
    }
    match std::fs::read_to_string(&path) {
        Err(_) => Vec::new(),
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
    }
}

pub fn write_highlights(root: &str, slug: &str, highlights: &[Highlight]) -> Result<(), String> {
    validate_slug(slug)?;
    let path = paper_dir(root, slug).join("highlights.json");
    let content = serde_json::to_string_pretty(highlights)
        .map_err(|e| format!("Failed to serialize highlights: {e}"))?;
    atomic_write(&path, &content)
}

// ── ReadingState ──────────────────────────────────────────────────────────────

pub fn read_reading_state(root: &str, slug: &str) -> Option<ReadingState> {
    if validate_slug(slug).is_err() {
        return None;
    }
    let path = paper_dir(root, slug).join("reading_state.json");
    if !path.exists() {
        return None;
    }
    match std::fs::read_to_string(&path) {
        Err(_) => None,
        Ok(content) => serde_json::from_str(&content).ok(),
    }
}

pub fn write_reading_state(root: &str, slug: &str, state: &ReadingState) -> Result<(), String> {
    validate_slug(slug)?;
    let path = paper_dir(root, slug).join("reading_state.json");
    let content = serde_json::to_string_pretty(state)
        .map_err(|e| format!("Failed to serialize reading_state: {e}"))?;
    atomic_write(&path, &content)
}

// ── PaperStatus ───────────────────────────────────────────────────────────────

pub fn read_status(paper_path: &Path) -> PaperStatus {
    let path = paper_path.join(".status.json");
    if !path.exists() {
        return PaperStatus::default();
    }
    match std::fs::read_to_string(&path) {
        Err(_) => PaperStatus::default(),
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
    }
}

pub fn read_status_for(root: &str, slug: &str) -> PaperStatus {
    if validate_slug(slug).is_err() {
        return PaperStatus::default();
    }
    read_status(&paper_dir(root, slug))
}

pub fn write_status(root: &str, slug: &str, status: &PaperStatus) -> Result<(), String> {
    validate_slug(slug)?;
    let path = paper_dir(root, slug).join(".status.json");
    // Read existing, merge, then write — preserves any fields added in the future.
    let merged = if path.exists() {
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str::<PaperStatus>(&c).ok())
        {
            Some(_existing) => status.clone(), // full replace since we own the whole struct
            None => status.clone(),
        }
    } else {
        status.clone()
    };
    let content = serde_json::to_string_pretty(&merged)
        .map_err(|e| format!("Failed to serialize status: {e}"))?;
    atomic_write(&path, &content)
}

/// Ensure default companion files exist for a newly created paper dir.
pub fn ensure_paper_files(root: &str, slug: &str) {
    if validate_slug(slug).is_err() {
        return;
    }
    let dir = paper_dir(root, slug);

    let notes = dir.join("notes.md");
    if !notes.exists() {
        let _ = std::fs::write(&notes, "");
    }

    let highlights = dir.join("highlights.json");
    if !highlights.exists() {
        let _ = std::fs::write(&highlights, "[]");
    }

    let status = dir.join(".status.json");
    if !status.exists() {
        if let Ok(s) = serde_json::to_string_pretty(&PaperStatus::default()) {
            let _ = std::fs::write(&status, s);
        }
    }
}
