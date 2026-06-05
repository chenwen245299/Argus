mod ai_manager;
mod ai_summary;
mod arxiv;
mod arxiv_scheduler;
mod canvas;
mod canvas_enhance;
mod collections;
mod commands;
mod copilot;
mod extraction;
mod library;
mod llm;
mod metadata;
mod models;
mod ocr;
mod paper;
mod path_guard;
mod rag;
mod search;
mod security_bookmark;
mod settings;
mod snippets;
mod token_usage;
mod url_import;

use std::sync::Mutex;
use tauri::Manager;
use tauri_plugin_window_state::StateFlags;

pub struct LibraryRoot(pub Mutex<Option<String>>);

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::SIZE | StateFlags::POSITION | StateFlags::MAXIMIZED)
                .skip_initial_state("paper-ai")
                .skip_initial_state("arxiv")
                .skip_initial_state("library-chat")
                .skip_initial_state("note-window")
                .build(),
        )
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(LibraryRoot(Mutex::new(None)))
        .setup(|app| {
            // Restore last-opened library path into in-memory state.
            use tauri_plugin_store::StoreExt;
            if let Ok(store) = app.store("settings.json") {
                if let Some(path) = store
                    .get("last_library")
                    .and_then(|v| v.as_str().map(|s| s.to_string()))
                {
                    let path = security_bookmark::ensure_library_access(app.handle(), &path);
                    let state: tauri::State<LibraryRoot> = app.state();
                    let mut guard = state.0.lock().unwrap();
                    *guard = Some(path.clone());
                    token_usage::set_root(&path);
                }
            }

            // Start background scheduler
            arxiv_scheduler::start_scheduler(app.handle().clone());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // ── M0-M3: Library management ──
            commands::pick_library_folder,
            commands::open_library,
            commands::get_current_library,
            commands::load_library_cache,
            commands::scan_library,
            commands::list_papers,
            // ── M0-M3: Single paper ──
            commands::get_paper_meta,
            commands::save_paper_meta,
            commands::get_notes,
            commands::save_notes,
            commands::list_notes,
            commands::get_canvas_note_titles,
            commands::get_note,
            commands::create_note,
            commands::save_note,
            commands::rename_note,
            commands::delete_note,
            commands::get_highlights,
            commands::save_highlights,
            commands::get_reading_state,
            commands::update_reading_state,
            commands::get_paper_status,
            commands::save_pdfjs_fulltext,
            commands::save_fulltext,
            commands::ocr_page_base64,
            commands::translate_text,
            commands::translate_text_stream,
            commands::generate_conversation_title,
            commands::set_reading_status,
            commands::delete_paper,
            commands::copy_paper_pdf,
            commands::open_paper_folder,
            commands::read_pdf_bytes,
            // ── M0-M3: Import + metadata ──
            commands::pick_pdf_files,
            commands::import_pdf,
            commands::fetch_metadata,
            commands::extract_metadata_ai,
            commands::extract_first_page_text,
            commands::rename_paper_folder,
            // ── M4: Settings ──
            commands::get_settings,
            commands::save_settings,
            // ── M4: Collections ──
            commands::get_collections,
            commands::create_collection,
            commands::rename_collection,
            commands::move_collection,
            commands::set_collection_emoji,
            commands::delete_collection,
            commands::add_paper_to_collection,
            commands::move_paper_to_collection,
            commands::remove_paper_from_collection,
            commands::list_papers_in_collection,
            commands::get_papers_folder_path,
            commands::get_collection_folder_path,
            // ── M4: Extraction ──
            commands::extract_fulltext,
            commands::get_fulltext,
            commands::batch_extract_fulltext,
            // ── M4: Search ──
            commands::index_paper_search,
            commands::rebuild_search_index,
            commands::search_fulltext,
            // ── M5: AI Service Center ──
            commands::get_ai_settings,
            commands::add_ai_provider,
            commands::update_ai_provider,
            commands::delete_ai_provider,
            commands::set_provider_enabled,
            commands::test_ai_provider,
            commands::fetch_provider_models,
            commands::save_provider_models,
            commands::set_default_model,
            // ── M5: AI Summary ──
            commands::generate_summary,
            commands::get_summary,
            commands::extract_abstract_ai,
            // ── M5: Copilot ──
            commands::chat_with_paper,
            commands::chat_with_paper_event,
            commands::get_chat_history,
            commands::save_chat_history,
            commands::clear_chat_history,
            commands::get_paper_ai_conversations,
            commands::save_paper_ai_conversations,
            commands::get_library_chat_history,
            commands::save_library_chat_history,
            commands::clear_library_chat_history,
            // ── M7: RAG Settings ──
            commands::get_rag_settings,
            commands::save_rag_settings,
            commands::get_vector_store_info,
            commands::sync_vectorized_flags,
            // ── M7: Vectorization ──
            commands::vectorize_paper,
            commands::rebuild_vector_store,
            commands::start_batch_vectorize,
            commands::cancel_batch_vectorize,
            commands::get_paper_vectorize_input,
            commands::embed_and_store_chunks,
            // ── M7: Search ──
            commands::search_paper_chunks,
            commands::search_library_chunks,
            // ── M7: Library chat ──
            commands::chat_with_library,
            commands::open_note_window,
            commands::open_paper_ai_window,
            commands::open_library_chat_window,
            commands::focus_main_window,
            commands::save_chat_window_size,
            commands::save_paper_ai_window_size,
            // ── M8: arXiv window ──
            commands::open_arxiv_window,
            commands::save_arxiv_window_size,
            // ── M8: arXiv config ──
            commands::get_arxiv_config,
            commands::save_arxiv_config,
            commands::set_arxiv_auto_fetch,
            commands::get_arxiv_schedule_status,
            // ── M8: arXiv fetch ──
            commands::store_arxiv_papers,
            commands::get_arxiv_inbox,
            commands::refresh_arxiv_inbox,
            // ── M8: read status & rating ──
            commands::mark_arxiv_paper_read,
            commands::rate_arxiv_paper,
            // ── M8: AI analysis ──
            commands::analyze_arxiv_paper,
            commands::start_arxiv_analysis,
            commands::cancel_arxiv_analysis,
            // ── M8: Add to library ──
            commands::add_arxiv_to_library,
            commands::import_arxiv_url,
            commands::import_paper_url,
            // ── M9: Canvas window ──
            commands::open_canvas_window,
            commands::save_canvas_window_size,
            // ── M9: Multi-canvas management ──
            commands::list_canvases,
            commands::create_canvas,
            commands::get_canvas,
            commands::save_canvas,
            commands::rename_canvas,
            commands::delete_canvas,
            // ── M9: Node hover content ──
            commands::get_node_display_content,
            // ── M9: Canvas settings ──
            commands::get_canvas_settings,
            commands::save_canvas_settings,
            commands::set_canvas_notes,
            commands::fetch_openrouter_endpoints,
            // ── M10: Canvas Enhance ──
            commands::suggest_canvas_edges,
            commands::compute_canvas_layout,
            commands::export_canvas_image,
            // ── Shell ──
            commands::open_url,
            commands::open_in_finder,
            // ── Folder paths ──
            commands::get_canvases_folder_path,
            commands::get_snippets_folder_path,
            // ── Library size ──
            commands::get_library_size,
            // ── Token usage ──
            commands::get_token_usage,
            commands::clear_token_usage,
            // ── Snippet Library ──
            commands::list_snippet_libraries,
            commands::create_snippet_library,
            commands::rename_snippet_library,
            commands::update_snippet_library_emoji,
            commands::delete_snippet_library,
            commands::get_snippets,
            commands::add_snippet,
            commands::update_snippet,
            commands::delete_snippet,
            commands::migrate_snippets_from_localstorage,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
