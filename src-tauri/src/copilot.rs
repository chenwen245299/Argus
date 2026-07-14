use std::path::{Path, PathBuf};

use tauri::Emitter;

use crate::models::{ChatContent, ChatContentPart, ChatMessage, FileData, PaperMeta, RetrievedChunk};
use crate::{ai_manager, ai_summary, extraction, llm, paper, rag};

// ── Chat history persistence ──────────────────────────────────────────────────

pub fn read_chat_history(root: &str, slug: &str) -> Vec<ChatMessage> {
    let path = paper::paper_dir(root, slug).join("chat.json");
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn write_chat_history(root: &str, slug: &str, messages: &[ChatMessage]) -> Result<(), String> {
    let path = paper::paper_dir(root, slug).join("chat.json");
    let content = serde_json::to_string_pretty(messages)
        .map_err(|e| format!("Serialize chat history: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| format!("Write chat.json: {e}"))
}

pub fn clear_chat_history(root: &str, slug: &str) -> Result<(), String> {
    let path = paper::paper_dir(root, slug).join("chat.json");
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete chat.json: {e}"))
    } else {
        Ok(())
    }
}

fn paper_ai_conversations_path(root: &str, slug: &str) -> PathBuf {
    paper::paper_dir(root, slug).join("ai_conversations.json")
}

pub fn read_paper_ai_conversations(root: &str, slug: &str) -> serde_json::Value {
    let path = paper_ai_conversations_path(root, slug);
    if !path.exists() {
        return serde_json::json!([]);
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .filter(|v: &serde_json::Value| v.is_array())
        .unwrap_or_else(|| serde_json::json!([]))
}

pub fn write_paper_ai_conversations(
    root: &str,
    slug: &str,
    conversations: &serde_json::Value,
) -> Result<(), String> {
    if !conversations.is_array() {
        return Err("Paper AI conversations must be an array.".to_string());
    }
    let path = paper_ai_conversations_path(root, slug);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create paper dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(conversations)
        .map_err(|e| format!("Serialize paper AI conversations: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write ai_conversations.json: {e}"))
}

fn library_chat_history_path(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("library_chat.json")
}

pub fn read_library_chat_history(root: &str) -> Vec<ChatMessage> {
    let path = library_chat_history_path(root);
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn write_library_chat_history(root: &str, messages: &[ChatMessage]) -> Result<(), String> {
    let path = library_chat_history_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create .argus dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(messages)
        .map_err(|e| format!("Serialize library chat history: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write library_chat.json: {e}"))
}

pub fn clear_library_chat_history(root: &str) -> Result<(), String> {
    let path = library_chat_history_path(root);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete library_chat.json: {e}"))
    } else {
        Ok(())
    }
}

// Library-wide "智能问答" conversations (multi-conversation model with titles,
// selected papers, timestamps). Stored per-library under `.argus/` so switching
// libraries never bleeds conversation data between them. The frontend owns the
// shape, so we persist it as an opaque JSON array (same pattern as
// `ai_conversations.json` for per-paper chat).
fn library_conversations_path(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("library_chats.json")
}

pub fn read_library_conversations(root: &str) -> serde_json::Value {
    let path = library_conversations_path(root);
    if !path.exists() {
        return serde_json::json!([]);
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .filter(|v: &serde_json::Value| v.is_array())
        .unwrap_or_else(|| serde_json::json!([]))
}

pub fn write_library_conversations(
    root: &str,
    conversations: &serde_json::Value,
) -> Result<(), String> {
    if !conversations.is_array() {
        return Err("Library conversations must be an array.".to_string());
    }
    let path = library_conversations_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create .argus dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(conversations)
        .map_err(|e| format!("Serialize library conversations: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write library_chats.json: {e}"))
}

// ── Copilot chat ──────────────────────────────────────────────────────────────

/// Build the paper context for injection into the LLM system prompt.
/// M5: uses fulltext (truncated to fit model context).
/// M7 hook: replace `get_fulltext_context` with RAG retrieval.
pub async fn chat_with_paper(
    root: &str,
    slug: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let event_name = format!("ai-chat-{}", slug);
    chat_with_paper_on_event(
        root,
        slug,
        messages,
        provider_id,
        model_id,
        &event_name,
        app,
        false,
        None,
        "fulltext",
        false,
        &[],
        None,
    )
    .await
}

pub async fn chat_with_paper_on_event(
    root: &str,
    slug: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    context_mode: &str,
    use_pdf: bool,
    section_titles: &[String],
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let meta = paper::read_meta(root, slug).ok();

    let mut all_messages: Vec<ChatMessage> = Vec::new();
    // Tracks the actual content injected per section for the transparency banner.
    let mut sent_metadata = String::new();
    let mut sent_summary = String::new();
    let mut sent_fulltext = String::new();

    match context_mode {
        "none" => {
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: "You are a research assistant. Answer the user's questions clearly and concisely.".into(),
            });
        }
        "metadata" => {
            sent_metadata = build_metadata_string(meta.as_ref());
            let system = build_system_prompt(meta.as_ref(), "", false, false, None);
            all_messages.push(ChatMessage { role: "system".to_string(), content: system.into() });
        }
        "summary" => {
            let summary = ai_summary::read_summary(root, slug);
            let summary_ctx = if summary.trim().is_empty() { None } else { Some(summary.as_str()) };
            sent_summary = summary.clone();
            // meta=None: user did not select 元数据
            let system = build_system_prompt(None, "", false, false, summary_ctx);
            all_messages.push(ChatMessage { role: "system".to_string(), content: system.into() });
        }
        "summary+fulltext" => {
            let summary = ai_summary::read_summary(root, slug);
            let (context, truncated) = get_fulltext_context(root, slug, &provider, &model);
            sent_summary = summary.clone();
            sent_fulltext = if truncated {
                format!("{context}\n\n[内容因上下文长度限制已截断]")
            } else {
                context.clone()
            };
            // Leading, cache-stable paper block (identical across tasks/turns),
            // then task-specific persona and any extra context after it.
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: build_paper_context_block(meta.as_ref(), &context).into(),
            });
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: PAPER_ASSISTANT_PERSONA.into(),
            });
            if !summary.trim().is_empty() {
                all_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: format!("--- AI SUMMARY ---\n{}", summary.trim()).into(),
                });
            }
            if truncated {
                all_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: "[Content truncated due to length. This is a partial view of the paper.]".into(),
                });
            }
        }
        _ => {
            // "fulltext"
            let (context, truncated) = get_fulltext_context(root, slug, &provider, &model);
            sent_fulltext = if truncated {
                format!("{context}\n\n[内容因上下文长度限制已截断]")
            } else {
                context.clone()
            };
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: build_paper_context_block(meta.as_ref(), &context).into(),
            });
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: PAPER_ASSISTANT_PERSONA.into(),
            });
            if truncated {
                all_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: "[Content truncated due to length. This is a partial view of the paper.]".into(),
                });
            }
        }
    }

    // User-selected chapters — injected as an extra context block on top of
    // whatever context_mode already added (metadata / summary / fulltext).
    let sent_sections = crate::sections::extract_selected_sections_text(root, slug, section_titles);
    if !sent_sections.is_empty() {
        all_messages.push(ChatMessage {
            role: "system".to_string(),
            content: format!(
                "以下是用户选定的论文章节内容，请优先依据这些内容回答：\n\n{sent_sections}"
            )
            .into(),
        });
    }

    // Emit the actually-sent context so the frontend can display it transparently.
    let _ = app.emit(
        &format!("{event_name}-context"),
        serde_json::json!({
            "metadata": sent_metadata,
            "summary":  sent_summary,
            "fulltext": sent_fulltext,
            "sections": sent_sections,
        }),
    );

    all_messages.extend_from_slice(&messages);

    // Only OpenRouter supports inline PDF `file` content parts. Other providers
    // fall back to the text context already injected into the system prompt.
    let use_pdf = use_pdf && provider_supports_inline_pdf(&provider);

    if use_pdf {
        let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
        // Strip the system-prompt fulltext context (already not injected for openrouter+pdf),
        // but keep metadata/summary context messages that were added above.
        return llm::chat_completion_stream_with_pdf(
            &provider,
            &api_key,
            &model,
            &all_messages,
            event_name,
            app,
            use_reasoning,
            reasoning_effort,
            "copilot",
            &pdf_path,
            cancel,
        )
        .await;
    }

    llm::chat_completion_stream(
        &provider,
        &api_key,
        &model,
        &all_messages,
        event_name,
        app,
        use_reasoning,
        reasoning_effort,
        "copilot",
        cancel,
    )
    .await
}

// ── Library chat ──────────────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize)]
struct LibrarySentContextSection {
    kind: String,
    label: String,
    content: String,
}

#[derive(Clone, serde::Serialize)]
struct LibrarySentContextPayload {
    mode: String,
    sections: Vec<LibrarySentContextSection>,
}

fn provider_supports_inline_pdf(provider: &crate::models::AiProvider) -> bool {
    // Only OpenRouter reliably supports OpenAI-compatible inline `file`
    // content parts for PDFs.
    provider.kind == "openrouter" || provider.base_url.to_lowercase().contains("openrouter")
}

pub async fn chat_with_library(
    root: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    event_name: &str,
    sources_event_name: &str,
    knowledge_source: Option<&str>,
    selected_paper_slugs: Option<&[String]>,
    attachments: Option<&[ChatContentPart]>,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    app: &tauri::AppHandle,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    use tauri::Emitter;

    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let use_snippets = knowledge_source.map_or(false, |s| s == "snippets");
    let use_selected_papers = knowledge_source.map_or(false, |s| s == "papers");

    let system;

    let mut pdf_attachments: Vec<ChatContentPart> = Vec::new();

    if use_selected_papers {
        let slugs = selected_paper_slugs.unwrap_or(&[]);
        let use_inline_pdf = provider_supports_inline_pdf(&provider);
        // Only attach raw PDFs for providers that accept inline `file` content
        // parts (OpenRouter). For everyone else (DeepSeek, etc.) we fall back to
        // injecting the extracted fulltext as text via
        // `build_selected_papers_system_prompt` below — attaching the PDF here
        // would send an unsupported `file` part and trigger an API 400.
        if use_inline_pdf {
            pdf_attachments = slugs
                .iter()
                .filter_map(|slug| {
                    let slug = slug.trim();
                    if slug.is_empty() {
                        return None;
                    }
                    let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
                    encode_pdf_attachment(&pdf_path)
                })
                .collect();
        }
        let use_pdf = use_inline_pdf && !pdf_attachments.is_empty();
        let (selected_system, selected_sources, selected_contexts) = if use_pdf {
            build_selected_papers_pdf_system_prompt(root, slugs)
        } else {
            build_selected_papers_system_prompt(root, slugs, &provider, &model)
        };
        let _ = app.emit(sources_event_name, selected_sources);
        let context_event_name = format!("{event_name}-context");
        let _ = app.emit(
            context_event_name.as_str(),
            LibrarySentContextPayload {
                mode: "papers".to_string(),
                sections: selected_contexts,
            },
        );
        system = selected_system;
    } else if use_snippets {
        let query_text = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .and_then(|m| match &m.content {
                ChatContent::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let settings = rag::get_rag_settings(root);
        let retrieved = if settings.is_configured() && !query_text.is_empty() {
            match rag::embed_query(root, &query_text, &settings).await {
                Ok(vec) => rag::search_snippet_chunks_with_vec(root, vec, 12).await.unwrap_or_default(),
                Err(_) => vec![],
            }
        } else {
            vec![]
        };

        // Surface the retrieved snippets as sources, reusing the library-chat
        // `RetrievedChunk` shape so the same UI renders them. A snippet's
        // `paper_id` stores the source paper's slug, so it doubles as `slug`
        // for click-to-open.
        let snippet_sources: Vec<crate::models::RetrievedChunk> = retrieved
            .iter()
            .map(|s| crate::models::RetrievedChunk {
                chunk_id: s.snippet_id.clone(),
                paper_id: s.paper_id.clone(),
                slug: s.paper_id.clone(),
                chunk_index: 0,
                text: s.text.clone(),
                score: s.score,
                paper_title: s.paper_title.clone(),
                source_type: "snippet".to_string(),
                source_id: Some(s.snippet_id.clone()),
                source_label: None,
            })
            .collect();
        let _ = app.emit(sources_event_name, snippet_sources);
        system = build_snippet_system_prompt(&retrieved);
    } else {
        let settings = rag::get_rag_settings(root);
        let rag_chunks = if settings.is_configured() {
            let query_text = messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .and_then(|m| match &m.content {
                    ChatContent::Text(s) => Some(s.clone()),
                    _ => None,
                });
            if let Some(q) = query_text {
                if let Ok(vec) = rag::embed_query(root, &q, &settings).await {
                    rag::search_library_chunks_with_vec(root, vec, settings.top_k * 2)
                        .await
                        .ok()
                        .filter(|v| !v.is_empty())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let _ = app.emit(
            sources_event_name,
            rag_chunks.as_deref().unwrap_or(&[]).to_vec(),
        );
        system = build_library_system_prompt(rag_chunks.as_deref());
    }

    let mut messages = messages;
    let mut extra_parts: Vec<ChatContentPart> = Vec::new();
    extra_parts.extend(pdf_attachments);
    if let Some(attachments) = attachments {
        extra_parts.extend(attachments.iter().cloned());
    }
    if !extra_parts.is_empty() {
        if let Some(idx) = messages.iter().rposition(|m| m.role == "user") {
            let mut parts: Vec<ChatContentPart> = match &messages[idx].content {
                ChatContent::Text(t) => {
                    vec![ChatContentPart::Text { text: t.clone() }]
                }
                ChatContent::Parts(p) => p.clone(),
            };
            parts.extend(extra_parts);
            messages[idx].content = ChatContent::Parts(parts);
        }
    }

    let mut all_messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system.into(),
    }];
    all_messages.extend_from_slice(&messages);

    llm::chat_completion_stream(
        &provider,
        &api_key,
        &model,
        &all_messages,
        event_name,
        app,
        use_reasoning,
        reasoning_effort,
        "library_chat",
        cancel,
    )
    .await
}

fn build_snippet_system_prompt(snippets: &[crate::models::RetrievedSnippet]) -> String {
    let mut prompt = String::from(
        "You are a research assistant helping the user explore their snippet library — \
         a personal collection of text excerpts saved from academic papers.\n\
         Rules:\n\
         1. Answer ONLY from the snippets provided below — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. When citing a snippet, reference the source paper title and page:\n\
            《论文标题》第 N 页\n\
         4. If multiple snippets are relevant, synthesize them.\n\n",
    );

    if snippets.is_empty() {
        prompt.push_str("[未找到相关素材。请先在「素材库」中嵌入素材（设置 → RAG 配置向量化）。]\n");
        return prompt;
    }

    prompt.push_str("--- 检索到的相关素材 ---\n\n");
    for (i, s) in snippets.iter().enumerate() {
        let tags = if s.tags.is_empty() {
            String::new()
        } else {
            format!(" | 标签: {}", s.tags.join(", "))
        };
        let note = if s.note.trim().is_empty() {
            String::new()
        } else {
            format!(" | 笔记: {}", s.note.trim())
        };
        prompt.push_str(&format!(
            "[素材 {n} | 来源: 《{title}》第 {page} 页{tags}{note}]\n{text}\n\n",
            n = i + 1,
            title = s.paper_title,
            page = s.page,
            text = s.text,
        ));
    }
    prompt
}

// ── Library chat window ──────────────────────────────────────────────────────

const WINDOW_SIZE_STORE_KEY: &str = "library_chat_window_size";
const PAPER_AI_WINDOW_SIZE_STORE_KEY: &str = "paper_ai_window_size_v3";
const DEFAULT_WINDOW_W: f64 = 760.0;
const DEFAULT_WINDOW_H: f64 = 560.0;
const LIBRARY_CHAT_MIN_WINDOW_W: f64 = 560.0;
const LIBRARY_CHAT_MIN_WINDOW_H: f64 = 400.0;
const PAPER_AI_DEFAULT_WINDOW_W: f64 = 540.0;
const PAPER_AI_DEFAULT_WINDOW_H: f64 = 660.0;
const PAPER_AI_MIN_WINDOW_W: f64 = 400.0;
const PAPER_AI_MIN_WINDOW_H: f64 = 500.0;

fn load_library_chat_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let v = store.get(WINDOW_SIZE_STORE_KEY)?;
    let w = v.get("w")?.as_f64()?;
    let h = v.get("h")?.as_f64()?;
    // Sanity check: ignore absurdly small/large values
    if w >= LIBRARY_CHAT_MIN_WINDOW_W
        && h >= LIBRARY_CHAT_MIN_WINDOW_H
        && w <= 4000.0
        && h <= 3000.0
    {
        Some((w, h))
    } else {
        None
    }
}

pub fn save_library_chat_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < LIBRARY_CHAT_MIN_WINDOW_W || height < LIBRARY_CHAT_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

fn load_paper_ai_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let v = store.get(PAPER_AI_WINDOW_SIZE_STORE_KEY)?;
    let w = v.get("w")?.as_f64()?;
    let h = v.get("h")?.as_f64()?;
    if w >= PAPER_AI_MIN_WINDOW_W && h >= PAPER_AI_MIN_WINDOW_H && w <= 4000.0 && h <= 3000.0 {
        Some((w, h))
    } else {
        None
    }
}

pub fn save_paper_ai_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < PAPER_AI_MIN_WINDOW_W || height < PAPER_AI_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            PAPER_AI_WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

pub fn open_paper_ai_window(app: &tauri::AppHandle, slug: Option<&str>) -> Result<(), String> {
    use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

    if let Some(win) = app.get_webview_window("paper-ai") {
        let _ = win.set_focus();
        if let Some(slug) = slug {
            let _ = win.emit("paper-ai-slug", slug.to_string());
        }
        return Ok(());
    }

    let (width, height) = load_paper_ai_window_size(app)
        .unwrap_or((PAPER_AI_DEFAULT_WINDOW_W, PAPER_AI_DEFAULT_WINDOW_H));

    let builder = WebviewWindowBuilder::new(
        app,
        "paper-ai",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 论文 AI")
    .inner_size(width, height)
    .min_inner_size(PAPER_AI_MIN_WINDOW_W, PAPER_AI_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    let win = builder
        .build()
        .map_err(|e| format!("Open paper AI window: {e}"))?;

    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_paper_ai_window_size(
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
    let slug = slug.map(|s| s.to_string());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = win_c.unmaximize();
        let _ = win_c.set_size(tauri::LogicalSize::new(width, height));
        if let Some(slug) = slug {
            let _ = win_c.emit("paper-ai-slug", slug);
        }
    });

    Ok(())
}

pub fn open_library_chat_window(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

    if let Some(win) = app.get_webview_window("library-chat") {
        let _ = win.set_focus();
        return Ok(());
    }

    let (width, height) =
        load_library_chat_window_size(app).unwrap_or((DEFAULT_WINDOW_W, DEFAULT_WINDOW_H));

    let builder = WebviewWindowBuilder::new(
        app,
        "library-chat",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 智能问答")
    .inner_size(width, height)
    .min_inner_size(LIBRARY_CHAT_MIN_WINDOW_W, LIBRARY_CHAT_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition { x: 14.0, y: 22.0 });

    // Windows has no overlay titlebar; drop the native decorations so the window
    // uses our custom in-app titlebar (WindowControls) instead of showing an
    // extra native title row above it — matching the main window.
    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false);

    let win = builder
        .build()
        .map_err(|e| format!("Open library chat window: {e}"))?;

    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_library_chat_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
                }
            }
        };
        match event {
            WindowEvent::Resized(_) | WindowEvent::CloseRequested { .. } => {
                save(&win_ref);
            }
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

// ── Context building ──────────────────────────────────────────────────────────

fn build_metadata_string(meta: Option<&crate::models::PaperMeta>) -> String {
    let Some(m) = meta else { return String::new() };
    let mut s = format!("标题：{}\n", m.title);
    if !m.authors.is_empty() {
        s.push_str(&format!("作者：{}\n", m.authors.join(", ")));
    }
    if let Some(y) = m.year { s.push_str(&format!("年份：{y}\n")); }
    if let Some(ref v) = m.venue { s.push_str(&format!("期刊/会议：{v}\n")); }
    if let Some(ref a) = m.paper_abstract {
        if !a.trim().is_empty() { s.push_str(&format!("摘要：{}\n", a.trim())); }
    }
    s
}

/// Shared budget (in chars) for how much paper fulltext to hand `model_id`.
/// Used by BOTH the chat and the summary/analysis paths so the truncated text —
/// and therefore the leading context block — is byte-identical across tasks,
/// which is what lets the provider reuse the cached prompt prefix.
pub fn paper_context_budget(provider: &crate::models::AiProvider, model_id: &str) -> usize {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.context_length)
        .map(|cl| ((cl as usize * 7 / 10) * 4).min(300_000))
        .unwrap_or(60_000)
        // Reserve space for the instructions and conversation history (~10K chars).
        .saturating_sub(10_000)
}

/// Canonical, byte-stable "paper context" block. Every full-text paper task
/// (AI chat, AI summary/analysis) emits THIS as its first system message so the
/// large paper text sits at an identical prompt prefix and the provider can
/// serve it from cache instead of re-billing it on every call. Anything
/// task-specific (persona, instructions, the user's question) must come AFTER.
pub fn build_paper_context_block(meta: Option<&PaperMeta>, fulltext: &str) -> String {
    let mut s = String::from("--- PAPER CONTEXT ---\n");
    if let Some(m) = meta {
        s.push_str(&format!("Title: {}\n", m.title));
        if !m.authors.is_empty() {
            s.push_str(&format!("Authors: {}\n", m.authors.join(", ")));
        }
        if let Some(y) = m.year {
            s.push_str(&format!("Year: {y}\n"));
        }
        if let Some(ref v) = m.venue {
            s.push_str(&format!("Venue: {v}\n"));
        }
        if let Some(ref a) = m.paper_abstract {
            if !a.trim().is_empty() {
                s.push_str(&format!("Abstract: {}\n", a.trim()));
            }
        }
    }
    s.push_str("\n--- PAPER CONTENT ---\n");
    s.push_str(fulltext);
    s
}

/// Stable research-assistant persona for single-paper chat. Sent as a system
/// message AFTER the canonical context block (task-specific, so kept out of the
/// shared cacheable prefix).
pub const PAPER_ASSISTANT_PERSONA: &str = "You are a research assistant helping the user understand and analyze a specific paper. Answer questions clearly and accurately based on the paper content provided.";

fn get_fulltext_context(
    root: &str,
    slug: &str,
    provider: &crate::models::AiProvider,
    model_id: &str,
) -> (String, bool) {
    let fulltext = extraction::read_fulltext(root, slug);
    if fulltext.is_empty() {
        return (String::new(), false);
    }
    let max_chars = paper_context_budget(provider, model_id);
    let total = fulltext.chars().count();
    let truncated = total > max_chars;
    let context: String = fulltext.chars().take(max_chars).collect();
    (context, truncated)
}

fn build_library_system_prompt(chunks: Option<&[RetrievedChunk]>) -> String {
    let mut prompt = String::from(
        "You are a research assistant managing a personal academic library.\n\
         Rules:\n\
         1. Answer ONLY from the retrieved context below — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. For every key claim, cite the source paper using this format:\n\
            **论文标题** (`slug`) — 来源: 类型\n\
            Example: **Attention Is All You Need** (`vaswani2017attention`) — 来源: PDF正文\n\
         4. When a metadata chunk directly answers the question (venue, authors, year), \
            lead with that information.\n\
         5. If multiple papers are relevant, list each separately with its citation.\n\n",
    );

    match chunks {
        Some(c) if !c.is_empty() => {
            prompt.push_str("--- 检索到的相关内容 ---\n\n");
            for (i, chunk) in c.iter().enumerate() {
                let type_label = match chunk.source_type.as_str() {
                    "metadata" => "元数据".to_string(),
                    "highlight" => chunk
                        .source_label
                        .clone()
                        .unwrap_or_else(|| "批注".to_string()),
                    "note" => chunk
                        .source_label
                        .clone()
                        .unwrap_or_else(|| "笔记".to_string()),
                    _ => "PDF正文".to_string(),
                };
                let paper_display = if chunk.paper_title.is_empty() {
                    format!("`{}`", chunk.slug)
                } else {
                    format!("**{}** (`{}`)", chunk.paper_title, chunk.slug)
                };
                prompt.push_str(&format!(
                    "[片段 {i_1} | 论文: {paper} | 类型: {src}]\n{text}\n\n",
                    i_1 = i + 1,
                    paper = paper_display,
                    src = type_label,
                    text = chunk.text,
                ));
            }
        }
        Some(_) => {
            prompt.push_str("[未找到相关内容。请先向量化文献库（设置 → RAG）。]\n");
        }
        None => {
            prompt.push_str("[RAG 未配置，将基于通用知识回答，无法引用具体文献。]\n");
        }
    }
    prompt
}

fn selected_papers_context_budget(
    provider: &crate::models::AiProvider,
    model_id: &str,
) -> usize {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.context_length)
        .map(|cl| ((cl as usize * 7 / 10) * 4).min(300_000))
        .unwrap_or(80_000)
        .saturating_sub(12_000)
}

fn take_chars(input: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return (String::new(), !input.is_empty());
    }
    let total = input.chars().count();
    if total <= max_chars {
        (input.to_string(), false)
    } else {
        (input.chars().take(max_chars).collect(), true)
    }
}

fn build_selected_papers_system_prompt(
    root: &str,
    slugs: &[String],
    provider: &crate::models::AiProvider,
    model_id: &str,
) -> (String, Vec<RetrievedChunk>, Vec<LibrarySentContextSection>) {
    let mut prompt = String::from(
        "You are a research assistant helping the user compare and analyze a selected set of academic papers.\n\
         Rules:\n\
         1. Answer ONLY from the selected papers provided below — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. For every key claim, cite the source paper using this format:\n\
            **论文标题** (`slug`)\n\
         4. If the selected papers do not contain enough evidence, say that clearly.\n\
         5. When multiple selected papers are relevant, synthesize them and distinguish their contributions.\n\n",
    );

    if slugs.is_empty() {
        prompt.push_str("[未选择文献。请先在「文献库」模式中添加要参与问答的论文。]\n");
        return (prompt, Vec::new(), Vec::new());
    }

    let mut seen = std::collections::HashSet::new();
    let selected: Vec<&str> = slugs
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert((*s).to_string()))
        .collect();

    if selected.is_empty() {
        prompt.push_str("[未选择文献。请先在「文献库」模式中添加要参与问答的论文。]\n");
        return (prompt, Vec::new(), Vec::new());
    }

    let mut sources = Vec::new();
    let mut contexts = Vec::new();
    let mut remaining_budget = selected_papers_context_budget(provider, model_id);
    let per_paper_budget = (remaining_budget / selected.len()).max(4_000);
    let mut found_count = 0usize;

    prompt.push_str("--- 用户选择的文献 ---\n\n");

    for slug in selected {
        if remaining_budget == 0 {
            break;
        }

        let Ok(meta) = paper::read_meta(root, slug) else {
            prompt.push_str(&format!("[未找到文献: `{slug}`]\n\n"));
            continue;
        };

        let metadata = build_metadata_string(Some(&meta));
        let fulltext = extraction::read_fulltext(root, slug);

        let reserved_for_meta = metadata.chars().count() + 700;
        let text_budget = per_paper_budget
            .min(remaining_budget)
            .saturating_sub(reserved_for_meta)
            .max(1_000)
            .min(remaining_budget);
        let (text_excerpt, text_truncated) = take_chars(&fulltext, text_budget);

        found_count += 1;
        let mut paper_context = format!(
            "[文献 {n}]\nSlug: `{slug}`\n{metadata}",
            n = found_count,
            slug = slug,
            metadata = metadata,
        );

        if !text_excerpt.trim().is_empty() {
            paper_context.push_str("\n全文内容:\n");
            paper_context.push_str(&text_excerpt);
            if text_truncated {
                paper_context.push_str("\n[该文全文已因上下文长度限制截断]");
            }
            paper_context.push('\n');
        } else {
            paper_context.push_str("\n[该文暂无全文文本。只能基于元数据回答。]\n");
        }
        paper_context.push('\n');
        prompt.push_str(&paper_context);
        contexts.push(LibrarySentContextSection {
            kind: "paper".to_string(),
            label: meta.title.clone(),
            content: paper_context,
        });

        let source_text = if !text_excerpt.trim().is_empty() {
            text_excerpt.chars().take(800).collect()
        } else {
            metadata.clone()
        };
        sources.push(RetrievedChunk {
            chunk_id: format!("selected-{slug}"),
            paper_id: meta.id.clone(),
            slug: slug.to_string(),
            chunk_index: found_count.saturating_sub(1) as u32,
            text: source_text,
            score: 1.0,
            paper_title: meta.title.clone(),
            source_type: "text".to_string(),
            source_id: None,
            source_label: Some("已选文献".to_string()),
        });

        let used = metadata.chars().count() + text_excerpt.chars().count() + 700;
        remaining_budget = remaining_budget.saturating_sub(used);
    }

    if found_count == 0 {
        prompt.push_str("[所选文献未找到。请重新添加文献。]\n");
    }

    (prompt, sources, contexts)
}

/// Build a minimal system prompt for providers that accept inline PDFs.
/// The actual paper content comes from the PDF file blocks injected into the
/// user message, so the prompt only needs to tell the model what to do.
fn build_selected_papers_pdf_system_prompt(
    root: &str,
    slugs: &[String],
) -> (String, Vec<RetrievedChunk>, Vec<LibrarySentContextSection>) {
    let mut prompt = String::from(
        "You are a research assistant helping the user compare and analyze a selected set of academic papers.\n\
         The full PDFs of the selected papers are attached to the user message.\n\
         Rules:\n\
         1. Answer ONLY from the attached PDFs — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. For every key claim, cite the source paper using this format:\n\
            **论文标题** (`slug`)\n\
         4. If the selected papers do not contain enough evidence, say that clearly.\n\
         5. When multiple selected papers are relevant, synthesize them and distinguish their contributions.\n\n",
    );

    if slugs.is_empty() {
        prompt.push_str("[未选择文献。请先在「文献库」模式中添加要参与问答的论文。]\n");
        return (prompt, Vec::new(), Vec::new());
    }

    let mut sources = Vec::new();
    let mut contexts = Vec::new();
    let mut found_count = 0usize;

    prompt.push_str("--- 用户选择的文献（PDF 已作为附件发送） ---\n\n");

    for slug in slugs {
        let slug = slug.trim();
        if slug.is_empty() {
            continue;
        }

        let Ok(meta) = paper::read_meta(root, slug) else {
            prompt.push_str(&format!("[未找到文献: `{slug}`]\n\n"));
            continue;
        };

        found_count += 1;
        let paper_line = format!("[文献 {n}] {title} (`{slug}`)\n", n = found_count, title = meta.title);
        prompt.push_str(&paper_line);

        let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
        let context_line = if pdf_path.exists() {
            format!("{paper_line}PDF 文件已直接发送给模型。\n")
        } else {
            format!("{paper_line}[未找到 PDF 文件]\n")
        };
        contexts.push(LibrarySentContextSection {
            kind: "paper".to_string(),
            label: meta.title.clone(),
            content: context_line,
        });

        sources.push(RetrievedChunk {
            chunk_id: format!("selected-{slug}"),
            paper_id: meta.id.clone(),
            slug: slug.to_string(),
            chunk_index: found_count.saturating_sub(1) as u32,
            text: meta.title.clone(),
            score: 1.0,
            paper_title: meta.title.clone(),
            source_type: "pdf".to_string(),
            source_id: None,
            source_label: Some("已选文献 PDF".to_string()),
        });
    }

    if found_count == 0 {
        prompt.push_str("[所选文献未找到。请重新添加文献。]\n");
    }

    (prompt, sources, contexts)
}

fn encode_pdf_attachment(pdf_path: &std::path::Path) -> Option<ChatContentPart> {
    use base64::Engine;
    if !pdf_path.exists() {
        return None;
    }
    let bytes = std::fs::read(pdf_path).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let filename = pdf_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("paper.pdf")
        .to_string();
    Some(ChatContentPart::File {
        file: FileData {
            filename,
            file_data: format!("data:application/pdf;base64,{b64}"),
        },
    })
}

fn build_system_prompt(
    meta: Option<&PaperMeta>,
    context: &str,
    truncated: bool,
    rag_used: bool,
    ai_summary: Option<&str>,
) -> String {
    let mut prompt = String::from(
        "You are a research assistant helping the user understand and analyze a specific paper. \
         Answer questions clearly and accurately based on the paper content provided.\n\n",
    );

    if let Some(m) = meta {
        prompt.push_str(&format!("Paper: {}\n", m.title));
        if !m.authors.is_empty() {
            prompt.push_str(&format!("Authors: {}\n", m.authors.join(", ")));
        }
        if let Some(y) = m.year {
            prompt.push_str(&format!("Year: {y}\n"));
        }
        if let Some(ref v) = m.venue {
            prompt.push_str(&format!("Venue: {v}\n"));
        }
        if let Some(ref a) = m.paper_abstract {
            if !a.trim().is_empty() {
                prompt.push_str(&format!("Abstract: {}\n", a.trim()));
            }
        }
    }

    // Inject AI-generated summary if provided
    if let Some(summary) = ai_summary {
        if !summary.trim().is_empty() {
            prompt.push_str("\n--- AI SUMMARY ---\n");
            prompt.push_str(summary.trim());
        }
    }

    if !context.is_empty() {
        if rag_used {
            prompt.push_str("\n--- RELEVANT EXCERPTS (RAG) ---\n");
        } else {
            prompt.push_str("\n--- PAPER CONTENT ---\n");
        }
        prompt.push_str(context);
        if truncated {
            prompt.push_str(
                "\n\n[Content truncated due to length. This is a partial view of the paper.]",
            );
        }
    } else if ai_summary.map(|s| !s.trim().is_empty()).unwrap_or(false) {
        // Have summary but no fulltext — that's fine, summary is sufficient context
    } else {
        prompt.push_str(
            "\n[Full text not available. I will answer based on metadata only. \
             For richer analysis, please extract the paper's fulltext first.]",
        );
    }

    prompt
}
