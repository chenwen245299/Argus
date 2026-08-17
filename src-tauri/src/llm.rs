use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

/// Ollama's native REST API (as opposed to its OpenAI-compatible `/v1` shim).
/// Selected explicitly by the provider kind so users can pick between the two.
fn is_ollama(provider: &AiProvider) -> bool {
    provider.kind == "ollama"
}

pub fn is_deepseek(provider: &AiProvider) -> bool {
    provider.base_url.to_lowercase().contains("deepseek")
}

/// Server-side web search is a DeepSeek Responses-API feature; no other provider
/// wired up here exposes one, so the toggle is only offered for DeepSeek.
pub fn supports_web_search(provider: &AiProvider) -> bool {
    is_deepseek(provider)
}

/// Ollama's native endpoints live at `/api/*` off the server root. Accept a
/// base URL configured either as the bare root (`http://localhost:11434`) or
/// with a trailing OpenAI-compat `/v1` segment, and reduce it to the root so
/// `{root}/api/chat`, `{root}/api/embed`, `{root}/api/tags` resolve correctly.
fn ollama_root(provider: &AiProvider) -> String {
    let base = provider.base_url.trim_end_matches('/');
    base.strip_suffix("/v1")
        .unwrap_or(base)
        .trim_end_matches('/')
        .to_string()
}

pub async fn chat_completion(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    source: &str,
) -> Result<String, String> {
    if is_ollama(provider) {
        chat_ollama(provider, api_key, model, messages, source).await
    } else if is_anthropic_protocol(provider) {
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
    cancel: Option<Arc<AtomicBool>>,
    web_search: bool,
) -> Result<String, String> {
    // Web search lives on a different protocol (DeepSeek's Responses API), so it
    // takes its own path rather than adding a `tools` field to a request the
    // /chat/completions endpoint would ignore.
    if web_search && supports_web_search(provider) {
        return stream_deepseek_responses(
            provider,
            api_key,
            model,
            messages,
            event_name,
            app,
            use_reasoning,
            reasoning_effort,
            source,
            cancel,
        )
        .await;
    }
    if is_ollama(provider) {
        stream_ollama(
            provider,
            api_key,
            model,
            messages,
            event_name,
            app,
            use_reasoning,
            reasoning_effort,
            source,
            cancel,
        )
        .await
    } else if is_anthropic_protocol(provider) {
        stream_anthropic(
            provider,
            api_key,
            model,
            messages,
            event_name,
            app,
            use_reasoning,
            source,
            cancel,
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
            cancel,
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
    cancel: Option<Arc<AtomicBool>>,
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
        cancel,
    )
    .await
}

/// Fetch available models from the provider.
/// OpenAI-compatible: GET {base_url}/models
/// Anthropic: returns a hardcoded well-known list (no public /models endpoint).
pub async fn list_models(provider: &AiProvider, api_key: &str) -> Result<Vec<AiModel>, String> {
    if is_ollama(provider) {
        // Ollama exposes locally-pulled models via GET /api/tags, and per-model
        // capabilities via POST /api/show.
        return fetch_ollama_models(provider, api_key).await;
    }
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

    // Ollama's native API has no /chat/completions; probe /api/tags instead,
    // which also confirms the local server is reachable.
    if is_ollama(provider) {
        let url = format!("{}/api/tags", ollama_root(provider));
        let mut req = client.get(&url).timeout(REQUEST_TIMEOUT);
        if !api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {api_key}"));
        }
        let resp = req.send().await.map_err(|e| {
            format!("Network error: {e}. Is Ollama running at {}?", ollama_root(provider))
        })?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if status >= 400 {
            return Err(friendly_error(status, &text));
        }
        let count = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|j| j["models"].as_array().map(|a| a.len()))
            .unwrap_or(0);
        return Ok(format!(
            "Connected to Ollama at {} ({count} local model(s)).",
            ollama_root(provider)
        ));
    }

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
        // No hardcoded fallback: without a configured model we cannot know which
        // model id this provider accepts. The UI blocks this case, but guard here
        // too so a missing model surfaces as a clear message instead of silently
        // probing with an unrelated default (which providers reject as invalid).
        provider
            .models
            .first()
            .map(|m| m.id.as_str())
            .ok_or("No model configured for this provider. Add and select a model before testing the connection.")?
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
    if is_ollama(provider) {
        return embed_ollama(provider, api_key, model, texts, source).await;
    }
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
    cancel: Option<Arc<AtomicBool>>,
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
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut usage_emitted = false;

    while let Some(chunk) = stream.next().await {
        // Backend cancellation: if the user pressed stop, break out of the loop.
        // Dropping `stream`/`resp` on scope exit closes the HTTP connection so the
        // provider stops generating (and billing). Return the partial text.
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        byte_buf.extend_from_slice(&bytes);
        // Decode only up to the last complete UTF-8 boundary; keep the trailing
        // incomplete bytes (a multi-byte char split across chunks) for next round.
        let valid_up_to = match std::str::from_utf8(&byte_buf) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            // Safe: bytes[..valid_up_to] is guaranteed valid UTF-8.
            buf.push_str(unsafe { std::str::from_utf8_unchecked(&byte_buf[..valid_up_to]) });
            byte_buf.drain(..valid_up_to);
        }

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
                                    cache_hit_tokens,
                                );
                            }
                            crate::token_usage::record_full(
                                source,
                                &provider.id,
                                model,
                                input_tokens,
                                output_tokens,
                                cost_usd,
                                cache_hit_tokens,
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
                                if let Some(v) = usage["prompt_cache_hit_tokens"].as_u64() {
                                    // DeepSeek reports cache hits here.
                                    cache_hit_tokens = v;
                                } else if let Some(v) =
                                    usage["prompt_tokens_details"]["cached_tokens"].as_u64()
                                {
                                    // OpenAI / Kimi / OpenRouter report them here.
                                    cache_hit_tokens = v;
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
                                    cache_hit_tokens,
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

    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        cost_usd,
        cache_hit_tokens,
    );
    if !usage_emitted {
        emit_stream_usage(
            app,
            event_name,
            input_tokens,
            output_tokens,
            input_tokens.saturating_add(output_tokens),
            cost_usd,
            cache_hit_tokens,
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
    let cache_hit_tokens = json["usage"]["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64())
        .unwrap_or(0);
    let cost_usd = if is_openrouter || is_kimi {
        usage_cost_usd(&json["usage"])
    } else {
        None
    };
    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        cost_usd,
        cache_hit_tokens,
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
    cancel: Option<Arc<AtomicBool>>,
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
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut usage_emitted = false;

    while let Some(chunk) = stream.next().await {
        // Backend cancellation: if the user pressed stop, break out of the loop.
        // Dropping `stream`/`resp` on scope exit closes the HTTP connection so the
        // provider stops generating (and billing). Return the partial text.
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        byte_buf.extend_from_slice(&bytes);
        // Decode only up to the last complete UTF-8 boundary; keep the trailing
        // incomplete bytes (a multi-byte char split across chunks) for next round.
        let valid_up_to = match std::str::from_utf8(&byte_buf) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            buf.push_str(unsafe { std::str::from_utf8_unchecked(&byte_buf[..valid_up_to]) });
            byte_buf.drain(..valid_up_to);
        }

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
                                    cache_hit_tokens,
                                );
                            }
                            crate::token_usage::record_full(
                                source,
                                &provider.id,
                                model,
                                input_tokens,
                                output_tokens,
                                if is_openrouter || is_kimi { cost_usd } else { None },
                                cache_hit_tokens,
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
                                if let Some(v) = usage["prompt_cache_hit_tokens"].as_u64() {
                                    // DeepSeek reports cache hits here.
                                    cache_hit_tokens = v;
                                } else if let Some(v) =
                                    usage["prompt_tokens_details"]["cached_tokens"].as_u64()
                                {
                                    // OpenAI / Kimi / OpenRouter report them here.
                                    cache_hit_tokens = v;
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
                                    cache_hit_tokens,
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

    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        if is_openrouter || is_kimi { cost_usd } else { None },
        cache_hit_tokens,
    );
    if !usage_emitted {
        emit_stream_usage(
            app,
            event_name,
            input_tokens,
            output_tokens,
            input_tokens.saturating_add(output_tokens),
            if is_openrouter || is_kimi { cost_usd } else { None },
            cache_hit_tokens,
        );
    }
    let _ = app.emit(event_name, serde_json::json!({"delta":"","done":true}));
    Ok(accumulated)
}

// ── DeepSeek Responses API (server-side web search) ───────────────────────────
//
// DeepSeek exposes its built-in web search only through the Responses API
// (`POST {base}/responses`), not through /chat/completions — so enabling the
// toggle switches protocol, not just a request field. Differences that matter:
//   * system messages become the top-level `instructions` string;
//   * the remaining turns go in `input` as `{role, content}` items;
//   * image/PDF parts are NOT supported here, so content is flattened to text;
//   * the SSE stream carries typed events, not `choices[].delta`.
// Unsupported request fields are documented as silently ignored, so sending the
// OpenAI-shaped `reasoning.effort` is safe even where DeepSeek's own naming
// differs.

/// Flatten a message to plain text: the Responses API rejects nothing here, but
/// DeepSeek ignores image/file input, so only the text parts carry meaning.
fn responses_input_text(m: &ChatMessage) -> String {
    match &m.content {
        ChatContent::Text(s) => s.clone(),
        ChatContent::Parts(parts) => parts
            .iter()
            .filter_map(|p| match p {
                ChatContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream_deepseek_responses(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/responses", provider.base_url.trim_end_matches('/'));

    let mut instructions = String::new();
    let mut input: Vec<serde_json::Value> = Vec::new();
    for m in messages {
        let text = responses_input_text(m);
        if m.role == "system" {
            if text.trim().is_empty() {
                continue;
            }
            if !instructions.is_empty() {
                instructions.push_str("\n\n");
            }
            instructions.push_str(&text);
        } else {
            input.push(serde_json::json!({ "role": m.role, "content": text }));
        }
    }

    let mut body = serde_json::json!({
        "model": model,
        "input": input,
        "tools": [{ "type": "web_search" }],
        "stream": true,
    });
    if !instructions.is_empty() {
        body["instructions"] = serde_json::json!(instructions);
    }
    if use_reasoning {
        body["reasoning"] = serde_json::json!({ "effort": reasoning_effort.unwrap_or("high") });
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
    // Search progress drives a "searching the web…" indicator in the composer.
    let search_event = format!("{event_name}-websearch");
    let mut stream = resp.bytes_stream();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut usage_emitted = false;

    while let Some(chunk) = stream.next().await {
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        byte_buf.extend_from_slice(&bytes);
        let valid_up_to = match std::str::from_utf8(&byte_buf) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            buf.push_str(unsafe { std::str::from_utf8_unchecked(&byte_buf[..valid_up_to]) });
            byte_buf.drain(..valid_up_to);
        }

        loop {
            let Some(pos) = buf.find('\n') else { break };
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..pos + 1);

            // `event:` lines are ignored: the type is repeated inside the JSON,
            // which is the one place it is guaranteed to be.
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim_start();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            match json["type"].as_str().unwrap_or("") {
                "response.output_text.delta" => {
                    if let Some(delta) = json["delta"].as_str().filter(|s| !s.is_empty()) {
                        accumulated.push_str(delta);
                        let _ = app
                            .emit(event_name, serde_json::json!({"delta": delta, "done": false}));
                    }
                }
                "response.reasoning_text.delta" => {
                    if let Some(delta) = json["delta"].as_str().filter(|s| !s.is_empty()) {
                        let _ = app.emit(
                            &reasoning_event,
                            serde_json::json!({"delta": delta, "done": false}),
                        );
                    }
                }
                t @ ("response.web_search_call.in_progress"
                | "response.web_search_call.searching"
                | "response.web_search_call.completed") => {
                    let phase = t.rsplit('.').next().unwrap_or("searching");
                    let _ = app.emit(&search_event, serde_json::json!({ "status": phase }));
                }
                "response.failed" | "response.incomplete" => {
                    let msg = json["response"]["error"]["message"]
                        .as_str()
                        .or_else(|| json["response"]["incomplete_details"]["reason"].as_str())
                        .unwrap_or("response did not complete");
                    // Partial text is worth keeping, so a late failure is only an
                    // error when nothing was produced at all.
                    if accumulated.is_empty() {
                        return Err(format!("DeepSeek: {msg}"));
                    }
                }
                "response.completed" => {
                    let usage = &json["response"]["usage"];
                    input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
                    output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                    cache_hit_tokens = usage["input_tokens_details"]["cached_tokens"]
                        .as_u64()
                        .unwrap_or(0);
                    let total = usage["total_tokens"]
                        .as_u64()
                        .unwrap_or_else(|| input_tokens.saturating_add(output_tokens));
                    emit_stream_usage(
                        app,
                        event_name,
                        input_tokens,
                        output_tokens,
                        total,
                        None,
                        cache_hit_tokens,
                    );
                    usage_emitted = true;
                }
                _ => {}
            }
        }
    }

    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        None,
        cache_hit_tokens,
    );
    if !usage_emitted {
        emit_stream_usage(
            app,
            event_name,
            input_tokens,
            output_tokens,
            input_tokens.saturating_add(output_tokens),
            None,
            cache_hit_tokens,
        );
    }
    let _ = app.emit(&search_event, serde_json::json!({ "status": "done" }));
    let _ = app.emit(event_name, serde_json::json!({"delta":"","done":true}));
    Ok(accumulated)
}

// ── Ollama native (/api/chat, /api/embed, /api/tags) ──────────────────────────

/// Convert our internal `ChatMessage` into an Ollama chat message. Ollama takes
/// a plain-string `content` plus a separate `images` array of **raw base64**
/// strings (no `data:` prefix). PDF `file` parts have no native Ollama block and
/// are dropped (vision models can't ingest PDFs directly).
fn to_ollama_message(m: &ChatMessage) -> serde_json::Value {
    match &m.content {
        ChatContent::Text(s) => serde_json::json!({"role": m.role, "content": s}),
        ChatContent::Parts(parts) => {
            let mut text = String::new();
            let mut images: Vec<String> = Vec::new();
            for part in parts {
                match part {
                    ChatContentPart::Text { text: t } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(t);
                    }
                    ChatContentPart::ImageUrl { image_url } => {
                        // Ollama wants the raw base64 payload. Strip a data URI
                        // wrapper when present; otherwise pass through anything
                        // that isn't a remote URL (Ollama can't fetch http(s)).
                        if let Some((media, data)) = parse_data_uri(&image_url.url) {
                            if media.starts_with("image/") {
                                images.push(data);
                            }
                        } else if !image_url.url.starts_with("http") {
                            images.push(image_url.url.clone());
                        }
                    }
                    ChatContentPart::File { .. } => {}
                }
            }
            let mut obj = serde_json::json!({"role": m.role, "content": text});
            if !images.is_empty() {
                obj["images"] = serde_json::json!(images);
            }
            obj
        }
    }
}

/// Non-streaming Ollama chat completion. Returns the full response text.
async fn chat_ollama(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    source: &str,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/api/chat", ollama_root(provider));
    let msgs: Vec<serde_json::Value> = messages.iter().map(to_ollama_message).collect();
    let body = serde_json::json!({"model": model, "messages": msgs, "stream": false});

    let mut req = client
        .post(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Content-Type", "application/json");
    // Ollama is usually keyless (local), but Ollama Cloud / an auth proxy accepts
    // a bearer token — send it when the user configured one.
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {api_key}"));
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}. Is Ollama running at {}?", ollama_root(provider)))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from Ollama: {e}"))?;

    let input_tokens = json["prompt_eval_count"].as_u64().unwrap_or(0);
    let output_tokens = json["eval_count"].as_u64().unwrap_or(0);
    crate::token_usage::record_with_cost(source, &provider.id, model, input_tokens, output_tokens, None);

    json["message"]["content"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Unexpected response format from Ollama".to_string())
}

/// Streaming Ollama chat completion. Ollama streams **newline-delimited JSON**
/// (one full object per line — no `data:` prefix, no `[DONE]` sentinel). Each
/// object carries `message.content` (answer) and, for thinking models,
/// `message.thinking` (reasoning); the final object has `done:true` plus token
/// stats (`prompt_eval_count`, `eval_count`). `message.tool_calls` (when the
/// model requests a tool) is accumulated and emitted on `{event_name}-tools`.
#[allow(clippy::too_many_arguments)]
async fn stream_ollama(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[ChatMessage],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/api/chat", ollama_root(provider));
    let msgs: Vec<serde_json::Value> = messages.iter().map(to_ollama_message).collect();

    let mut body = serde_json::json!({"model": model, "messages": msgs, "stream": true});
    // Thinking control. Ollama's `think` accepts a level string ("low"/"medium"/
    // "high") for all thinking-capable models (and gpt-oss *requires* a level
    // rather than a boolean), so send the effort level. Only send it when the
    // user enabled reasoning — passing `think` to a non-thinking model errors.
    if use_reasoning {
        body["think"] = serde_json::json!(reasoning_effort.unwrap_or("high"));
    }

    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json");
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {api_key}"));
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}. Is Ollama running at {}?", ollama_root(provider)))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let tools_event = format!("{event_name}-tools");
    let mut stream = resp.bytes_stream();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut tool_calls: Vec<serde_json::Value> = Vec::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;

    while let Some(chunk) = stream.next().await {
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        byte_buf.extend_from_slice(&bytes);
        // Decode up to the last complete UTF-8 boundary; keep trailing partial
        // bytes (a multi-byte char split across chunks) for the next round.
        let valid_up_to = match std::str::from_utf8(&byte_buf) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            buf.push_str(unsafe { std::str::from_utf8_unchecked(&byte_buf[..valid_up_to]) });
            byte_buf.drain(..valid_up_to);
        }

        loop {
            let Some(pos) = buf.find('\n') else { break };
            let line = buf[..pos].trim().to_string();
            buf.drain(..pos + 1);
            if line.is_empty() {
                continue;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };

            // Answer delta.
            if let Some(delta) = json["message"]["content"].as_str().filter(|s| !s.is_empty()) {
                accumulated.push_str(delta);
                let _ = app.emit(event_name, serde_json::json!({"delta": delta, "done": false}));
            }
            // Reasoning/thinking delta.
            if let Some(r) = json["message"]["thinking"].as_str().filter(|s| !s.is_empty()) {
                let _ = app.emit(&reasoning_event, serde_json::json!({"delta": r, "done": false}));
            }
            // Tool calls (accumulate — arguments are already a JSON object).
            if let Some(calls) = json["message"]["tool_calls"].as_array() {
                for c in calls {
                    tool_calls.push(c.clone());
                }
            }

            if json["done"].as_bool().unwrap_or(false) {
                input_tokens = json["prompt_eval_count"].as_u64().unwrap_or(input_tokens);
                output_tokens = json["eval_count"].as_u64().unwrap_or(output_tokens);
                break;
            }
        }
    }

    if !tool_calls.is_empty() {
        let _ = app.emit(tools_event.as_str(), serde_json::json!({"tool_calls": tool_calls}));
    }
    crate::token_usage::record_with_cost(source, &provider.id, model, input_tokens, output_tokens, None);
    emit_stream_usage(
        app,
        event_name,
        input_tokens,
        output_tokens,
        input_tokens.saturating_add(output_tokens),
        None,
        0,
    );
    let _ = app.emit(event_name, serde_json::json!({"delta": "", "done": true}));
    Ok(accumulated)
}

/// Embed texts via Ollama's POST /api/embed (batch `input` array). Returns one
/// vector per input, in order. L2-normalized unit vectors per Ollama's docs.
async fn embed_ollama(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    texts: &[String],
    source: &str,
) -> Result<Vec<Vec<f32>>, String> {
    let client = build_client()?;
    let url = format!("{}/api/embed", ollama_root(provider));
    let body = serde_json::json!({"model": model, "input": texts});

    let mut req = client
        .post(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Content-Type", "application/json");
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {api_key}"));
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}. Is Ollama running at {}?", ollama_root(provider)))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| format!("Invalid JSON from Ollama /api/embed: {e}"))?;

    let rows = json["embeddings"].as_array().ok_or_else(|| {
        format!(
            "No 'embeddings' array in Ollama response: {}",
            char_prefix(&text, 200)
        )
    })?;
    let mut vecs: Vec<Vec<f32>> = Vec::with_capacity(rows.len());
    for row in rows {
        let v: Vec<f32> = row
            .as_array()
            .map(|arr| arr.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect())
            .unwrap_or_default();
        if v.is_empty() {
            return Err("Empty embedding vector from Ollama — check the model name.".to_string());
        }
        vecs.push(v);
    }

    let total_tokens = json["prompt_eval_count"].as_u64().unwrap_or(0);
    crate::token_usage::record(source, &provider.id, model, total_tokens, 0);
    Ok(vecs)
}

/// List locally-available Ollama models (GET /api/tags) and enrich each with its
/// capabilities from POST /api/show (vision / tools / thinking / embedding),
/// mapped onto Argus's canonical capability tags.
async fn fetch_ollama_models(provider: &AiProvider, api_key: &str) -> Result<Vec<AiModel>, String> {
    let client = build_client()?;
    let root = ollama_root(provider);
    let mut req = client.get(format!("{root}/api/tags")).timeout(REQUEST_TIMEOUT);
    if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {api_key}"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("Network error: {e}. Is Ollama running at {root}?"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from /api/tags: {e}"))?;
    let list = json["models"].as_array().cloned().unwrap_or_default();

    // Fetch capabilities concurrently via /api/show, keeping input order. Each
    // future owns its data ('static) — borrowing an iterator item across
    // `buffered` trips rustc's higher-ranked lifetime inference (see embed_openrouter).
    let names: Vec<String> = list
        .iter()
        .map(|m| m["name"].as_str().unwrap_or_default().to_string())
        .collect();
    let shows = names.into_iter().map(|name| {
        let client = client.clone();
        let root = root.clone();
        let api_key = api_key.to_string();
        async move {
            let mut req = client
                .post(format!("{root}/api/show"))
                .timeout(REQUEST_TIMEOUT)
                .header("Content-Type", "application/json");
            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {api_key}"));
            }
            let caps = match req.json(&serde_json::json!({"model": name})).send().await {
                Ok(r) => r
                    .text()
                    .await
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .map(|j| j["capabilities"].clone())
                    .unwrap_or(serde_json::Value::Null),
                Err(_) => serde_json::Value::Null,
            };
            caps
        }
    });
    let caps_list: Vec<serde_json::Value> = futures::stream::iter(shows)
        .buffered(6)
        .collect::<Vec<_>>()
        .await;

    let models = list
        .iter()
        .zip(caps_list.into_iter())
        .filter_map(|(item, caps_json)| {
            let id = item["name"].as_str()?.to_string();
            let mut caps: Vec<String> = Vec::new();
            if let Some(arr) = caps_json.as_array() {
                for c in arr {
                    match c.as_str().unwrap_or("").to_lowercase().as_str() {
                        "vision" => add_capability(&mut caps, "vision"),
                        "tools" => add_capability(&mut caps, "tool_calling"),
                        "thinking" => add_capability(&mut caps, "reasoning"),
                        "embedding" => add_capability(&mut caps, "embedding"),
                        _ => {}
                    }
                }
            }
            // Fall back to name-based heuristics when /api/show gave nothing.
            let lower = id.to_lowercase();
            if caps.is_empty() {
                if looks_like_embedding_model(&lower) {
                    add_capability(&mut caps, "embedding");
                }
                if looks_like_reasoning_model(&lower) {
                    add_capability(&mut caps, "reasoning");
                }
            }
            let context_length = item["details"]["context_length"].as_u64();
            let id_for_size = id.clone();
            Some(AiModel {
                id: id.clone(),
                display_name: id,
                capabilities: caps,
                context_length,
                enabled: true,
                input_price_per_million: None,
                output_price_per_million: None,
                peak_pricing: false,
                peak_input_price_per_million: None,
                peak_output_price_per_million: None,
                cache_hit_input_price_per_million: None,
                input_price_usd_per_million: None,
                output_price_usd_per_million: None,
                provider_order: vec![],
                // Ollama tags carry the size directly ("qwen3.6:35b").
                param_billions: scan_param_size(&id_for_size),
                // Local models: free in the sense that matters, but the tag is
                // about a provider's price list and Ollama has none.
                is_free: false,
                discount_percent: None,
                discount_windows: vec![],
            })
        })
        .collect();

    Ok(models)
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
    let is_kimi_coding = is_kimi_coding_endpoint(provider);
    // Only real Anthropic gets cache_control breakpoints. The Kimi coding
    // endpoint speaks the same protocol but may not accept the structured
    // system-block form, so it stays on the plain-string path.
    let (system, conv) = split_system_cached(messages, provider.kind == "anthropic");
    // Kimi Code allows larger output windows; Anthropic defaults stay conservative.
    let max_tokens: i64 = if is_kimi_coding { 8192 } else { 4096 };
    let mut body = serde_json::json!({"model": model, "max_tokens": max_tokens, "messages": conv});
    if !system.is_null() {
        body["system"] = system;
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

    let base_input = json["usage"]["input_tokens"].as_u64().unwrap_or(0);
    let cache_read = json["usage"]["cache_read_input_tokens"].as_u64().unwrap_or(0);
    let cache_write = json["usage"]["cache_creation_input_tokens"].as_u64().unwrap_or(0);
    let input_tokens = base_input + cache_read + cache_write;
    let output_tokens = json["usage"]["output_tokens"].as_u64().unwrap_or(0);
    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        None,
        cache_read,
    );

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
    cancel: Option<Arc<AtomicBool>>,
) -> Result<String, String> {
    let client = build_client()?;
    let url = format!("{}/messages", provider.base_url.trim_end_matches('/'));
    let is_kimi_coding = is_kimi_coding_endpoint(provider);
    let (system, conv) = split_system_cached(messages, provider.kind == "anthropic");

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
    if !system.is_null() {
        body["system"] = system;
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
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    // Anthropic reports cache hits separately from `input_tokens`.
    let mut cache_read: u64 = 0;

    while let Some(chunk) = stream.next().await {
        // Backend cancellation: if the user pressed stop, break out of the loop.
        // Dropping `stream`/`resp` on scope exit closes the HTTP connection so the
        // provider stops generating (and billing). Return the partial text.
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        byte_buf.extend_from_slice(&bytes);
        // Decode only up to the last complete UTF-8 boundary; keep the trailing
        // incomplete bytes (a multi-byte char split across chunks) for next round.
        let valid_up_to = match std::str::from_utf8(&byte_buf) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            buf.push_str(unsafe { std::str::from_utf8_unchecked(&byte_buf[..valid_up_to]) });
            byte_buf.drain(..valid_up_to);
        }

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
                                    let u = &json["message"]["usage"];
                                    let base = u["input_tokens"].as_u64().unwrap_or(0);
                                    cache_read = u["cache_read_input_tokens"].as_u64().unwrap_or(0);
                                    let cache_write =
                                        u["cache_creation_input_tokens"].as_u64().unwrap_or(0);
                                    // Count cached + freshly-written input in the total so the
                                    // hit ratio (cache_read / input) reflects the real prompt.
                                    input_tokens = base + cache_read + cache_write;
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
                                        cache_read,
                                    );
                                    crate::token_usage::record_full(
                                        source,
                                        &provider.id,
                                        model,
                                        input_tokens,
                                        output_tokens,
                                        None,
                                        cache_read,
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

    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        None,
        cache_read,
    );
    emit_stream_usage(
        app,
        event_name,
        input_tokens,
        output_tokens,
        input_tokens.saturating_add(output_tokens),
        None,
        cache_read,
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
    cache_hit_tokens: u64,
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
            // Cached (prompt-cache-hit) input tokens, when the provider reports
            // them (e.g. DeepSeek). Used to estimate cost at the cheaper cache rate.
            "cache_hit_tokens": cache_hit_tokens,
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
    let is_free = quotes_free(&item["pricing"]);
    let param_billions = parse_param_billions(item);
    let (discount_percent, discount_windows) = parse_time_discount(&item["pricing"]);
    Some(AiModel {
        id: id.to_string(),
        display_name,
        capabilities,
        context_length,
        enabled: true,
        input_price_per_million: None,
        output_price_per_million: None,
        peak_pricing: false,
        peak_input_price_per_million: None,
        peak_output_price_per_million: None,
        cache_hit_input_price_per_million: None,
        input_price_usd_per_million,
        output_price_usd_per_million,
        provider_order: vec![],
        is_free,
        param_billions,
        discount_percent,
        discount_windows,
    })
}

/// Whether the catalogue quotes nothing for either direction.
///
/// Both must be zero. A model that is free to read but charges to generate is
/// not free, and labelling it so would be the expensive kind of wrong.
fn quotes_free(pricing: &serde_json::Value) -> bool {
    let zero = |v: &serde_json::Value| {
        parse_price_usd_per_million(v).is_some_and(|p| p == 0.0)
    };
    zero(&pricing["prompt"]) && zero(&pricing["completion"])
}

/// A standing time-of-day discount, if the catalogue advertises one.
///
/// OpenRouter puts two unrelated things in `pricing.overrides`:
///
/// - `min_prompt_tokens` entries, which *raise* the price above a context
///   threshold. At the time of writing 64 of 414 models carry one of these and
///   every one is a surcharge. Reading them as discounts would tag the most
///   expensive long-context models as bargains.
/// - `utc_start` / `utc_end` entries (HHMM), which is how off-peak pricing is
///   expressed. Only these are considered, and only when they are cheaper than
///   the base rate.
fn parse_time_discount(pricing: &serde_json::Value) -> (Option<u32>, Vec<[u32; 2]>) {
    let Some(base) = parse_price_usd_per_million(&pricing["prompt"]).filter(|p| *p > 0.0) else {
        return (None, Vec::new());
    };
    let Some(overrides) = pricing["overrides"].as_array() else {
        return (None, Vec::new());
    };

    let mut windows = Vec::new();
    let mut deepest = 0u32;
    for entry in overrides {
        let (Some(start), Some(end)) = (entry["utc_start"].as_u64(), entry["utc_end"].as_u64())
        else {
            continue; // a size-based surcharge, not a schedule
        };
        let Some(discounted) = parse_price_usd_per_million(&entry["prompt"]) else {
            continue;
        };
        if discounted >= base {
            continue; // the peak-rate half of the schedule
        }
        let percent = (((base - discounted) / base) * 100.0).round() as u32;
        if percent == 0 {
            continue;
        }
        deepest = deepest.max(percent);
        windows.push([start as u32, end as u32]);
    }

    if windows.is_empty() {
        (None, Vec::new())
    } else {
        (Some(deepest.min(100)), windows)
    }
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

/// The promotional discount OpenRouter is currently running on a model.
///
/// `Ok(Some(percent))` when a promotion is running, `Ok(None)` when the
/// catalogue was read and says there is none, and `Err` when it could not be
/// read at all. The caller has to be able to tell those last two apart: writing
/// a failed lookup to disk as "no promotion" is how a working discount silently
/// loses its badge because the network blinked.
///
/// **This is not in the bulk `/models` list.** At the time of writing, zero of
/// its 414 entries carry `pricing.discount`; it appears only per endpoint under
/// `/models/{id}/endpoints`. That is why the first version of this feature saw
/// no discounts at all — it was reading a field that endpoint never sends.
///
/// A model is served by several endpoints at different prices and different
/// discounts (`gpt-5.6-luna-pro`: 50% off via OpenAI, nothing via Azure). The
/// one that matters is the endpoint whose price is the one being displayed, so
/// `quoted_prompt_usd_per_million` selects it. Reporting the best discount
/// across all endpoints would advertise a rate the user's requests may never be
/// billed at.
pub async fn fetch_openrouter_discount(
    provider: &AiProvider,
    api_key: &str,
    model_id: &str,
    quoted_prompt_usd_per_million: Option<f64>,
) -> Result<Option<u32>, String> {
    let client = build_client().map_err(|e| e.to_string())?;
    let url = format!(
        "{}/models/{model_id}/endpoints",
        provider.base_url.trim_end_matches('/')
    );
    let resp = client
        .get(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| format!("{model_id}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{model_id}: HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("{model_id}: malformed response: {e}"))?;
    Ok(discount_of_quoted_endpoint(
        &json["data"]["endpoints"],
        quoted_prompt_usd_per_million,
    ))
}

/// Pick the endpoint being quoted and read its discount. Split out so the
/// selection rule is testable against real catalogue shapes.
fn discount_of_quoted_endpoint(
    endpoints: &serde_json::Value,
    quoted_prompt_usd_per_million: Option<f64>,
) -> Option<u32> {
    let list = endpoints.as_array()?;
    let priced = |e: &serde_json::Value| parse_price_usd_per_million(&e["pricing"]["prompt"]);

    let chosen = quoted_prompt_usd_per_million
        .and_then(|quoted| {
            list.iter().find(|e| {
                // Float equality through two string round-trips; compare with a
                // relative tolerance rather than `==`.
                priced(e).is_some_and(|p| (p - quoted).abs() <= quoted.abs() * 1e-6 + 1e-9)
            })
        })
        // Endpoints arrive in OpenRouter's own routing order, so the first is
        // the sensible guess when the quoted price matches nothing.
        .or_else(|| list.first())?;

    let fraction = chosen["pricing"]["discount"]
        .as_f64()
        .or_else(|| chosen["pricing"]["discount"].as_str()?.parse().ok())?;
    if !(fraction.is_finite() && fraction > 0.0) {
        return None;
    }
    let percent = (fraction * 100.0).round() as u32;
    (percent > 0 && percent < 100).then_some(percent)
}

/// Parameter count in billions, dug out of whatever the catalogue says.
///
/// No provider publishes this as a field, so it comes from the naming
/// (`nemotron-3-embed-1b`, `qwen3.8-2.4t-a95b`) and, failing that, the prose
/// description. That reaches about a third of OpenRouter's catalogue; the rest
/// are closed models whose size is simply not public, and `None` says so.
pub fn parse_param_billions(item: &serde_json::Value) -> Option<f64> {
    let named = [
        item["id"].as_str(),
        item["name"].as_str(),
        item["canonical_slug"].as_str(),
        item["hugging_face_id"].as_str(),
    ];
    for text in named.into_iter().flatten() {
        if let Some(v) = scan_param_size(text) {
            return Some(v);
        }
    }
    // Descriptions are prose and can mention other numbers, so they are the
    // last resort rather than the first.
    let description = item["description"].as_str()?;
    scan_param_size(&description.chars().take(600).collect::<String>())
}

/// Largest plausible `<number><unit>` in `text`, in billions.
///
/// Largest, not first: a mixture-of-experts model is named for its total *and*
/// its active parameters (`550b-a55b`), and the total is the size people mean.
fn scan_param_size(text: &str) -> Option<f64> {
    let lower = text.to_lowercase().replace(":free", " ");
    let bytes: Vec<char> = lower.chars().collect();
    let mut best: Option<f64> = None;
    let mut i = 0usize;

    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // A digit preceded by a letter, digit or dot is part of a version or an
        // identifier ("qwen3.8", "gpt-5.6"), not a size.
        if i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == '.') {
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == '.') {
            i += 1;
        }
        let number: String = bytes[start..i].iter().collect();
        let Some(unit) = bytes.get(i).copied() else { break };
        let multiplier = match unit {
            'm' => 0.001,
            'b' => 1.0,
            't' => 1000.0,
            _ => continue,
        };
        // The unit has to end the token: "3ba" is not three billion.
        if bytes.get(i + 1).is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let Ok(value) = number.parse::<f64>() else {
            continue;
        };
        let billions = value * multiplier;
        // Bounds keep years, context sizes and prices out.
        if (0.05..=100_000.0).contains(&billions) {
            best = Some(best.map_or(billions, |b: f64| b.max(billions)));
        }
    }
    best
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
            peak_pricing: false,
            peak_input_price_per_million: None,
            peak_output_price_per_million: None,
            cache_hit_input_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
            param_billions: None,
            is_free: false,
            discount_percent: None,
            discount_windows: vec![],
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
            peak_pricing: false,
            peak_input_price_per_million: None,
            peak_output_price_per_million: None,
            cache_hit_input_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
            param_billions: None,
            is_free: false,
            discount_percent: None,
            discount_windows: vec![],
        },
        AiModel {
            id: "claude-sonnet-4-5".to_string(),
            display_name: "Claude Sonnet 4.5".to_string(),
            capabilities: vec!["vision".to_string()],
            context_length: Some(200_000),
            enabled: true,
            input_price_per_million: None,
            output_price_per_million: None,
            peak_pricing: false,
            peak_input_price_per_million: None,
            peak_output_price_per_million: None,
            cache_hit_input_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
            param_billions: None,
            is_free: false,
            discount_percent: None,
            discount_windows: vec![],
        },
        AiModel {
            id: "claude-haiku-4-5-20251001".to_string(),
            display_name: "Claude Haiku 4.5".to_string(),
            capabilities: vec!["vision".to_string()],
            context_length: Some(200_000),
            enabled: true,
            input_price_per_million: None,
            output_price_per_million: None,
            peak_pricing: false,
            peak_input_price_per_million: None,
            peak_output_price_per_million: None,
            cache_hit_input_price_per_million: None,
            input_price_usd_per_million: None,
            output_price_usd_per_million: None,
            provider_order: vec![],
            param_billions: None,
            is_free: false,
            discount_percent: None,
            discount_windows: vec![],
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

/// Split system and conversation messages for the Anthropic Messages API.
/// When `enable_cache` is set the system prompt is the system prompt is
/// emitted as an array of text blocks (one per system message) with an
/// `ephemeral` cache_control breakpoint on the FIRST block. Callers put the
/// large, stable "paper context" block first, so Anthropic serves that prefix
/// from its prompt cache on repeat calls instead of re-billing the paper text.
/// Returns `Value::Null` when there is no system content, an array when caching
/// is enabled, or a plain string otherwise (unchanged behavior).
fn split_system_cached(
    messages: &[ChatMessage],
    enable_cache: bool,
) -> (serde_json::Value, Vec<serde_json::Value>) {
    let sys: Vec<&str> = messages
        .iter()
        .filter(|m| m.role == "system")
        .map(|m| chat_content_text(&m.content))
        .collect();
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

    let system = if sys.is_empty() {
        serde_json::Value::Null
    } else if enable_cache {
        let blocks: Vec<serde_json::Value> = sys
            .iter()
            .enumerate()
            .map(|(i, text)| {
                if i == 0 {
                    serde_json::json!({
                        "type": "text",
                        "text": text,
                        "cache_control": { "type": "ephemeral" }
                    })
                } else {
                    serde_json::json!({ "type": "text", "text": text })
                }
            })
            .collect();
        serde_json::Value::Array(blocks)
    } else {
        serde_json::Value::String(sys.join("\n"))
    };
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

// ── Tool calling ─────────────────────────────────────────────────────────────
//
// Used by the library Q&A agent mode. Deliberately non-streaming: the agent
// loop needs the *complete* set of tool calls before it can run them, and
// reconstructing them from three different streaming dialects (OpenAI-compat,
// Anthropic, Ollama) would be a lot of parser surface for no user-visible gain.
// The user sees progress through per-tool events instead, and the final answer
// is streamed by the normal path once the tools are done.

/// One tool invocation the model asked for.
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// Provider-assigned id, echoed back with the result so the model can match
    /// them up when it requested several at once.
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// What one round of an agent loop cost.
///
/// Reported back to the caller instead of emitted, because an agent answer is
/// several rounds and the user is owed their sum, not the last one's figures.
#[derive(Debug, Default, Clone, Copy)]
pub struct TurnUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cost_usd: Option<f64>,
}

impl TurnUsage {
    /// Fold another round in. `cost_usd` stays `None` until some round reports
    /// one, so "the provider never told us" does not become "it was free".
    pub fn add(&mut self, other: &TurnUsage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_hit_tokens = self.cache_hit_tokens.saturating_add(other.cache_hit_tokens);
        if let Some(c) = other.cost_usd {
            *self.cost_usd.get_or_insert(0.0) += c;
        }
    }
}

/// What the model returned when it had tools available.
#[derive(Debug, Default)]
pub struct ToolTurn {
    /// Prose the model emitted alongside its tool calls, if any.
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: TurnUsage,
}

/// Send a usage figure to the front-end for `event_name`.
///
/// Public so the agent loop can report the total for a multi-round answer; every
/// single-request path emits its own from inside `llm`.
pub fn emit_usage(app: &tauri::AppHandle, event_name: &str, usage: &TurnUsage) {
    emit_stream_usage(
        app,
        event_name,
        usage.input_tokens,
        usage.output_tokens,
        usage.input_tokens.saturating_add(usage.output_tokens),
        usage.cost_usd,
        usage.cache_hit_tokens,
    );
}

/// Whether this provider can take a `tools` parameter at all.
///
/// The agent loop checks this up front so the user gets "this model cannot do
/// agent mode" rather than a confusing 400 from the API.
pub fn supports_tool_calling(provider: &AiProvider) -> bool {
    // OpenAI-compatible `/chat/completions` carries `tools` — that covers
    // DeepSeek, OpenRouter, Kimi and any custom OpenAI-compatible endpoint.
    // Anthropic and Ollama use different shapes and are not wired up here yet.
    !is_anthropic_protocol(provider) && !is_ollama(provider)
}

/// Arguments arrive as a JSON *string* in OpenAI-compatible responses. A model
/// that emits nothing, or malformed JSON, should not abort the whole turn — the
/// tool layer already rejects arguments it cannot use, with a message the model
/// can read and correct on the next round.
fn parse_tool_arguments(raw: Option<&str>) -> serde_json::Value {
    match raw.map(str::trim) {
        None | Some("") => serde_json::json!({}),
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| serde_json::json!({})),
    }
}

/// The smallest request that still refreshes a provider's prompt cache.
///
/// Providers with automatic prefix caching (DeepSeek, Kimi, OpenAI) keep an
/// entry alive for a handful of minutes after it is last *used* — DeepSeek's
/// expires in about ten. Re-sending the same prefix with `max_tokens: 1` counts
/// as a use, so the next real question still hits the cache instead of paying
/// full price to re-read the whole conversation.
///
/// The input is billed at the cache-hit rate, which is where the saving comes
/// from: a hit costs roughly a tenth of a miss, so one ping is far cheaper than
/// the miss it prevents. It is still the user's money, so the call is recorded
/// in the usage ledger under its own source name.
///
/// Returns the cache-hit tokens the provider reported, which is the only
/// evidence available that the caching is real for this provider.
pub async fn touch_prompt_cache(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[serde_json::Value],
    tools: &[serde_json::Value],
) -> Result<u64, String> {
    let client = build_client()?;
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": false,
        // One token. The answer is discarded; only the prefix read matters.
        "max_tokens": 1,
    });
    // The tool declarations are part of the prompt the provider hashes, so a
    // ping without them would refresh a prefix nothing else will ever ask for.
    if !tools.is_empty() {
        body["tools"] = serde_json::json!(tools);
        body["tool_choice"] = serde_json::json!("auto");
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

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid response: {e}"))?;
    let usage = &json["usage"];
    let input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
    let cache_hit_tokens = usage["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64())
        .unwrap_or(0);

    // Its own source, so this background spend is visible in the usage stats
    // rather than folded into the answers the user actually asked for.
    crate::token_usage::record_full(
        "cache-keepalive",
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        usage_cost_usd(usage),
        cache_hit_tokens,
    );
    Ok(cache_hit_tokens)
}

/// One round-trip with tools available, streamed.
///
/// Content deltas go to `event_name` as they arrive, so the user watches the
/// answer appear instead of waiting for the whole turn. Tool calls arrive in the
/// same stream, spread across chunks: each `delta.tool_calls[i]` carries a
/// fragment of `function.arguments` that must be concatenated by index before
/// the call can be parsed.
///
/// Does **not** emit the terminal `{done:true}` — the agent loop may run several
/// of these for one answer, and the UI must see exactly one completion.
#[allow(clippy::too_many_arguments)]
pub async fn stream_with_tools(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    messages: &[serde_json::Value],
    tools: &[serde_json::Value],
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    source: &str,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<ToolTurn, String> {
    if !supports_tool_calling(provider) {
        return Err(format!(
            "{} does not support tool calling in Argus yet.",
            provider.name
        ));
    }

    let client = build_client()?;
    let url = format!(
        "{}/chat/completions",
        provider.base_url.trim_end_matches('/')
    );
    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");
    let is_kimi = provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com");

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    // An empty tool list must be omitted, not sent as `[]`: some gateways reject
    // `tools: []` outright, and it is how the loop says "no more tools".
    if !tools.is_empty() {
        body["tools"] = serde_json::json!(tools);
        body["tool_choice"] = serde_json::json!("auto");
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
    if use_reasoning {
        if is_deepseek(provider) {
            body["thinking"] = serde_json::json!({"type": "enabled"});
            body["reasoning_effort"] = serde_json::json!(match reasoning_effort.unwrap_or("high") {
                "high" => "max",
                _ => "high",
            });
        } else if is_openrouter {
            body["reasoning"] = serde_json::json!({
                "effort": reasoning_effort.unwrap_or("high"),
                "exclude": false
            });
        } else if !is_kimi {
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
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    let mut accumulated = String::new();
    // Tool calls keyed by the `index` the provider assigns, since fragments for
    // several concurrent calls interleave in the stream.
    let mut partial: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;

    'outer: while let Some(chunk) = stream.next().await {
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {e}"))?;
        byte_buf.extend_from_slice(&bytes);
        let valid_up_to = match std::str::from_utf8(&byte_buf) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        if valid_up_to > 0 {
            buf.push_str(unsafe { std::str::from_utf8_unchecked(&byte_buf[..valid_up_to]) });
            byte_buf.drain(..valid_up_to);
        }

        while let Some(pos) = buf.find('\n') {
            let line = buf[..pos].trim_end_matches('\r').to_string();
            buf.drain(..pos + 1);

            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim_start();
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };

            if let Some(usage) = json.get("usage").filter(|v| !v.is_null()) {
                if let Some(v) = usage["prompt_tokens"].as_u64() {
                    input_tokens = v;
                }
                if let Some(v) = usage["completion_tokens"].as_u64() {
                    output_tokens = v;
                }
                if let Some(v) = usage["prompt_cache_hit_tokens"]
                    .as_u64()
                    .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64())
                {
                    cache_hit_tokens = v;
                }
                if (is_openrouter || is_kimi) && cost_usd.is_none() {
                    cost_usd = usage_cost_usd(usage);
                }
            }

            let delta = &json["choices"][0]["delta"];

            if let Some(text) = delta["content"].as_str().filter(|s| !s.is_empty()) {
                accumulated.push_str(text);
                let _ = app.emit(event_name, serde_json::json!({"delta": text, "done": false}));
            }
            if let Some(r) = delta["reasoning_content"]
                .as_str()
                .or_else(|| delta["reasoning"].as_str())
                .or_else(|| delta["thinking"].as_str())
                .filter(|s| !s.is_empty())
            {
                let _ = app.emit(
                    &reasoning_event,
                    serde_json::json!({"delta": r, "done": false}),
                );
            }

            if let Some(calls) = delta["tool_calls"].as_array() {
                for c in calls {
                    let idx = c["index"].as_u64().unwrap_or(0);
                    let slot = partial.entry(idx).or_default();
                    if let Some(id) = c["id"].as_str() {
                        slot.0 = id.to_string();
                    }
                    if let Some(name) = c["function"]["name"].as_str() {
                        slot.1.push_str(name);
                    }
                    if let Some(frag) = c["function"]["arguments"].as_str() {
                        slot.2.push_str(frag);
                    }
                }
            }
        }
    }

    crate::token_usage::record_full(
        source,
        &provider.id,
        model,
        input_tokens,
        output_tokens,
        if is_openrouter || is_kimi { cost_usd } else { None },
        cache_hit_tokens,
    );
    // Deliberately *not* emitted here: one answer is several of these rounds,
    // and emitting per round would both flash a cost strip at the user mid-run
    // and leave them looking at the last round's figures instead of the total.
    // The agent loop sums these and emits once. See `emit_usage`.

    let tool_calls = partial
        .into_iter()
        .filter(|(_, (_, name, _))| !name.is_empty())
        .map(|(idx, (id, name, args))| ToolCall {
            id: if id.is_empty() { format!("call_{idx}") } else { id },
            arguments: parse_tool_arguments(Some(&args)),
            name,
        })
        .collect();

    Ok(ToolTurn {
        content: accumulated,
        tool_calls,
        usage: TurnUsage {
            input_tokens,
            output_tokens,
            cache_hit_tokens,
            cost_usd: if is_openrouter || is_kimi { cost_usd } else { None },
        },
    })
}

#[cfg(test)]
mod offer_tests {
    use super::*;

    /// Both directions must be zero. A model free to read but charging to
    /// generate is not free, and the tag would cost the user money.
    #[test]
    fn free_needs_both_sides_at_zero() {
        assert!(quotes_free(&serde_json::json!({"prompt": "0", "completion": "0"})));
        assert!(!quotes_free(&serde_json::json!({"prompt": "0", "completion": "0.000003"})));
        assert!(!quotes_free(&serde_json::json!({"prompt": "0.0000005", "completion": "0"})));
        // Absent pricing is unknown, not free.
        assert!(!quotes_free(&serde_json::json!({})));
    }

    /// Verbatim from OpenRouter's catalogue: `deepseek/deepseek-v4-pro` prices
    /// two of the day's four windows at half rate.
    #[test]
    fn a_time_of_day_schedule_reads_as_a_discount() {
        let pricing = serde_json::json!({
            "prompt": "0.00000132",
            "completion": "0.00000396",
            "overrides": [
                {"utc_start": 1000, "utc_end": 100,
                 "prompt": "0.00000066", "completion": "0.00000198"},
                {"utc_start": 100, "utc_end": 400,
                 "prompt": "0.00000132", "completion": "0.00000396"},
                {"utc_start": 400, "utc_end": 600,
                 "prompt": "0.00000066", "completion": "0.00000198"},
                {"utc_start": 600, "utc_end": 1000,
                 "prompt": "0.00000132", "completion": "0.00000396"}
            ]
        });
        let (percent, windows) = parse_time_discount(&pricing);
        assert_eq!(percent, Some(50));
        assert_eq!(windows, vec![[1000, 100], [400, 600]], "the full-rate windows were kept");
    }

    /// The trap. OpenRouter reuses `overrides` for long-context *surcharges* —
    /// 64 of 414 models carry one, and every one raises the price. Reading them
    /// as discounts would tag the priciest models as bargains.
    #[test]
    fn a_long_context_surcharge_is_not_a_discount() {
        // Verbatim from `x-ai/grok-4.6`: double price above 200k prompt tokens.
        let pricing = serde_json::json!({
            "prompt": "0.000002",
            "completion": "0.000006",
            "overrides": [
                {"min_prompt_tokens": 200000, "prompt": "0.000004", "completion": "0.000012"}
            ]
        });
        assert_eq!(parse_time_discount(&pricing), (None, Vec::new()));
    }

    /// The size is in the name for open models and nowhere for closed ones.
    #[test]
    fn a_size_in_the_name_is_read_out() {
        assert_eq!(scan_param_size("nvidia/nemotron-3-embed-1b"), Some(1.0));
        assert_eq!(scan_param_size("liquid/lfm-2.5-2.6b:free"), Some(2.6));
        assert_eq!(scan_param_size("qwen/qwen3.8-27b"), Some(27.0));
    }

    /// A mixture-of-experts model is named for its total *and* its active
    /// parameters. "550B" is the size people mean, not "55B".
    #[test]
    fn a_mixture_of_experts_reports_its_total() {
        assert_eq!(scan_param_size("nvidia/nemotron-3-ultra-550b-a55b"), Some(550.0));
        assert_eq!(scan_param_size("qwen/qwen3.8-2.4t-a95b"), Some(2400.0));
    }

    /// The trap: version numbers look exactly like sizes. `gpt-5.6` is not a
    /// 5.6-billion-parameter model, and `qwen3.8` is not 3.8B.
    #[test]
    fn a_version_number_is_not_a_size() {
        assert_eq!(scan_param_size("openai/gpt-5.6-luna-pro"), None);
        assert_eq!(scan_param_size("x-ai/grok-4.6"), None);
        assert_eq!(scan_param_size("meituan/longcat-2.0"), None);
        assert_eq!(scan_param_size("deepseek/deepseek-v4-pro-0813"), None);
    }

    /// A digit glued to more letters is an identifier, not a measurement.
    #[test]
    fn a_unit_must_end_its_token() {
        assert_eq!(scan_param_size("model-3ba-preview"), None);
        assert_eq!(scan_param_size("seed-2-1-turbo"), None);
    }

    #[test]
    fn the_description_is_the_last_resort() {
        let item = serde_json::json!({
            "id": "vendor/opaque-name",
            "description": "A 284B-parameter mixture-of-experts model."
        });
        assert_eq!(parse_param_billions(&item), Some(284.0));

        // The naming wins when it has an answer, prose being the less reliable
        // of the two.
        let named = serde_json::json!({
            "id": "vendor/thing-7b",
            "description": "Trained on 15T tokens."
        });
        assert_eq!(parse_param_billions(&named), Some(7.0));
    }

    /// Verbatim from `/models/openai/gpt-5.6-luna-pro/endpoints`: OpenAI serves
    /// it at half price, Azure at full. The badge must describe the endpoint
    /// whose price is on screen, not the best one going.
    #[test]
    fn the_discount_follows_the_price_being_quoted() {
        let endpoints = serde_json::json!([
            {"provider_name": "OpenAI", "pricing": {"prompt": "0.0000001", "discount": 0.5}},
            {"provider_name": "OpenAI", "pricing": {"prompt": "0.00000005", "discount": 0.5}},
            {"provider_name": "Azure",  "pricing": {"prompt": "0.0000002", "discount": 0}}
        ]);
        // $0.10/M is what the catalogue quotes → the first OpenAI endpoint.
        assert_eq!(discount_of_quoted_endpoint(&endpoints, Some(0.1)), Some(50));
        // $0.20/M is Azure, which is running no promotion.
        assert_eq!(discount_of_quoted_endpoint(&endpoints, Some(0.2)), None);
    }

    /// `deepseek-v4-pro` is quoted at the first-party endpoint's price, which
    /// carries no promotion even though cheaper resellers are discounting it.
    #[test]
    fn a_cheaper_endpoints_promotion_is_not_borrowed() {
        let endpoints = serde_json::json!([
            {"provider_name": "StreamLake", "pricing": {"prompt": "0.00000069426", "discount": 0.601}},
            {"provider_name": "DeepSeek",   "pricing": {"prompt": "0.00000132", "discount": 0}}
        ]);
        assert_eq!(discount_of_quoted_endpoint(&endpoints, Some(1.32)), None);
    }

    #[test]
    fn an_unmatched_price_falls_back_to_the_default_route() {
        let endpoints = serde_json::json!([
            {"pricing": {"prompt": "0.0000003", "discount": 0.6}},
            {"pricing": {"prompt": "0.0000009", "discount": 0}}
        ]);
        assert_eq!(discount_of_quoted_endpoint(&endpoints, Some(99.0)), Some(60));
        assert_eq!(discount_of_quoted_endpoint(&endpoints, None), Some(60));
        assert_eq!(discount_of_quoted_endpoint(&serde_json::json!([]), None), None);
    }

    /// `discount: 0` is the overwhelmingly common value and must not become a
    /// "0折" badge on every model.
    #[test]
    fn no_promotion_is_no_badge() {
        let endpoints = serde_json::json!([{"pricing": {"prompt": "0.000002", "discount": 0}}]);
        assert_eq!(discount_of_quoted_endpoint(&endpoints, None), None);
        let missing = serde_json::json!([{"pricing": {"prompt": "0.000002"}}]);
        assert_eq!(discount_of_quoted_endpoint(&missing, None), None);
    }

    #[test]
    fn a_flat_price_advertises_nothing() {
        let pricing = serde_json::json!({"prompt": "0.000002", "completion": "0.000006"});
        assert_eq!(parse_time_discount(&pricing), (None, Vec::new()));
        // A free model has no base to discount from; dividing by it would be
        // an infinite percentage off.
        let free = serde_json::json!({"prompt": "0", "completion": "0"});
        assert_eq!(parse_time_discount(&free), (None, Vec::new()));
    }
}

#[cfg(test)]
mod tool_call_tests {
    use super::*;

    #[test]
    fn arguments_survive_the_json_string_encoding() {
        let v = parse_tool_arguments(Some(r#"{"slug":"attention-2017","limit":3}"#));
        assert_eq!(v["slug"], "attention-2017");
        assert_eq!(v["limit"], 3);
    }

    /// An agent answer is several provider calls. Reporting the last one's
    /// figures would tell the user a five-round answer cost what its final
    /// round did.
    #[test]
    fn rounds_are_summed_not_overwritten() {
        let mut total = TurnUsage::default();
        total.add(&TurnUsage {
            input_tokens: 3_000,
            output_tokens: 200,
            cache_hit_tokens: 1_000,
            cost_usd: Some(0.001),
        });
        total.add(&TurnUsage {
            input_tokens: 5_000,
            output_tokens: 400,
            cache_hit_tokens: 2_500,
            cost_usd: Some(0.002),
        });
        assert_eq!(total.input_tokens, 8_000);
        assert_eq!(total.output_tokens, 600);
        assert_eq!(total.cache_hit_tokens, 3_500);
        assert_eq!(total.cost_usd, Some(0.003));
    }

    /// A provider that reports no cost must leave it unknown, not claim zero —
    /// the front-end falls back to estimating from token counts.
    #[test]
    fn an_unreported_cost_stays_unknown() {
        let mut total = TurnUsage::default();
        total.add(&TurnUsage {
            input_tokens: 100,
            ..Default::default()
        });
        assert_eq!(total.cost_usd, None);

        total.add(&TurnUsage {
            cost_usd: Some(0.5),
            ..Default::default()
        });
        assert_eq!(total.cost_usd, Some(0.5), "a later report must still land");
    }

    /// A model that emits `""` or broken JSON must not abort the turn — the
    /// tool layer reports the problem in a way the model can act on.
    #[test]
    fn malformed_arguments_degrade_to_empty() {
        for raw in [None, Some(""), Some("   "), Some("{not json")] {
            assert_eq!(parse_tool_arguments(raw), serde_json::json!({}), "{raw:?}");
        }
    }
}
