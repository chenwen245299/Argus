use std::path::{Path, PathBuf};

use crate::models::{
    Highlight, HighlightTombstone, HighlightsDoc, Note, NoteSnapshot, DeletedNote, PaperMeta, PaperStatus, ReadingState,
};

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

/// Slugs are library-wide identities, including papers inside collections.
/// Reserve before downloading/writing; two concurrent imports must never share
/// a directory, even before either has written its meta.json.
pub(crate) fn namespace_lock(root: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<LockMap> = std::sync::OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.entry(root.to_string()).or_default().clone()
}

pub(crate) fn free_slug(root: &str, base: &str, parent: &Path) -> Result<String, String> {
    validate_slug(base)?;
    let occupied: std::collections::HashSet<String> = list_paper_dirs(root)?
        .into_iter().map(|(slug, _)| slug).collect();
    for n in 1..=10_000 {
        let slug = if n == 1 { base.to_string() } else { format!("{base}-{n}") };
        if !occupied.contains(&slug) && !parent.join(&slug).exists() {
            return Ok(slug);
        }
    }
    Ok(format!("{base}-{}", uuid::Uuid::new_v4()))
}

pub fn reserve_paper_dir(root: &str, base: &str) -> Result<PathBuf, String> {
    let lock = namespace_lock(root);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let parent = papers_dir(root);
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let slug = free_slug(root, base, &parent)?;
    let dir = parent.join(slug);
    std::fs::create_dir(&dir).map_err(|e| format!("Reserve paper directory: {e}"))?;
    Ok(dir)
}

pub async fn reserve_import_dir(root: &str, base: &str) -> Result<PathBuf, String> {
    let (root, base) = (root.to_string(), base.to_string());
    tauri::async_runtime::spawn_blocking(move || reserve_paper_dir(&root, &base))
        .await.map_err(|e| format!("Reserve paper task: {e}"))?
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

fn read_notes_index_checked(root: &str, slug: &str) -> Result<Vec<Note>, String> {
    validate_slug(slug)?;
    let path = notes_dir(root, slug).join("index.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("Invalid notes index: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("Read notes index: {e}")),
    }
}

fn read_notes_index(root: &str, slug: &str) -> Vec<Note> {
    read_notes_index_checked(root, slug).unwrap_or_default()
}

fn notes_lock(root: &str, slug: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<LockMap> = std::sync::OnceLock::new();
    let mut locks = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    locks.entry(format!("{root}\0{slug}")).or_default().clone()
}

fn check_notes_dir(root: &str, slug: &str) -> Result<(), String> {
    validate_slug(slug)?;
    if find_paper_dir(root, slug).is_none() { return Err("Paper no longer exists.".into()); }
    match std::fs::symlink_metadata(notes_dir(root, slug)) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err("Notes folder must be a real directory.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Read notes folder: {e}")),
    }
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
    if atomic_write(&dir.join(format!("{id}.md")), &content).is_ok()
        && write_notes_index(root, slug, &[note]).is_ok() {
        let _ = std::fs::remove_file(legacy);
    }
}

pub fn list_notes(root: &str, slug: &str) -> Vec<Note> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if check_notes_dir(root, slug).is_err() { return vec![]; }
    maybe_migrate_legacy_note(root, slug);
    read_notes_index(root, slug)
}

pub fn get_note(root: &str, slug: &str, note_id: &str) -> String {
    note_snapshot(root, slug, note_id).map(|s| s.content).unwrap_or_default()
}

pub fn get_note_by_title(root: &str, slug: &str, title: &str) -> Option<String> {
    maybe_migrate_legacy_note(root, slug);
    read_notes_index(root, slug)
        .into_iter()
        .find(|n| n.title == title)
        .map(|n| get_note(root, slug, &n.id))
}

pub fn create_note(root: &str, slug: &str) -> Result<Note, String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    check_notes_dir(root, slug)?;
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
    let mut notes = read_notes_index_checked(root, slug)?;
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
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    check_notes_dir(root, slug)?;
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
    let mut notes = read_notes_index_checked(root, slug)?;
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
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    check_notes_dir(root, slug)?;
    validate_slug(slug)?;
    maybe_migrate_legacy_note(root, slug);
    let dir = notes_dir(root, slug);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create notes dir: {e}"))?;

    let mut notes = read_notes_index_checked(root, slug)?;
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
    save_note_if_current(root, slug, note_id, content, None)
}

pub fn save_note_if_current(root: &str, slug: &str, note_id: &str, content: &str, expected_content: Option<&str>) -> Result<(), String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let before = note_snapshot_unlocked(root, slug, note_id)?;
    if expected_content.is_some_and(|expected| expected != before.content) {
        return Err("Note changed in another window or by AI. Your draft is preserved; reopen the note to review the saved version.".into());
    }
    update_note_unlocked(root, slug, &before, &before.note.title, content).map(|_| ())
}

pub fn rename_note(root: &str, slug: &str, note_id: &str, title: &str) -> Result<(), String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let before = note_snapshot_unlocked(root, slug, note_id)?;
    update_note_unlocked(root, slug, &before, title, &before.content).map(|_| ())
}

pub fn delete_note(root: &str, slug: &str, note_id: &str) -> Result<(), String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let before = note_snapshot_unlocked(root, slug, note_id)?;
    trash_note_unlocked(root, slug, &before).map(|_| ())
}

fn note_snapshot_unlocked(root: &str, slug: &str, note_id: &str) -> Result<NoteSnapshot, String> {
    check_notes_dir(root, slug)?;
    let path = note_file_path(root, slug, note_id)?;
    let note = read_notes_index_checked(root, slug)?.into_iter().find(|n| n.id == note_id)
        .ok_or("Note no longer exists or is in Recently Deleted.")?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| format!("Read note: {e}"))?;
    if !meta.is_file() || meta.file_type().is_symlink() { return Err("Note must be a regular file.".into()); }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("Read note: {e}"))?;
    Ok(NoteSnapshot { note, content })
}

pub fn note_snapshot(root: &str, slug: &str, note_id: &str) -> Result<NoteSnapshot, String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    note_snapshot_unlocked(root, slug, note_id)
}

fn stamp_after(time: &str) -> String {
    let now = chrono::Utc::now();
    let stamp = chrono::DateTime::parse_from_rfc3339(time).ok()
        .map(|t| t.with_timezone(&chrono::Utc) + chrono::Duration::milliseconds(1))
        .map(|t| t.max(now)).unwrap_or(now);
    stamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn update_note_unlocked(root: &str, slug: &str, before: &NoteSnapshot, title: &str, content: &str) -> Result<Note, String> {
    let mut notes = read_notes_index_checked(root, slug)?;
    let note = notes.iter_mut().find(|n| n.id == before.note.id).ok_or("Note no longer exists.")?;
    note.title = title.to_string();
    note.updated_at = stamp_after(&before.note.updated_at);
    let updated = note.clone();
    atomic_write(&note_file_path(root, slug, &before.note.id)?, content)?;
    if let Err(error) = write_notes_index(root, slug, &notes) {
        let _ = atomic_write(&note_file_path(root, slug, &before.note.id)?, &before.content);
        return Err(error);
    }
    Ok(updated)
}

pub fn update_note_checked(root: &str, slug: &str, before: &NoteSnapshot, title: &str, content: &str) -> Result<Note, String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if note_snapshot_unlocked(root, slug, &before.note.id)? != *before {
        return Err("Note changed since the preview. Nothing was saved; read it again.".into());
    }
    update_note_unlocked(root, slug, before, title, content)
}

fn trash_dir(root: &str, slug: &str, create: bool) -> Result<PathBuf, String> {
    check_notes_dir(root, slug)?;
    let dir = notes_dir(root, slug).join(".trash");
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => return Err("Recently Deleted must be a real directory.".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
            std::fs::create_dir(&dir).map_err(|e| format!("Create Recently Deleted: {e}"))?;
        }
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("Read Recently Deleted: {e}")),
        _ => {}
    }
    Ok(dir)
}

pub const NOTE_RETENTION_DAYS: i64 = 30;
fn note_expired(note: &DeletedNote, now: chrono::DateTime<chrono::Utc>) -> Result<bool, String> {
    let deleted = chrono::DateTime::parse_from_rfc3339(&note.deleted_at).map_err(|e| format!("Invalid deletion time: {e}"))?;
    Ok(now >= deleted + chrono::Duration::days(NOTE_RETENTION_DAYS))
}

fn deleted_notes_unlocked(root: &str, slug: &str) -> Result<Vec<DeletedNote>, String> {
    let dir = trash_dir(root, slug, false)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("Read Recently Deleted: {e}")),
    };
    let mut notes = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().map_or(true, |ext| ext != "json") { continue; }
        let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.file_type().is_symlink() { return Err("Deleted note must be a regular file.".into()); }
        let note: DeletedNote = serde_json::from_str(&std::fs::read_to_string(entry.path()).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid deleted note: {e}"))?;
        validate_note_id(&note.snapshot.note.id)?;
        if entry.file_name() != std::ffi::OsString::from(format!("{}.json", note.snapshot.note.id)) { return Err("Deleted note identity does not match its file.".into()); }
        notes.push(note);
    }
    Ok(notes)
}

pub fn list_deleted_notes(root: &str, slug: &str) -> Result<Vec<DeletedNote>, String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let active = read_notes_index_checked(root, slug)?;
    let now = chrono::Utc::now();
    let mut notes = deleted_notes_unlocked(root, slug)?;
    notes.retain(|n| !active.iter().any(|a| a.id == n.snapshot.note.id) && !note_expired(n, now).unwrap_or(true));
    notes.sort_by(|a, b| b.deleted_at.cmp(&a.deleted_at));
    Ok(notes)
}

fn trash_note_unlocked(root: &str, slug: &str, before: &NoteSnapshot) -> Result<DeletedNote, String> {
    let now = chrono::Utc::now();
    let deleted = DeletedNote { snapshot: before.clone(), deleted_at: now.to_rfc3339(),
        expires_at: (now + chrono::Duration::days(NOTE_RETENTION_DAYS)).to_rfc3339() };
    let dir = trash_dir(root, slug, true)?;
    let path = dir.join(format!("{}.json", before.note.id));
    if std::fs::symlink_metadata(&path).is_ok() { return Err("A deleted copy with this id already exists. Resolve it before deleting again.".into()); }
    atomic_write(&path, &serde_json::to_string_pretty(&deleted).map_err(|e| e.to_string())?)?;
    let mut notes = read_notes_index_checked(root, slug)?;
    notes.retain(|n| n.id != before.note.id);
    if let Err(error) = write_notes_index(root, slug, &notes) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    // The index is authoritative. A stale editor cannot recreate this file.
    let _ = std::fs::remove_file(note_file_path(root, slug, &before.note.id)?);
    if before.note.title == "AI总结" {
        let mut status = read_status_for(root, slug);
        status.ai_summary_done = false;
        let _ = write_status(root, slug, &status);
    }
    Ok(deleted)
}

pub fn delete_note_checked(root: &str, slug: &str, before: &NoteSnapshot) -> Result<DeletedNote, String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    if note_snapshot_unlocked(root, slug, &before.note.id)? != *before {
        return Err("Note changed since the preview. Nothing was deleted; read it again.".into());
    }
    trash_note_unlocked(root, slug, before)
}

pub fn restore_note(root: &str, slug: &str, note_id: &str, expected: Option<&DeletedNote>) -> Result<Note, String> {
    validate_note_id(note_id)?;
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let deleted = deleted_notes_unlocked(root, slug)?.into_iter().find(|n| n.snapshot.note.id == note_id)
        .ok_or("Deleted note was not found.")?;
    if note_expired(&deleted, chrono::Utc::now())? { return Err("This note's 30-day recovery period has expired.".into()); }
    if expected.is_some_and(|old| old != &deleted) { return Err("Deleted note changed since the preview. Read it again.".into()); }
    let mut notes = read_notes_index_checked(root, slug)?;
    if notes.iter().any(|n| n.id == note_id) { return Err("This note has already been restored; no existing note was overwritten.".into()); }
    let mut note = deleted.snapshot.note.clone();
    note.updated_at = stamp_after(&note.updated_at);
    let path = note_file_path(root, slug, note_id)?;
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) { return Err("Note must not be a link.".into()); }
    atomic_write(&path, &deleted.snapshot.content)?;
    notes.push(note.clone());
    write_notes_index(root, slug, &notes)?;
    let _ = std::fs::remove_file(trash_dir(root, slug, false)?.join(format!("{note_id}.json")));
    if note.title == "AI总结" {
        let mut status = read_status_for(root, slug);
        status.ai_summary_done = true;
        let _ = write_status(root, slug, &status);
    }
    Ok(note)
}

pub fn prune_deleted_notes(root: &str, slug: &str, now: chrono::DateTime<chrono::Utc>) -> Result<usize, String> {
    let lock = notes_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let dir = trash_dir(root, slug, false)?;
    let active = read_notes_index_checked(root, slug)?;
    let mut count = 0;
    for deleted in deleted_notes_unlocked(root, slug)? {
        if note_expired(&deleted, now)? {
            if !active.iter().any(|n| n.id == deleted.snapshot.note.id) {
                match std::fs::remove_file(note_file_path(root, slug, &deleted.snapshot.note.id)?) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.to_string()),
                    _ => {}
                }
            }
            std::fs::remove_file(dir.join(format!("{}.json", deleted.snapshot.note.id))).map_err(|e| e.to_string())?;
            count += 1;
        }
    }
    Ok(count)
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

fn highlights_path(root: &str, slug: &str) -> PathBuf {
    paper_dir(root, slug).join("highlights.json")
}

/// The effective time of a highlight for conflict resolution: its last edit, or
/// its creation time when it has never been edited.
pub fn highlight_time(h: &Highlight) -> &str {
    h.updated_at.as_deref().unwrap_or(&h.created_at)
}

/// Parse the highlights file, accepting both the merge-friendly doc object and
/// the legacy bare array. A missing/corrupt file reads as empty.
pub fn read_highlights_doc(root: &str, slug: &str) -> HighlightsDoc {
    if validate_slug(slug).is_err() {
        return HighlightsDoc::default();
    }
    let Ok(content) = std::fs::read_to_string(highlights_path(root, slug)) else {
        return HighlightsDoc::default();
    };
    // v2 object first (an empty `{}` lands here too), then the legacy bare array.
    if let Ok(doc) = serde_json::from_str::<HighlightsDoc>(&content) {
        return doc;
    }
    match serde_json::from_str::<Vec<Highlight>>(&content) {
        Ok(highlights) => HighlightsDoc { highlights, tombstones: Vec::new() },
        Err(_) => HighlightsDoc::default(),
    }
}

/// Effective (non-deleted) highlights — what the UI, RAG and search consume.
pub fn read_highlights(root: &str, slug: &str) -> Vec<Highlight> {
    effective_highlights(&read_highlights_doc(root, slug))
}

fn effective_highlights(doc: &HighlightsDoc) -> Vec<Highlight> {
    let deleted: std::collections::HashMap<&str, &str> = doc
        .tombstones
        .iter()
        .map(|t| (t.id.as_str(), t.deleted_at.as_str()))
        .collect();
    doc.highlights
        .iter()
        .filter(|h| match deleted.get(h.id.as_str()) {
            // A delete wins only if it is at least as new as the last edit.
            Some(&d) => highlight_time(h) > d,
            None => true,
        })
        .cloned()
        .collect()
}

/// Merge one machine's `incoming` highlights/tombstones into the on-disk doc:
/// union highlights by id (newer edit wins), union tombstones (newer delete
/// wins), and drop any highlight a not-yet-superseded tombstone covers. A stale
/// machine's save can no longer wipe annotations another machine synced in, and a
/// real delete propagates instead of resurrecting.
fn merge_highlights(
    disk: HighlightsDoc,
    incoming_highlights: Vec<Highlight>,
    incoming_tombstones: Vec<HighlightTombstone>,
) -> HighlightsDoc {
    use std::collections::HashMap;

    let mut by_id: HashMap<String, Highlight> = HashMap::new();
    for h in disk.highlights.into_iter().chain(incoming_highlights) {
        match by_id.get(&h.id) {
            Some(existing) if highlight_time(existing) >= highlight_time(&h) => {}
            _ => {
                by_id.insert(h.id.clone(), h);
            }
        }
    }

    let mut tombs: HashMap<String, String> = HashMap::new();
    for t in disk.tombstones.into_iter().chain(incoming_tombstones) {
        if tombs.get(&t.id).map(|d| t.deleted_at > *d).unwrap_or(true) {
            tombs.insert(t.id, t.deleted_at);
        }
    }
    // A re-edit at/after the delete revives the highlight and retires its tombstone.
    tombs.retain(|id, deleted_at| match by_id.get(id) {
        Some(h) => highlight_time(h) <= deleted_at.as_str(),
        None => true,
    });

    let mut highlights: Vec<Highlight> = by_id
        .into_iter()
        .filter(|(id, _)| !tombs.contains_key(id))
        .map(|(_, h)| h)
        .collect();
    highlights.sort_by(|a, b| a.page.cmp(&b.page).then_with(|| a.created_at.cmp(&b.created_at)));
    let mut tombstones: Vec<HighlightTombstone> = tombs
        .into_iter()
        .map(|(id, deleted_at)| HighlightTombstone { id, deleted_at })
        .collect();
    tombstones.sort_by(|a, b| a.id.cmp(&b.id));

    HighlightsDoc { highlights, tombstones }
}

/// Read-merge-write the highlights file under a per-paper lock so concurrent
/// saves can't interleave and drop a highlight.
pub fn save_highlights_merged(
    root: &str,
    slug: &str,
    incoming_highlights: Vec<Highlight>,
    incoming_tombstones: Vec<HighlightTombstone>,
) -> Result<(), String> {
    validate_slug(slug)?;
    let lock = highlights_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());

    let disk = read_highlights_doc(root, slug);
    let merged = merge_highlights(disk, incoming_highlights, incoming_tombstones);
    let content = serde_json::to_string_pretty(&merged)
        .map_err(|e| format!("Failed to serialize highlights: {e}"))?;
    atomic_write(&highlights_path(root, slug), &content)
}

/// Append a validated agent batch without replacing user edits or tombstones.
/// Retrying an identical tool call reuses the existing records.
pub fn append_highlights(root: &str, slug: &str, incoming: Vec<Highlight>) -> Result<Vec<Highlight>, String> {
    validate_slug(slug)?;
    let lock = highlights_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let mut disk = read_highlights_doc(root, slug);
    let mut result = Vec::new();
    for h in incoming {
        if let Some(existing) = disk.highlights.iter().find(|e| e.page == h.page && e.text == h.text
            && e.color == h.color && e.style == h.style && e.note == h.note) {
            result.push(existing.clone());
        } else {
            result.push(h.clone());
            disk.highlights.push(h);
        }
    }
    let merged = merge_highlights(disk, Vec::new(), Vec::new());
    let content = serde_json::to_string_pretty(&merged).map_err(|e| e.to_string())?;
    atomic_write(&highlights_path(root, slug), &content)?;
    Ok(result)
}

/// Apply an approved agent change to existing highlights under the per-paper
/// lock. `change` sees the effective records as they are on disk now and
/// returns replacement records and tombstones, which go through the same merge
/// as every save: each must be stamped newer than what it replaces, and a newer
/// edit or delete from elsewhere still wins. An error writes nothing.
pub fn change_highlights<T>(
    root: &str,
    slug: &str,
    change: impl FnOnce(&[Highlight]) -> Result<(Vec<Highlight>, Vec<HighlightTombstone>, T), String>,
) -> Result<T, String> {
    validate_slug(slug)?;
    let lock = highlights_lock(root, slug);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let disk = read_highlights_doc(root, slug);
    let (highlights, tombstones, result) = change(&effective_highlights(&disk))?;
    let merged = merge_highlights(disk, highlights, tombstones);
    let content = serde_json::to_string_pretty(&merged)
        .map_err(|e| format!("Failed to serialize highlights: {e}"))?;
    atomic_write(&highlights_path(root, slug), &content)?;
    Ok(result)
}

type LockMap = std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<std::sync::Mutex<()>>>>;

fn highlights_lock(root: &str, slug: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<LockMap> = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = format!("{root}\u{0}{slug}");
    let mut map = locks.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key)
        .or_insert_with(|| std::sync::Arc::new(std::sync::Mutex::new(())))
        .clone()
}

#[cfg(test)]
mod duplicate_import_tests {
    use super::*;
    struct Library(String);
    impl Library {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("argus-duplicate-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join(".argus")).unwrap();
            Self(root.to_string_lossy().into())
        }
        fn seed(&self, base: &str) -> String {
            let dir = reserve_paper_dir(&self.0, base).unwrap();
            let slug = dir.file_name().unwrap().to_string_lossy().to_string();
            let meta: PaperMeta = serde_json::from_value(serde_json::json!({
                "id": uuid::Uuid::new_v4().to_string(), "title": "Same Paper", "authors": ["Alice"],
                "doi": "10.1000/same", "arxiv_id": "2401.12345", "year": 2024,
                "added_at": "2026-01-01", "reading_status": "unread",
            })).unwrap();
            write_meta(&self.0, &slug, &meta).unwrap();
            ensure_paper_files(&self.0, &slug);
            std::fs::write(dir.join("paper.pdf"), b"independent original PDF").unwrap();
            slug
        }
    }
    impl Drop for Library { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn duplicate_copies_stay_independent_inside_different_collections() {
        let lib = Library::new();
        let original = lib.seed("same");
        let original_meta = read_meta(&lib.0, &original).unwrap();
        let parent = crate::collections::create_collection(&lib.0, "Library".into(), None).unwrap();
        let col = crate::collections::create_collection(&lib.0, "Originals".into(), Some(parent.id)).unwrap();
        crate::collections::move_paper_to_collection(&lib.0, &original_meta.id, &col.id).unwrap();
        let original_path = paper_dir(&lib.0, &original);
        let meta_before = std::fs::read(original_path.join("meta.json")).unwrap();
        let note = create_note_with(&lib.0, &original, "Notes", "original note").unwrap();
        let highlights_before = std::fs::read(original_path.join("highlights.json")).unwrap();
        crate::copilot::write_paper_ai_conversations(&lib.0, &original, &serde_json::json!([
            {"id":"original-chat", "nodes":[], "updatedAt":"2026-01-01"}
        ]), &serde_json::json!([])).unwrap();
        let chat_before = std::fs::read(original_path.join("ai_conversations.json")).unwrap();

        // Original lives in a collection, so checking only papers/same would miss it.
        let copy = lib.seed("same");
        assert_eq!(copy, "same-2");
        let copy_meta = read_meta(&lib.0, &copy).unwrap();
        assert_ne!(original_meta.id, copy_meta.id);
        let canonical = crate::metadata::rename_folder(&lib.0, &original).unwrap();
        let copy = crate::metadata::rename_folder(&lib.0, &copy).unwrap();
        assert_ne!(canonical, copy);
        assert_eq!(crate::metadata::rename_folder(&lib.0, &copy).unwrap(), copy, "copy slug stays stable");
        create_note_with(&lib.0, &copy, "Notes", "copy note").unwrap();
        write_reading_state(&lib.0, &copy, &ReadingState { page: 5, ..Default::default() }).unwrap();
        crate::copilot::write_paper_ai_conversations(&lib.0, &copy, &serde_json::json!([
            {"id":"copy-chat", "nodes":[], "updatedAt":"2026-01-02"}
        ]), &serde_json::json!([])).unwrap();
        let h: Highlight = serde_json::from_value(serde_json::json!({
            "id":"copy-highlight", "page":1, "rects":[], "text":"Same text", "color":"#FFEB3B", "note":"Copy only", "created_at":"2026-01-02"
        })).unwrap();
        append_highlights(&lib.0, &copy, vec![h]).unwrap();
        let path = paper_dir(&lib.0, &canonical);
        assert_eq!(std::fs::read(path.join("meta.json")).unwrap(), meta_before);
        assert_eq!(std::fs::read(path.join("highlights.json")).unwrap(), highlights_before);
        assert_eq!(std::fs::read(path.join("ai_conversations.json")).unwrap(), chat_before);
        assert_eq!(get_note(&lib.0, &canonical, &note.id), "original note");
        assert!(read_reading_state(&lib.0, &canonical).is_none());
        assert_eq!(read_highlights(&lib.0, &copy).len(), 1);
        crate::collections::purge_paper(&lib.0, &copy_meta.id).unwrap();
        std::fs::remove_dir_all(paper_dir(&lib.0, &copy)).unwrap();
        assert_eq!(std::fs::read(crate::metadata::find_pdf_in_dir(&lib.0, &canonical)).unwrap(), b"independent original PDF");
        assert_eq!(crate::collections::list_papers_in_collection(&lib.0, &col.id).unwrap().into_iter()
            .map(|p| p.id).collect::<Vec<_>>(), vec![original_meta.id]);
    }

    #[test]
    fn concurrent_import_reservations_never_share_an_unfinished_directory() {
        let lib = Library::new();
        let handles: Vec<_> = (0..8).map(|_| {
            let root = lib.0.clone();
            std::thread::spawn(move || reserve_paper_dir(&root, "same").unwrap())
        }).collect();
        let paths: std::collections::HashSet<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(paths.len(), 8);
        assert!(paths.iter().all(|p| p.is_dir()));
    }
}

#[cfg(test)]
mod highlight_tests {
    use super::*;

    fn hl(id: &str, created: &str, updated: Option<&str>) -> Highlight {
        Highlight {
            id: id.to_string(),
            page: 1,
            rects: Vec::new(),
            text: String::new(),
            color: "#FFEB3B".to_string(),
            note: None,
            created_at: created.to_string(),
            updated_at: updated.map(|s| s.to_string()),
            style: "highlight".to_string(),
            start_offset: None,
            end_offset: None,
            anchor_prefix: None,
            anchor_suffix: None,
            keep_line_breaks: None,
            agent: None,
        }
    }

    fn tomb(id: &str, at: &str) -> HighlightTombstone {
        HighlightTombstone { id: id.to_string(), deleted_at: at.to_string() }
    }

    fn ids(doc: &HighlightsDoc) -> Vec<String> {
        let mut v: Vec<String> = doc.highlights.iter().map(|h| h.id.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn stale_save_keeps_the_other_machines_highlight() {
        // Disk has A's highlight; B saves only its own — A must survive (union, not
        // overwrite). This is the reported bug class for annotations.
        let disk = HighlightsDoc { highlights: vec![hl("A", "2026-01-01", None)], tombstones: vec![] };
        let merged = merge_highlights(disk, vec![hl("B", "2026-01-02", None)], vec![]);
        assert_eq!(ids(&merged), vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn newer_edit_wins_on_the_same_id() {
        let disk = HighlightsDoc { highlights: vec![hl("A", "2026-01-01", Some("2026-01-01T00:00:00Z"))], tombstones: vec![] };
        let incoming = vec![hl("A", "2026-01-01", Some("2026-01-05T00:00:00Z"))];
        let merged = merge_highlights(disk, incoming.clone(), vec![]);
        assert_eq!(merged.highlights.len(), 1);
        assert_eq!(merged.highlights[0].updated_at.as_deref(), Some("2026-01-05T00:00:00Z"));
    }

    #[test]
    fn delete_propagates_and_does_not_resurrect() {
        // Disk still has A; incoming carries a tombstone for it → A is removed and
        // the tombstone is kept so other machines drop it too.
        let disk = HighlightsDoc { highlights: vec![hl("A", "2026-01-01", None)], tombstones: vec![] };
        let merged = merge_highlights(disk, vec![], vec![tomb("A", "2026-01-03T00:00:00Z")]);
        assert!(merged.highlights.is_empty(), "deleted highlight removed");
        assert_eq!(merged.tombstones.len(), 1, "tombstone retained to keep propagating");

        // A stale machine that still has A re-submits it — the delete still wins.
        let merged2 = merge_highlights(merged, vec![hl("A", "2026-01-01", None)], vec![]);
        assert!(merged2.highlights.is_empty(), "delete beats a stale re-add");
    }

    #[test]
    fn re_edit_after_delete_revives_the_highlight() {
        let disk = HighlightsDoc { highlights: vec![], tombstones: vec![tomb("A", "2026-01-03T00:00:00Z")] };
        // Edited AFTER the delete → comes back, tombstone retired.
        let incoming = vec![hl("A", "2026-01-01", Some("2026-01-04T00:00:00Z"))];
        let merged = merge_highlights(disk, incoming, vec![]);
        assert_eq!(ids(&merged), vec!["A".to_string()]);
        assert!(merged.tombstones.is_empty());
    }

    #[test]
    fn legacy_bare_array_reads_as_a_doc() {
        let arr = serde_json::to_string(&vec![hl("A", "2026-01-01", None)]).unwrap();
        let doc: HighlightsDoc = serde_json::from_str::<HighlightsDoc>(&arr)
            .or_else(|_| serde_json::from_str::<Vec<Highlight>>(&arr).map(|h| HighlightsDoc { highlights: h, tombstones: vec![] }))
            .unwrap();
        assert_eq!(ids(&doc), vec!["A".to_string()]);
    }
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

#[cfg(test)]
mod notes_lifecycle_tests {
    use super::*;
    struct Library(String);
    impl Library {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("argus-notes-{}", uuid::Uuid::new_v4()));
            let dir = root.join("papers/paper");
            std::fs::create_dir_all(root.join(".argus")).unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("meta.json"), serde_json::json!({"id":"paper-id","title":"Paper","authors":[],"added_at":"2026-01-01","reading_status":"unread"}).to_string()).unwrap();
            Self(root.to_string_lossy().to_string())
        }
    }
    impl Drop for Library { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    #[test]
    fn deleting_and_restoring_preserves_exact_note_identity_content_and_image_links() {
        let lib = Library::new();
        let note = create_note_with(&lib.0, "paper", "AI总结", "# Full note\n\n$x^2$\n\n![](assets/figure.png)").unwrap();
        let assets = notes_dir(&lib.0, "paper").join("assets");
        std::fs::create_dir(&assets).unwrap(); std::fs::write(assets.join("figure.png"), b"unchanged pixels").unwrap();
        let original = note_snapshot(&lib.0, "paper", &note.id).unwrap();
        delete_note(&lib.0, "paper", &note.id).unwrap();
        assert!(list_notes(&lib.0, "paper").is_empty());
        assert!(!note_file_path(&lib.0, "paper", &note.id).unwrap().exists());
        let deleted = list_deleted_notes(&lib.0, "paper").unwrap().pop().unwrap();
        assert_eq!(deleted.snapshot, original);
        let expiry = chrono::DateTime::parse_from_rfc3339(&deleted.expires_at).unwrap();
        let start = chrono::DateTime::parse_from_rfc3339(&deleted.deleted_at).unwrap();
        assert_eq!(expiry - start, chrono::Duration::days(30));
        assert!(save_note(&lib.0, "paper", &note.id, "late autosave").is_err());
        let restored = restore_note(&lib.0, "paper", &note.id, Some(&deleted)).unwrap();
        assert_eq!(restored.id, note.id); assert_eq!(restored.title, note.title); assert_eq!(restored.created_at, note.created_at);
        assert_eq!(get_note(&lib.0, "paper", &note.id), original.content);
        assert!(list_deleted_notes(&lib.0, "paper").unwrap().is_empty());
        assert_eq!(std::fs::read(assets.join("figure.png")).unwrap(), b"unchanged pixels");
        assert!(restore_note(&lib.0, "paper", &note.id, Some(&deleted)).is_err());
        delete_note(&lib.0, "paper", &note.id).unwrap();
        assert_eq!(list_deleted_notes(&lib.0, "paper").unwrap().len(), 1);
    }

    #[test]
    fn retention_uses_exact_deletion_time_and_never_erases_live_notes() {
        let lib = Library::new();
        let note = create_note_with(&lib.0, "paper", "Deleted", "private text").unwrap();
        let live = create_note_with(&lib.0, "paper", "Keep", "live text").unwrap();
        delete_note(&lib.0, "paper", &note.id).unwrap();
        let deleted = list_deleted_notes(&lib.0, "paper").unwrap().pop().unwrap();
        let expiry = chrono::DateTime::parse_from_rfc3339(&deleted.expires_at).unwrap().with_timezone(&chrono::Utc);
        assert_eq!(prune_deleted_notes(&lib.0, "paper", expiry - chrono::Duration::milliseconds(1)).unwrap(), 0);
        assert_eq!(prune_deleted_notes(&lib.0, "paper", expiry).unwrap(), 1);
        assert!(list_deleted_notes(&lib.0, "paper").unwrap().is_empty());
        assert!(save_note(&lib.0, "paper", &note.id, "resurrection").is_err());
        assert_eq!(get_note(&lib.0, "paper", &live.id), "live text");
        assert!(restore_note(&lib.0, "paper", &note.id, None).is_err());
    }

    #[test]
    fn stale_approved_writes_and_editor_buffers_cannot_replace_newer_content() {
        let lib = Library::new();
        let note = create_note_with(&lib.0, "paper", "Title", "before").unwrap();
        let reviewed = note_snapshot(&lib.0, "paper", &note.id).unwrap();
        save_note(&lib.0, "paper", &note.id, "user's newer words").unwrap();
        assert!(update_note_checked(&lib.0, "paper", &reviewed, "AI title", "AI content").is_err());
        assert!(delete_note_checked(&lib.0, "paper", &reviewed).is_err());
        assert!(save_note_if_current(&lib.0, "paper", &note.id, "stale autosave", Some("before")).is_err());
        assert_eq!(get_note(&lib.0, "paper", &note.id), "user's newer words");
        assert!(list_deleted_notes(&lib.0, "paper").unwrap().is_empty());
    }

    #[test]
    fn concurrent_note_creation_and_deletion_cannot_drop_sibling_records() {
        let lib = Library::new();
        let workers: Vec<_> = (0..12).map(|i| { let root = lib.0.clone(); std::thread::spawn(move || create_note_with(&root, "paper", &format!("Note {i}"), "Body").unwrap()) }).collect();
        let notes: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
        assert_eq!(list_notes(&lib.0, "paper").len(), 12);
        let workers: Vec<_> = notes.into_iter().map(|note| { let root = lib.0.clone(); std::thread::spawn(move || delete_note(&root, "paper", &note.id).unwrap()) }).collect();
        for w in workers { w.join().unwrap(); }
        assert!(list_notes(&lib.0, "paper").is_empty());
        assert_eq!(list_deleted_notes(&lib.0, "paper").unwrap().len(), 12);
    }

    #[test]
    fn expired_and_malformed_archives_are_not_restored_or_silently_replaced() {
        let lib = Library::new();
        let note = create_note_with(&lib.0, "paper", "Title", "Words").unwrap();
        delete_note(&lib.0, "paper", &note.id).unwrap();
        let mut deleted = list_deleted_notes(&lib.0, "paper").unwrap().pop().unwrap();
        deleted.deleted_at = (chrono::Utc::now() - chrono::Duration::days(31)).to_rfc3339();
        // A forged future expiry cannot extend retention beyond deletion + 30d.
        deleted.expires_at = (chrono::Utc::now() + chrono::Duration::days(200)).to_rfc3339();
        let path = trash_dir(&lib.0, "paper", false).unwrap().join(format!("{}.json", note.id));
        std::fs::write(&path, serde_json::to_vec(&deleted).unwrap()).unwrap();
        assert!(list_deleted_notes(&lib.0, "paper").unwrap().is_empty());
        assert!(restore_note(&lib.0, "paper", &note.id, None).is_err());
        std::fs::write(&path, "broken json").unwrap();
        assert!(prune_deleted_notes(&lib.0, "paper", chrono::Utc::now()).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken json");
        assert!(restore_note(&lib.0, "paper", "../escape", None).is_err());
    }

    #[test]
    fn deletion_does_not_reindex_the_old_legacy_note_as_live_content() {
        let lib = Library::new();
        let dir = paper_dir(&lib.0, "paper");
        std::fs::write(dir.join("notes.md"), "uniquelegacysecret").unwrap();
        let note = list_notes(&lib.0, "paper").pop().unwrap();
        crate::search::index_paper(&lib.0, "paper").unwrap();
        assert_eq!(crate::search::search_fulltext(&lib.0, "uniquelegacysecret").unwrap().len(), 1);
        delete_note(&lib.0, "paper", &note.id).unwrap();
        // Also covers files migrated by older app versions that retained notes.md.
        std::fs::write(dir.join("notes.md"), "uniquelegacysecret").unwrap();
        crate::search::index_paper(&lib.0, "paper").unwrap();
        assert!(crate::search::search_fulltext(&lib.0, "uniquelegacysecret").unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn trash_and_note_symlinks_never_target_files_outside_the_paper() {
        let lib = Library::new();
        let note = create_note_with(&lib.0, "paper", "Title", "Body").unwrap();
        let external = std::path::Path::new(&lib.0).join("outside"); std::fs::create_dir(&external).unwrap();
        std::os::unix::fs::symlink(&external, notes_dir(&lib.0, "paper").join(".trash")).unwrap();
        assert!(delete_note(&lib.0, "paper", &note.id).is_err());
        assert_eq!(get_note(&lib.0, "paper", &note.id), "Body");
        assert_eq!(std::fs::read_dir(&external).unwrap().count(), 0);
    }
}
