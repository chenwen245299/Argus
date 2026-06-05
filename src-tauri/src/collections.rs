use std::path::{Path, PathBuf};

use crate::{
    models::{normalize_import_source, Assignment, Collection, CollectionsFile, PaperIndexEntry},
    paper,
};

fn collections_path(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("collections.json")
}

// ── Read / write ──────────────────────────────────────────────────────────────

pub fn read_collections(root: &str) -> CollectionsFile {
    let path = collections_path(root);
    if !path.exists() {
        return CollectionsFile::default();
    }
    match std::fs::read_to_string(&path) {
        Ok(c) => serde_json::from_str(&c).unwrap_or_default(),
        Err(_) => CollectionsFile::default(),
    }
}

fn write_collections(root: &str, file: &CollectionsFile) -> Result<(), String> {
    let path = collections_path(root);
    let content =
        serde_json::to_string_pretty(file).map_err(|e| format!("Serialize collections: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Write collections.json: {e}"))
}

fn random_collection_emoji() -> String {
    const EMOJIS: &[&str] = &[
        "📚", "📖", "🧠", "🔬", "🧪", "📝", "💡", "🎯", "⭐", "🌱", "🚀", "🧩", "🗂️", "🔎", "🧭",
        "⚙️", "📌", "🏷️",
    ];
    let idx = (uuid::Uuid::new_v4().as_u128() % EMOJIS.len() as u128) as usize;
    EMOJIS[idx].to_string()
}

fn ensure_collection_emojis(file: &mut CollectionsFile) -> bool {
    let mut changed = false;
    for col in &mut file.collections {
        let missing = col
            .emoji
            .as_ref()
            .map(|emoji| emoji.trim().is_empty())
            .unwrap_or(true);
        if missing {
            col.emoji = Some(random_collection_emoji());
            changed = true;
        }
    }
    changed
}

pub fn get_collections(root: &str) -> Result<CollectionsFile, String> {
    let mut file = read_collections(root);
    if ensure_collection_emojis(&mut file) {
        write_collections(root, &file)?;
    }
    Ok(file)
}

// ── Folder path helpers ───────────────────────────────────────────────────────

/// Strip characters that are invalid in folder names on any OS.
fn sanitize_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c if c.is_control() => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .trim()
        .to_string();

    if cleaned.is_empty() || cleaned == "." || cleaned == ".." {
        "Untitled".to_string()
    } else {
        cleaned
    }
}

/// Build the filesystem path for a collection folder by walking the parent chain.
/// Top-level collections sit at `<library>/papers/<name>/`.
/// Nested ones sit at `<library>/papers/<parent>/<name>/`.
fn collection_folder_path(library_root: &str, collections: &[Collection], col_id: &str) -> PathBuf {
    let mut chain: Vec<String> = Vec::new();
    let mut cur = col_id.to_string();
    let mut visited = std::collections::HashSet::new();

    loop {
        if visited.contains(&cur) {
            break; // cycle guard
        }
        visited.insert(cur.clone());
        match collections.iter().find(|c| c.id == cur) {
            Some(col) => {
                chain.push(sanitize_name(&col.name));
                match col.parent_id.as_ref() {
                    Some(pid) => cur = pid.clone(),
                    None => break,
                }
            }
            None => break,
        }
    }

    chain.reverse();
    let mut path = Path::new(library_root).join("papers");
    for part in &chain {
        path = path.join(part);
    }
    path
}

pub fn collection_folder_path_for(root: &str, collection_id: &str) -> Result<PathBuf, String> {
    let file = read_collections(root);
    if !file.collections.iter().any(|c| c.id == collection_id) {
        return Err(format!("Collection {collection_id} not found"));
    }
    let folder = collection_folder_path(root, &file.collections, collection_id);
    std::fs::create_dir_all(&folder).map_err(|e| format!("Create collection folder: {e}"))?;
    Ok(folder)
}

fn move_paper_folder_to_collection(
    root: &str,
    collections: &[Collection],
    slug: &str,
    collection_id: &str,
) -> Result<(), String> {
    let collection_folder = collection_folder_path(root, collections, collection_id);
    std::fs::create_dir_all(&collection_folder)
        .map_err(|e| format!("Create collection folder: {e}"))?;

    let current_dir = paper::find_paper_dir(root, slug)
        .ok_or_else(|| format!("Paper directory not found for {slug}"))?;
    let target_dir = collection_folder.join(slug);

    if target_dir.symlink_metadata().is_ok() {
        let target_meta = std::fs::symlink_metadata(&target_dir)
            .map_err(|e| format!("Read target folder: {e}"))?;
        if target_meta.file_type().is_symlink() {
            std::fs::remove_file(&target_dir)
                .map_err(|e| format!("Remove legacy collection link: {e}"))?;
        } else {
            let same_target = target_dir
                .canonicalize()
                .ok()
                .zip(current_dir.canonicalize().ok())
                .is_some_and(|(a, b)| a == b);

            if same_target {
                return Ok(());
            }

            return Err(format!("Target collection already contains {slug}"));
        }
    }

    std::fs::rename(&current_dir, &target_dir)
        .map_err(|e| format!("Move paper into collection folder: {e}"))
}

/// Scan `<library>/papers/` to find the slug (folder name) for a given paper UUID.
fn find_paper_slug(library_root: &str, paper_id: &str) -> Option<String> {
    for (slug, path) in paper::list_paper_dirs(library_root).ok()? {
        if let Ok(content) = std::fs::read_to_string(path.join("meta.json")) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&content) {
                if meta.get("id").and_then(|v| v.as_str()) == Some(paper_id) {
                    return Some(slug);
                }
            }
        }
    }
    None
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

pub fn create_collection(
    root: &str,
    name: String,
    parent_id: Option<String>,
) -> Result<Collection, String> {
    let mut file = read_collections(root);
    let col = Collection {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        emoji: Some(random_collection_emoji()),
        parent_id,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    file.collections.push(col.clone());
    write_collections(root, &file)?;

    // Create the matching folder in the library.
    let folder = collection_folder_path(root, &file.collections, &col.id);
    std::fs::create_dir_all(&folder).map_err(|e| format!("Create collection folder: {e}"))?;

    Ok(col)
}

pub fn rename_collection(root: &str, id: &str, new_name: String) -> Result<(), String> {
    let mut file = read_collections(root);

    // Capture the old folder path BEFORE updating the name.
    let old_folder = collection_folder_path(root, &file.collections, id);

    let col = file
        .collections
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("Collection {id} not found"))?;
    col.name = new_name;

    let new_folder = collection_folder_path(root, &file.collections, id);

    if old_folder.exists() {
        if old_folder != new_folder {
            std::fs::rename(&old_folder, &new_folder)
                .map_err(|e| format!("Rename collection folder: {e}"))?;
        }
    } else {
        std::fs::create_dir_all(&new_folder)
            .map_err(|e| format!("Create collection folder: {e}"))?;
    }

    write_collections(root, &file)
}

pub fn move_collection(root: &str, id: &str, new_parent_id: Option<String>) -> Result<(), String> {
    let mut file = read_collections(root);

    if !file.collections.iter().any(|c| c.id == id) {
        return Err(format!("Collection {id} not found"));
    }

    let new_parent_id = new_parent_id.filter(|parent| !parent.trim().is_empty());
    if new_parent_id.as_deref() == Some(id) {
        return Err("Cannot move a collection into itself".to_string());
    }

    if let Some(parent_id) = new_parent_id.as_deref() {
        if !file.collections.iter().any(|c| c.id == parent_id) {
            return Err(format!("Target collection {parent_id} not found"));
        }
        if collect_subtree(&file.collections, id).contains(parent_id) {
            return Err("Cannot move a collection into one of its descendants".to_string());
        }
    }

    let old_folder = collection_folder_path(root, &file.collections, id);
    let current_parent_id = file
        .collections
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| c.parent_id.clone());

    if current_parent_id == new_parent_id {
        if !old_folder.exists() {
            std::fs::create_dir_all(&old_folder)
                .map_err(|e| format!("Create collection folder: {e}"))?;
        }
        return Ok(());
    }

    {
        let col = file
            .collections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| format!("Collection {id} not found"))?;
        col.parent_id = new_parent_id;
    }

    let new_folder = collection_folder_path(root, &file.collections, id);
    if old_folder != new_folder {
        if let Some(parent) = new_folder.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Create target parent folder: {e}"))?;
        }

        if old_folder.exists() {
            if new_folder.exists() {
                let same_target = old_folder
                    .canonicalize()
                    .ok()
                    .zip(new_folder.canonicalize().ok())
                    .is_some_and(|(a, b)| a == b);
                if !same_target {
                    return Err(format!(
                        "Target collection folder already exists: {}",
                        new_folder.display()
                    ));
                }
            } else {
                std::fs::rename(&old_folder, &new_folder)
                    .map_err(|e| format!("Move collection folder: {e}"))?;
            }
        } else {
            std::fs::create_dir_all(&new_folder)
                .map_err(|e| format!("Create collection folder: {e}"))?;
        }
    }

    write_collections(root, &file)
}

pub fn set_collection_emoji(root: &str, id: &str, emoji: Option<String>) -> Result<(), String> {
    let mut file = read_collections(root);
    let col = file
        .collections
        .iter_mut()
        .find(|c| c.id == id)
        .ok_or_else(|| format!("Collection {id} not found"))?;

    col.emoji = emoji
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| Some(random_collection_emoji()));

    write_collections(root, &file)
}

/// Delete a collection and cascade-remove all descendants and their assignments.
pub fn delete_collection(root: &str, id: &str) -> Result<(), String> {
    let mut file = read_collections(root);
    if !file.collections.iter().any(|c| c.id == id) {
        return Err(format!("Collection {id} not found"));
    }

    // Compute folder path BEFORE removing the collection from the list.
    let folder = collection_folder_path(root, &file.collections, id);
    move_papers_out_of_collection(root, &folder)?;

    let to_remove = collect_subtree(&file.collections, id);
    file.collections.retain(|c| !to_remove.contains(&c.id));
    file.assignments
        .retain(|a| !to_remove.contains(&a.collection_id));
    write_collections(root, &file)?;

    Ok(())
}

fn move_papers_out_of_collection(root: &str, folder: &Path) -> Result<(), String> {
    if !folder.exists() {
        return Ok(());
    }

    let papers_root = paper::papers_dir(root);
    std::fs::create_dir_all(&papers_root).map_err(|e| format!("Create papers folder: {e}"))?;

    let mut paper_dirs = Vec::new();
    let mut visited_dirs = Vec::new();
    collect_paper_dirs_under(folder, &mut paper_dirs, &mut visited_dirs)?;

    for current_dir in paper_dirs {
        let slug = current_dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("Invalid paper folder name: {}", current_dir.display()))?
            .to_string();
        let target = unique_paper_target(&papers_root, &slug);

        let same_target = current_dir
            .canonicalize()
            .ok()
            .zip(target.canonicalize().ok())
            .is_some_and(|(a, b)| a == b);
        if same_target {
            continue;
        }

        std::fs::rename(&current_dir, &target).map_err(|e| {
            format!(
                "Move paper out of deleted collection ({} -> {}): {e}",
                current_dir.display(),
                target.display()
            )
        })?;
    }

    // Only remove directories that became empty. Unknown user files are left in place.
    visited_dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
    for dir in visited_dirs {
        let _ = std::fs::remove_dir(&dir);
    }

    Ok(())
}

fn collect_paper_dirs_under(
    dir: &Path,
    paper_dirs: &mut Vec<PathBuf>,
    visited_dirs: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir)
        .map_err(|e| format!("Read collection folder {}: {e}", dir.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Ok(());
    }

    visited_dirs.push(dir.to_path_buf());
    for entry in std::fs::read_dir(dir).map_err(|e| format!("Read collection folder: {e}"))? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }

        if path.join("meta.json").exists() {
            paper_dirs.push(path);
        } else {
            collect_paper_dirs_under(&path, paper_dirs, visited_dirs)?;
        }
    }
    Ok(())
}

fn unique_paper_target(papers_root: &Path, slug: &str) -> PathBuf {
    let candidate = papers_root.join(slug);
    if !candidate.exists() {
        return candidate;
    }

    let mut n = 2u32;
    loop {
        let candidate = papers_root.join(format!("{slug}-moved-{n}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Recursively collect `root_id` and all its descendant IDs.
fn collect_subtree(collections: &[Collection], root_id: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    set.insert(root_id.to_string());
    loop {
        let before = set.len();
        for col in collections {
            if let Some(ref pid) = col.parent_id {
                if set.contains(pid) {
                    set.insert(col.id.clone());
                }
            }
        }
        if set.len() == before {
            break;
        }
    }
    set
}

// ── Assignments ───────────────────────────────────────────────────────────────

pub fn add_paper_to_collection(
    root: &str,
    paper_id: &str,
    collection_id: &str,
) -> Result<(), String> {
    let mut file = read_collections(root);

    if !file.collections.iter().any(|c| c.id == collection_id) {
        return Err(format!("Collection {collection_id} not found"));
    }

    let exists = file
        .assignments
        .iter()
        .any(|a| a.paper_id == paper_id && a.collection_id == collection_id);
    if !exists {
        file.assignments.push(Assignment {
            paper_id: paper_id.to_string(),
            collection_id: collection_id.to_string(),
        });
        write_collections(root, &file)?;
    }

    Ok(())
}

pub fn move_paper_to_collection(
    root: &str,
    paper_id: &str,
    collection_id: &str,
) -> Result<(), String> {
    let mut file = read_collections(root);

    if !file.collections.iter().any(|c| c.id == collection_id) {
        return Err(format!("Collection {collection_id} not found"));
    }

    let slug =
        find_paper_slug(root, paper_id).ok_or_else(|| format!("Paper {paper_id} not found"))?;

    move_paper_folder_to_collection(root, &file.collections, &slug, collection_id)?;

    file.assignments.retain(|a| a.paper_id != paper_id);
    file.assignments.push(Assignment {
        paper_id: paper_id.to_string(),
        collection_id: collection_id.to_string(),
    });
    write_collections(root, &file)
}

pub fn remove_paper_from_collection(
    root: &str,
    paper_id: &str,
    collection_id: &str,
) -> Result<(), String> {
    let mut file = read_collections(root);
    let before = file.assignments.len();
    file.assignments
        .retain(|a| !(a.paper_id == paper_id && a.collection_id == collection_id));

    if file.assignments.len() != before {
        write_collections(root, &file)?;
    }

    Ok(())
}

pub fn list_papers_in_collection(
    root: &str,
    collection_id: &str,
) -> Result<Vec<PaperIndexEntry>, String> {
    let file = read_collections(root);
    let ids: std::collections::HashSet<String> = file
        .assignments
        .iter()
        .filter(|a| a.collection_id == collection_id)
        .map(|a| a.paper_id.clone())
        .collect();

    if ids.is_empty() {
        return Ok(vec![]);
    }

    let mut entries = Vec::new();
    for (slug, path) in paper::list_paper_dirs(root)? {
        let meta_content = match std::fs::read_to_string(path.join("meta.json")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let meta: crate::models::PaperMeta = match serde_json::from_str(&meta_content) {
            Ok(m) => m,
            Err(_) => continue,
        };

        if !ids.contains(&meta.id) {
            continue;
        }

        let status = crate::paper::read_status(&path);
        let import_source =
            normalize_import_source(meta.import_source.as_deref(), meta.arxiv_id.as_deref());
        entries.push(PaperIndexEntry {
            slug,
            id: meta.id,
            title: meta.title,
            authors: meta.authors,
            year: meta.year,
            venue: meta.venue,
            tags: meta.tags,
            status,
            added_at: meta.added_at,
            reading_status: meta.reading_status,
            meta_mtime: 0,
            import_source: Some(import_source),
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PaperMeta;

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "argus-collections-test-{}",
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn delete_collection_keeps_papers() {
        let root = test_root();
        let root_str = root.to_string_lossy().to_string();
        std::fs::create_dir_all(root.join(".argus")).unwrap();
        let collection_dir = root.join("papers").join("Topic");
        let paper_dir = collection_dir.join("paper-a");
        std::fs::create_dir_all(&paper_dir).unwrap();

        let paper_id = "paper-id-1".to_string();
        let meta = PaperMeta {
            id: paper_id.clone(),
            title: "Paper A".to_string(),
            authors: vec![],
            year: None,
            doi: None,
            arxiv_id: None,
            venue: None,
            tags: vec![],
            added_at: "2026-01-01T00:00:00Z".to_string(),
            original_filename: None,
            reading_status: "unread".to_string(),
            paper_abstract: None,
            bibtex: None,
            canvas_notes: vec![],
            import_source: Some("file".to_string()),
        };
        std::fs::write(
            paper_dir.join("meta.json"),
            serde_json::to_string_pretty(&meta).unwrap(),
        )
        .unwrap();

        let collection = Collection {
            id: "collection-1".to_string(),
            name: "Topic".to_string(),
            emoji: None,
            parent_id: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        write_collections(
            &root_str,
            &CollectionsFile {
                collections: vec![collection],
                assignments: vec![Assignment {
                    paper_id,
                    collection_id: "collection-1".to_string(),
                }],
            },
        )
        .unwrap();

        delete_collection(&root_str, "collection-1").unwrap();

        assert!(root.join("papers").join("paper-a").join("meta.json").exists());
        assert!(!collection_dir.exists());
        let file = read_collections(&root_str);
        assert!(file.collections.is_empty());
        assert!(file.assignments.is_empty());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sanitize_name_blocks_dot_segments() {
        assert_eq!(sanitize_name(".."), "Untitled");
        assert_eq!(sanitize_name("."), "Untitled");
    }
}
