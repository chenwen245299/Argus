use tauri::{Emitter, State};

use crate::models::{
    AiModel, AiProviderInfo, AiProviderInput, AiSettingsInfo, AppSettings, ArxivConfig, ArxivInbox,
    ArxivPaper, ArxivScheduleStatus, Canvas, CanvasIndexEntry, CanvasSettings, ChatMessage,
    CliAnalysisEntry, CliPromptTemplate, CliSettings, CliTool, Collection, CollectionsFile,
    Highlight, LibraryConfig, NodePosition, Note, PaperIndexEntry, PaperMeta, PaperStatus,
    RagSettings, ReadingState, RetrievedChunk, SearchHit, SuggestedEdge, VectorStoreInfo,
};
use crate::LibraryRoot;
use crate::{
    ai_manager, ai_summary, arxiv, arxiv_scheduler, canvas, canvas_enhance, cli_manager,
    cli_runner, collections, copilot, extraction, library, llm, metadata, paper, rag,
    search, settings, url_import,
};
// ── Library management ────────────────────────────────────────────────────────

#[tauri::command]
pub async fn pick_library_folder(app: tauri::AppHandle) -> Result<String, String> {
    use tauri_plugin_dialog::DialogExt;

    let (tx, rx) = tokio::sync::oneshot::channel::<Option<tauri_plugin_dialog::FilePath>>();

    app.dialog().file().pick_folder(move |result| {
        let _ = tx.send(result);
    });

    match rx.await.map_err(|e| e.to_string())? {
        Some(path) => Ok(path.to_string()),
        None => Err("cancelled".to_string()),
    }
}

#[tauri::command]
pub async fn open_library(
    root: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<LibraryConfig, String> {
    let config = library::open_library(&root)?;

    // Clean up stale temp directories left by interrupted imports.
    let papers_dir = std::path::Path::new(&root).join("papers");
    if let Ok(entries) = std::fs::read_dir(&papers_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("importing_") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
    }

    {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        *guard = Some(root.clone());
    }

    persist_library_path(&app, &root);

    // Track current root for token usage recording.
    crate::token_usage::set_root(&root);

    // Update scheduler enabled state based on config.
    let arxiv_cfg = arxiv::get_arxiv_config(&root);
    arxiv_scheduler::on_auto_fetch_changed(arxiv_cfg.auto_fetch_enabled);

    let root_c = root.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(e) = search::ensure_current(&root_c) {
            eprintln!("[search] ensure current index: {e}");
        }
    });

    Ok(config)
}

#[tauri::command]
pub async fn get_current_library(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<Option<String>, String> {
    // Check in-memory state first (already loaded this session).
    {
        let guard = state.0.lock().map_err(|e| e.to_string())?;
        if let Some(ref p) = *guard {
            return Ok(Some(p.clone()));
        }
    }

    // Fall back to persisted store.
    let stored = load_stored_library_path(&app);
    if let Some(ref path) = stored {
        let mut guard = state.0.lock().map_err(|e| e.to_string())?;
        *guard = Some(path.clone());
    }
    Ok(stored)
}

#[tauri::command]
pub async fn load_library_cache(
    state: State<'_, LibraryRoot>,
) -> Result<Vec<PaperIndexEntry>, String> {
    let root = get_root(&state)?;
    Ok(library::load_library_cache(&root))
}

#[tauri::command]
pub async fn scan_library(state: State<'_, LibraryRoot>) -> Result<Vec<PaperIndexEntry>, String> {
    let root = get_root(&state)?;
    library::scan_library(&root)
}

// ── Paper list ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn list_papers(state: State<'_, LibraryRoot>) -> Result<Vec<PaperIndexEntry>, String> {
    // list_papers re-scans so the list is always fresh.
    let root = get_root(&state)?;
    library::scan_library(&root)
}

// ── Single paper ──────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_paper_meta(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<PaperMeta, String> {
    let root = get_root(&state)?;
    paper::read_meta(&root, &slug)
}

#[tauri::command]
pub async fn save_paper_meta(
    slug: String,
    meta: PaperMeta,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::write_meta(&root, &slug, &meta)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

#[tauri::command]
pub async fn get_notes(slug: String, state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    Ok(paper::read_notes(&root, &slug))
}

#[tauri::command]
pub async fn save_notes(
    slug: String,
    content: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::write_notes(&root, &slug, &content)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

#[tauri::command]
pub async fn list_notes(slug: String, state: State<'_, LibraryRoot>) -> Result<Vec<Note>, String> {
    let root = get_root(&state)?;
    Ok(paper::list_notes(&root, &slug))
}

#[tauri::command]
pub async fn get_note(
    slug: String,
    note_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    Ok(paper::get_note(&root, &slug, &note_id))
}

#[tauri::command]
pub async fn create_note(slug: String, state: State<'_, LibraryRoot>) -> Result<Note, String> {
    let root = get_root(&state)?;
    let note = paper::create_note(&root, &slug)?;
    refresh_search_index(&root, &slug);
    Ok(note)
}

#[tauri::command]
pub async fn save_note(
    slug: String,
    note_id: String,
    content: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::save_note(&root, &slug, &note_id, &content)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

#[tauri::command]
pub async fn rename_note(
    slug: String,
    note_id: String,
    title: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::rename_note(&root, &slug, &note_id, &title)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

#[tauri::command]
pub async fn delete_note(
    slug: String,
    note_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::delete_note(&root, &slug, &note_id)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

#[tauri::command]
pub async fn get_highlights(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<Highlight>, String> {
    let root = get_root(&state)?;
    Ok(paper::read_highlights(&root, &slug))
}

#[tauri::command]
pub async fn save_highlights(
    slug: String,
    highlights: Vec<Highlight>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::write_highlights(&root, &slug, &highlights)
}

#[tauri::command]
pub async fn get_reading_state(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Option<ReadingState>, String> {
    let root = get_root(&state)?;
    Ok(paper::read_reading_state(&root, &slug))
}

#[tauri::command]
pub async fn update_reading_state(
    slug: String,
    reading_state: ReadingState,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    paper::write_reading_state(&root, &slug, &reading_state)
}

#[tauri::command]
pub async fn get_paper_status(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<PaperStatus, String> {
    let root = get_root(&state)?;
    Ok(paper::read_status_for(&root, &slug))
}

/// Update the user-controlled reading status: "unread" | "reading" | "read"
#[tauri::command]
pub async fn set_reading_status(
    slug: String,
    status: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let mut meta = paper::read_meta(&root, &slug)?;
    meta.reading_status = status;
    paper::write_meta(&root, &slug, &meta)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

// ── PDF bytes ─────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn read_pdf_bytes(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<u8>, String> {
    let root = get_root(&state)?;
    let path = metadata::find_pdf_in_dir(&root, &slug);
    std::fs::read(&path).map_err(|e| format!("Cannot read PDF for {slug}: {e}"))
}

// ── PDF import ────────────────────────────────────────────────────────────────

/// Pick one or more PDF files via the native file dialog.
/// Returns a list of absolute file paths.
#[tauri::command]
pub async fn pick_pdf_files(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<Vec<tauri_plugin_dialog::FilePath>>>();
    app.dialog()
        .file()
        .add_filter("PDF files", &["pdf"])
        .pick_files(move |result| {
            let _ = tx.send(result);
        });
    match rx.await.map_err(|e| e.to_string())? {
        Some(paths) => Ok(paths.iter().map(|p| p.to_string()).collect()),
        None => Ok(vec![]),
    }
}

/// Copy a PDF into the library and set up initial companion files.
/// Returns the temporary slug so the caller can immediately show the paper.
#[tauri::command]
pub async fn import_pdf(
    source_path: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;

    let id = uuid::Uuid::new_v4();
    let short_id: String = id.to_string().replace('-', "").chars().take(8).collect();
    let temp_slug = format!("importing_{}", short_id);

    let src = std::path::Path::new(&source_path);
    let filename = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.pdf")
        .to_string();
    let title = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Untitled")
        .to_string();

    // Create paper directory
    let papers_dir = std::path::Path::new(&root).join("papers");
    let paper_dir = papers_dir.join(&temp_slug);
    std::fs::create_dir_all(&paper_dir).map_err(|e| format!("Cannot create paper dir: {e}"))?;

    // Copy PDF (non-blocking) — named after the sanitized original file stem
    let pdf_filename = format!("{}.pdf", metadata::sanitize_filename(&title));
    let dest = paper_dir.join(&pdf_filename);
    let source_path_c = source_path.clone();
    tauri::async_runtime::spawn_blocking(move || std::fs::copy(&source_path_c, &dest))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Cannot copy PDF: {e}"))?;

    // Write initial meta.json
    let meta = PaperMeta {
        id: id.to_string(),
        title,
        authors: vec![],
        year: None,
        doi: None,
        arxiv_id: None,
        venue: None,
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(filename),
        reading_status: "unread".to_string(),
        paper_abstract: None,
        bibtex: None,
        canvas_notes: vec![],
    };
    paper::write_meta(&root, &temp_slug, &meta)?;

    // Create companion files (notes.md, highlights.json, .status.json)
    paper::ensure_paper_files(&root, &temp_slug);
    refresh_search_index(&root, &temp_slug);

    Ok(temp_slug)
}

/// Delete a paper by removing its entire directory from the library.
#[tauri::command]
pub async fn delete_paper(slug: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    let dir = paper::paper_dir(&root, &slug);
    if !dir.exists() {
        return Err(format!("Paper directory not found: {}", slug));
    }
    // Remove from search index before deleting the directory
    let _ = search::remove_paper(&root, &slug);
    // Remove RAG vector chunks so the vector DB doesn't accumulate stale data
    if let Ok(meta) = paper::read_meta(&root, &slug) {
        let _ = rag::delete_paper_chunks(&root, &meta.id).await;
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Cannot delete paper: {e}"))
}

/// Open the paper's actual folder in the system file manager.
#[tauri::command]
pub async fn open_paper_folder(slug: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    let dir = paper::paper_dir(&root, &slug);
    if !dir.exists() {
        return Err(format!("Paper directory not found: {}", slug));
    }
    open_in_finder(dir.to_string_lossy().to_string())
}

/// Copy the paper PDF itself to the system clipboard, so it can be pasted in Finder.
#[tauri::command]
pub async fn copy_paper_pdf(slug: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    let path = metadata::find_pdf_in_dir(&root, &slug);
    if !path.exists() {
        return Err(format!("PDF file not found: {}", path.display()));
    }

    let path = path
        .canonicalize()
        .map_err(|e| format!("Cannot resolve PDF path: {e}"))?;
    copy_file_to_clipboard(path.to_string_lossy().to_string())
}

/// Run the full metadata fetch cascade for a slug and return the updated meta.
#[tauri::command]
pub async fn fetch_metadata(
    slug: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<PaperMeta, String> {
    let root = get_root(&state)?;
    let meta = metadata::fetch_and_apply(&root, &slug, &app).await?;
    refresh_search_index(&root, &slug);
    Ok(meta)
}

/// Manually trigger AI-based metadata extraction for a paper (via context menu).
#[tauri::command]
pub async fn extract_metadata_ai(
    slug: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<PaperMeta, String> {
    let root = get_root(&state)?;
    let meta = metadata::fetch_metadata_with_ai(&root, &slug, &app).await?;
    refresh_search_index(&root, &slug);
    Ok(meta)
}

/// Extract first-page text from a paper's PDF (useful for manual inspection / AI calls).
#[tauri::command]
pub async fn extract_first_page_text(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    let slug_c = slug.clone();
    tauri::async_runtime::spawn_blocking(move || metadata::extract_text_sync(&root_c, &slug_c))
        .await
        .map_err(|e| e.to_string())?
}

/// Rename the paper folder to a canonical slug derived from meta.title + meta.year.
/// Returns the new slug. Never overwrites an existing folder.
#[tauri::command]
pub async fn rename_paper_folder(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    let slug_c = slug.clone();
    let new_slug =
        tauri::async_runtime::spawn_blocking(move || metadata::rename_folder(&root_c, &slug_c))
            .await
            .map_err(|e| e.to_string())??;

    if new_slug != slug {
        let _ = search::remove_paper(&root, &slug);
    }
    refresh_search_index(&root, &new_slug);
    Ok(new_slug)
}

// ── M4: Settings ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_settings(state: State<'_, LibraryRoot>) -> Result<AppSettings, String> {
    let root = get_root(&state)?;
    Ok(settings::read_settings(&root))
}

#[tauri::command]
pub async fn save_settings(
    settings_data: AppSettings,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    settings::write_settings(&root, &settings_data)
}

// ── M4: Collections ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_collections(state: State<'_, LibraryRoot>) -> Result<CollectionsFile, String> {
    let root = get_root(&state)?;
    collections::get_collections(&root)
}

#[tauri::command]
pub async fn create_collection(
    name: String,
    parent_id: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<Collection, String> {
    let root = get_root(&state)?;
    collections::create_collection(&root, name, parent_id)
}

#[tauri::command]
pub async fn rename_collection(
    id: String,
    new_name: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::rename_collection(&root, &id, new_name)
}

#[tauri::command]
pub async fn set_collection_emoji(
    id: String,
    emoji: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::set_collection_emoji(&root, &id, emoji)
}

#[tauri::command]
pub async fn delete_collection(id: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::delete_collection(&root, &id)
}

#[tauri::command]
pub async fn add_paper_to_collection(
    paper_id: String,
    collection_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::add_paper_to_collection(&root, &paper_id, &collection_id)
}

#[tauri::command]
pub async fn move_paper_to_collection(
    paper_id: String,
    collection_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::move_paper_to_collection(&root, &paper_id, &collection_id)
}

#[tauri::command]
pub async fn remove_paper_from_collection(
    paper_id: String,
    collection_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::remove_paper_from_collection(&root, &paper_id, &collection_id)
}

#[tauri::command]
pub async fn list_papers_in_collection(
    collection_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<PaperIndexEntry>, String> {
    let root = get_root(&state)?;
    collections::list_papers_in_collection(&root, &collection_id)
}

#[tauri::command]
pub async fn get_papers_folder_path(state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    let path = std::path::Path::new(&root).join("papers");
    std::fs::create_dir_all(&path).map_err(|e| format!("Create papers folder: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_collection_folder_path(
    collection_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let path = collections::collection_folder_path_for(&root, &collection_id)?;
    Ok(path.to_string_lossy().to_string())
}

// ── M4: Full-text extraction ──────────────────────────────────────────────────

#[tauri::command]
pub async fn extract_fulltext(slug: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    let s = settings::read_settings(&root);
    let root_c = root.clone();
    let slug_c = slug.clone();
    tauri::async_runtime::spawn_blocking(move || {
        match extraction::extract_and_write(&root_c, &slug_c, &s) {
            extraction::ExtractionResult::Failed(e) => Err(e),
            extraction::ExtractionResult::NoText => {
                Err("No extractable text found in this PDF.".to_string())
            }
            _ => {
                let _ = search::index_paper(&root_c, &slug_c);
                Ok(())
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_fulltext(slug: String, state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    Ok(extraction::read_fulltext(&root, &slug))
}

/// Batch-extract fulltext for all papers missing it. Emits progress events.
#[tauri::command]
pub async fn batch_extract_fulltext(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = get_root(&state)?;

    let slugs: Vec<String> = paper::list_paper_dirs(&root)?
        .into_iter()
        .map(|(slug, _)| slug)
        .collect();

    let total = slugs.len();

    tauri::async_runtime::spawn(async move {
        let s = settings::read_settings(&root);
        for (i, slug) in slugs.iter().enumerate() {
            let status = paper::read_status_for(&root, slug);
            if status.text_extracted {
                continue;
            }
            let r = {
                let root_c = root.clone();
                let slug_c = slug.clone();
                let sc = s.clone();
                tauri::async_runtime::spawn_blocking(move || {
                    extraction::extract_and_write(&root_c, &slug_c, &sc)
                })
                .await
            };

            let event_payload = match r {
                Ok(extraction::ExtractionResult::Text) => {
                    // Also index in FTS.
                    let _ = search::index_paper(&root, slug);
                    serde_json::json!({
                        "slug": slug, "done": i + 1, "total": total, "ok": true
                    })
                }
                Ok(extraction::ExtractionResult::NoText) => serde_json::json!({
                    "slug": slug, "done": i + 1, "total": total, "ok": false,
                    "reason": "no_text"
                }),
                _ => serde_json::json!({
                    "slug": slug, "done": i + 1, "total": total, "ok": false,
                    "reason": "error"
                }),
            };

            let _ = app.emit("extraction_progress", event_payload);
        }
        let _ = app.emit("extraction_done", serde_json::json!({ "total": total }));
    });

    Ok(())
}

// ── M4: Full-text search ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn index_paper_search(slug: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    let slug_c = slug.clone();
    tauri::async_runtime::spawn_blocking(move || search::index_paper(&root_c, &slug_c))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn rebuild_search_index(state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    tauri::async_runtime::spawn_blocking(move || search::rebuild_index(&root_c))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn search_fulltext(
    query: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<SearchHit>, String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    tauri::async_runtime::spawn_blocking(move || search::search_fulltext(&root_c, &query))
        .await
        .map_err(|e| e.to_string())?
}

// ── M5: AI Service Center ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_ai_settings(state: State<'_, LibraryRoot>) -> Result<AiSettingsInfo, String> {
    let root = get_root(&state)?;
    Ok(ai_manager::to_info(
        &root,
        &ai_manager::read_ai_settings(&root),
    ))
}

#[tauri::command]
pub async fn add_ai_provider(
    provider: AiProviderInput,
    api_key: String,
    state: State<'_, LibraryRoot>,
) -> Result<AiProviderInfo, String> {
    let root = get_root(&state)?;
    let p = ai_manager::add_provider(&root, provider, &api_key)?;
    Ok(AiProviderInfo {
        has_key: ai_manager::has_api_key(&root, &p.id),
        id: p.id,
        name: p.name,
        kind: p.kind,
        base_url: p.base_url,
        enabled: p.enabled,
        models: p.models,
    })
}

#[tauri::command]
pub async fn update_ai_provider(
    provider: AiProviderInput,
    api_key: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    ai_manager::update_provider(&root, provider, api_key.as_deref())
}

#[tauri::command]
pub async fn delete_ai_provider(id: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    ai_manager::delete_provider(&root, &id)
}

#[tauri::command]
pub async fn set_provider_enabled(
    id: String,
    enabled: bool,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    ai_manager::set_provider_enabled(&root, &id, enabled)
}

#[tauri::command]
pub async fn test_ai_provider(id: String, state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    let settings = ai_manager::read_ai_settings(&root);
    let provider = settings
        .providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Provider not found: {id}"))?;
    let key =
        ai_manager::get_api_key(&root, &id).ok_or("No API key configured for this provider")?;
    match llm::list_models(provider, &key).await {
        Ok(models) => Ok(format!("Connected. Found {} model(s).", models.len())),
        Err(e) => Err(e),
    }
}

// ── M5: Model management ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn fetch_provider_models(
    id: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<AiModel>, String> {
    let root = get_root(&state)?;
    let settings = ai_manager::read_ai_settings(&root);
    let provider = settings
        .providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Provider not found: {id}"))?;
    let key =
        ai_manager::get_api_key(&root, &id).ok_or("No API key configured for this provider")?;
    llm::list_models(provider, &key).await
}

#[tauri::command]
pub async fn save_provider_models(
    id: String,
    models: Vec<AiModel>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    ai_manager::save_provider_models(&root, &id, models)
}

#[tauri::command]
pub async fn set_default_model(
    provider_id: String,
    model_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    ai_manager::set_default_model(&root, &provider_id, &model_id)
}

// ── M5: AI Summary ────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn generate_summary(
    slug: String,
    provider_id: Option<String>,
    model_id: Option<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<Note, String> {
    let root = get_root(&state)?;
    ai_summary::generate_summary(
        &root,
        &slug,
        provider_id.as_deref(),
        model_id.as_deref(),
        &app,
    )
    .await
}

#[tauri::command]
pub async fn get_summary(slug: String, state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    Ok(ai_summary::read_summary(&root, &slug))
}

/// Extract the original Abstract/摘要 passage for a paper and save it to meta.json.
/// Uses the first 512 words of fulltext; falls back to the PDF front matter if no
/// fulltext is available.
#[tauri::command]
pub async fn extract_abstract_ai(
    slug: String,
    provider_id: Option<String>,
    model_id: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<PaperMeta, String> {
    let root = get_root(&state)?;
    let s = settings::read_settings(&root);
    let mut meta = paper::read_meta(&root, &slug)?;

    let fulltext = extraction::read_fulltext(&root, &slug);
    let context = if fulltext.trim().is_empty() {
        let root_c = root.clone();
        let slug_c = slug.clone();
        let front_matter = tauri::async_runtime::spawn_blocking(move || {
            metadata::extract_text_sync(&root_c, &slug_c).unwrap_or_default()
        })
        .await
        .unwrap_or_default();
        metadata::leading_words(&front_matter, 512)
    } else {
        metadata::leading_words(&fulltext, 512)
    };

    if context.trim().is_empty() {
        return Err("No text could be extracted from this PDF.".to_string());
    }

    let prompt_template = s.abstract_ai_prompt.clone();
    let prompt = if prompt_template.contains("{fulltext}") {
        prompt_template.replace("{fulltext}", &context)
    } else {
        format!("{}\n\n{}", prompt_template, context)
    };

    let prov_id = provider_id
        .as_deref()
        .or(s.abstract_ai_provider_id.as_deref());
    let mdl_id = model_id.as_deref().or(s.abstract_ai_model_id.as_deref());
    let (provider, api_key, model) = ai_manager::resolve_provider_model(&root, prov_id, mdl_id)?;

    let messages = vec![
        crate::models::ChatMessage {
            role: "system".to_string(),
            content: "你是一名信息抽取助手。只能从给定原文中抽取 Abstract/摘要段落，禁止生成、改写、翻译或概括。".to_string(),
        },
        crate::models::ChatMessage {
            role: "user".to_string(),
            content: prompt,
        },
    ];

    let abstract_text = llm::chat_completion(&provider, &api_key, &model, &messages, "summary")
        .await
        .map_err(|e| format!("AI call failed: {e}"))?;

    let abstract_text = abstract_text
        .trim()
        .trim_matches('`')
        .trim()
        .trim_start_matches("Abstract")
        .trim_start_matches("ABSTRACT")
        .trim_start_matches("摘要")
        .trim_start_matches([':', '：', '.', '。', '\n', '\r', ' '])
        .trim()
        .to_string();

    if abstract_text.is_empty() || abstract_text.eq_ignore_ascii_case("not_found") {
        return Err("No abstract passage was found in the provided text.".to_string());
    }

    meta.paper_abstract = Some(abstract_text);
    paper::write_meta(&root, &slug, &meta)?;
    refresh_search_index(&root, &slug);
    Ok(meta)
}

// ── M5: Copilot ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn chat_with_paper(
    slug: String,
    messages: Vec<ChatMessage>,
    provider_id: Option<String>,
    model_id: Option<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    copilot::chat_with_paper(
        &root,
        &slug,
        messages,
        provider_id.as_deref(),
        model_id.as_deref(),
        &app,
    )
    .await
}

#[tauri::command]
pub async fn chat_with_paper_event(
    slug: String,
    messages: Vec<ChatMessage>,
    provider_id: Option<String>,
    model_id: Option<String>,
    event_name: String,
    use_reasoning: Option<bool>,
    reasoning_effort: Option<String>,
    context_mode: Option<String>,
    use_pdf: Option<bool>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    copilot::chat_with_paper_on_event(
        &root,
        &slug,
        messages,
        provider_id.as_deref(),
        model_id.as_deref(),
        &event_name,
        &app,
        use_reasoning.unwrap_or(false),
        reasoning_effort.as_deref(),
        context_mode.as_deref().unwrap_or("fulltext"),
        use_pdf.unwrap_or(false),
    )
    .await
}

// ── Codex CLI integration ─────────────────────────────────────────────────────

#[tauri::command]
pub async fn check_codex_available() -> bool {
    tokio::process::Command::new("codex")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tauri::command]
pub async fn chat_paper_with_codex(
    slug: String,
    messages: Vec<ChatMessage>,
    event_name: String,
    context_mode: Option<String>,
    use_pdf: Option<bool>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let mode = context_mode.as_deref().unwrap_or("metadata");
    let attach_pdf = use_pdf.unwrap_or(false);

    // Build paper metadata header
    let meta = paper::read_meta(&root, &slug).ok();
    let mut ctx = String::new();

    if let Some(ref m) = meta {
        ctx.push_str(&format!("Paper: {}\n", m.title));
        if !m.authors.is_empty() {
            ctx.push_str(&format!("Authors: {}\n", m.authors.join(", ")));
        }
        if let Some(y) = m.year {
            ctx.push_str(&format!("Year: {y}\n"));
        }
        if let Some(ref v) = m.venue {
            ctx.push_str(&format!("Venue: {v}\n"));
        }
        if let Some(ref a) = m.paper_abstract {
            if !a.trim().is_empty() {
                ctx.push_str(&format!("Abstract: {}\n", a.trim()));
            }
        }
    }

    if mode == "summary" || mode == "summary+fulltext" {
        let summary = crate::ai_summary::read_summary(&root, &slug);
        if !summary.trim().is_empty() {
            ctx.push_str("\n--- AI SUMMARY ---\n");
            ctx.push_str(summary.trim());
        }
    }

    // Embed fulltext in prompt when fulltext mode is selected and PDF not attached
    if !attach_pdf && (mode == "fulltext" || mode == "summary+fulltext") {
        let txt = crate::extraction::read_fulltext(&root, &slug);
        if !txt.trim().is_empty() {
            let truncated: String = txt.chars().take(60_000).collect();
            ctx.push_str("\n--- PAPER CONTENT ---\n");
            ctx.push_str(&truncated);
            if txt.len() > 60_000 {
                ctx.push_str("\n[Content truncated]");
            }
        }
    }

    // Build conversation history text
    let mut history_text = String::new();
    for msg in &messages {
        let role = if msg.role == "user" {
            "User"
        } else {
            "Assistant"
        };
        history_text.push_str(&format!("{}: {}\n\n", role, msg.content.trim()));
    }

    // Build prompt after ctx is fully assembled
    let prompt = format!(
        "You are a research assistant helping the user discuss an academic paper. \
         Answer accurately based on the provided paper content. \
         Respond in the same language the user uses. \
         IMPORTANT: Do NOT read or use fulltext.txt under any circumstances. \
         Do NOT create, modify, or delete any files in this directory.\n\n\
         {ctx}\n\n\
         {history_text}"
    );

    // Run codex from the paper directory so it can access paper.pdf directly.
    // The prompt is passed via stdin because -i consumes positional arguments.
    let paper_dir = crate::paper::paper_dir(&root, &slug);

    let mut cmd = tokio::process::Command::new("codex");
    cmd.args([
        "exec",
        "--json",
        "--ephemeral",
        "--skip-git-repo-check",
        "--color",
        "never",
    ]);
    if attach_pdf {
        let pdf_path = crate::metadata::find_pdf_in_dir(&root, &slug);
        if pdf_path.exists() {
            cmd.args(["-i", "paper.pdf"]);
        }
    }
    cmd.arg("-"); // read prompt from stdin
    cmd.current_dir(&paper_dir);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to run codex: {e}"))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(prompt.as_bytes()).await;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for codex: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut response = String::new();

    for line in stdout.lines() {
        let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if json.get("type").and_then(|t| t.as_str()) == Some("item.completed") {
            if let Some(item) = json.get("item") {
                if item.get("type").and_then(|t| t.as_str()) == Some("agent_message") {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        response = text.to_string();
                    }
                }
            }
        }
    }

    if response.is_empty() {
        return Err(
            "Codex returned no response. Is codex installed and authenticated?".to_string(),
        );
    }

    // Optionally polish with a configured AI model before emitting
    let cli_settings = cli_manager::read_settings(&root);
    let polish = &cli_settings.polish;
    if polish.enabled && !polish.provider_id.is_empty() {
        if let Ok((provider, api_key, model)) = ai_manager::resolve_provider_model(
            &root,
            Some(&polish.provider_id),
            if polish.model_id.is_empty() { None } else { Some(&polish.model_id) },
        ) {
            let polish_messages = vec![
                ChatMessage { role: "system".into(), content: polish.prompt.clone() },
                ChatMessage { role: "user".into(), content: response.clone() },
            ];
            let _ = llm::chat_completion_stream(
                &provider, &api_key, &model, &polish_messages,
                &event_name, &app, false, None, "cli-polish",
            )
            .await;
            return Ok(response);
        }
    }

    // Emit raw response as a single event
    let _ = app.emit(
        &event_name,
        serde_json::json!({ "delta": &response, "done": false }),
    );
    let _ = app.emit(
        &event_name,
        serde_json::json!({ "delta": "", "done": true }),
    );
    Ok(response)
}

#[tauri::command]
pub async fn get_chat_history(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<ChatMessage>, String> {
    let root = get_root(&state)?;
    Ok(copilot::read_chat_history(&root, &slug))
}

#[tauri::command]
pub async fn save_chat_history(
    slug: String,
    messages: Vec<ChatMessage>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    copilot::write_chat_history(&root, &slug, &messages)
}

#[tauri::command]
pub async fn clear_chat_history(slug: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    copilot::clear_chat_history(&root, &slug)
}

#[tauri::command]
pub async fn get_paper_ai_conversations(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<serde_json::Value, String> {
    let root = get_root(&state)?;
    Ok(copilot::read_paper_ai_conversations(&root, &slug))
}

#[tauri::command]
pub async fn save_paper_ai_conversations(
    slug: String,
    conversations: serde_json::Value,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    copilot::write_paper_ai_conversations(&root, &slug, &conversations)
}

#[tauri::command]
pub async fn get_library_chat_history(
    state: State<'_, LibraryRoot>,
) -> Result<Vec<ChatMessage>, String> {
    let root = get_root(&state)?;
    Ok(copilot::read_library_chat_history(&root))
}

#[tauri::command]
pub async fn save_library_chat_history(
    messages: Vec<ChatMessage>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    copilot::write_library_chat_history(&root, &messages)
}

#[tauri::command]
pub async fn clear_library_chat_history(state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    copilot::clear_library_chat_history(&root)
}

// ── M6: CLI Tools configuration ───────────────────────────────────────────────

#[tauri::command]
pub async fn get_cli_settings(state: State<'_, LibraryRoot>) -> Result<CliSettings, String> {
    let root = get_root(&state)?;
    Ok(cli_manager::read_settings(&root))
}

#[tauri::command]
pub async fn save_cli_polish(
    polish: crate::models::CliOutputPolish,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let mut settings = cli_manager::read_settings(&root);
    settings.polish = polish;
    cli_manager::write_settings(&root, &settings)
}

#[tauri::command]
pub async fn detect_cli_tools(state: State<'_, LibraryRoot>) -> Result<Vec<CliTool>, String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    tauri::async_runtime::spawn_blocking(move || cli_manager::detect_tools(&root_c))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_cli_tool(tool: CliTool, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    cli_manager::save_tool(&root, tool)
}

#[tauri::command]
pub async fn delete_cli_tool(id: String, state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    cli_manager::delete_tool(&root, &id)
}

#[tauri::command]
pub async fn test_cli_tool(id: String, state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    cli_manager::test_tool(&root, &id).await
}

// ── M6: Prompt templates ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_cli_prompt_templates(
    state: State<'_, LibraryRoot>,
) -> Result<Vec<CliPromptTemplate>, String> {
    let root = get_root(&state)?;
    Ok(cli_manager::get_prompt_templates(&root))
}

#[tauri::command]
pub async fn save_cli_prompt_template(
    template: CliPromptTemplate,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    cli_manager::save_prompt_template(&root, template)
}

#[tauri::command]
pub async fn delete_cli_prompt_template(
    id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    cli_manager::delete_prompt_template(&root, &id)
}

// ── M6: Analysis execution ────────────────────────────────────────────────────

#[tauri::command]
pub async fn run_cli_analysis(
    slug: String,
    tool_id: String,
    prompt: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let settings = cli_manager::read_settings(&root);
    let tool = settings
        .tools
        .iter()
        .find(|t| t.id == tool_id)
        .ok_or_else(|| format!("CLI tool not found: {tool_id}"))?
        .clone();
    cli_runner::run(root, slug, tool, prompt, app).await
}

#[tauri::command]
pub async fn cancel_cli_analysis(run_id: String) -> Result<(), String> {
    cli_runner::cancel(&run_id)
}

// ── M6: Analysis results ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn save_cli_analysis_result(
    slug: String,
    name: String,
    content: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    cli_manager::save_analysis(&root, &slug, &name, &content)
}

#[tauri::command]
pub async fn list_cli_analyses(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<CliAnalysisEntry>, String> {
    let root = get_root(&state)?;
    cli_manager::list_analyses(&root, &slug)
}

#[tauri::command]
pub async fn get_cli_analysis(
    slug: String,
    filename: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    cli_manager::get_analysis(&root, &slug, &filename)
}

// ── M7: RAG Settings ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_rag_settings(state: State<'_, LibraryRoot>) -> Result<RagSettings, String> {
    let root = get_root(&state)?;
    Ok(rag::get_rag_settings(&root))
}

#[tauri::command]
pub async fn save_rag_settings(
    settings: RagSettings,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    rag::save_rag_settings(&root, &settings)
}

#[tauri::command]
pub async fn get_vector_store_info(
    state: State<'_, LibraryRoot>,
) -> Result<VectorStoreInfo, String> {
    let root = get_root(&state)?;
    rag::get_vector_store_info(&root).await
}

// ── M7: Vectorization ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn vectorize_paper(
    slug: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = get_root(&state)?;
    rag::vectorize_paper(&root, &slug, &app).await
}

#[tauri::command]
pub async fn rebuild_vector_store(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<usize, String> {
    let root = get_root(&state)?;
    rag::rebuild_vector_store(&root, &app).await
}

#[tauri::command]
pub async fn start_batch_vectorize(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = get_root(&state)?;
    rag::rebuild_vector_store(&root, &app).await.map(|_| ())
}

#[tauri::command]
pub async fn cancel_batch_vectorize() -> Result<(), String> {
    rag::cancel_batch_vectorize();
    Ok(())
}

#[tauri::command]
pub async fn get_paper_vectorize_input(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<crate::models::PaperVectorizeInput, String> {
    let root = get_root(&state)?;
    rag::get_paper_vectorize_input(&root, &slug)
}

#[tauri::command]
pub async fn embed_and_store_chunks(
    slug: String,
    paper_id: String,
    paper_title: String,
    chunks: Vec<crate::models::ChunkInput>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<usize, String> {
    let root = get_root(&state)?;
    rag::embed_and_store_chunks(&root, &slug, &paper_id, &paper_title, chunks, &app).await
}

// ── M7: Search ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn search_paper_chunks(
    slug: String,
    query: String,
    top_k: usize,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<RetrievedChunk>, String> {
    let root = get_root(&state)?;
    rag::search_paper_chunks(&root, &slug, &query, top_k).await
}

#[tauri::command]
pub async fn search_library_chunks(
    query: String,
    top_k: usize,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<RetrievedChunk>, String> {
    let root = get_root(&state)?;
    rag::search_library_chunks(&root, &query, top_k).await
}

// ── M7: Library chat ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn chat_with_library(
    messages: Vec<ChatMessage>,
    provider_id: Option<String>,
    model_id: Option<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    copilot::chat_with_library(
        &root,
        messages,
        provider_id.as_deref(),
        model_id.as_deref(),
        &app,
    )
    .await
}

#[tauri::command]
pub async fn open_paper_ai_window(
    slug: Option<String>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    copilot::open_paper_ai_window(&app, slug.as_deref())
}

#[tauri::command]
pub async fn open_library_chat_window(app: tauri::AppHandle) -> Result<(), String> {
    copilot::open_library_chat_window(&app)
}

#[tauri::command]
pub async fn save_chat_window_size(
    width: f64,
    height: f64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    copilot::save_library_chat_window_size(&app, width, height);
    Ok(())
}

#[tauri::command]
pub async fn save_paper_ai_window_size(
    width: f64,
    height: f64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    copilot::save_paper_ai_window_size(&app, width, height);
    Ok(())
}

// ── M8: arXiv window ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn open_arxiv_window(app: tauri::AppHandle) -> Result<(), String> {
    arxiv::open_arxiv_window(&app)
}

#[tauri::command]
pub async fn save_arxiv_window_size(
    width: f64,
    height: f64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    arxiv::save_arxiv_window_size(&app, width, height);
    Ok(())
}

// ── M8: arXiv config ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_arxiv_config(state: State<'_, LibraryRoot>) -> Result<ArxivConfig, String> {
    let root = get_root(&state)?;
    Ok(arxiv::get_arxiv_config(&root))
}

#[tauri::command]
pub async fn save_arxiv_config(
    config: ArxivConfig,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    arxiv::save_arxiv_config(&root, &config)
}

#[tauri::command]
pub async fn set_arxiv_auto_fetch(
    enabled: bool,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let mut config = arxiv::get_arxiv_config(&root);
    config.auto_fetch_enabled = enabled;
    arxiv::save_arxiv_config(&root, &config)?;
    arxiv_scheduler::set_enabled(enabled);
    Ok(())
}

#[tauri::command]
pub async fn get_arxiv_schedule_status(
    state: State<'_, LibraryRoot>,
) -> Result<ArxivScheduleStatus, String> {
    let root = get_root(&state)?;
    Ok(arxiv::get_schedule_status(&root))
}

// ── M8: arXiv fetch (HTTP in TypeScript via plugin-http, Rust only stores) ───

#[tauri::command]
pub async fn store_arxiv_papers(
    papers: Vec<ArxivPaper>,
    update_last_fetch: bool,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<Vec<ArxivPaper>, String> {
    let root = get_root(&state)?;
    let inbox = arxiv::merge_into_inbox(&root, papers)?;
    if update_last_fetch {
        let mut config = arxiv::get_arxiv_config(&root);
        config.last_fetch_date = Some(chrono::Local::now().format("%Y-%m-%d").to_string());
        let _ = arxiv::save_arxiv_config(&root, &config);
    }
    let _ = app.emit(
        "arxiv-new-recommendations",
        serde_json::json!({ "count": inbox.papers.iter().filter(|p| !p.in_library).count() }),
    );
    Ok(inbox.papers)
}

#[tauri::command]
pub async fn get_arxiv_inbox(state: State<'_, LibraryRoot>) -> Result<ArxivInbox, String> {
    let root = get_root(&state)?;

    // Auto-prune papers below relevance threshold every time the inbox is opened.
    let cfg = arxiv::get_arxiv_config(&root);
    if cfg.ai_filter_enabled {
        let _ = arxiv::prune_low_relevance(&root);
    }

    let mut inbox = arxiv::get_inbox(&root);
    // Refresh in_library flags on every read
    for p in inbox.papers.iter_mut() {
        p.in_library = {
            let papers_dir = std::path::Path::new(&root).join("papers");
            let mut found = false;
            if let Ok(entries) = std::fs::read_dir(&papers_dir) {
                for entry in entries.flatten() {
                    let meta_path = entry.path().join("meta.json");
                    if let Ok(text) = std::fs::read_to_string(&meta_path) {
                        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) {
                            if meta.get("arxiv_id").and_then(|v| v.as_str()) == Some(&p.arxiv_id) {
                                found = true;
                                break;
                            }
                        }
                    }
                }
            }
            found
        };
    }
    Ok(inbox)
}

#[tauri::command]
pub async fn refresh_arxiv_inbox(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<ArxivInbox, String> {
    let root = get_root(&state)?;
    let inbox = arxiv::prune_low_relevance(&root)?;
    let _ = app.emit(
        "arxiv-new-recommendations",
        serde_json::json!({ "count": inbox.papers.iter().filter(|p| !p.in_library).count() }),
    );
    Ok(inbox)
}

// ── M8: arXiv read status & rating ───────────────────────────────────────────

#[tauri::command]
pub async fn mark_arxiv_paper_read(
    arxiv_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    arxiv::mark_paper_read(&root, &arxiv_id)
}

#[tauri::command]
pub async fn rate_arxiv_paper(
    arxiv_id: String,
    rating: u8,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    arxiv::rate_paper(&root, &arxiv_id, rating)
}

// ── M8: arXiv AI analysis ────────────────────────────────────────────────────

#[tauri::command]
pub async fn analyze_arxiv_paper(
    arxiv_id: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    let app_c = app.clone();
    tokio::spawn(async move {
        if let Err(e) = arxiv::analyze_single(&root_c, &arxiv_id, &app_c).await {
            eprintln!("Single analysis error: {}", e);
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn start_arxiv_analysis(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let root_c = root.clone();
    let app_c = app.clone();
    tokio::spawn(async move {
        if let Err(e) = arxiv::start_analysis(&root_c, &app_c).await {
            eprintln!("Analysis error: {}", e);
            let _ = app_c.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": 0, "total": 0, "arxiv_id": "", "status": "error",
                    "message": e
                }),
            );
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn cancel_arxiv_analysis() -> Result<(), String> {
    arxiv::cancel_analysis();
    Ok(())
}

// ── M8: Add to library ───────────────────────────────────────────────────────

#[tauri::command]
pub async fn add_arxiv_to_library(
    arxiv_id: String,
    collection_id: Option<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    arxiv::add_to_library(&root, &arxiv_id, collection_id.as_deref(), &app).await
}

// ── URL-based arXiv import ────────────────────────────────────────────────────

/// Generic URL import: auto-detects arXiv or OpenReview by URL pattern.
#[tauri::command]
pub async fn import_paper_url(
    url: String,
    collection_id: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    url_import::import_by_url(&root, &url, &collection_id, &app).await
}

// Keep old command for backward compat
#[tauri::command]
pub async fn import_arxiv_url(
    url: String,
    collection_id: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    arxiv::import_by_url(&root, &url, &collection_id, &app).await
}

// ── Shell / Finder ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd")
        .args(["/c", "start", "", &url])
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&url)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn open_in_finder(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    std::process::Command::new("open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("explorer")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "linux")]
    std::process::Command::new("xdg-open")
        .arg(&path)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn copy_file_to_clipboard(path: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("set the clipboard to (POSIX file \"{}\")", escaped);
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "Failed to copy PDF file".to_string()
        } else {
            err
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("Copying files to clipboard is currently only supported on macOS".to_string())
    }
}

// ── M9: Canvas ────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn open_canvas_window(app: tauri::AppHandle) -> Result<(), String> {
    canvas::open_canvas_window(&app)
}

#[tauri::command]
pub async fn save_canvas_window_size(
    width: f64,
    height: f64,
    app: tauri::AppHandle,
) -> Result<(), String> {
    canvas::save_canvas_window_size(&app, width, height);
    Ok(())
}

#[tauri::command]
pub async fn list_canvases(state: State<'_, LibraryRoot>) -> Result<Vec<CanvasIndexEntry>, String> {
    let root = get_root(&state)?;
    canvas::list_canvases(&root)
}

#[tauri::command]
pub async fn create_canvas(state: State<'_, LibraryRoot>, name: String) -> Result<Canvas, String> {
    let root = get_root(&state)?;
    canvas::create_canvas(&root, name)
}

#[tauri::command]
pub async fn get_canvas(state: State<'_, LibraryRoot>, id: String) -> Result<Canvas, String> {
    let root = get_root(&state)?;
    canvas::get_canvas(&root, &id)
}

#[tauri::command]
pub async fn save_canvas(state: State<'_, LibraryRoot>, canvas_data: Canvas) -> Result<(), String> {
    let root = get_root(&state)?;
    canvas::save_canvas(&root, canvas_data)
}

#[tauri::command]
pub async fn rename_canvas(
    state: State<'_, LibraryRoot>,
    id: String,
    new_name: String,
) -> Result<(), String> {
    let root = get_root(&state)?;
    canvas::rename_canvas(&root, &id, new_name)
}

#[tauri::command]
pub async fn delete_canvas(state: State<'_, LibraryRoot>, id: String) -> Result<(), String> {
    let root = get_root(&state)?;
    canvas::delete_canvas(&root, &id)
}

#[tauri::command]
pub async fn get_node_display_content(
    state: State<'_, LibraryRoot>,
    paper_id: String,
    source: String,
) -> Result<String, String> {
    let root = get_root(&state)?;
    canvas::get_node_display_content(&root, &paper_id, &source)
}

#[tauri::command]
pub async fn set_canvas_notes(
    state: State<'_, LibraryRoot>,
    slug: String,
    note_ids: Vec<String>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let mut meta = paper::read_meta(&root, &slug)?;
    meta.canvas_notes = note_ids;
    paper::write_meta(&root, &slug, &meta)?;
    refresh_search_index(&root, &slug);
    Ok(())
}

#[tauri::command]
pub async fn get_canvas_settings(state: State<'_, LibraryRoot>) -> Result<CanvasSettings, String> {
    let root = get_root(&state)?;
    Ok(canvas::read_canvas_settings(&root))
}

#[tauri::command]
pub async fn save_canvas_settings(
    state: State<'_, LibraryRoot>,
    settings: CanvasSettings,
) -> Result<(), String> {
    let root = get_root(&state)?;
    canvas::write_canvas_settings(&root, &settings)
}

// ── M10: Canvas Enhance ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn suggest_canvas_edges(
    state: State<'_, LibraryRoot>,
    canvas_id: String,
    source: String,
    ai_provider_id: Option<String>,
    ai_model_id: Option<String>,
) -> Result<Vec<SuggestedEdge>, String> {
    let root = get_root(&state)?;
    canvas_enhance::suggest_canvas_edges(&root, &canvas_id, &source, ai_provider_id, ai_model_id)
        .await
}

#[tauri::command]
pub async fn compute_canvas_layout(
    state: State<'_, LibraryRoot>,
    canvas_id: String,
    layout: String,
    direction: String,
) -> Result<Vec<NodePosition>, String> {
    let root = get_root(&state)?;
    canvas_enhance::compute_canvas_layout(&root, &canvas_id, &layout, &direction)
}

#[tauri::command]
pub async fn export_canvas_image(
    format: String,
    image_data: String,
    save_path: String,
) -> Result<String, String> {
    canvas_enhance::export_canvas_image(&format, &image_data, &save_path)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn refresh_search_index(root: &str, slug: &str) {
    if let Err(e) = search::index_paper(root, slug) {
        eprintln!("[search] refresh {slug}: {e}");
    }
}

fn get_root(state: &State<'_, LibraryRoot>) -> Result<String, String> {
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "No library is currently open".to_string())
}

fn persist_library_path(app: &tauri::AppHandle, path: &str) {
    use tauri_plugin_store::StoreExt;
    if let Ok(store) = app.store("settings.json") {
        store.set("last_library", serde_json::json!(path));
        let _ = store.save();
    }
}

fn load_stored_library_path(app: &tauri::AppHandle) -> Option<String> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    store
        .get("last_library")
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

// ── Token usage ───────────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_token_usage(
    state: State<'_, LibraryRoot>,
) -> Result<Vec<crate::token_usage::UsageRecord>, String> {
    let root = get_root(&state)?;
    Ok(crate::token_usage::get_all(&root))
}

#[tauri::command]
pub fn clear_token_usage(state: State<'_, LibraryRoot>) -> Result<(), String> {
    let root = get_root(&state)?;
    crate::token_usage::clear(&root)
}
