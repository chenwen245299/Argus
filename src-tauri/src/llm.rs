use std::time::Duration;

use futures::StreamExt;
use tauri::Emitter;

use crate::models::{AiModel, AiProvider, ChatContent, ChatContentPart, ChatMessage};

// ── Public API ────────────────────────────────────────────────────────────────

/// Non-streaming chat completion. Returns the full response text.
fn is_kimi_coding_endpoint(provider: &AiProvider) -> bool {
    provider.base_url.to_lowercase().contains("api.kimi.com")
}

fn is_anthropic_protocol(provider: &AiProvider) -> bool {
    provider.kind == "anthropic" || is_kimi_coding_endpoint(provider)
}

pub async fn chat_completion(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    source: &str,
) -> Result<String, String> {
    if is_anthropic_protocol(provider) {
        chat_anthropic(provider, api_key, model, messages, source).await
    } else {
        chat_openai_compat(provider, api_key, model, messages, source).await
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
    if is_anthropic_protocol(provider) {
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
    } else {
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

/// Like `chat_completion_stream` but for providers that accept an inline PDF.
/// Currently only OpenRouter supports OpenAI-compatible `file` content parts.
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
    stream_with_pdf_injected(
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
    if provider.kind == "kimi" || provider.base_url.to_lowercase().contains("api.kimi.com") {
        // Kimi Code / Moonshot does not expose a public /models endpoint for
        // ordinary API keys (it typically returns 401). Return a hard-coded
        // well-known list instead.
        return Ok(kimi_known_models());
    }
    match provider.kind.as_str() {
        "anthropic" => Ok(anthropic_known_models()),
        _ => fetch_openai_models(provider, api_key).await,
    }
}

/// Test provider connectivity by sending a tiny non-streaming chat completion.
/// Unlike /models, this works for providers such as Kimi Code that do not
/// expose a public model-list endpoint.
pub async fn test_connection(provider: &AiProvider, api_key: &str) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/chat/completions", provider.base_url.trim_end_matches('/'));
    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");
    let is_kimi = provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com");

    // Pick a model id to probe. For Kimi Code / Moonshot use a known id.
    let model = if is_kimi {
        provider
            .models
            .iter()
            .find(|m| m.id == "kimi-for-coding" || m.id.starts_with("kimi-k2"))
            .map(|m| m.id.as_str())
            .unwrap_or("kimi-for-coding")
    } else {
        provider
            .models
            .first()
            .map(|m| m.id.as_str())
            .unwrap_or("gpt-4o-mini")
    };

    let is_kimi_k2 = is_kimi && model.starts_with("kimi-k2");
    let is_kimi_for_coding = is_kimi && model == "kimi-for-coding";

    // Kimi Code's /coding endpoint is sensitive to extra parameters; keep the
    // probe minimal. Other providers get a tiny max_tokens cap.
    let mut body = if is_kimi_for_coding {
        serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "Hi"}]
        })
    } else {
        serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": "Hi"}],
            "max_tokens": 1
        })
    };

    // The Moonshot /models endpoint is gone for Kimi Code; avoid Moonshot-only
    // extensions such as usage.include on the /coding endpoint.
    if is_openrouter || (is_kimi && !is_kimi_for_coding) {
        body["usage"] = serde_json::json!({"include": true});
    }

    if is_kimi_k2 {
        body["thinking"] = serde_json::json!({"type": "enabled"});
        body["temperature"] = serde_json::json!(1.0);
        body["top_p"] = serde_json::json!(0.95);
        body["n"] = serde_json::json!(1);
        body["presence_penalty"] = serde_json::json!(0.0);
        body["frequency_penalty"] = serde_json::json!(0.0);
    }

    let is_kimi_coding_endpoint = provider.base_url.to_lowercase().contains("api.kimi.com");
    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if is_kimi_coding_endpoint {
        // Kimi Code's /coding endpoint gates access by User-Agent whitelist.
        // Pretend to be a whitelisted coding agent so ordinary API keys work.
        req = req.header("User-Agent", "KimiCLI/1.5");
    }
    let resp = req.json(&body).send().await.map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(format!(
            "{} [kind={}, base_url={}, model={}]",
            friendly_error(status, &text),
            provider.kind,
            provider.base_url,
            model
        ));
    }

    Ok(format!("Connected. Provider responded with status {status} (model={model})."))
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
        .timeout(REQUEST_TIMEOUT)
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
            char_prefix(&text, 200)
        )
    })?;

    let total_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let vecs = parse_embedding_data(data)?;
    crate::token_usage::record(source, &provider.id, model, total_tokens, 0);
    Ok(vecs)
}

/// OpenRouter-specific embedding: one request per text (some models reject
/// array input), explicit float format, with base64 fallback parsing and
/// required attribution header. Requests run a few at a time; `buffered`
/// keeps results in input order so embeddings stay aligned with their chunks.
async fn embed_openrouter(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    texts: &[String],
    source: &str,
) -> Result<Vec<Vec<f32>>, String> {
    use futures::TryStreamExt;

    const EMBED_CONCURRENCY: usize = 4;

    let client = build_client()?;
    let url = format!("{}/embeddings", provider.base_url.trim_end_matches('/'));

    // Each request future owns its data ('static) — borrowing across
    // `buffered` trips rustc's higher-ranked lifetime inference when this
    // future is later awaited inside a spawned task.
    let requests = texts.to_vec().into_iter().map(|text| {
        let client = client.clone();
        let url = url.clone();
        let api_key = api_key.to_string();
        let model = model.to_string();
        async move {
            let body = serde_json::json!({
                "model": model,
                "input": text,
                "encoding_format": "float",
            });

            let resp = client
                .post(&url)
                .timeout(REQUEST_TIMEOUT)
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

            let tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
            let cost = usage_cost_usd(&json["usage"]);

            let data = json["data"].as_array().ok_or_else(|| {
                format!(
                    "No 'data' array in embeddings response: {}",
                    char_prefix(&resp_text, 200)
                )
            })?;

            Ok::<_, String>((parse_embedding_data(data)?, tokens, cost))
        }
    });

    let results: Vec<(Vec<Vec<f32>>, u64, Option<f64>)> = futures::stream::iter(requests)
        .buffered(EMBED_CONCURRENCY)
        .try_collect()
        .await?;

    let mut total_tokens: u64 = 0;
    let mut total_cost_usd: Option<f64> = None;
    let mut vecs: Vec<Vec<f32>> = Vec::with_capacity(texts.len());
    for (mut batch, tokens, cost) in results {
        total_tokens += tokens;
        if let Some(v) = cost {
            total_cost_usd = Some(total_cost_usd.unwrap_or(0.0) + v);
        }
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

// ── OpenAI-compatible providers with inline PDF ───────────────────────────────

/// Build the `messages` array with the PDF injected as a `file` content block
/// into the first user message. Works for OpenRouter and Kimi.
fn build_messages_with_pdf(
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
                    let content = match &m.content {
                        ChatContent::Text(s) => {
                            serde_json::json!([{"type": "text", "text": s.as_str()}, fb])
                        }
                        ChatContent::Parts(parts) => {
                            let mut arr = serde_json::to_value(parts)
                                .ok()
                                .and_then(|v| v.as_array().cloned())
                                .unwrap_or_default();
                            arr.push(fb.clone());
                            serde_json::Value::Array(arr)
                        }
                    };
                    return serde_json::json!({"role": "user", "content": content});
                }
            }
            serde_json::json!({"role": m.role, "content": &m.content})
        })
        .collect()
}

async fn stream_with_pdf_injected(
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
        build_messages_with_pdf(messages, pdf_path)
    } else {
        messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": &m.content}))
            .collect()
    };

    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");
    let is_kimi = provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com");
    let is_kimi_k2 = is_kimi && model.starts_with("kimi-k2");
    let is_kimi_for_coding = is_kimi && model == "kimi-for-coding";

    let mut body = serde_json::json!({
        "model": model,
        "messages": msgs,
        "stream": true,
        "stream_options": {"include_usage": true}
    });

    if is_openrouter || (is_kimi && !is_kimi_for_coding) {
        body["usage"] = serde_json::json!({"include": true});
    }

    if is_openrouter {
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

    if use_reasoning || is_kimi_k2 {
        if is_openrouter {
            body["reasoning"] = serde_json::json!({
                "effort": reasoning_effort.unwrap_or("high"),
                "exclude": false
            });
        } else if is_kimi_k2 {
            // Kimi K2.* series requires thinking enabled and fixed sampling params.
            body["thinking"] = serde_json::json!({"type": "enabled"});
            body["temperature"] = serde_json::json!(1.0);
            body["top_p"] = serde_json::json!(0.95);
            body["n"] = serde_json::json!(1);
            body["presence_penalty"] = serde_json::json!(0.0);
            body["frequency_penalty"] = serde_json::json!(0.0);
        } else if is_kimi_for_coding {
            // Kimi Code subscription model supports thinking but does not require it.
            body["thinking"] = serde_json::json!({"type": "enabled"});
        }
    }

    let is_kimi_coding_endpoint = provider.base_url.to_lowercase().contains("api.kimi.com");
    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if is_kimi_coding_endpoint {
        req = req.header("User-Agent", "KimiCLI/1.5");
        // reqwest's bytes_stream() does not decompress gzip. Kimi Code may return
        // a gzipped SSE stream, so ask for identity encoding to keep it plain text.
        req = req.header("Accept-Encoding", "identity");
    }
    let resp = req.json(&body).send().await.map_err(|e| format!("Network error: {e}"))?;

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

                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim_start();
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
                            let content_delta = json["choices"][0]["delta"]["content"]
                                .as_str()
                                .unwrap_or("");
                            let reasoning_delta = json["choices"][0]["delta"]["reasoning_content"]
                                .as_str()
                                .or_else(|| json["choices"][0]["delta"]["reasoning"].as_str())
                                .or_else(|| json["choices"][0]["delta"]["thinking"].as_str())
                                .unwrap_or("");

                            if !content_delta.is_empty() {
                                accumulated.push_str(content_delta);
                                let _ = app.emit(
                                    event_name,
                                    serde_json::json!({"delta": content_delta, "done": false}),
                                );
                            } else if is_kimi_for_coding && !reasoning_delta.is_empty() {
                                // kimi-for-coding emits its response as reasoning_content by default.
                                accumulated.push_str(reasoning_delta);
                                let _ = app.emit(
                                    event_name,
                                    serde_json::json!({"delta": reasoning_delta, "done": false}),
                                );
                            } else if !reasoning_delta.is_empty() {
                                let _ = app.emit(
                                    &reasoning_event,
                                    serde_json::json!({"delta": reasoning_delta, "done": false}),
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
    let is_kimi = provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com");
    let is_kimi_k2 = is_kimi && model.starts_with("kimi-k2");
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| serde_json::json!({"role": m.role, "content": &m.content}))
        .collect();
    let is_kimi_for_coding = is_kimi && model == "kimi-for-coding";

    let mut body = serde_json::json!({"model": model, "messages": msgs});

    if is_openrouter || (is_kimi && !is_kimi_for_coding) {
        body["usage"] = serde_json::json!({"include": true});
    }

    if is_openrouter {
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

    if is_kimi_k2 {
        body["thinking"] = serde_json::json!({"type": "enabled"});
        body["temperature"] = serde_json::json!(1.0);
        body["top_p"] = serde_json::json!(0.95);
        body["n"] = serde_json::json!(1);
        body["presence_penalty"] = serde_json::json!(0.0);
        body["frequency_penalty"] = serde_json::json!(0.0);
    }

    let is_kimi_coding_endpoint = provider.base_url.to_lowercase().contains("api.kimi.com");
    let mut req = client
        .post(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if is_kimi_coding_endpoint {
        req = req.header("User-Agent", "KimiCLI/1.5");
    }
    let resp = req.json(&body).send().await.map_err(|e| format!("Network error: {e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from API: {e}"))?;

    let input_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    let cost_usd = if is_openrouter || is_kimi {
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
        .map(|m| serde_json::json!({"role": m.role, "content": &m.content}))
        .collect();

    let is_deepseek = provider.base_url.to_lowercase().contains("deepseek");
    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");
    let is_kimi = provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com");
    let is_kimi_k2 = is_kimi && model.starts_with("kimi-k2");
    let is_kimi_for_coding = is_kimi && model == "kimi-for-coding";

    let mut body = serde_json::json!({
        "model": model, "messages": msgs, "stream": true,
        "stream_options": {"include_usage": true}
    });

    if is_openrouter || (is_kimi && !is_kimi_for_coding) {
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

    if use_reasoning || is_kimi_k2 {
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
        } else if is_kimi_k2 {
            // Kimi K2.7 Code/K2.6/K2.5 require thinking enabled and fixed sampling params.
            body["thinking"] = serde_json::json!({"type": "enabled"});
            body["temperature"] = serde_json::json!(1.0);
            body["top_p"] = serde_json::json!(0.95);
            body["n"] = serde_json::json!(1);
            body["presence_penalty"] = serde_json::json!(0.0);
            body["frequency_penalty"] = serde_json::json!(0.0);
        } else if is_kimi_for_coding && use_reasoning {
            // Kimi Code subscription model supports thinking but does not require it.
            body["thinking"] = serde_json::json!({"type": "enabled"});
        } else {
            body["reasoning_effort"] = serde_json::json!(reasoning_effort.unwrap_or("high"));
        }
    }

    let is_kimi_coding_endpoint = provider.base_url.to_lowercase().contains("api.kimi.com");
    let mut req = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json");
    if is_kimi_coding_endpoint {
        req = req.header("User-Agent", "KimiCLI/1.5");
        // Kimi Code may return a gzipped SSE stream; ask for identity to keep it plain text.
        req = req.header("Accept-Encoding", "identity");
    }
    let resp = req.json(&body).send().await.map_err(|e| format!("Network error: {e}"))?;

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

                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim_start();
                        if data == "[DONE]" {
                            if !usage_emitted {
                                emit_stream_usage(
                                    app,
                                    event_name,
                                    input_tokens,
                                    output_tokens,
                                    input_tokens.saturating_add(output_tokens),
                                    if is_openrouter || is_kimi { cost_usd } else { None },
                                );
                            }
                            crate::token_usage::record_with_cost(
                                source,
                                &provider.id,
                                model,
                                input_tokens,
                                output_tokens,
                                if is_openrouter || is_kimi { cost_usd } else { None },
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
                                if is_openrouter || is_kimi {
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
                                    if is_openrouter || is_kimi { cost_usd } else { None },
                                );
                                usage_emitted = true;
                            }
                            // Main content delta
                            let content_delta = json["choices"][0]["delta"]["content"].as_str();
                            let reasoning_delta = json["choices"][0]["delta"]["reasoning_content"]
                                .as_str()
                                .or_else(|| json["choices"][0]["delta"]["reasoning"].as_str())
                                .or_else(|| json["choices"][0]["delta"]["thinking"].as_str());

                            if let Some(delta) = content_delta.filter(|s| !s.is_empty()) {
                                accumulated.push_str(delta);
                                let _ = app.emit(
                                    event_name,
                                    serde_json::json!({"delta": delta, "done": false}),
                                );
                            } else if is_kimi_for_coding {
                                // kimi-for-coding emits its response as reasoning_content by
                                // default. Treat it as the main answer so users see output even
                                // without the reasoning toggle.
                                if let Some(delta) = reasoning_delta.filter(|s| !s.is_empty()) {
                                    accumulated.push_str(delta);
                                    let _ = app.emit(
                                        event_name,
                                        serde_json::json!({"delta": delta, "done": false}),
                                    );
                                }
                            }

                            // Reasoning/thinking content for other providers (DeepSeek, OpenRouter, Ollama).
                            // For kimi-for-coding we already folded reasoning_content into the main answer above.
                            if !is_kimi_for_coding {
                                if let Some(r) = reasoning_delta.filter(|s| !s.is_empty()) {
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
        if is_openrouter || is_kimi { cost_usd } else { None },
    );
    if !usage_emitted {
        emit_stream_usage(
            app,
            event_name,
            input_tokens,
            output_tokens,
            input_tokens.saturating_add(output_tokens),
            if is_openrouter || is_kimi { cost_usd } else { None },
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
    let is_kimi_coding = is_kimi_coding_endpoint(provider);
    // Kimi Code allows larger output windows; Anthropic defaults stay conservative.
    let max_tokens: i64 = if is_kimi_coding { 8192 } else { 4096 };
    let mut body = serde_json::json!({"model": model, "max_tokens": max_tokens, "messages": conv});
    if !system.is_empty() {
        body["system"] = serde_json::json!(system);
    }

    let mut req = client
        .post(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Content-Type", "application/json");
    if is_kimi_coding {
        // Kimi Code's /coding/v1 endpoint authenticates with a standard Bearer
        // token and gates access by User-Agent whitelist.
        req = req
            .header("Authorization", format!("Bearer {api_key}"))
            .header("User-Agent", "KimiCLI/1.5");
    } else {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    }
    let resp = req
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
    let is_kimi_coding = is_kimi_coding_endpoint(provider);

    let thinking_budget: i64 = 10_000;
    let max_tokens = if use_reasoning && !is_kimi_coding {
        std::cmp::max(16_384, thinking_budget + 4_096)
    } else if is_kimi_coding {
        8192
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
    if use_reasoning && !is_kimi_coding {
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": thinking_budget
        });
    }

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json");
    if is_kimi_coding {
        // Kimi Code may gzip SSE streams; ask for identity to keep parsing simple.
        req = req
            .header("Authorization", format!("Bearer {api_key}"))
            .header("User-Agent", "KimiCLI/1.5")
            .header("Accept-Encoding", "identity");
    } else {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    }
    let resp = req
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

                    if let Some(data) = line.strip_prefix("data:") {
                        let data = data.trim_start();
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
        .timeout(REQUEST_TIMEOUT)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", "application/json")
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
            char_prefix(&text, 200)
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
            .timeout(REQUEST_TIMEOUT)
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
        || search_text.contains("kimi-k2")
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

pub fn kimi_known_models() -> Vec<AiModel> {
    vec![
        AiModel {
            id: "kimi-for-coding".to_string(),
            display_name: "Kimi for Coding".to_string(),
            capabilities: vec!["vision".to_string(), "reasoning".to_string(), "tool_calling".to_string()],
            context_length: Some(256_000),
            enabled: true,
            input_price_per_million: None,
            output_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
        },
    ]
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

/// Total-request timeout for non-streaming calls. Streaming calls only get the
/// connect timeout: their body legitimately takes as long as the generation
/// (reasoning models regularly exceed 2 minutes).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Process-wide shared client: reuses the connection pool (TCP + TLS sessions)
/// across requests instead of paying a fresh handshake per AI call, which
/// matters most for request bursts like batch analysis and embeddings.
fn build_client() -> Result<reqwest::Client, String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        // Per-read idle timeout: kills silently stalled connections without
        // capping total stream duration (long generations stay alive as long
        // as tokens keep arriving).
        .read_timeout(Duration::from_secs(180))
        .user_agent("Argus/0.1")
        // Some providers (notably Kimi Code's /coding endpoint) send SSE streams
        // that behave more reliably over HTTP/1.1.
        .http1_only()
        .build()
        .map_err(|e| e.to_string())?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

fn chat_content_text(content: &ChatContent) -> &str {
    match content {
        ChatContent::Text(s) => s.as_str(),
        ChatContent::Parts(_) => "",
    }
}

/// Convert our internal `ChatContent` into an Anthropic Messages API content
/// array. Text parts become `{type:"text"}`, images become `{type:"image"}`,
/// and PDF file parts become `{type:"document"}` with a base64 source.
fn to_anthropic_content(content: &ChatContent) -> Vec<serde_json::Value> {
    match content {
        ChatContent::Text(s) => {
            vec![serde_json::json!({"type": "text", "text": s})]
        }
        ChatContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ChatContentPart::Text { text } => {
                    Some(serde_json::json!({"type": "text", "text": text}))
                }
                ChatContentPart::ImageUrl { image_url } => {
                    let (media_type, data) = parse_data_uri(&image_url.url)?;
                    if !media_type.starts_with("image/") {
                        return None;
                    }
                    Some(serde_json::json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": media_type,
                            "data": data
                        }
                    }))
                }
                // Kimi Code's /coding endpoint accepts Anthropic image blocks but
                // does not support PDF document blocks, so drop file attachments.
                ChatContentPart::File { .. } => None,
            })
            .collect(),
    }
}

/// Parse a `data:<mime>;base64,<payload>` URI. Returns the media type and the
/// raw base64 payload. Non-data URIs are rejected.
fn parse_data_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(",")?;
    let media_type = meta.split(';').next().unwrap_or("application/octet-stream");
    Some((media_type.to_string(), payload.to_string()))
}

fn split_system(messages: &[ChatMessage]) -> (String, Vec<serde_json::Value>) {
    let system: String = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| chat_content_text(&m.content))
        .collect::<Vec<_>>()
        .join("\n");
    let conv: Vec<serde_json::Value> = messages
        .iter()
        .filter(|m| m.role != "system")
        .map(|m| {
            serde_json::json!({
                "role": m.role,
                "content": to_anthropic_content(&m.content)
            })
        })
        .collect();
    (system, conv)
}

/// First `n` characters of `s` — char-safe, never panics on UTF-8 boundaries.
/// (Plain `&s[..n]` byte-slicing panics when the cut lands mid-character, which
/// is common for non-ASCII error bodies / titles.)
fn char_prefix(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn friendly_error(status: u16, body: &str) -> String {
    let preview = char_prefix(body, 300);
    match status {
        401 => "Authentication failed (401). Check your API key in Settings → AI Services.".to_string(),
        403 => format!("Access denied (403). Your key may lack permission for this model. Response: {preview}"),
        404 => format!("Endpoint or model not found (404). Verify your API address and model ID. Response: {preview}"),
        429 => "Rate limited (429). Please wait a moment and try again.".to_string(),
        _ => format!("API error {status}: {preview}"),
    }
}
