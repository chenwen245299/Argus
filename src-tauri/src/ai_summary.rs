use crate::models::{ChatMessage, Note, PaperMeta};
use crate::{ai_manager, extraction, llm, paper, search, settings};
use tauri::Emitter;

// Reserve ~8K chars for system prompt, meta, and conversation overhead.
const CONTEXT_OVERHEAD: usize = 8_000;

pub fn read_summary(root: &str, slug: &str) -> String {
    paper::get_note_by_title(root, slug, "AI总结").unwrap_or_else(|| {
        let legacy = paper::paper_dir(root, slug).join("ai_summary.md");
        std::fs::read_to_string(&legacy).unwrap_or_default()
    })
}

pub fn has_summary(root: &str, slug: &str) -> bool {
    !read_summary(root, slug).trim().is_empty()
}

pub async fn generate_summary(
    root: &str,
    slug: &str,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    app: &tauri::AppHandle,
) -> Result<Note, String> {
    let app_settings = settings::read_settings(root);
    emit_progress(app, slug, "fulltext", serde_json::json!({}));

    let root_for_text = root.to_string();
    let slug_for_text = slug.to_string();
    let settings_for_text = app_settings.clone();
    let app_for_text = app.clone();
    let fulltext = tokio::task::spawn_blocking(move || {
        ensure_fulltext(
            &root_for_text,
            &slug_for_text,
            &settings_for_text,
            &app_for_text,
        )
    })
    .await
    .map_err(|e| format!("Text extraction task failed: {e}"))??;

    if fulltext.trim().is_empty() {
        return Err("No fulltext available after text extraction.".to_string());
    }

    let fulltext_chars = fulltext.chars().count();
    emit_progress(
        app,
        slug,
        "preparing",
        serde_json::json!({ "fulltext_chars": fulltext_chars }),
    );

    let meta = paper::read_meta(root, slug).unwrap_or_else(|_| default_meta());

    let configured_provider_id = provider_id.or(app_settings.ai_summary_provider_id.as_deref());
    let configured_model_id = model_id.or(app_settings.ai_summary_model_id.as_deref());
    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, configured_provider_id, configured_model_id)?;

    let max_chars = context_budget(&provider, &model);
    let truncated = fulltext_chars > max_chars;
    let context: String = fulltext.chars().take(max_chars).collect();
    let context_chars = context.chars().count();

    let system = "你是一名严谨的研究助理。请只输出 Markdown 正文，不要输出代码块包裹。总结必须忠于论文内容，信息不足时明确说明。".to_string();
    let mut user = render_prompt(&app_settings.ai_summary_prompt, &meta, &context);
    if truncated {
        user.push_str("\n\n> 注：由于模型上下文限制，本次摘要基于截断后的论文全文生成。");
    }

    let event_name = format!("ai-summary-{}", slug_to_event_id(slug));
    emit_progress(
        app,
        slug,
        "ai",
        serde_json::json!({
            "provider": &provider.name,
            "model": &model,
            "fulltext_chars": fulltext_chars,
            "context_chars": context_chars,
            "truncated": truncated,
        }),
    );

    let summary_text = llm::chat_completion_stream(
        &provider,
        &api_key,
        &model,
        &[
            ChatMessage {
                role: "system".to_string(),
                content: system.into(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: user.into(),
            },
        ],
        &event_name,
        app,
        false,
        None,
        "summary",
        None,
    )
    .await?;

    emit_progress(app, slug, "saving", serde_json::json!({}));
    let note = paper::upsert_note_by_title(root, slug, "AI总结", &summary_text)?;
    let _ = search::index_paper(root, slug);

    let mut status = paper::read_status_for(root, slug);
    status.ai_summary_done = true;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    paper::write_status(root, slug, &status)?;

    emit_progress(app, slug, "done", serde_json::json!({}));
    Ok(note)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn ensure_fulltext(
    root: &str,
    slug: &str,
    app_settings: &crate::models::AppSettings,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let existing = extraction::read_fulltext(root, slug);
    if !existing.trim().is_empty() {
        emit_progress(
            app,
            slug,
            "fulltext_ready",
            serde_json::json!({
                "source": "cache",
                "fulltext_chars": existing.chars().count(),
            }),
        );
        return Ok(existing);
    }

    emit_progress(app, slug, "text", serde_json::json!({}));
    match extraction::extract_and_write(root, slug, app_settings) {
        extraction::ExtractionResult::Text => {
            let fulltext = extraction::read_fulltext(root, slug);
            let _ = search::index_paper(root, slug);
            emit_progress(
                app,
                slug,
                "fulltext_ready",
                serde_json::json!({
                    "source": "text",
                    "fulltext_chars": fulltext.chars().count(),
                }),
            );
            return Ok(fulltext);
        }
        extraction::ExtractionResult::NoText => {
            return Err("PDF has no extractable embedded text.".to_string());
        }
        extraction::ExtractionResult::Failed(e) => return Err(e),
    }
}

fn emit_progress(app: &tauri::AppHandle, slug: &str, stage: &str, extra: serde_json::Value) {
    let mut payload = serde_json::Map::new();
    payload.insert("slug".to_string(), serde_json::json!(slug));
    payload.insert("stage".to_string(), serde_json::json!(stage));
    if let Some(extra) = extra.as_object() {
        for (key, value) in extra {
            payload.insert(key.clone(), value.clone());
        }
    }
    let _ = app.emit("ai-summary-progress", serde_json::Value::Object(payload));
}

fn render_prompt(template: &str, meta: &PaperMeta, fulltext: &str) -> String {
    let authors = if meta.authors.is_empty() {
        "未知".to_string()
    } else {
        meta.authors.join(", ")
    };
    let year = meta
        .year
        .map(|y| y.to_string())
        .unwrap_or_else(|| "未知".to_string());
    let venue = meta.venue.clone().unwrap_or_else(|| "未知".to_string());
    let doi = meta.doi.clone().unwrap_or_else(|| "无".to_string());
    let arxiv_id = meta.arxiv_id.clone().unwrap_or_else(|| "无".to_string());

    let had_fulltext_placeholder = template.trim().is_empty() || template.contains("{fulltext}");
    let mut prompt = if template.trim().is_empty() {
        crate::models::default_ai_summary_prompt()
    } else {
        template.to_string()
    };

    let replacements = [
        ("{title}", meta.title.as_str()),
        ("{authors}", authors.as_str()),
        ("{year}", year.as_str()),
        ("{venue}", venue.as_str()),
        ("{doi}", doi.as_str()),
        ("{arxiv_id}", arxiv_id.as_str()),
        ("{abstract}", ""),
        ("{fulltext}", fulltext),
    ];
    for (key, value) in replacements {
        prompt = prompt.replace(key, value);
    }

    if !had_fulltext_placeholder {
        prompt.push_str("\n\n论文全文：\n");
        prompt.push_str(fulltext);
    }

    prompt
}

fn context_budget(provider: &crate::models::AiProvider, model_id: &str) -> usize {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.context_length)
        .map(|cl| {
            // 70% of context window, assuming ~4 chars/token, capped at 300K chars
            ((cl as usize * 7 / 10) * 4).min(300_000)
        })
        .unwrap_or(60_000)
        .saturating_sub(CONTEXT_OVERHEAD)
}

fn slug_to_event_id(slug: &str) -> String {
    slug.chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '/' | ':') { c } else { '-' })
        .collect()
}

fn default_meta() -> PaperMeta {
    PaperMeta {
        id: String::new(),
        title: "Unknown Paper".to_string(),
        authors: vec![],
        year: None,
        doi: None,
        arxiv_id: None,
        venue: None,
        tags: vec![],
        added_at: String::new(),
        original_filename: None,
        reading_status: "unread".to_string(),
        paper_abstract: None,
        bibtex: None,
        canvas_notes: vec![],
        import_source: None,
        cite_count: None,
    }
}
