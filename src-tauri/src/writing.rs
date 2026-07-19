use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One writing reference list: a named set of paper ids the user is citing in a
/// manuscript. Unlike collections these never move paper folders on disk — they
/// are pure membership groupings, so a single paper may belong to several lists.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WritingList {
    pub id: String,
    pub name: String,
    pub created_at: String,
    #[serde(default)]
    pub paper_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WritingListsFile {
    #[serde(default)]
    pub lists: Vec<WritingList>,
}

/// Serializes read-modify-write cycles on lists.json (Tauri commands run
/// concurrently, mirroring the collections lock).
static WRITING_LOCK: Mutex<()> = Mutex::new(());

fn lock_writing() -> std::sync::MutexGuard<'static, ()> {
    WRITING_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Top-level `writing/` folder, a sibling of `papers/`, `canvases/`, `inbox/`
/// and `snippets/`.
fn writing_dir(root: &str) -> PathBuf {
    Path::new(root).join("writing")
}

fn writing_path(root: &str) -> PathBuf {
    writing_dir(root).join("lists.json")
}

fn read_file(root: &str) -> WritingListsFile {
    let path = writing_path(root);
    if !path.exists() {
        return WritingListsFile::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(c) => serde_json::from_str(&c).unwrap_or_default(),
        Err(_) => WritingListsFile::default(),
    }
}

fn write_file(root: &str, file: &WritingListsFile) -> Result<(), String> {
    std::fs::create_dir_all(writing_dir(root))
        .map_err(|e| format!("Create writing folder: {e}"))?;
    let content =
        serde_json::to_string_pretty(file).map_err(|e| format!("Serialize writing lists: {e}"))?;
    crate::fsutil::atomic_write_str(&writing_path(root), &content)
        .map_err(|e| format!("Write writing/lists.json: {e}"))
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

pub fn list_writing_lists(root: &str) -> WritingListsFile {
    let _g = lock_writing();
    read_file(root)
}

pub fn create_writing_list(root: &str, name: String) -> Result<WritingList, String> {
    let _g = lock_writing();
    let mut file = read_file(root);
    let list = WritingList {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.trim().to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        paper_ids: Vec::new(),
    };
    file.lists.push(list.clone());
    write_file(root, &file)?;
    Ok(list)
}

pub fn rename_writing_list(root: &str, id: &str, new_name: String) -> Result<(), String> {
    let _g = lock_writing();
    let mut file = read_file(root);
    let list = file
        .lists
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| format!("Writing list {id} not found"))?;
    list.name = new_name.trim().to_string();
    write_file(root, &file)
}

pub fn delete_writing_list(root: &str, id: &str) -> Result<(), String> {
    let _g = lock_writing();
    let mut file = read_file(root);
    let before = file.lists.len();
    file.lists.retain(|l| l.id != id);
    if file.lists.len() != before {
        write_file(root, &file)?;
    }
    Ok(())
}

// ── Membership ──────────────────────────────────────────────────────────────────

pub fn add_papers_to_writing_list(
    root: &str,
    id: &str,
    paper_ids: Vec<String>,
) -> Result<(), String> {
    let _g = lock_writing();
    let mut file = read_file(root);
    let list = file
        .lists
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| format!("Writing list {id} not found"))?;
    for pid in paper_ids {
        if !list.paper_ids.contains(&pid) {
            list.paper_ids.push(pid);
        }
    }
    write_file(root, &file)
}

pub fn remove_paper_from_writing_list(
    root: &str,
    id: &str,
    paper_id: &str,
) -> Result<(), String> {
    let _g = lock_writing();
    let mut file = read_file(root);
    let list = file
        .lists
        .iter_mut()
        .find(|l| l.id == id)
        .ok_or_else(|| format!("Writing list {id} not found"))?;
    let before = list.paper_ids.len();
    list.paper_ids.retain(|p| p != paper_id);
    if list.paper_ids.len() != before {
        write_file(root, &file)?;
    }
    Ok(())
}

/// Drop a paper from every writing list — called when the paper is deleted so no
/// list keeps an id pointing at a paper that no longer exists.
pub fn purge_paper(root: &str, paper_id: &str) -> Result<(), String> {
    let _g = lock_writing();
    let mut file = read_file(root);
    let mut changed = false;
    for list in &mut file.lists {
        let before = list.paper_ids.len();
        list.paper_ids.retain(|p| p != paper_id);
        if list.paper_ids.len() != before {
            changed = true;
        }
    }
    if changed {
        write_file(root, &file)?;
    }
    Ok(())
}

pub fn writing_folder_path(root: &str) -> Result<PathBuf, String> {
    let dir = writing_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Create writing folder: {e}"))?;
    Ok(dir)
}
