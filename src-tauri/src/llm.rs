use std::time::Duration;

use futures::StreamExt;
use tauri::Emitter;

use crate::models::{AiModel, AiProvider, ChatMessage};

// ── Public API ────────────────────────────────────────────────────────────────

/// Non-streaming chat completion. Returns the full response text.
pub async fn chat_completion(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    source: &str,
) -> Result<String, String> {
    match provider.kind.as_str() {
        "anthropic" => chat_anthropic(provider, api_key, model, messages, source).await,
        _ => chat_openai_compat(provider, api_key, model, messages, source).await,
    }
}

/// Streaming chat completion.
/// Emits `{delta, done}` payloads to `event_name` on the app handle.
/// Reasoning/thinking tokens are emitted to `${event_name}-reasoning`.
/// Returns the full accumulated response text.
pub async fn chat_completion_stream(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
) -> Result<String, String> {
    match provider.kind.as_str() {
        "anthropic" => {
            stream_anthropic(
                provider,
                api_key,
                model,
                messages,
                event_name,
                app,
                use_reasoning,
                source,
            )
            .await
        }
        _ => {
            stream_openai_compat(
                provider,
                api_key,
                model,
                messages,
                event_name,
                app,
                use_reasoning,
                reasoning_effort,
                source,
            )
            .await
        }
    }
}

/// Like `chat_completion_stream` but for OpenRouter with an attached PDF.
/// Encodes the PDF as base64 and injects it as a `file` content block into
/// the first user message so the model can read the paper directly.
pub async fn chat_completion_stream_with_pdf(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
    pdf_path: &std::path::Path,
) -> Result<String, String> {
    stream_openrouter_with_pdf(
        provider,
        api_key,
        model,
        messages,
        event_name,
        app,
        use_reasoning,
        reasoning_effort,
        source,
        pdf_path,
    )
    .await
}

/// Fetch available models from the provider.
/// OpenAI-compatible: GET {base_url}/models
/// Anthropic: returns a hardcoded well-known list (no public /models endpoint).
pub async fn list_models(provider: &AiProvider, api_key: &str) -> Result<Vec<AiModel>, String> {
    match provider.kind.as_str() {
        "anthropic" => Ok(anthropic_known_models()),
        _ => fetch_openai_models(provider, api_key).await,
    }
}

/// Embed texts using the provider's /embeddings endpoint (OpenAI-compatible).
pub async fn embeddings(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    texts: &[String],
    source: &str,
) -> Result<Vec<Vec<f32>>, String> {
    if provider.kind.as_str() == "anthropic" {
        return Err(
            "Anthropic does not support embeddings. Use an OpenAI-compatible provider.".to_string(),
        );
    }

    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");

    if is_openrouter {
        embed_openrouter(provider, api_key, model, texts, source).await
    } else {
        embed_openai_compat(provider, api_key, model, texts, source).await
    }
}

/// Standard OpenAI-compatible batch embedding.
async fn embed_openai_compat(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    texts: &[String],
    source: &str,
) -> Result<Vec<Vec<f32>>, String> {
    let client = build_client()?;
    let url = format!("{}/embeddings", provider.base_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "model": model,
        "input": texts,
        "encoding_format": "float",
    });

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }

    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Invalid JSON from embeddings API: {e}"))?;

    let data = json["data"].as_array().ok_or_else(|| {
        format!(
            "No 'data' array in embeddings response: {}",
            &text[..text.len().min(200)]
        )
    })?;

    let total_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let vecs = parse_embedding_data(data)?;
    crate::token_usage::record(source, &provider.id, model, total_tokens, 0);
    Ok(vecs)
}

/// OpenRouter-specific embedding: one request per text, explicit float format,
/// with base64 fallback parsing and required attribution header.
async fn embed_openrouter(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    texts: &[String],
    source: &str,
) -> Result<Vec<Vec<f32>>, String> {
    let client = build_client()?;
    let url = format!("{}/embeddings", provider.base_url.trim_end_matches('/'));
    let mut total_tokens: u64 = 0;
    let mut total_cost_usd: Option<f64> = None;
    let mut vecs: Vec<Vec<f32>> = Vec::with_capacity(texts.len());

    // OpenRouter: send one text at a time — some models reject array input.
    for text in texts {
        let body = serde_json::json!({
            "model": model,
            "input": text,
            "encoding_format": "float",
        });

        let resp = client
            .post(&url)
            .header("Authorization", format!("Bearer {api_key}"))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/argus-app/argus")
            .header("X-Title", "Argus")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Network error: {e}"))?;

        let status = resp.status().as_u16();
        let resp_text = resp.text().await.unwrap_or_default();
        if status >= 400 {
            return Err(friendly_error(status, &resp_text));
        }

        let json: serde_json::Value = serde_json::from_str(&resp_text)
            .map_err(|e| format!("Invalid JSON from embeddings API: {e}"))?;

        total_tokens += json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
        if let Some(v) = usage_cost_usd(&json["usage"]) {
            total_cost_usd = Some(total_cost_usd.unwrap_or(0.0) + v);
        }

        let data = json["data"].as_array().ok_or_else(|| {
            format!(
                "No 'data' array in embeddings response: {}",
                &resp_text[..resp_text.len().min(200)]
            )
        })?;

        let mut batch = parse_embedding_data(data)?;
        vecs.append(&mut batch);
    }

    crate::token_usage::record_with_cost(
        source,
        &provider.id,
        model,
        total_tokens,
        0,
        total_cost_usd,
    );
    Ok(vecs)
}

/// Parse the `data` array from an embeddings response.
/// Handles both float-array and base64-encoded embedding fields.
fn parse_embedding_data(data: &[serde_json::Value]) -> Result<Vec<Vec<f32>>, String> {
    let mut vecs: Vec<Vec<f32>> = Vec::with_capacity(data.len());

    for item in data {
        let emb = &item["embedding"];

        let vec: Vec<f32> = if let Some(arr) = emb.as_array() {
            // Standard float array
            arr.iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect()
        } else if let Some(b64) = emb.as_str() {
            // Base64-encoded little-endian float32 array (some providers)
            use base64::Engine;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("Failed to decode base64 embedding: {e}"))?;
            if bytes.len() % 4 != 0 {
                return Err("Base64 embedding byte length is not a multiple of 4".to_string());
            }
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        } else {
            return Err(format!("Unexpected embedding field type: {}", emb));
        };

        if vec.is_empty() {
            return Err("Empty embedding vector returned — check the model name.".to_string());
        }
        vecs.push(vec);
    }

    Ok(vecs)
}

// ── OpenRouter with PDF ───────────────────────────────────────────────────────

/// Build the `messages` array for OpenRouter with the PDF injected as a
/// `file` content block into the first user message.
fn build_openrouter_messages_with_pdf(
    messages: &[ChatMessage],
    pdf_path: &std::path::Path,
) -> Vec<serde_json::Value> {
    use base64::Engine;

    let file_block = std::fs::read(pdf_path).ok().map(|bytes| {
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let filename = pdf_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("paper.pdf");
        serde_json::json!({
            "type": "file",
            "file": {
                "filename": filename,
                "file_data": format!("data:application/pdf;base64,{b64}")
            }
        })
    });

    let first_user_idx = messages.iter().position(|m| m.role == "user");

    messages
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if Some(i) == first_user_idx {
                if let Some(ref fb) = file_block {
                    return serde_json::json!({
                        "role": "user",
                        "content": [
                            {"type": "text", "text": &m.content},
                            fb
                        ]
                    });
                }
            }
            serde_json::json!({"role": m.role, "content": &m.content})
        })
        .collect()
}

async fn stream_openrouter_with_pdf(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
    pdf_path: &std::path::Path,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let msgs = if pdf_path.exists() {
        build_openrouter_messages_with_pdf(messages, pdf_path)
    } else {
        messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
            .collect()
    };

    let mut body = serde_json::json!({
        "model": model,
        "messages": msgs,
        "stream": true,
        "stream_options": {"include_usage": true},
        "usage": {"include": true}
    });

    {
        let order: Vec<&str> = provider
            .models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.provider_order.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        if !order.is_empty() {
            body["provider"] = serde_json::json!({ "order": order, "allow_fallbacks": false });
        }
    }

    if use_reasoning {
        body["reasoning"] = serde_json::json!({
            "effort": reasoning_effort.unwrap_or("high"),
            "exclude": false
        });
    }

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut usage_emitted = false;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        loop {
            match buf.find('\n') {
                None => break,
                Some(pos) => {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..pos + 1);

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            if !usage_emitted {
                                emit_stream_usage(
                                    app,
                                    event_name,
                                    input_tokens,
                                    output_tokens,
                                    input_tokens.saturating_add(output_tokens),
                                    cost_usd,
                                );
                            }
                            crate::token_usage::record_with_cost(
                                source,
                                &provider.id,
                                model,
                                input_tokens,
                                output_tokens,
                                cost_usd,
                            );
                            let _ =
                                app.emit(event_name, serde_json::json!({"delta":"","done":true}));
                            return Ok(accumulated);
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(usage) = json.get("usage").filter(|v| !v.is_null()) {
                                if let Some(v) = usage["prompt_tokens"].as_u64() {
                                    input_tokens = v;
                                }
                                if let Some(v) = usage["completion_tokens"].as_u64() {
                                    output_tokens = v;
                                }
                                if let Some(v) = usage_cost_usd(usage) {
                                    cost_usd = Some(v);
                                }
                                let total_tokens = usage["total_tokens"]
                                    .as_u64()
                                    .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
                                emit_stream_usage(
                                    app,
                                    event_name,
                                    input_tokens,
                                    output_tokens,
                                    total_tokens,
                                    cost_usd,
                                );
                                usage_emitted = true;
                            }
                            let delta = json["choices"][0]["delta"]["content"]
                                .as_str()
                                .unwrap_or("");
                            let reasoning_delta = json["choices"][0]["delta"]["reasoning"]
                                .as_str()
                                .unwrap_or("");
                            if !reasoning_delta.is_empty() {
                                let _ = app.emit(
                                    &reasoning_event,
                                    serde_json::json!({"delta": reasoning_delta, "done": false}),
                                );
                            }
                            if !delta.is_empty() {
                                accumulated.push_str(delta);
                                let _ = app.emit(
                                    event_name,
                                    serde_json::json!({"delta": delta, "done": false}),
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    crate::token_usage::record_with_cost(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        cost_usd,
    );
    if !usage_emitted {
        emit_stream_usage(
            app,
            event_name,
            input_tokens,
            output_tokens,
            input_tokens.saturating_add(output_tokens),
            cost_usd,
        );
    }
    let _ = app.emit(event_name, serde_json::json!({"delta":"","done":true}));
    Ok(accumulated)
}

// ── OpenAI-compatible ─────────────────────────────────────────────────────────

async fn chat_openai_compat(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    source: &str,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();
    let mut body = serde_json::json!({"model": model, "messages": msgs});

    if is_openrouter {
        body["usage"] = serde_json::json!({"include": true});
        let order: Vec<&str> = provider
            .models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.provider_order.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        if !order.is_empty() {
            body["provider"] = serde_json::json!({ "order": order, "allow_fallbacks": false });
        }
    }

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from API: {e}"))?;

    let input_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    let cost_usd = if is_openrouter {
        usage_cost_usd(&json["usage"])
    } else {
        None
    };
    crate::token_usage::record_with_cost(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        cost_usd,
    );

    json["choices"][0]["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Unexpected response format from API".to_string())
}

async fn stream_openai_compat(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();

    let is_deepseek = provider.base_url.to_lowercase().contains("deepseek");
    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");

    let mut body = serde_json::json!({
        "model": model, "messages": msgs, "stream": true,
        "stream_options": {"include_usage": true}
    });

    if is_openrouter {
        body["usage"] = serde_json::json!({"include": true});
        let order: Vec<&str> = provider
            .models
            .iter()
            .find(|m| m.id == model)
            .map(|m| m.provider_order.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        if !order.is_empty() {
            body["provider"] = serde_json::json!({ "order": order, "allow_fallbacks": false });
        }
    }

    if use_reasoning {
        if is_deepseek {
            body["thinking"] = serde_json::json!({"type": "enabled"});
            // DeepSeek: low/medium -> "high", high -> "max"
            let ds_effort = match reasoning_effort.unwrap_or("high") {
                "high" => "max",
                _ => "high",
            };
            body["reasoning_effort"] = serde_json::json!(ds_effort);
        } else if is_openrouter {
            body["reasoning"] = serde_json::json!({
                "effort": reasoning_effort.unwrap_or("high"),
                "exclude": false
            });
        } else {
            body["reasoning_effort"] = serde_json::json!(reasoning_effort.unwrap_or("high"));
        }
    }

    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut usage_emitted = false;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        loop {
            match buf.find('\n') {
                None => break,
                Some(pos) => {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..pos + 1);

                    if let Some(data) = line.strip_prefix("data: ") {
                        if data == "[DONE]" {
                            if !usage_emitted {
                                emit_stream_usage(
                                    app,
                                    event_name,
                                    input_tokens,
                                    output_tokens,
                                    input_tokens.saturating_add(output_tokens),
                                    if is_openrouter { cost_usd } else { None },
                                );
                            }
                            crate::token_usage::record_with_cost(
                                source,
                                &provider.id,
                                model,
                                input_tokens,
                                output_tokens,
                                if is_openrouter { cost_usd } else { None },
                            );
                            let _ =
                                app.emit(event_name, serde_json::json!({"delta":"","done":true}));
                            return Ok(accumulated);
                        }
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            // Capture usage from the final usage chunk
                            if let Some(usage) = json.get("usage").filter(|v| !v.is_null()) {
                                if let Some(v) = usage["prompt_tokens"].as_u64() {
                                    input_tokens = v;
                                }
                                if let Some(v) = usage["completion_tokens"].as_u64() {
                                    output_tokens = v;
                                }
                                if is_openrouter {
                                    if let Some(v) = usage_cost_usd(usage) {
                                        cost_usd = Some(v);
                                    }
                                }
                                let total_tokens = usage["total_tokens"]
                                    .as_u64()
                                    .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
                                emit_stream_usage(
                                    app,
                                    event_name,
                                    input_tokens,
                                    output_tokens,
                                    total_tokens,
                                    if is_openrouter { cost_usd } else { None },
                                );
                                usage_emitted = true;
                            }
                            // Main content delta
                            if let Some(delta) = json["choices"][0]["delta"]["content"].as_str() {
                                if !delta.is_empty() {
                                    accumulated.push_str(delta);
                                    let _ = app.emit(
                                        event_name,
                                        serde_json::json!({"delta": delta, "done": false}),
                                    );
                                }
                            }
                            // Reasoning/thinking content (DeepSeek: reasoning_content, OpenRouter: reasoning, Ollama: thinking)
                            let reasoning = json["choices"][0]["delta"]["reasoning_content"]
                                .as_str()
                                .or_else(|| json["choices"][0]["delta"]["reasoning"].as_str())
                                .or_else(|| json["choices"][0]["delta"]["thinking"].as_str());
                            if let Some(r) = reasoning {
                                if !r.is_empty() {
                                    let _ = app.emit(
                                        &reasoning_event,
                                        serde_json::json!({"delta": r, "done": false}),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    crate::token_usage::record_with_cost(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        if is_openrouter { cost_usd } else { None },
    );
    if !usage_emitted {
        emit_stream_usage(
            app,
            event_name,
            input_tokens,
            output_tokens,
            input_tokens.saturating_add(output_tokens),
            if is_openrouter { cost_usd } else { None },
        );
    }
    let _ = app.emit(event_name, serde_json::json!({"delta":"","done":true}));
    Ok(accumulated)
}

// ── Anthropic native ──────────────────────────────────────────────────────────

async fn chat_anthropic(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    source: &str,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/messages", provider.base_url.trim_end_matches('/'));
    let (system, conv) = split_system(messages);
    let mut body = serde_json::json!({"model": model, "max_tokens": 4096, "messages": conv});
    if !system.is_empty() {
        body["system"] = serde_json::json!(system);
    }

    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from Anthropic: {e}"))?;

    let input_tokens = json["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let output_tokens = json["usage"]["output_tokens"].as_u64().unwrap_or(0);
    crate::token_usage::record(source, &provider.id, model, input_tokens, output_tokens);

    json["content"][0]["text"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Unexpected Anthropic response format".to_string())
}

async fn stream_anthropic(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    source: &str,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/messages", provider.base_url.trim_end_matches('/'));
    let (system, conv) = split_system(messages);

    let thinking_budget: i64 = 10_000;
    let max_tokens = if use_reasoning {
        std::cmp::max(16_384, thinking_budget + 4_096)
    } else {
        4_096
    };
    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "messages": conv,
        "stream": true
    });
    if !system.is_empty() {
        body["system"] = serde_json::json!(system);
    }
    if use_reasoning {
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": thinking_budget
        });
    }

    let resp = client
        .post(&url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        buf.push_str(&String::from_utf8_lossy(&bytes));

        loop {
            match buf.find('\n') {
                None => break,
                Some(pos) => {
                    let line = buf[..pos].trim_end_matches('\r').to_string();
                    buf.drain(..pos + 1);

                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            match json["type"].as_str() {
                                Some("message_start") => {
                                    if let Some(v) =
                                        json["message"]["usage"]["input_tokens"].as_u64()
                                    {
                                        input_tokens = v;
                                    }
                                }
                                Some("message_delta") => {
                                    if let Some(v) = json["usage"]["output_tokens"].as_u64() {
                                        output_tokens = v;
                                    }
                                }
                                Some("content_block_delta") => {
                                    let delta_type = json["delta"]["type"].as_str().unwrap_or("");
                                    if delta_type == "thinking_delta" {
                                        if let Some(t) = json["delta"]["thinking"].as_str() {
                                            if !t.is_empty() {
                                                let _ = app.emit(
                                                    &reasoning_event,
                                                    serde_json::json!({"delta": t, "done": false}),
                                                );
                                            }
                                        }
                                    } else if delta_type == "text_delta" {
                                        if let Some(t) = json["delta"]["text"].as_str() {
                                            if !t.is_empty() {
                                                accumulated.push_str(t);
                                                let _ = app.emit(
                                                    event_name,
                                                    serde_json::json!({"delta": t, "done": false}),
                                                );
                                            }
                                        }
                                    }
                                }
                                Some("message_stop") => {
                                    emit_stream_usage(
                                        app,
                                        event_name,
                                        input_tokens,
                                        output_tokens,
                                        input_tokens.saturating_add(output_tokens),
                                        None,
                                    );
                                    crate::token_usage::record(
                                        source,
                                        &provider.id,
                                        model,
                                        input_tokens,
                                        output_tokens,
                                    );
                                    let _ = app.emit(
                                        event_name,
                                        serde_json::json!({"delta":"","done":true}),
                                    );
                                    return Ok(accumulated);
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    crate::token_usage::record(source, &provider.id, model, input_tokens, output_tokens);
    emit_stream_usage(
        app,
        event_name,
        input_tokens,
        output_tokens,
        input_tokens.saturating_add(output_tokens),
        None,
    );
    let _ = app.emit(event_name, serde_json::json!({"delta":"","done":true}));
    Ok(accumulated)
}

fn emit_stream_usage(
    app: &tauri::AppHandle,
    event_name: &str,
    input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
    cost_usd: Option<f64>,
) {
    if input_tokens == 0 && output_tokens == 0 && total_tokens == 0 && cost_usd.is_none() {
        return;
    }
    let usage_event = format!("{event_name}-usage");
    let _ = app.emit(
        usage_event.as_str(),
        serde_json::json!({
            "input_tokens": input_tokens,
            "output_tokens": output_tokens,
            "total_tokens": total_tokens,
            "cost_usd": cost_usd,
        }),
    );
}

fn usage_cost_usd(usage: &serde_json::Value) -> Option<f64> {
    let value = usage["cost"]
        .as_f64()
        .or_else(|| usage["cost"].as_str().and_then(|s| s.parse::<f64>().ok()))?;
    if value.is_finite() && value >= 0.0 {
        Some(value)
    } else {
        None
    }
}

// ── Model listing ─────────────────────────────────────────────────────────────

async fn fetch_openai_models(provider: &AiProvider, api_key: &str) -> Result<Vec<AiModel>, String> {
    let client = build_client()?;
    let base = provider.base_url.trim_end_matches('/');
    let url = format!("{base}/models");

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }

    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from /models: {e}"))?;

    let data = json["data"].as_array().ok_or_else(|| {
        format!(
            "No 'data' array in /models response. Got: {}",
            &text[..text.len().min(200)]
        )
    })?;

    let mut models: Vec<AiModel> = data
        .iter()
        .filter_map(|item| parse_model_item(item))
        .collect();

    // OpenRouter keeps embedding models in a separate endpoint that the standard
    // /models endpoint never returns. Fetch and merge them when we detect OpenRouter.
    if base.to_lowercase().contains("openrouter") {
        let embed_url = format!("{base}/embeddings/models");
        if let Ok(embed_resp) = client
            .get(&embed_url)
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
        {
            if embed_resp.status().is_success() {
                if let Ok(embed_text) = embed_resp.text().await {
                    if let Ok(embed_json) = serde_json::from_str::<serde_json::Value>(&embed_text) {
                        if let Some(embed_data) = embed_json["data"].as_array() {
                            let existing_ids: std::collections::HashSet<String> =
                                models.iter().map(|m| m.id.clone()).collect();
                            for item in embed_data {
                                if let Some(mut m) = parse_model_item(item) {
                                    if !existing_ids.contains(&m.id) {
                                        // Guarantee the embedding capability is set
                                        if !m.capabilities.iter().any(|c| c == "embedding") {
                                            m.capabilities.push("embedding".to_string());
                                        }
                                        models.push(m);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(models)
}

fn parse_model_item(item: &serde_json::Value) -> Option<AiModel> {
    let id = item["id"].as_str()?;
    let display_name = item["name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .unwrap_or(id)
        .to_string();
    let context_length = item["context_length"]
        .as_u64()
        .or_else(|| item["context_window"].as_u64());
    let capabilities = parse_capabilities(item);
    let input_price_usd_per_million = parse_price_usd_per_million(&item["pricing"]["prompt"]);
    let output_price_usd_per_million =
        parse_price_usd_per_million(&item["pricing"]["completion"]);
    Some(AiModel {
        id: id.to_string(),
        display_name,
        capabilities,
        context_length,
        enabled: true,
        input_price_per_million: None,
        output_price_per_million: None,
        input_price_usd_per_million,
        output_price_usd_per_million,
        provider_order: vec![],
    })
}

fn parse_price_usd_per_million(value: &serde_json::Value) -> Option<f64> {
    let per_token = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))?;
    if per_token.is_finite() && per_token >= 0.0 {
        Some(per_token * 1_000_000.0)
    } else {
        None
    }
}

fn parse_capabilities(item: &serde_json::Value) -> Vec<String> {
    let mut caps = Vec::new();

    let id = item["id"].as_str().unwrap_or("").to_lowercase();
    let name = item["name"].as_str().unwrap_or("").to_lowercase();
    let search_text = format!("{id} {name}");

    // OpenRouter architecture fields.
    if let Some(arch) = item.get("architecture") {
        let modality = arch["modality"].as_str().unwrap_or("").to_lowercase();
        if modality.contains("image") || modality.contains("vision") {
            add_capability(&mut caps, "vision");
        }
        if modality.contains("embedding")
            || modality.contains("embed")
            || modality.contains("vector")
        {
            add_capability(&mut caps, "embedding");
        }
        if array_has_any(&arch["input_modalities"], &["image", "vision"]) {
            add_capability(&mut caps, "vision");
        }
        if array_has_any(
            &arch["output_modalities"],
            &["embedding", "embed", "vector"],
        ) {
            add_capability(&mut caps, "embedding");
        }
    }

    // OpenAI modalities field
    if let Some(modalities) = item["modalities"].as_array() {
        if modalities.iter().any(|v| v.as_str() == Some("image")) {
            add_capability(&mut caps, "vision");
        }
    }

    if array_has_any(
        &item["supported_parameters"],
        &["tools", "tool_choice", "functions"],
    ) {
        add_capability(&mut caps, "tool_calling");
    }
    if array_has_any(
        &item["supported_parameters"],
        &["reasoning", "include_reasoning", "reasoning_effort"],
    ) {
        add_capability(&mut caps, "reasoning");
    }
    if array_has_any(
        &item["capabilities"],
        &[
            "embedding",
            "embeddings",
            "embed",
            "vision",
            "image",
            "tools",
            "tool_calling",
            "function_calling",
            "reasoning",
        ],
    ) {
        add_capabilities_from_values(&mut caps, &item["capabilities"]);
    }

    if looks_like_embedding_model(&search_text) {
        add_capability(&mut caps, "embedding");
    }
    if looks_like_reasoning_model(&search_text) {
        add_capability(&mut caps, "reasoning");
    }
    if search_text.contains("vision")
        || search_text.contains("qwen-vl")
        || search_text.contains("llava")
        || search_text.contains("pixtral")
        || search_text.contains("gemini")
        || search_text.contains("gpt-4o")
    {
        add_capability(&mut caps, "vision");
    }

    caps
}

fn add_capability(caps: &mut Vec<String>, cap: &str) {
    if !caps.iter().any(|existing| existing == cap) {
        caps.push(cap.to_string());
    }
}

fn array_has_any(value: &serde_json::Value, needles: &[&str]) -> bool {
    value
        .as_array()
        .map(|items| {
            items.iter().any(|item| {
                item.as_str()
                    .map(|s| {
                        let s = s.to_lowercase();
                        needles.iter().any(|needle| s.contains(needle))
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn add_capabilities_from_values(caps: &mut Vec<String>, value: &serde_json::Value) {
    let Some(items) = value.as_array() else {
        return;
    };
    for item in items {
        let Some(raw) = item.as_str() else { continue };
        let cap = raw.to_lowercase();
        if cap.contains("embedding") || cap.contains("embed") {
            add_capability(caps, "embedding");
        }
        if cap.contains("vision") || cap.contains("image") {
            add_capability(caps, "vision");
        }
        if cap.contains("tool") || cap.contains("function") {
            add_capability(caps, "tool_calling");
        }
        if cap.contains("reason") {
            add_capability(caps, "reasoning");
        }
    }
}

fn looks_like_embedding_model(text: &str) -> bool {
    text.contains("embedding")
        || text.contains("embed")
        || text.contains("text-embedding")
        || text.contains("bge-")
        || text.contains("gte-")
        || text.contains("e5-")
        || text.contains("voyage-")
        || text.contains("jina-embeddings")
        || text.contains("nomic-embed")
}

fn looks_like_reasoning_model(text: &str) -> bool {
    text.contains("reasoning")
        || text.contains("reasoner")
        || text.contains("thinking")
        || text.contains("/r1")
        || text.contains("-r1")
        || text.contains("/o1")
        || text.contains("-o1")
        || text.contains("/o3")
        || text.contains("-o3")
        || text.contains("/o4")
        || text.contains("-o4")
        || text.contains("qwq")
}

fn anthropic_known_models() -> Vec<AiModel> {
    vec![
        AiModel {
            id: "claude-opus-4-5".to_string(),
            display_name: "Claude Opus 4.5".to_string(),
            capabilities: vec!["vision".to_string()],
            context_length: Some(200_000),
            enabled: true,
            input_price_per_million: None,
            output_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
        },
        AiModel {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            capabilities: vec!["vision".to_string()],
            context_length: Some(200_000),
            enabled: true,
            input_price_per_million: None,
            output_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
        },
        AiModel {
            id: "claude-haiku-4-5-20251001".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            capabilities: vec!["vision".to_string()],
            context_length: Some(200_000),
            enabled: true,
            input_price_per_million: None,
            output_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
        },
    ]
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn build_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .user_agent("Argus/0.1")
        .build()
        .map_err(|e| e.to_string())
}

fn split_system(messages: &[ChatMessage]) -> (String, Vec<serde_json::Value>) {
    let system: String = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let conv: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| serde_json::json!({"role": m.role, "content": m.content}))
        .collect();
    (system, conv)
}

fn friendly_error(status: u16, body: &str) -> String {
    let preview = &body[..body.len().min(300)];
    match status {
        401 => "Authentication failed (401). Check your API key in Settings → AI Services.".to_string(),
        403 => "Access denied (403). Your key may lack permission for this model.".to_string(),
        404 => format!("Endpoint or model not found (404). Verify your API address and model ID. Response: {preview}"),
        429 => "Rate limited (429). Please wait a moment and try again.".to_string(),
        _ => format!("API error {status}: {preview}"),
    }
}
