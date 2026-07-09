use tauri::{Emitter, Manager, State};

use crate::models::{
    AiModel, AiProviderInfo, AiProviderInput, AiSettingsInfo, AppSettings, ArxivConfig, ArxivInbox,
    ArxivPaper, ArxivScheduleStatus, Canvas, CanvasIndexEntry, CanvasSettings, ChatMessage,
    Collection, CollectionsFile, Highlight, LibraryConfig, NodePosition, Note, PaperIndexEntry,
    PaperMeta, PaperStatus,
    RagSettings, ReadingState, RetrievedChunk, SearchHit, SuggestedEdge, VectorStoreInfo,
};
use crate::LibraryRoot;
use crate::{
    ai_manager, ai_summary, arxiv, arxiv_scheduler, canvas,
    canvas_enhance, collections, copilot, ebook, extraction, library, llm, metadata, paper, rag,
    search, sections, security_bookmark, settings, snippets, url_import,
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

    // Clean up stale temp directories left by *interrupted* imports — but only
    // ones that never finished writing their meta.json. A completed import that
    // simply didn't get renamed (e.g. the app closed, or metadata fetch failed
    // before `rename_paper_folder` ran) still holds the user's PDF and notes, so
    // it must be preserved rather than deleted.
    let papers_dir = std::path::Path::new(&root).join("papers");
    if let Ok(entries) = std::fs::read_dir(&papers_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("importing_") && !entry.path().join("meta.json").exists() {
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
    security_bookmark::persist_library_bookmark(&app, &root);

    // Notify secondary windows (e.g. the "智能问答" library-chat window) that the
    // active library changed, so they can reload per-library data instead of
    // showing / persisting stale data against the wrong library.
    let _ = app.emit("library-changed", root.clone());

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
    let stored = load_stored_library_path(&app)
        .map(|path| security_bookmark::ensure_library_access(&app, &path));
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

/// Remove a tag from all papers in the library (global tag deletion).
#[tauri::command]
pub async fn delete_tag(
    tag: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let dirs = paper::list_paper_dirs(&root)?;
    for (slug, _) in dirs {
        if let Ok(mut meta) = paper::read_meta(&root, &slug) {
            let before = meta.tags.len();
            meta.tags.retain(|t| t != &tag);
            if meta.tags.len() != before {
                let _ = paper::write_meta(&root, &slug, &meta);
                refresh_search_index(&root, &slug);
            }
        }
    }
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
pub async fn get_canvas_note_titles(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Vec<String>, String> {
    let root = get_root(&state)?;
    let meta = paper::read_meta(&root, &slug)?;
    if meta.canvas_notes.is_empty() {
        return Ok(vec![]);
    }
    let pinned: std::collections::HashSet<&str> =
        meta.canvas_notes.iter().map(|s| s.as_str()).collect();
    let titles = paper::list_notes(&root, &slug)
        .into_iter()
        .filter(|n| pinned.contains(n.id.as_str()))
        .map(|n| n.title)
        .collect();
    Ok(titles)
}

#[tauri::command]
pub async fn get_canvas_note_titles_map(
    slugs: Vec<String>,
    state: State<'_, LibraryRoot>,
) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
    let root = get_root(&state)?;
    let mut out = std::collections::HashMap::new();

    for slug in slugs {
        let Ok(meta) = paper::read_meta(&root, &slug) else {
            continue;
        };
        if meta.canvas_notes.is_empty() {
            continue;
        }

        let pinned: std::collections::HashSet<&str> =
            meta.canvas_notes.iter().map(|s| s.as_str()).collect();
        let titles: Vec<String> = paper::list_notes(&root, &slug)
            .into_iter()
            .filter(|n| pinned.contains(n.id.as_str()))
            .map(|n| n.title)
            .collect();

        if !titles.is_empty() {
            out.insert(slug, titles);
        }
    }

    Ok(out)
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

/// Save fulltext extracted by pdfjs in the frontend (fallback when lopdf/pdftotext fail).
/// Only writes if text is non-trivially long and the paper exists.
#[tauri::command]
pub async fn save_pdfjs_fulltext(
    slug: String,
    text: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    if text.trim().is_empty() {
        return Ok(());
    }
    let dir = paper::paper_dir(&root, &slug);
    if !dir.exists() {
        return Err(format!("Paper not found: {slug}"));
    }
    crate::fsutil::atomic_write_str(&dir.join("fulltext.txt"), &text)
        .map_err(|e| format!("Write fulltext: {e}"))?;
    let _ = search::index_paper(&root, &slug);
    let mut status = paper::read_status_for(&root, &slug);
    status.text_extracted = true;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    paper::write_status(&root, &slug, &status)
}

#[tauri::command]
pub async fn save_fulltext(
    slug: String,
    text: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let dir = paper::paper_dir(&root, &slug);
    if !dir.exists() {
        return Err(format!("Paper not found: {slug}"));
    }

    crate::fsutil::atomic_write_str(&dir.join("fulltext.txt"), &text)
        .map_err(|e| format!("Write fulltext: {e}"))?;
    let _ = search::index_paper(&root, &slug);

    let mut status = paper::read_status_for(&root, &slug);
    status.text_extracted = !text.trim().is_empty();
    status.last_updated = chrono::Utc::now().to_rfc3339();
    paper::write_status(&root, &slug, &status)
}

// ── Sections (chapter structure) ─────────────────────────────────────────────

/// Read the stored section index for a paper (None if not yet computed).
#[tauri::command]
pub async fn get_sections(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<Option<sections::PaperSections>, String> {
    let root = get_root(&state)?;
    Ok(sections::read_sections(&root, &slug))
}

/// Persist a section index computed by the frontend (embedded outline / heuristics).
#[tauri::command]
pub async fn save_sections(
    slug: String,
    data: sections::PaperSections,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let dir = paper::paper_dir(&root, &slug);
    if !dir.exists() {
        return Err(format!("Paper not found: {slug}"));
    }
    sections::write_sections(&root, &slug, &data)
}

/// LLM fallback: segment the paper into sections when structural detection failed.
/// Manually triggered from the paper context menu — never runs automatically.
#[tauri::command]
pub async fn ai_split_sections(
    slug: String,
    provider_id: Option<String>,
    model_id: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<sections::PaperSections, String> {
    let root = get_root(&state)?;
    sections::ai_split_sections(&root, &slug, provider_id.as_deref(), model_id.as_deref()).await
}

/// OCR a single page image (base64-encoded JPEG).
/// Uses macOS Vision framework if available, otherwise falls back to the external `tesseract` binary.
/// Returns the recognized text, or an error string (e.g. "tesseract not installed").
#[tauri::command]
pub async fn ocr_page_base64(page_base64: String) -> Result<String, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&page_base64)
        .map_err(|e| format!("base64 decode: {e}"))?;
    tauri::async_runtime::spawn_blocking(move || crate::ocr::ocr_jpeg_bytes(&bytes))
        .await
        .map_err(|e| e.to_string())?
}

/// Translate a text snippet using the configured AI provider and translate prompt.
/// Replaces `{text}` in the prompt template with the supplied text.
#[tauri::command]
pub async fn translate_text(
    text: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let s = settings::read_settings(&root);

    let (provider, api_key, model) = ai_manager::resolve_provider_model(
        &root,
        s.translate_ai_provider_id.as_deref(),
        s.translate_ai_model_id.as_deref(),
    )?;

    let prompt = s.translate_ai_prompt.replace("{text}", &text);
    let messages = vec![crate::models::ChatMessage {
        role: "user".to_string(),
        content: prompt.into(),
    }];

    tauri::async_runtime::spawn(async move {
        crate::llm::chat_completion(&provider, &api_key, &model, &messages, "translate").await
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Streaming variant: emits `translate-stream-{event_id}` events with `{delta, done}`.
#[tauri::command]
pub async fn translate_text_stream(
    text: String,
    event_id: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let root = get_root(&state)?;
    let s = settings::read_settings(&root);

    let (provider, api_key, model) = ai_manager::resolve_provider_model(
        &root,
        s.translate_ai_provider_id.as_deref(),
        s.translate_ai_model_id.as_deref(),
    )?;

    let prompt = s.translate_ai_prompt.replace("{text}", &text);
    let messages = vec![crate::models::ChatMessage {
        role: "user".to_string(),
        content: prompt.into(),
    }];
    let event_name = format!("translate-stream-{}", event_id);

    tauri::async_runtime::spawn(async move {
        let _ = crate::llm::chat_completion_stream(
            &provider, &api_key, &model, &messages,
            &event_name, &app, false, None, "translate", None,
        )
        .await;
    });

    Ok(())
}

/// Generate a conversation title from the first user message and AI response.
#[tauri::command]
pub async fn generate_conversation_title(
    user_msg: String,
    ai_msg: String,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let s = settings::read_settings(&root);

    let (provider, api_key, model) = ai_manager::resolve_provider_model(
        &root,
        s.title_ai_provider_id.as_deref(),
        s.title_ai_model_id.as_deref(),
    )?;

    let prompt = s.title_ai_prompt
        .replace("{user_msg}", &user_msg)
        .replace("{ai_msg}", &ai_msg);
    let messages = vec![crate::models::ChatMessage {
        role: "user".to_string(),
        content: prompt.into(),
    }];

    let result = crate::llm::chat_completion(&provider, &api_key, &model, &messages, "title").await?;
    Ok(result.trim().to_string())
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
        import_source: Some("file".to_string()),
        cite_count: None,
        file_type: None,
    };
    paper::write_meta(&root, &temp_slug, &meta)?;

    // Create companion files (notes.md, highlights.json, .status.json)
    paper::ensure_paper_files(&root, &temp_slug);
    refresh_search_index(&root, &temp_slug);

    Ok(temp_slug)
}

// ── Ebook import / reading ────────────────────────────────────────────────────

/// Pick documents (PDF or ebooks) via the native file dialog.
/// `pick_pdf_files` is kept untouched for any PDF-only flows.
#[tauri::command]
pub async fn pick_import_files(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<Vec<tauri_plugin_dialog::FilePath>>>();
    app.dialog()
        .file()
        .add_filter(
            "PDF / Ebooks",
            &["pdf", "epub", "mobi", "azw3", "azw", "fb2", "txt", "zip"],
        )
        .pick_files(move |result| {
            let _ = tx.send(result);
        });
    match rx.await.map_err(|e| e.to_string())? {
        Some(paths) => Ok(paths.iter().map(|p| p.to_string()).collect()),
        None => Ok(vec![]),
    }
}

/// Import an ebook (EPUB / MOBI / AZW3 / FB2 / TXT) into the library.
///
/// Unlike `import_pdf` (which returns a temp slug and lets the frontend drive
/// metadata fetching / renaming / extraction), the whole ebook pipeline runs
/// here: books carry their own metadata, so there is no online lookup stage.
/// Returns the final slug.
#[tauri::command]
pub async fn import_ebook(
    source_path: String,
    collection_id: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    if let Some(cid) = collection_id.as_deref().filter(|c| !c.trim().is_empty()) {
        collections::ensure_collection_can_receive_papers(&root, cid)?;
    }

    let src = std::path::PathBuf::from(&source_path);
    let format = ebook::detect_format(&src)
        .ok_or_else(|| format!("Unsupported ebook format: {source_path}"))?;

    // Parse fully before creating anything — a broken/DRM'd book must not
    // leave a half-imported folder behind.
    let src_c = src.clone();
    let parsed =
        tauri::async_runtime::spawn_blocking(move || ebook::parse(&src_c, format))
            .await
            .map_err(|e| e.to_string())??;

    let id = uuid::Uuid::new_v4();
    let short_id: String = id.to_string().replace('-', "").chars().take(8).collect();
    let temp_slug = format!("importing_{}", short_id);

    let filename = src
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();
    let title = if parsed.meta.title.trim().is_empty() {
        src.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string()
    } else {
        parsed.meta.title.trim().to_string()
    };

    let papers_dir = std::path::Path::new(&root).join("papers");
    let paper_dir = papers_dir.join(&temp_slug);
    std::fs::create_dir_all(&paper_dir).map_err(|e| format!("Cannot create paper dir: {e}"))?;

    // Copy the book file, named after the sanitized title with its original extension.
    let lower_name = filename.to_ascii_lowercase();
    let ext = if lower_name.ends_with(".fb2.zip") {
        "fb2.zip".to_string()
    } else {
        src.extension()
            .and_then(|e| e.to_str())
            .unwrap_or(format)
            .to_ascii_lowercase()
    };
    let mut stem: String = metadata::sanitize_filename(&title).chars().take(150).collect();
    // Never collide with companion files (fulltext.txt / notes.md) or produce
    // a hidden dotfile from an all-symbols title.
    if stem.trim().is_empty() || stem.eq_ignore_ascii_case("fulltext") || stem.eq_ignore_ascii_case("notes") {
        stem = "book".to_string();
    }
    let book_filename = format!("{stem}.{ext}");
    let dest = paper_dir.join(&book_filename);
    let source_path_c = source_path.clone();
    tauri::async_runtime::spawn_blocking(move || std::fs::copy(&source_path_c, &dest))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("Cannot copy ebook: {e}"))?;

    // meta.json straight from the book's own metadata — no online lookup.
    let meta = PaperMeta {
        id: id.to_string(),
        title,
        authors: parsed.meta.authors.clone(),
        year: parsed.meta.year,
        doi: None,
        arxiv_id: None,
        venue: parsed.meta.publisher.clone(),
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(filename),
        reading_status: "unread".to_string(),
        paper_abstract: parsed.meta.description.clone(),
        bibtex: None,
        canvas_notes: vec![],
        import_source: Some("file".to_string()),
        cite_count: None,
        file_type: Some(format.to_string()),
    };
    paper::write_meta(&root, &temp_slug, &meta)?;
    paper::ensure_paper_files(&root, &temp_slug);

    // Chapter structure from the book's own TOC.
    if let Some(secs) = ebook::sections_from(&parsed) {
        let _ = sections::write_sections(&root, &temp_slug, &secs);
    }

    // Fulltext + status flag (search/RAG/AI all read fulltext.txt).
    let fulltext = ebook::fulltext_from(&parsed);
    if !fulltext.trim().is_empty() {
        let ft_path = paper::paper_dir(&root, &temp_slug).join("fulltext.txt");
        if crate::fsutil::atomic_write_str(&ft_path, &fulltext).is_ok() {
            let mut status = paper::read_status_for(&root, &temp_slug);
            status.text_extracted = true;
            status.last_updated = chrono::Utc::now().to_rfc3339();
            let _ = paper::write_status(&root, &temp_slug, &status);
        }
    }

    // Canonical folder name (title-based).
    let root_c = root.clone();
    let temp_c = temp_slug.clone();
    let final_slug =
        tauri::async_runtime::spawn_blocking(move || metadata::rename_folder(&root_c, &temp_c))
            .await
            .map_err(|e| e.to_string())?
            .unwrap_or_else(|_| temp_slug.clone());
    if final_slug != temp_slug {
        let _ = search::remove_paper(&root, &temp_slug);
    }
    refresh_search_index(&root, &final_slug);

    if let Some(cid) = collection_id.filter(|c| !c.trim().is_empty()) {
        collections::move_paper_to_collection(&root, &id.to_string(), &cid)?;
    }

    Ok(final_slug)
}

/// Book manifest for the viewer: metadata + TOC + chapter list (no HTML yet).
#[tauri::command]
pub async fn read_ebook_manifest(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<ebook::EbookManifest, String> {
    let root = get_root(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        ebook::get_parsed(&root, &slug).map(|p| p.manifest())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One chapter's sanitize-ready HTML (1-based index).
#[tauri::command]
pub async fn read_ebook_chapter(
    slug: String,
    index: u32,
    state: State<'_, LibraryRoot>,
) -> Result<String, String> {
    let root = get_root(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let parsed = ebook::get_parsed(&root, &slug)?;
        let i = (index as usize)
            .checked_sub(1)
            .ok_or_else(|| "Chapter index is 1-based".to_string())?;
        parsed
            .chapter_html
            .get(i)
            .cloned()
            .ok_or_else(|| format!("Chapter {index} out of range"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One image resource, base64-encoded. `href` is looked up in the parsed
/// book's resource map only — it never becomes a filesystem path.
#[tauri::command]
pub async fn read_ebook_resource(
    slug: String,
    href: String,
    state: State<'_, LibraryRoot>,
) -> Result<ebook::EbookResource, String> {
    let root = get_root(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let parsed = ebook::get_parsed(&root, &slug)?;
        ebook::read_resource(&parsed, &href)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Rebuild sections.json from the book's own TOC (the ebook counterpart of
/// the PDF outline/heuristic re-detect).
#[tauri::command]
pub async fn regen_ebook_sections(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<sections::PaperSections, String> {
    let root = get_root(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let parsed = ebook::get_parsed(&root, &slug)?;
        let secs = ebook::sections_from(&parsed)
            .ok_or_else(|| "No chapter structure found in this book".to_string())?;
        sections::write_sections(&root, &slug, &secs)?;
        Ok(secs)
    })
    .await
    .map_err(|e| e.to_string())?
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
    // Drop any cached parsed ebook so its Arc doesn't outlive the paper
    ebook::evict_from_cache(&slug);
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

/// Fetch only the citation count from Semantic Scholar and update meta.
#[tauri::command]
pub async fn fetch_citation_count(
    slug: String,
    state: State<'_, LibraryRoot>,
) -> Result<PaperMeta, String> {
    let root = get_root(&state)?;
    let count = metadata::fetch_citation_count(&root, &slug).await?;
    let mut meta = crate::paper::read_meta(&root, &slug)?;
    if let Some(n) = count {
        meta.cite_count = Some(n);
        crate::paper::write_meta(&root, &slug, &meta)?;
        refresh_search_index(&root, &slug);
    }
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
pub async fn move_collection(
    id: String,
    new_parent_id: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    collections::move_collection(&root, &id, new_parent_id)
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
    // Ollama is normally keyless (local); allow an empty key for it.
    let key = ai_manager::get_api_key(&root, &id)
        .or_else(|| (provider.kind == "ollama").then(String::new))
        .ok_or("No API key configured for this provider")?;
    llm::test_connection(provider, &key).await
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
    // Ollama is normally keyless (local); allow an empty key for it.
    let key = ai_manager::get_api_key(&root, &id)
        .or_else(|| (provider.kind == "ollama").then(String::new))
        .ok_or("No API key configured for this provider")?;
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
            content: "你是一名信息抽取助手。只能从给定原文中抽取 Abstract/摘要段落，禁止生成、改写、翻译或概括。".into(),
        },
        crate::models::ChatMessage {
            role: "user".to_string(),
            content: prompt.into(),
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
    section_titles: Option<Vec<String>>,
    request_id: Option<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    // Register a cancel flag; the guard unregisters it on every exit path.
    let (_cancel_guard, cancel) = crate::cancel::CancelGuard::new(request_id);
    let section_titles = section_titles.unwrap_or_default();
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
        &section_titles,
        cancel,
    )
    .await
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

#[tauri::command]
pub async fn get_library_conversations(
    state: State<'_, LibraryRoot>,
) -> Result<serde_json::Value, String> {
    let root = get_root(&state)?;
    Ok(copilot::read_library_conversations(&root))
}

#[tauri::command]
pub async fn save_library_conversations(
    conversations: serde_json::Value,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    copilot::write_library_conversations(&root, &conversations)
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

#[tauri::command]
pub async fn sync_vectorized_flags(
    state: State<'_, LibraryRoot>,
) -> Result<(usize, usize), String> {
    let root = get_root(&state)?;
    rag::sync_vectorized_flags(&root).await
}

/// Delete every vector stored under one embedding model. Returns rows removed.
#[tauri::command]
pub async fn delete_model_embeddings(
    model: String,
    state: State<'_, LibraryRoot>,
) -> Result<usize, String> {
    let root = get_root(&state)?;
    rag::delete_model_embeddings(&root, &model).await
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
    event_name: Option<String>,
    sources_event_name: Option<String>,
    knowledge_source: Option<String>,
    selected_paper_slugs: Option<Vec<String>>,
    attachments: Option<Vec<crate::models::ChatContentPart>>,
    use_reasoning: Option<bool>,
    reasoning_effort: Option<String>,
    request_id: Option<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let root = get_root(&state)?;
    let event_name = event_name.unwrap_or_else(|| "library-chat".to_string());
    let sources_event_name =
        sources_event_name.unwrap_or_else(|| "library-chat-sources".to_string());
    // Register a cancel flag; the guard unregisters it on every exit path.
    let (_cancel_guard, cancel) = crate::cancel::CancelGuard::new(request_id);
    copilot::chat_with_library(
        &root,
        messages,
        provider_id.as_deref(),
        model_id.as_deref(),
        &event_name,
        &sources_event_name,
        knowledge_source.as_deref(),
        selected_paper_slugs.as_deref(),
        attachments.as_deref(),
        use_reasoning.unwrap_or(false),
        reasoning_effort.as_deref(),
        &app,
        cancel,
    )
    .await
}

#[tauri::command]
pub async fn open_note_window(
    app: tauri::AppHandle,
    slug: String,
    note_id: String,
    title: String,
    window_label: Option<String>,
) -> Result<(), String> {
    use tauri::{Emitter, WebviewUrl, WebviewWindowBuilder, WindowEvent};

    let (width, height) =
        load_note_window_size(&app).unwrap_or((NOTE_DEFAULT_WINDOW_W, NOTE_DEFAULT_WINDOW_H));
    let window_label = window_label
        .filter(|label| is_valid_note_window_label(label))
        .unwrap_or_else(unique_note_window_label);

    let builder = WebviewWindowBuilder::new(
        &app,
        &window_label,
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title(&format!("{} — Argus 笔记", title))
    .inner_size(width, height)
    .min_inner_size(NOTE_MIN_WINDOW_W, NOTE_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    let win = builder
        .build()
        .map_err(|e| format!("Open note window: {e}"))?;

    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_note_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
                }
            }
        };
        match event {
            WindowEvent::Resized(_) | WindowEvent::CloseRequested { .. } => save(&win_ref),
            _ => {}
        }
    });

    let win_c = win.clone();
    let app_c = app.clone();
    let window_label_c = window_label.clone();
    let data_event = note_window_data_event(&window_label_c);
    let data = serde_json::json!({ "slug": slug, "noteId": note_id, "title": title });
    tauri::async_runtime::spawn(async move {
        // Wait for macOS frame restoration to settle, then re-apply the saved size.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = win_c.unmaximize();
        let _ = win_c.set_size(tauri::LogicalSize::new(width, height));
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let _ = app_c.emit_to(
            tauri::EventTarget::webview_window(window_label_c),
            &data_event,
            data,
        );
    });

    Ok(())
}

fn unique_note_window_label() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("note-window-{nanos}")
}

fn is_valid_note_window_label(label: &str) -> bool {
    label.starts_with("note-window-")
        && label.len() <= 128
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
}

fn note_window_data_event(window_label: &str) -> String {
    format!("note-window-data-{window_label}")
}

const NOTE_WINDOW_SIZE_STORE_KEY: &str = "note_window_size_v1";
const NOTE_DEFAULT_WINDOW_W: f64 = 680.0;
const NOTE_DEFAULT_WINDOW_H: f64 = 760.0;
const NOTE_MIN_WINDOW_W: f64 = 400.0;
const NOTE_MIN_WINDOW_H: f64 = 400.0;

fn load_note_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let value = store.get(NOTE_WINDOW_SIZE_STORE_KEY)?;
    let width = value.get("w")?.as_f64()?;
    let height = value.get("h")?.as_f64()?;
    if width >= NOTE_MIN_WINDOW_W
        && height >= NOTE_MIN_WINDOW_H
        && width <= 4000.0
        && height <= 3000.0
    {
        Some((width, height))
    } else {
        None
    }
}

fn save_note_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < NOTE_MIN_WINDOW_W || height < NOTE_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            NOTE_WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
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

// ── Embedding map window ──────────────────────────────────────────────────────

const EMBED_MAP_WINDOW_SIZE_KEY: &str = "embedding_map_window_size";
const EMBED_MAP_DEFAULT_W: f64 = 1080.0;
const EMBED_MAP_DEFAULT_H: f64 = 720.0;
const EMBED_MAP_MIN_W: f64 = 680.0;
const EMBED_MAP_MIN_H: f64 = 480.0;

fn load_embedding_map_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let v = store.get(EMBED_MAP_WINDOW_SIZE_KEY)?;
    let w = v.get("w")?.as_f64()?;
    let h = v.get("h")?.as_f64()?;
    if w >= EMBED_MAP_MIN_W && h >= EMBED_MAP_MIN_H && w <= 4000.0 && h <= 3000.0 {
        Some((w, h))
    } else {
        None
    }
}

fn save_embedding_map_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < EMBED_MAP_MIN_W || height < EMBED_MAP_MIN_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            EMBED_MAP_WINDOW_SIZE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

#[tauri::command]
pub async fn open_embedding_map_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::{WebviewUrl, WebviewWindowBuilder, WindowEvent};

    if let Some(win) = app.get_webview_window("embedding-map") {
        let _ = win.set_focus();
        return Ok(());
    }

    let (width, height) = load_embedding_map_window_size(&app)
        .unwrap_or((EMBED_MAP_DEFAULT_W, EMBED_MAP_DEFAULT_H));

    let builder = WebviewWindowBuilder::new(
        &app,
        "embedding-map",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 向量图谱")
    .inner_size(width, height)
    .min_inner_size(EMBED_MAP_MIN_W, EMBED_MAP_MIN_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition { x: 14.0, y: 22.0 });

    let win = builder
        .build()
        .map_err(|e| format!("Open embedding map window: {e}"))?;

    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_embedding_map_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
                }
            }
        };
        match event {
            WindowEvent::Resized(_) | WindowEvent::CloseRequested { .. } => save(&win_ref),
            _ => {}
        }
    });

    let win_c = win.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = win_c.unmaximize();
        let _ = win_c.set_size(tauri::LogicalSize::new(width, height));
    });

    Ok(())
}

#[tauri::command]
pub async fn get_embedding_map(
    model: Option<String>,
    state: State<'_, LibraryRoot>,
) -> Result<crate::models::EmbeddingMapData, String> {
    let root = get_root(&state)?;
    rag::get_embedding_map(&root, model).await
}

#[tauri::command]
pub async fn focus_main_window(app: tauri::AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "Main window not found".to_string())?;

    if let Some(chat) = app.get_webview_window("library-chat") {
        let _ = chat.set_always_on_top(false);
        let _ = chat.set_focusable(false);
    }

    #[cfg(target_os = "macos")]
    {
        let _ = app.show();
    }

    let _ = main.show();
    let _ = main.unminimize();
    let _ = main.set_always_on_top(true);
    let _ = main.set_focus();

    let app_for_retry = app.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(140)).await;
        if let Some(main) = app_for_retry.get_webview_window("main") {
            let _ = main.show();
            let _ = main.unminimize();
            let _ = main.set_focus();
        }
    });

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(700)).await;
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.set_focus();
            let _ = main.set_always_on_top(false);
            let _ = main.set_focus();
        }
        if let Some(chat) = app.get_webview_window("library-chat") {
            let _ = chat.set_focusable(true);
        }
    });

    Ok(())
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

// ── Snippet RAG ───────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_snippet_store_info(
    state: State<'_, LibraryRoot>,
) -> Result<crate::models::SnippetStoreInfo, String> {
    let root = get_root(&state)?;
    rag::get_snippet_store_info(&root).await
}

#[tauri::command]
pub async fn embed_all_snippets(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(usize, usize), String> {
    let root = get_root(&state)?;
    let snippets = rag::get_unembedded_snippets(&root)?;
    rag::embed_and_store_snippets(&root, snippets, &app).await
}

#[tauri::command]
pub async fn get_library_embedded_count(
    library_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<usize, String> {
    let root = get_root(&state)?;
    rag::get_library_embedded_count(&root, &library_id).await
}

#[tauri::command]
pub async fn embed_library_snippets(
    library_id: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(usize, usize), String> {
    let root = get_root(&state)?;
    // Reuse the store-wide "unembedded" scan, then keep only this library's.
    let snippets: Vec<_> = rag::get_unembedded_snippets(&root)?
        .into_iter()
        .filter(|s| s.library_id == library_id)
        .collect();
    rag::embed_and_store_snippets(&root, snippets, &app).await
}

#[tauri::command]
pub async fn embed_all_snippets_force(
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<(usize, usize), String> {
    let root = get_root(&state)?;
    // Collect all snippets regardless of embedding status
    let libs = snippets::list_snippet_libraries(&root)?;
    let mut all = vec![];
    for lib in &libs {
        all.extend(snippets::get_snippets(&root, &lib.id).unwrap_or_default());
    }
    rag::embed_and_store_snippets(&root, all, &app).await
}

#[tauri::command]
pub async fn delete_snippet_vector(
    snippet_id: String,
    state: State<'_, LibraryRoot>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    rag::delete_snippet_chunk(&root, &snippet_id).await
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

    let mut inbox = arxiv::get_inbox(&root);
    arxiv::mark_in_library_statuses(&root, &mut inbox.papers);
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

// ── M8: arXiv batch delete ────────────────────────────────────────────────────

#[tauri::command]
pub async fn delete_arxiv_inbox_by_date(
    date: String,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<ArxivInbox, String> {
    let root = get_root(&state)?;
    let inbox = arxiv::delete_inbox_by_date(&root, &date)?;
    let _ = app.emit(
        "arxiv-new-recommendations",
        serde_json::json!({ "count": inbox.papers.iter().filter(|p| !p.in_library).count() }),
    );
    Ok(inbox)
}

#[tauri::command]
pub async fn delete_arxiv_papers(
    arxiv_ids: Vec<String>,
    state: State<'_, LibraryRoot>,
    app: tauri::AppHandle,
) -> Result<ArxivInbox, String> {
    let root = get_root(&state)?;
    let inbox = arxiv::delete_inbox_papers(&root, &arxiv_ids)?;
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
            let _ = app_c.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": 0, "total": 0, "arxiv_id": arxiv_id,
                    "status": "error", "message": e
                }),
            );
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
                    "bulk": true,
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
    if let Some(cid) = collection_id.as_deref().filter(|c| !c.trim().is_empty()) {
        collections::ensure_collection_can_receive_papers(&root, cid)?;
    }
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
    collections::ensure_collection_can_receive_papers(&root, &collection_id)?;
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
    collections::ensure_collection_can_receive_papers(&root, &collection_id)?;
    arxiv::import_by_url(&root, &url, &collection_id, &app).await
}

// ── Shell / Finder ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let lower = url.trim().to_lowercase();
    if !lower.starts_with("https://") && !lower.starts_with("http://") {
        return Err(format!("Blocked: only http/https URLs are allowed (got: {url})"));
    }
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
        // Escape characters that could break out of the AppleScript string literal.
        // Newlines and CRs are especially important: they terminate the current
        // `osascript -e` statement and allow injection of a second command.
        let escaped = path
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\0', "");
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

/// Fetch available endpoints (providers) for an OpenRouter model.
/// Returns raw JSON from GET https://openrouter.ai/api/v1/models/{model_id}/endpoints
#[tauri::command]
pub async fn fetch_openrouter_endpoints(
    state: State<'_, LibraryRoot>,
    provider_id: String,
    model_id: String,
) -> Result<serde_json::Value, String> {
    let root = get_root(&state)?;
    let api_key = ai_manager::get_api_key(&root, &provider_id)
        .ok_or_else(|| "No API key configured for this provider".to_string())?;
    let url = format!(
        "https://openrouter.ai/api/v1/models/{}/endpoints",
        model_id.trim()
    );
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("Build client: {e}"))?;
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(format!("OpenRouter API error {status}: {text}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("Invalid JSON: {e}"))
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

// ── Library size ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_library_size(state: State<'_, LibraryRoot>) -> Result<u64, String> {
    let root = get_root(&state)?;
    fn dir_size(path: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(path) else { return 0 };
        entries.flatten().fold(0u64, |acc, entry| {
            let p = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { return acc };
            if meta.is_dir() { acc + dir_size(&p) } else { acc + meta.len() }
        })
    }
    Ok(dir_size(std::path::Path::new(&root)))
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

// ── Activity log ─────────────────────────────────────────────────────────────

fn activity_log_path(root: &str) -> std::path::PathBuf {
    std::path::Path::new(root).join(".argus").join("activity.json")
}

fn empty_activity_log() -> serde_json::Value {
    serde_json::json!({ "version": 1, "days": {} })
}

#[tauri::command]
pub fn get_activity_log(root: String) -> Result<serde_json::Value, String> {
    let path = activity_log_path(&root);
    if !path.exists() {
        return Ok(empty_activity_log());
    }
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("Read activity.json: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Parse activity.json: {e}"))
}

#[tauri::command]
pub fn save_activity_log(root: String, data: serde_json::Value) -> Result<(), String> {
    let argus_dir = std::path::Path::new(&root).join(".argus");
    std::fs::create_dir_all(&argus_dir).map_err(|e| format!("Create .argus: {e}"))?;
    let content =
        serde_json::to_string_pretty(&data).map_err(|e| format!("Serialize activity.json: {e}"))?;
    crate::fsutil::atomic_write_str(&argus_dir.join("activity.json"), &content)
        .map_err(|e| format!("Write activity.json: {e}"))
}

// ── Per-library UI state ─────────────────────────────────────────────────────

fn ui_state_path(root: &str) -> std::path::PathBuf {
    std::path::Path::new(root).join(".argus").join("ui_state.json")
}

#[tauri::command]
pub fn get_library_ui_state(root: String) -> Result<serde_json::Value, String> {
    let path = ui_state_path(&root);
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|e| format!("Read ui_state.json: {e}"))?;
    serde_json::from_str(&content).map_err(|e| format!("Parse ui_state.json: {e}"))
}

#[tauri::command]
pub fn save_library_ui_state(root: String, state: serde_json::Value) -> Result<(), String> {
    let argus_dir = std::path::Path::new(&root).join(".argus");
    std::fs::create_dir_all(&argus_dir).map_err(|e| format!("Create .argus: {e}"))?;
    let content =
        serde_json::to_string_pretty(&state).map_err(|e| format!("Serialize ui_state.json: {e}"))?;
    crate::fsutil::atomic_write_str(&argus_dir.join("ui_state.json"), &content)
        .map_err(|e| format!("Write ui_state.json: {e}"))
}

#[tauri::command]
pub fn patch_library_ui_state(root: String, patch: serde_json::Value) -> Result<(), String> {
    let mut state = get_library_ui_state(root.clone()).unwrap_or_else(|_| serde_json::json!({}));
    if !state.is_object() {
        state = serde_json::json!({});
    }
    if let (Some(target), Some(source)) = (state.as_object_mut(), patch.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    save_library_ui_state(root, state)
}

// ── Folder paths ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_canvases_folder_path(state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    let path = std::path::Path::new(&root).join("canvases");
    std::fs::create_dir_all(&path).map_err(|e| format!("Create canvases folder: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn get_snippets_folder_path(state: State<'_, LibraryRoot>) -> Result<String, String> {
    let root = get_root(&state)?;
    let path = std::path::Path::new(&root).join("snippets");
    std::fs::create_dir_all(&path).map_err(|e| format!("Create snippets folder: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

// ── Snippet Library ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn list_snippet_libraries(
    state: State<'_, LibraryRoot>,
) -> Result<Vec<crate::models::SnippetLibrary>, String> {
    let root = get_root(&state)?;
    snippets::list_snippet_libraries(&root)
}

#[tauri::command]
pub fn create_snippet_library(
    state: State<'_, LibraryRoot>,
    name: String,
    emoji: Option<String>,
) -> Result<crate::models::SnippetLibrary, String> {
    let root = get_root(&state)?;
    snippets::create_snippet_library(&root, name, emoji)
}

#[tauri::command]
pub fn rename_snippet_library(
    state: State<'_, LibraryRoot>,
    id: String,
    name: String,
) -> Result<(), String> {
    let root = get_root(&state)?;
    snippets::rename_snippet_library(&root, &id, name)
}

#[tauri::command]
pub fn update_snippet_library_emoji(
    state: State<'_, LibraryRoot>,
    id: String,
    emoji: Option<String>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    snippets::update_snippet_library_emoji(&root, &id, emoji)
}

#[tauri::command]
pub fn delete_snippet_library(
    state: State<'_, LibraryRoot>,
    id: String,
) -> Result<(), String> {
    let root = get_root(&state)?;
    snippets::delete_snippet_library(&root, &id)
}

#[tauri::command]
pub fn get_snippets(
    state: State<'_, LibraryRoot>,
    library_id: String,
) -> Result<Vec<crate::models::Snippet>, String> {
    let root = get_root(&state)?;
    snippets::get_snippets(&root, &library_id)
}

#[tauri::command]
pub fn add_snippet(
    state: State<'_, LibraryRoot>,
    input: snippets::AddSnippetInput,
) -> Result<crate::models::Snippet, String> {
    let root = get_root(&state)?;
    snippets::add_snippet(&root, input)
}

#[tauri::command]
pub fn update_snippet(
    state: State<'_, LibraryRoot>,
    library_id: String,
    id: String,
    tags: Option<Vec<String>>,
    note: Option<String>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    snippets::update_snippet(&root, &library_id, &id, tags, note)
}

#[tauri::command]
pub fn delete_snippet(
    state: State<'_, LibraryRoot>,
    library_id: String,
    id: String,
) -> Result<(), String> {
    let root = get_root(&state)?;
    snippets::delete_snippet(&root, &library_id, &id)
}

#[tauri::command]
pub async fn move_snippet(
    state: State<'_, LibraryRoot>,
    id: String,
    target_library_id: String,
) -> Result<crate::models::Snippet, String> {
    let root = get_root(&state)?;
    let snippet = snippets::move_snippet(&root, &id, &target_library_id)?;
    let _ = crate::rag::update_snippet_library_id(&root, &id, &target_library_id).await;
    Ok(snippet)
}

// ── File export ───────────────────────────────────────────────────────────────
/// Write bytes to a user-chosen export path.
///
/// This is a generic write primitive exposed to the webview, so it is restricted
/// to known export file types as defence-in-depth: even if some content-injection
/// bug let attacker-controlled script reach this command, it cannot be used to
/// drop an executable, shell profile, or config file at an arbitrary path.
#[tauri::command]
pub fn write_bytes_to_file(path: String, bytes: Vec<u8>) -> Result<(), String> {
    const ALLOWED_EXT: &[&str] = &[
        "pdf", "png", "jpg", "jpeg", "webp", "svg", "gif", "json", "csv", "md",
        "txt", "bib", "bibtex", "html",
    ];
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    match ext {
        Some(e) if ALLOWED_EXT.contains(&e.as_str()) => {}
        _ => return Err("Refused: unsupported export file type".to_string()),
    }
    // Refuse to follow a symlink at the destination: otherwise a pre-planted
    // link could redirect the write to overwrite a file outside the intended
    // export location.
    if let Ok(meta) = std::fs::symlink_metadata(&path) {
        if meta.file_type().is_symlink() {
            return Err("Refused: export target is a symlink".to_string());
        }
    }
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn migrate_snippets_from_localstorage(
    state: State<'_, LibraryRoot>,
    libraries: Vec<crate::models::SnippetLibrary>,
    snippets_by_library: Vec<(String, Vec<crate::models::Snippet>)>,
) -> Result<(), String> {
    let root = get_root(&state)?;
    snippets::migrate_from_localstorage(&root, libraries, snippets_by_library)
}
