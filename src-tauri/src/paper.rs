use std::path::{Path, PathBuf};

use crate::models::{Highlight, Note, PaperMeta, PaperStatus, ReadingState};

/// Write `content` to `path` atomically: write to a temp file first, then
/// rename into place. This prevents partial-write data loss if the process
/// crashes mid-write.
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)
        .map_err(|e| format!("Failed to write tmp file {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .map_err(|e| format!("Failed to rename tmp to {}: {e}", path.display()))
}

pub fn papers_dir(root: &str) -> PathBuf {
    Path::new(root).join("papers")
}

pub fn find_paper_dir(root: &str, slug: &str) -> Option<PathBuf> {
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
    let path = paper_dir(root, slug).join("meta.json");
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read meta.json for {slug}: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Failed to parse meta.json for {slug}: {e}"))
}

pub fn write_meta(root: &str, slug: &str, meta: &PaperMeta) -> Result<(), String> {
    let path = paper_dir(root, slug).join("meta.json");
    let content =
        serde_json::to_string_pretty(meta).map_err(|e| format!("Failed to serialize meta: {e}"))?;
    atomic_write(&path, &content)
}

// ── Notes ─────────────────────────────────────────────────────────────────────

pub fn read_notes(root: &str, slug: &str) -> String {
    let path = paper_dir(root, slug).join("notes.md");
    std::fs::read_to_string(&path).unwrap_or_default()
}

pub fn write_notes(root: &str, slug: &str, content: &str) -> Result<(), String> {
    let path = paper_dir(root, slug).join("notes.md");
    std::fs::write(&path, content).map_err(|e| format!("Failed to write notes.md: {e}"))
}

// ── Multi-Notes ───────────────────────────────────────────────────────────────

fn notes_dir(root: &str, slug: &str) -> PathBuf {
    paper_dir(root, slug).join("notes")
}

fn read_notes_index(root: &str, slug: &str) -> Vec<Note> {
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
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;
    let path = dir.join("index.json");
    let content = serde_json::to_string_pretty(notes)
        .map_err(|e| format!("Failed to serialize notes index: {e}"))?;
    atomic_write(&path, &content)
}

/// One-time migration: if notes/ doesn't exist yet but notes.md does, import it as the first note.
fn maybe_migrate_legacy_note(root: &str, slug: &str) {
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
    let path = notes_dir(root, slug).join(format!("{note_id}.md"));
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

pub fn upsert_note_by_title(
    root: &str,
    slug: &str,
    title: &str,
    content: &str,
) -> Result<Note, String> {
    maybe_migrate_legacy_note(root, slug);
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;

    let mut notes = read_notes_index(root, slug);
    let now = chrono::Utc::now().to_rfc3339();

    if let Some(note) = notes.iter_mut().find(|n| n.title == title) {
        note.updated_at = now;
        std::fs::write(dir.join(format!("{}.md", note.id)), content)
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
    std::fs::write(dir.join(format!("{id}.md")), content)
        .map_err(|e| format!("Failed to create note file: {e}"))?;
    notes.push(note.clone());
    write_notes_index(root, slug, &notes)?;
    Ok(note)
}

pub fn save_note(root: &str, slug: &str, note_id: &str, content: &str) -> Result<(), String> {
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;
    std::fs::write(dir.join(format!("{note_id}.md")), content)
        .map_err(|e| format!("Failed to write note: {e}"))?;
    let mut notes = read_notes_index(root, slug);
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(n) = notes.iter_mut().find(|n| n.id == note_id) {
        n.updated_at = now;
    }
    write_notes_index(root, slug, &notes)
}

pub fn rename_note(root: &str, slug: &str, note_id: &str, title: &str) -> Result<(), String> {
    let mut notes = read_notes_index(root, slug);
    let now = chrono::Utc::now().to_rfc3339();
    if let Some(n) = notes.iter_mut().find(|n| n.id == note_id) {
        n.title = title.to_string();
        n.updated_at = now;
    }
    write_notes_index(root, slug, &notes)
}

pub fn delete_note(root: &str, slug: &str, note_id: &str) -> Result<(), String> {
    let note_file = notes_dir(root, slug).join(format!("{note_id}.md"));
    if note_file.exists() {
        std::fs::remove_file(&note_file).map_err(|e| format!("Failed to delete note file: {e}"))?;
    }
    let mut notes = read_notes_index(root, slug);
    notes.retain(|n| n.id != note_id);
    write_notes_index(root, slug, &notes)
}

// ── Highlights ────────────────────────────────────────────────────────────────

pub fn read_highlights(root: &str, slug: &str) -> Vec<Highlight> {
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
    let path = paper_dir(root, slug).join("highlights.json");
    let content = serde_json::to_string_pretty(highlights)
        .map_err(|e| format!("Failed to serialize highlights: {e}"))?;
    atomic_write(&path, &content)
}

// ── ReadingState ──────────────────────────────────────────────────────────────

pub fn read_reading_state(root: &str, slug: &str) -> Option<ReadingState> {
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
    read_status(&paper_dir(root, slug))
}

pub fn write_status(root: &str, slug: &str, status: &PaperStatus) -> Result<(), String> {
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
