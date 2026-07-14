use chrono::Utc;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::models::{Canvas, CanvasIndexEntry, CanvasSettings, Viewport};

const CANVAS_WINDOW_SIZE_STORE_KEY: &str = "canvas_window_size_v1";
const CANVAS_DEFAULT_WINDOW_W: f64 = 1280.0;
const CANVAS_DEFAULT_WINDOW_H: f64 = 820.0;
const CANVAS_MIN_WINDOW_W: f64 = 900.0;
const CANVAS_MIN_WINDOW_H: f64 = 600.0;

// ── Paths ─────────────────────────────────────────────────────────────────────

fn canvases_dir(root: &str) -> PathBuf {
    Path::new(root).join("canvases")
}

fn legacy_canvases_dir(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("canvases")
}

fn validate_canvas_id(id: &str) -> Result<(), String> {
    crate::path_guard::validate_segment("canvas id", id)
}

fn canvas_path(root: &str, id: &str) -> Result<PathBuf, String> {
    validate_canvas_id(id)?;
    Ok(canvases_dir(root).join(format!("{}.json", id)))
}

fn index_path(root: &str) -> PathBuf {
    canvases_dir(root).join("index.json")
}

/// Write `content` to `path` atomically (temp file + rename) so a crash or power
/// loss mid-write can never leave a truncated, unparseable JSON file behind.
fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    crate::fsutil::atomic_write_str(path, content)
}

/// Rebuild the index by scanning every `*.json` canvas file in the directory.
/// Used to recover when `index.json` is missing or corrupt instead of silently
/// presenting an empty list (which a subsequent save would then persist, wiping
/// the user's other canvases from the listing).
fn rebuild_index_from_disk(root: &str) -> Vec<CanvasIndexEntry> {
    let dir = canvases_dir(root);
    let mut entries = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for entry in rd.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // Skip the index file itself.
            if path.file_name().and_then(|n| n.to_str()) == Some("index.json") {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(canvas) = serde_json::from_str::<Canvas>(&content) {
                    entries.push(CanvasIndexEntry {
                        id: canvas.id,
                        name: canvas.name,
                        node_count: canvas.nodes.len().min(u32::MAX as usize) as u32,
                        updated_at: canvas.updated_at,
                    });
                }
            }
        }
    }
    entries
}

fn ensure_canvases_dir(root: &str) -> Result<(), String> {
    let dir = canvases_dir(root);
    let legacy = legacy_canvases_dir(root);

    if !dir.exists() && legacy.exists() {
        std::fs::rename(&legacy, &dir).map_err(|e| format!("Migrate canvases dir: {e}"))?;
        return Ok(());
    }

    std::fs::create_dir_all(&dir).map_err(|e| format!("Create canvases dir: {e}"))?;

    if legacy.exists() {
        for entry in std::fs::read_dir(&legacy)
            .map_err(|e| format!("Read legacy canvases dir: {e}"))?
            .flatten()
        {
            let source = entry.path();
            let target = dir.join(entry.file_name());
            if target.exists() {
                continue;
            }
            let _ = std::fs::rename(&source, &target);
        }
        let _ = std::fs::remove_dir(&legacy);
    }

    Ok(())
}

// ── Index helpers ─────────────────────────────────────────────────────────────

fn read_index(root: &str) -> Vec<CanvasIndexEntry> {
    let path = index_path(root);
    if !path.exists() {
        // No index yet: recover any canvases already on disk (e.g. after a
        // migration or a deleted index) rather than reporting none.
        return rebuild_index_from_disk(root);
    }
    match std::fs::read_to_string(&path) {
        Ok(c) => match serde_json::from_str(&c) {
            Ok(entries) => entries,
            // Corrupt index (e.g. truncated by an interrupted write): rebuild
            // from the canvas files so a later save can't overwrite a good
            // listing with an empty one.
            Err(e) => {
                eprintln!("canvas index.json corrupt ({e}); rebuilding from disk");
                rebuild_index_from_disk(root)
            }
        },
        Err(e) => {
            eprintln!("canvas index.json unreadable ({e}); rebuilding from disk");
            rebuild_index_from_disk(root)
        }
    }
}

fn write_index(root: &str, entries: &[CanvasIndexEntry]) -> Result<(), String> {
    let content = serde_json::to_string_pretty(entries)
        .map_err(|e| format!("Serialize canvas index: {e}"))?;
    atomic_write(&index_path(root), &content).map_err(|e| format!("Write canvas index: {e}"))
}

fn upsert_index(root: &str, canvas: &Canvas) {
    let mut index = read_index(root);
    if let Some(entry) = index.iter_mut().find(|e| e.id == canvas.id) {
        entry.name = canvas.name.clone();
        entry.node_count = canvas.nodes.len().min(u32::MAX as usize) as u32;
        entry.updated_at = canvas.updated_at.clone();
    } else {
        index.push(CanvasIndexEntry {
            id: canvas.id.clone(),
            name: canvas.name.clone(),
            node_count: canvas.nodes.len().min(u32::MAX as usize) as u32,
            updated_at: canvas.updated_at.clone(),
        });
    }
    let _ = write_index(root, &index);
}

// ── Public API ────────────────────────────────────────────────────────────────

pub fn list_canvases(root: &str) -> Result<Vec<CanvasIndexEntry>, String> {
    ensure_canvases_dir(root)?;
    Ok(read_index(root))
}

pub fn create_canvas(root: &str, name: String) -> Result<Canvas, String> {
    ensure_canvases_dir(root)?;
    let now = Utc::now().to_rfc3339();
    let canvas = Canvas {
        id: Uuid::new_v4().to_string(),
        name,
        nodes: vec![],
        edges: vec![],
        viewport: Viewport::default(),
        created_at: now.clone(),
        updated_at: now,
    };
    let content =
        serde_json::to_string_pretty(&canvas).map_err(|e| format!("Serialize canvas: {e}"))?;
    atomic_write(&canvas_path(root, &canvas.id)?, &content)
        .map_err(|e| format!("Write canvas: {e}"))?;
    upsert_index(root, &canvas);
    Ok(canvas)
}

pub fn get_canvas(root: &str, id: &str) -> Result<Canvas, String> {
    let path = canvas_path(root, id)?;
    if !path.exists() {
        return Err(format!("Canvas not found: {id}"));
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("Read canvas: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Parse canvas: {e}"))
}

pub fn save_canvas(root: &str, mut canvas: Canvas) -> Result<(), String> {
    ensure_canvases_dir(root)?;
    validate_canvas_id(&canvas.id)?;
    canvas.updated_at = Utc::now().to_rfc3339();
    let content =
        serde_json::to_string_pretty(&canvas).map_err(|e| format!("Serialize canvas: {e}"))?;
    atomic_write(&canvas_path(root, &canvas.id)?, &content)
        .map_err(|e| format!("Write canvas: {e}"))?;
    upsert_index(root, &canvas);
    Ok(())
}

pub fn rename_canvas(root: &str, id: &str, new_name: String) -> Result<(), String> {
    let path = canvas_path(root, id)?;
    let mut canvas: Canvas = {
        let c = std::fs::read_to_string(&path).map_err(|e| format!("Read canvas: {e}"))?;
        serde_json::from_str(&c).map_err(|e| format!("Parse canvas: {e}"))?
    };
    canvas.name = new_name;
    canvas.updated_at = Utc::now().to_rfc3339();
    let content =
        serde_json::to_string_pretty(&canvas).map_err(|e| format!("Serialize canvas: {e}"))?;
    atomic_write(&path, &content).map_err(|e| format!("Write canvas: {e}"))?;
    upsert_index(root, &canvas);
    Ok(())
}

pub fn delete_canvas(root: &str, id: &str) -> Result<(), String> {
    let path = canvas_path(root, id)?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete canvas: {e}"))?;
    }
    let mut index = read_index(root);
    index.retain(|e| e.id != id);
    write_index(root, &index)
}

// ── Hover content ─────────────────────────────────────────────────────────────

pub fn get_node_display_content(
    root: &str,
    paper_id: &str,
    source: &str,
) -> Result<String, String> {
    let slug = find_slug_by_paper_id(root, paper_id)?;
    let paper_dir = crate::paper::paper_dir(root, &slug);

    match source {
        "notes" => {
            let notes_dir = paper_dir.join("notes");
            let notes_index_path = notes_dir.join("index.json");

            // Read canvas_notes from meta to decide which notes are pinned
            let pinned: Vec<String> = {
                let meta_path = paper_dir.join("meta.json");
                let c = std::fs::read_to_string(&meta_path).unwrap_or_default();
                let meta: serde_json::Value = serde_json::from_str(&c).unwrap_or_default();
                meta.get("canvas_notes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default()
            };

            if !pinned.is_empty() {
                // Load each pinned note in order; build a note-title index for headings
                let index: Vec<serde_json::Value> = if notes_index_path.exists() {
                    let c = std::fs::read_to_string(&notes_index_path).unwrap_or_default();
                    serde_json::from_str(&c).unwrap_or_default()
                } else {
                    vec![]
                };
                let title_of = |note_id: &str| -> String {
                    index
                        .iter()
                        .find(|e| e.get("id").and_then(|v| v.as_str()) == Some(note_id))
                        .and_then(|e| e.get("title").and_then(|v| v.as_str()))
                        .unwrap_or(note_id)
                        .to_string()
                };
                let mut parts = Vec::new();
                for note_id in &pinned {
                    let content = crate::paper::get_note(root, &slug, note_id);
                    if !content.trim().is_empty() {
                        let title = title_of(note_id);
                        parts.push(format!("## {}\n\n{}", title, content.trim()));
                    }
                }
                return Ok(parts.join("\n\n---\n\n"));
            }

            // No pins: fall back to most recently updated note
            if notes_index_path.exists() {
                let c = std::fs::read_to_string(&notes_index_path).unwrap_or_default();
                let mut entries: Vec<serde_json::Value> =
                    serde_json::from_str(&c).unwrap_or_default();
                entries.sort_by(|a, b| {
                    let ta = a.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
                    let tb = b.get("updated_at").and_then(|v| v.as_str()).unwrap_or("");
                    tb.cmp(ta)
                });
                if let Some(first) = entries.first() {
                    if let Some(note_id) = first.get("id").and_then(|v| v.as_str()) {
                        let content = crate::paper::get_note(root, &slug, note_id);
                        if !content.trim().is_empty() {
                            return Ok(content);
                        }
                    }
                }
            }
            // Legacy notes.md fallback
            let legacy = paper_dir.join("notes.md");
            if legacy.exists() {
                return std::fs::read_to_string(&legacy).map_err(|e| format!("Read notes.md: {e}"));
            }
            Ok(String::new())
        }
        "summary" => {
            let path = paper_dir.join("summary").join("data.md");
            if path.exists() {
                std::fs::read_to_string(&path).map_err(|e| format!("Read summary: {e}"))
            } else {
                Ok(String::new())
            }
        }
        "abstract" => {
            let meta_path = paper_dir.join("meta.json");
            if meta_path.exists() {
                let c = std::fs::read_to_string(&meta_path).unwrap_or_default();
                let meta: serde_json::Value = serde_json::from_str(&c).unwrap_or_default();
                // Try "abstract" field first, then "summary"
                for field in &["abstract", "summary"] {
                    if let Some(text) = meta.get(field).and_then(|v| v.as_str()) {
                        if !text.trim().is_empty() {
                            return Ok(text.to_string());
                        }
                    }
                }
            }
            Ok(String::new())
        }
        _ => Ok(String::new()),
    }
}

fn find_slug_by_paper_id(root: &str, paper_id: &str) -> Result<String, String> {
    for (slug, path) in crate::paper::list_paper_dirs(root)? {
        let meta_path = path.join("meta.json");
        let c = match std::fs::read_to_string(&meta_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let meta: serde_json::Value = match serde_json::from_str(&c) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if meta.get("id").and_then(|v| v.as_str()) == Some(paper_id) {
            return Ok(slug);
        }
    }
    Err(format!("Paper not found: {paper_id}"))
}

// ── Canvas settings ───────────────────────────────────────────────────────────

pub fn read_canvas_settings(root: &str) -> CanvasSettings {
    let path = Path::new(root).join(".argus").join("config.json");
    if !path.exists() {
        return CanvasSettings::default();
    }
    let c = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return CanvasSettings::default(),
    };
    let config: serde_json::Value = match serde_json::from_str(&c) {
        Ok(v) => v,
        Err(_) => return CanvasSettings::default(),
    };
    config
        .get("canvas_settings")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub fn write_canvas_settings(root: &str, settings: &CanvasSettings) -> Result<(), String> {
    let path = Path::new(root).join(".argus").join("config.json");
    let mut config: serde_json::Value = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    config["canvas_settings"] =
        serde_json::to_value(settings).map_err(|e| format!("Serialize CanvasSettings: {e}"))?;
    let content =
        serde_json::to_string_pretty(&config).map_err(|e| format!("Serialize config.json: {e}"))?;
    atomic_write(&path, &content).map_err(|e| format!("Write config.json: {e}"))
}

// ── Window ────────────────────────────────────────────────────────────────────

fn load_canvas_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let value = store.get(CANVAS_WINDOW_SIZE_STORE_KEY)?;
    let width = value.get("w")?.as_f64()?;
    let height = value.get("h")?.as_f64()?;
    if width >= CANVAS_MIN_WINDOW_W
        && height >= CANVAS_MIN_WINDOW_H
        && width <= 4000.0
        && height <= 3000.0
    {
        Some((width, height))
    } else {
        None
    }
}

pub fn save_canvas_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < CANVAS_MIN_WINDOW_W || height < CANVAS_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            CANVAS_WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

pub fn open_canvas_window(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    if let Some(win) = app.get_webview_window("canvas") {
        let _ = win.set_focus();
        return Ok(());
    }

    let (width, height) =
        load_canvas_window_size(app).unwrap_or((CANVAS_DEFAULT_WINDOW_W, CANVAS_DEFAULT_WINDOW_H));

    let win = WebviewWindowBuilder::new(
        app,
        "canvas",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 画布")
    .inner_size(width, height)
    .min_inner_size(CANVAS_MIN_WINDOW_W, CANVAS_MIN_WINDOW_H)
    .build()
    .map_err(|e| format!("Open canvas window: {e}"))?;

    let app_handle = app.clone();
    win.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { .. } = event {
            if let Some(w) = app_handle.get_webview_window("canvas") {
                if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                    save_canvas_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
                }
            }
        }
    });

    Ok(())
}
