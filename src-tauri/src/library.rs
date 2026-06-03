use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::ai_summary;
use crate::models::{IndexFile, LibraryConfig, PaperIndexEntry};
use crate::paper;

fn mkdir_if_missing(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        std::fs::create_dir_all(path)
            .map_err(|e| format!("Failed to create directory {}: {e}", path.display()))?;
    }
    Ok(())
}

fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn synced_status(root: &str, slug: &str, paper_path: &Path) -> crate::models::PaperStatus {
    let mut status = paper::read_status(paper_path);
    let has_ai_summary = ai_summary::has_summary(root, slug);
    if status.ai_summary_done != has_ai_summary {
        status.ai_summary_done = has_ai_summary;
        status.last_updated = chrono::Utc::now().to_rfc3339();
        let _ = paper::write_status(root, slug, &status);
    }
    status
}

/// Open (or initialize) a library at `root`.
///
/// Idempotent: ensures the library folders exist, then reads or creates config.
pub fn open_library(root: &str) -> Result<LibraryConfig, String> {
    let root_path = Path::new(root);
    let argus_dir = root_path.join(".argus");
    let config_path = argus_dir.join("config.json");

    mkdir_if_missing(&argus_dir)?;
    mkdir_if_missing(&root_path.join("papers"))?;
    mkdir_if_missing(&root_path.join("canvases"))?;
    mkdir_if_missing(&root_path.join("inbox"))?;

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("Failed to read config.json: {e}"))?;
        return serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse config.json: {e}"));
    }

    let config = LibraryConfig {
        version: "1.0.0".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let content = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    std::fs::write(&config_path, content)
        .map_err(|e| format!("Failed to write config.json: {e}"))?;

    Ok(config)
}

/// Read the on-disk index.json cache without touching any paper directories.
/// Returns an empty list if the cache is missing or unreadable — never fails.
pub fn load_library_cache(root: &str) -> Vec<PaperIndexEntry> {
    let index_path = Path::new(root).join(".argus").join("index.json");
    let content = match std::fs::read_to_string(&index_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    match serde_json::from_str::<IndexFile>(&content) {
        Ok(f) => f.papers,
        Err(e) => {
            eprintln!("[cache] corrupt index.json: {e}");
            Vec::new()
        }
    }
}

/// Incremental scan of `papers/` — only re-reads meta.json when its mtime changed.
///
/// Algorithm:
/// 1. Load existing index.json into a HashMap keyed by slug.
/// 2. Enumerate paper directories (one readdir).
/// 3. For each dir: stat meta.json; if mtime matches the cached entry → skip the read,
///    but always re-read .status.json (status changes independently of meta).
/// 4. Drop entries whose directories no longer exist.
/// 5. Persist the updated index.json.
pub fn scan_library(root: &str) -> Result<Vec<PaperIndexEntry>, String> {
    // ── Step 1: Load existing cache ──────────────────────────────────────────
    let index_path = Path::new(root).join(".argus").join("index.json");
    let mut cache: HashMap<String, PaperIndexEntry> = HashMap::new();
    if let Ok(content) = std::fs::read_to_string(&index_path) {
        if let Ok(index_file) = serde_json::from_str::<IndexFile>(&content) {
            for entry in index_file.papers {
                cache.insert(entry.slug.clone(), entry);
            }
        }
    }

    // ── Step 2: Walk paper directories ───────────────────────────────────────
    let mut entries: Vec<PaperIndexEntry> = Vec::new();

    for (slug, path) in paper::list_paper_dirs(root)? {
        let meta_path = path.join("meta.json");
        let current_mtime = file_mtime(&meta_path);

        // ── Step 3a: Cache hit — meta unchanged, refresh status only ─────────
        if let Some(cached) = cache.get(&slug) {
            if cached.meta_mtime > 0 && cached.meta_mtime == current_mtime {
                let mut entry = cached.clone();
                entry.status = synced_status(root, &slug, &path);
                entries.push(entry);
                continue;
            }
        }

        // ── Step 3b: Cache miss / meta changed — full re-read ─────────────
        let meta_content = match std::fs::read_to_string(&meta_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("[scan] missing meta.json for {slug}: {e}");
                continue;
            }
        };
        let meta: crate::models::PaperMeta = match serde_json::from_str(&meta_content) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[scan] corrupt meta.json for {slug}: {e}");
                continue;
            }
        };

        let status = synced_status(root, &slug, &path);
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
            meta_mtime: current_mtime,
            import_source: meta.import_source.or_else(|| {
                // Backfill for legacy entries: infer "arxiv" if arxiv_id is present.
                meta.arxiv_id.as_ref().map(|_| "arxiv".to_string())
            }),
        });
    }

    // ── Step 5: Persist updated index ────────────────────────────────────────
    let index_file = IndexFile {
        papers: entries.clone(),
    };
    if let Ok(content) = serde_json::to_string_pretty(&index_file) {
        if let Err(e) = std::fs::write(&index_path, content) {
            eprintln!("[scan] failed to write index.json: {e}");
        }
    }

    Ok(entries)
}
