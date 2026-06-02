use std::path::{Path, PathBuf};

use crate::models::{ChatMessage, PaperMeta, RetrievedChunk};
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
    std::fs::write(&path, content).map_err(|e| format!("Write chat.json: {e}"))
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
    std::fs::write(&path, content).map_err(|e| format!("Write ai_conversations.json: {e}"))
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
    std::fs::write(&path, content).map_err(|e| format!("Write library_chat.json: {e}"))
}

pub fn clear_library_chat_history(root: &str) -> Result<(), String> {
    let path = library_chat_history_path(root);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete library_chat.json: {e}"))
    } else {
        Ok(())
    }
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
) -> Result<String, String> {
    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let meta = paper::read_meta(root, slug).ok();

    let mut all_messages: Vec<ChatMessage> = Vec::new();

    match context_mode {
        "none" => {
            // No context injection — just a bare system message
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: "You are a research assistant. Answer the user's questions clearly and concisely.".to_string(),
            });
        }
        "metadata" => {
            // Only inject paper metadata (title, authors, year, venue, abstract)
            let system = build_system_prompt(meta.as_ref(), "", false, false, None);
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: system,
            });
        }
        "summary" => {
            // Only inject the AI-generated summary (no fulltext)
            let summary = ai_summary::read_summary(root, slug);
            let summary_ctx = if summary.trim().is_empty() {
                None
            } else {
                Some(summary.as_str())
            };
            let system = build_system_prompt(meta.as_ref(), "", false, false, summary_ctx);
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: system,
            });
        }
        "summary+fulltext" => {
            // Inject both the AI summary and the fulltext/RAG context
            let summary = ai_summary::read_summary(root, slug);
            let summary_ctx = if summary.trim().is_empty() {
                None
            } else {
                Some(summary.as_str())
            };
            let (context, truncated, rag_used) =
                build_rag_context(root, slug, &messages, &provider, &api_key, &model).await;
            let system =
                build_system_prompt(meta.as_ref(), &context, truncated, rag_used, summary_ctx);
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: system,
            });
        }
        _ => {
            // Default: "fulltext" — RAG or truncated fulltext
            let (context, truncated, rag_used) =
                build_rag_context(root, slug, &messages, &provider, &api_key, &model).await;
            let system = build_system_prompt(meta.as_ref(), &context, truncated, rag_used, None);
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: system,
            });
        }
    }

    all_messages.extend_from_slice(&messages);

    // OpenRouter with PDF toggle: send the PDF file directly.
    let use_pdf = use_pdf && provider.kind == "openrouter";

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
    )
    .await
}

// ── Library chat ──────────────────────────────────────────────────────────────

pub async fn chat_with_library(
    root: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    use tauri::Emitter;

    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let settings = rag::get_rag_settings(root);
    let rag_chunks = if settings.is_configured() {
        let query = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .map(|m| m.content.clone());
        if let Some(q) = query {
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

    // Emit retrieved chunks so the UI can display source citations.
    let _ = app.emit(
        "library-chat-sources",
        rag_chunks.as_deref().unwrap_or(&[]).to_vec(),
    );

    let system = build_library_system_prompt(rag_chunks.as_deref());

    let mut all_messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system,
    }];
    all_messages.extend_from_slice(&messages);

    llm::chat_completion_stream(
        &provider,
        &api_key,
        &model,
        &all_messages,
        "library-chat",
        app,
        false,
        None,
        "library_chat",
    )
    .await
}

// ── Library chat window ──────────────────────────────────────────────────────

const WINDOW_SIZE_STORE_KEY: &str = "library_chat_window_size";
const PAPER_AI_WINDOW_SIZE_STORE_KEY: &str = "paper_ai_window_size_v3";
const DEFAULT_WINDOW_W: f64 = 760.0;
const DEFAULT_WINDOW_H: f64 = 560.0;
const PAPER_AI_DEFAULT_WINDOW_W: f64 = 540.0;
const PAPER_AI_DEFAULT_WINDOW_H: f64 = 660.0;

fn load_library_chat_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let v = store.get(WINDOW_SIZE_STORE_KEY)?;
    let w = v.get("w")?.as_f64()?;
    let h = v.get("h")?.as_f64()?;
    // Sanity check: ignore absurdly small/large values
    if w >= 400.0 && h >= 300.0 && w <= 4000.0 && h <= 3000.0 {
        Some((w, h))
    } else {
        None
    }
}

pub fn save_library_chat_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
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
    if w >= 400.0 && h >= 500.0 && w <= 4000.0 && h <= 3000.0 {
        Some((w, h))
    } else {
        None
    }
}

pub fn save_paper_ai_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if let Ok(store) = app.store("settings.json") {
        store.set(
            PAPER_AI_WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

pub fn open_paper_ai_window(app: &tauri::AppHandle, slug: Option<&str>) -> Result<(), String> {
    use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};

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
    .min_inner_size(400.0, 500.0);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    let win = builder
        .build()
        .map_err(|e| format!("Open paper AI window: {e}"))?;

    if let Some(slug) = slug {
        let _ = win.emit("paper-ai-slug", slug.to_string());
    }

    Ok(())
}

pub fn open_library_chat_window(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    if let Some(win) = app.get_webview_window("library-chat") {
        let _ = win.set_focus();
        return Ok(());
    }

    let (width, height) =
        load_library_chat_window_size(app).unwrap_or((DEFAULT_WINDOW_W, DEFAULT_WINDOW_H));

    WebviewWindowBuilder::new(
        app,
        "library-chat",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 智能问答")
    .inner_size(width, height)
    .min_inner_size(560.0, 400.0)
    .build()
    .map_err(|e| format!("Open library chat window: {e}"))?;

    Ok(())
}

// ── Context building ──────────────────────────────────────────────────────────

/// Try RAG first; fall back to fulltext if paper is not vectorized or RAG fails.
async fn build_rag_context(
    root: &str,
    slug: &str,
    messages: &[ChatMessage],
    provider: &crate::models::AiProvider,
    _api_key: &str,
    model_id: &str,
) -> (String, bool, bool) {
    let status = paper::read_status_for(root, slug);
    if !status.vectorized {
        let (ctx, trunc) = get_fulltext_context(root, slug, provider, model_id);
        return (ctx, trunc, false);
    }

    let settings = rag::get_rag_settings(root);
    if !settings.is_configured() {
        let (ctx, trunc) = get_fulltext_context(root, slug, provider, model_id);
        return (ctx, trunc, false);
    }

    let query = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.clone());
    let query = match query {
        Some(q) if !q.is_empty() => q,
        _ => {
            let (ctx, trunc) = get_fulltext_context(root, slug, provider, model_id);
            return (ctx, trunc, false);
        }
    };

    let query_vec = match rag::embed_query(root, &query, &settings).await {
        Ok(v) => v,
        Err(_) => {
            let (ctx, trunc) = get_fulltext_context(root, slug, provider, model_id);
            return (ctx, trunc, false);
        }
    };

    let chunks =
        match rag::search_paper_chunks_with_vec(root, slug, query_vec, settings.top_k).await {
            Ok(c) if !c.is_empty() => c,
            _ => {
                let (ctx, trunc) = get_fulltext_context(root, slug, provider, model_id);
                return (ctx, trunc, false);
            }
        };

    let context = chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let label = match c.source_type.as_str() {
                "metadata" => "元数据".to_string(),
                "highlight" => c.source_label.clone().unwrap_or_else(|| "批注".to_string()),
                "note" => c.source_label.clone().unwrap_or_else(|| "笔记".to_string()),
                _ => "PDF正文".to_string(),
            };
            format!("[片段 {} | 类型: {}]\n{}", i + 1, label, c.text)
        })
        .collect::<Vec<_>>()
        .join("\n\n");

    (context, false, true)
}

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

    // Reserve space for the system prompt template and conversation history (~10K chars).
    let max_chars = provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.context_length)
        .map(|cl| ((cl as usize * 7 / 10) * 4).min(300_000))
        .unwrap_or(60_000)
        .saturating_sub(10_000);

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
