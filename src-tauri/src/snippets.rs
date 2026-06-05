use chrono::Utc;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::models::{Snippet, SnippetLibrary};

// ── Paths ─────────────────────────────────────────────────────────────────────

fn snippets_dir(root: &str) -> PathBuf {
    Path::new(root).join("snippets")
}

fn libraries_path(root: &str) -> PathBuf {
    snippets_dir(root).join("libraries.json")
}

fn validate_library_id(library_id: &str) -> Result<(), String> {
    crate::path_guard::validate_segment("snippet library id", library_id)
}

fn library_snippets_path(root: &str, library_id: &str) -> Result<PathBuf, String> {
    validate_library_id(library_id)?;
    Ok(snippets_dir(root).join(format!("{}.json", library_id)))
}

fn ensure_snippets_dir(root: &str) -> Result<(), String> {
    std::fs::create_dir_all(snippets_dir(root)).map_err(|e| format!("Create snippets dir: {e}"))
}

// ── Libraries ─────────────────────────────────────────────────────────────────

fn read_libraries(root: &str) -> Vec<SnippetLibrary> {
    let path = libraries_path(root);
    if !path.exists() {
        return vec![];
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_libraries(root: &str, libs: &[SnippetLibrary]) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(libs).map_err(|e| format!("Serialize libraries: {e}"))?;
    std::fs::write(libraries_path(root), content).map_err(|e| format!("Write libraries: {e}"))
}

pub fn list_snippet_libraries(root: &str) -> Result<Vec<SnippetLibrary>, String> {
    ensure_snippets_dir(root)?;
    Ok(read_libraries(root))
}

pub fn create_snippet_library(
    root: &str,
    name: String,
    emoji: Option<String>,
) -> Result<SnippetLibrary, String> {
    ensure_snippets_dir(root)?;
    let lib = SnippetLibrary {
        id: Uuid::new_v4().to_string(),
        name: name.trim().to_string(),
        emoji,
        created_at: Utc::now().to_rfc3339(),
    };
    let mut libs = read_libraries(root);
    libs.push(lib.clone());
    write_libraries(root, &libs)?;
    // Create empty snippets file
    let snippets_path = library_snippets_path(root, &lib.id)?;
    if !snippets_path.exists() {
        std::fs::write(&snippets_path, "[]").map_err(|e| format!("Init snippets file: {e}"))?;
    }
    Ok(lib)
}

pub fn rename_snippet_library(root: &str, id: &str, name: String) -> Result<(), String> {
    ensure_snippets_dir(root)?;
    validate_library_id(id)?;
    let mut libs = read_libraries(root);
    if let Some(lib) = libs.iter_mut().find(|l| l.id == id) {
        lib.name = name.trim().to_string();
    }
    write_libraries(root, &libs)
}

pub fn update_snippet_library_emoji(
    root: &str,
    id: &str,
    emoji: Option<String>,
) -> Result<(), String> {
    ensure_snippets_dir(root)?;
    validate_library_id(id)?;
    let mut libs = read_libraries(root);
    if let Some(lib) = libs.iter_mut().find(|l| l.id == id) {
        lib.emoji = emoji;
    }
    write_libraries(root, &libs)
}

pub fn delete_snippet_library(root: &str, id: &str) -> Result<(), String> {
    ensure_snippets_dir(root)?;
    validate_library_id(id)?;
    let mut libs = read_libraries(root);
    libs.retain(|l| l.id != id);
    write_libraries(root, &libs)?;
    let snippets_path = library_snippets_path(root, id)?;
    if snippets_path.exists() {
        std::fs::remove_file(&snippets_path).map_err(|e| format!("Delete snippets file: {e}"))?;
    }
    Ok(())
}

// ── Snippets ──────────────────────────────────────────────────────────────────

fn read_snippets(root: &str, library_id: &str) -> Vec<Snippet> {
    let Ok(path) = library_snippets_path(root, library_id) else {
        return vec![];
    };
    if !path.exists() {
        return vec![];
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_snippets(root: &str, library_id: &str, snippets: &[Snippet]) -> Result<(), String> {
    validate_library_id(library_id)?;
    let content =
        serde_json::to_string_pretty(snippets).map_err(|e| format!("Serialize snippets: {e}"))?;
    std::fs::write(library_snippets_path(root, library_id)?, content)
        .map_err(|e| format!("Write snippets: {e}"))
}

pub fn get_snippets(root: &str, library_id: &str) -> Result<Vec<Snippet>, String> {
    ensure_snippets_dir(root)?;
    validate_library_id(library_id)?;
    Ok(read_snippets(root, library_id))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddSnippetInput {
    pub library_id: String,
    pub text: String,
    pub tags: Vec<String>,
    pub note: String,
    pub paper_id: String,
    pub paper_title: String,
    pub page: u32,
    pub color: Option<String>,
}

pub fn add_snippet(root: &str, input: AddSnippetInput) -> Result<Snippet, String> {
    ensure_snippets_dir(root)?;
    validate_library_id(&input.library_id)?;
    let snippet = Snippet {
        id: Uuid::new_v4().to_string(),
        library_id: input.library_id.clone(),
        text: input.text,
        tags: input.tags,
        note: input.note,
        paper_id: input.paper_id,
        paper_title: input.paper_title,
        page: input.page,
        color: input.color,
        created_at: Utc::now().to_rfc3339(),
    };
    let mut snippets = read_snippets(root, &input.library_id);
    snippets.push(snippet.clone());
    write_snippets(root, &input.library_id, &snippets)?;
    Ok(snippet)
}

pub fn update_snippet(
    root: &str,
    library_id: &str,
    id: &str,
    tags: Option<Vec<String>>,
    note: Option<String>,
) -> Result<(), String> {
    ensure_snippets_dir(root)?;
    validate_library_id(library_id)?;
    let mut snippets = read_snippets(root, library_id);
    if let Some(s) = snippets.iter_mut().find(|s| s.id == id) {
        if let Some(t) = tags {
            s.tags = t;
        }
        if let Some(n) = note {
            s.note = n;
        }
    }
    write_snippets(root, library_id, &snippets)
}

pub fn delete_snippet(root: &str, library_id: &str, id: &str) -> Result<(), String> {
    ensure_snippets_dir(root)?;
    validate_library_id(library_id)?;
    let mut snippets = read_snippets(root, library_id);
    snippets.retain(|s| s.id != id);
    write_snippets(root, library_id, &snippets)
}

// ── Migration from localStorage ───────────────────────────────────────────────
// Called once when the frontend sends its localStorage data on first launch.

pub fn migrate_from_localstorage(
    root: &str,
    libraries: Vec<SnippetLibrary>,
    snippets_by_library: Vec<(String, Vec<Snippet>)>,
) -> Result<(), String> {
    ensure_snippets_dir(root)?;
    // Only migrate if the libraries file doesn't exist yet
    if libraries_path(root).exists() {
        return Ok(());
    }
    for lib in &libraries {
        validate_library_id(&lib.id)?;
    }
    write_libraries(root, &libraries)?;
    for (library_id, snippets) in snippets_by_library {
        write_snippets(root, &library_id, &snippets)?;
    }
    Ok(())
}
