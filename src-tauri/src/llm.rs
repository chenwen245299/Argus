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

/// Run a DeepSeek request's `messages` through the multimodal guard before it is
/// sent. A no-op for every other provider, and for a DeepSeek request that
/// carries no attachments — the common case, which must stay byte-identical so
/// the prompt-cache prefix keeps hitting.
async fn prepare_deepseek_messages(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    msgs: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    if !is_deepseek(provider) {
        return Ok(msgs);
    }
    crate::deepseek::prepare_chat_messages(provider, api_key, model, msgs).await
}

/// Qwen (Alibaba Model Studio / QwenCloud Token Plan). OpenAI-compatible, so it
/// rides the generic `/chat/completions` path; detection is only needed for the
/// features layered on top of it — native web search and model enrichment.
///
/// Both providers that expose a server-side web search do so differently:
/// DeepSeek needs its Responses API (see the dispatch in `chat_completion_stream`),
/// while Qwen keeps the standard `/chat/completions` shape and only adds an
/// `enable_search` field to the body — which is why the two are detected
/// separately rather than behind one `supports_web_search` predicate.
pub fn is_qwen(provider: &AiProvider) -> bool {
    provider.kind == "qwenai"
        || provider.base_url.to_lowercase().contains("dashscope")
        || provider.base_url.to_lowercase().contains("maas.aliyuncs")
}

/// Attach a provider's auth header(s) to an OpenAI-compatible request.
///
/// Everything on this path authenticates with `Authorization: Bearer`. MiMo's
/// platform documents an `api-key:` header instead; it also accepts Bearer (its
/// OpenAI-SDK compatibility guarantees that), so both are sent and the endpoint
/// simply ignores the one it does not read. A no-op change of headers for every
/// other provider.
fn openai_auth(
    req: reqwest::RequestBuilder,
    provider: &AiProvider,
    api_key: &str,
) -> reqwest::RequestBuilder {
    let req = req.header("Authorization", format!("Bearer {api_key}"));
    if crate::mimo::is_mimo(provider) {
        req.header("api-key", api_key)
    } else {
        req
    }
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
    // DeepSeek's web search lives on a different protocol (its Responses API), so
    // it takes its own path. Qwen's, by contrast, is just an `enable_search` flag
    // on the ordinary /chat/completions body, so it stays on the openai-compat
    // path below with `web_search` threaded through.
    if web_search && is_deepseek(provider) {
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
            web_search,
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
    if is_qwen(provider) {
        // QwenCloud's /models lists the OpenAI-compatible chat models available to
        // this key — the text/VL/reasoning ones, not the console's full media
        // catalogue (image/video/audio/embedding ride DashScope-native endpoints).
        // It returns bare ids with no modality, so overlay an id-derived capability
        // table onto each. Errors surface like every other provider's rather than
        // being masked by a stale hardcoded list the user can't tell apart from a
        // genuinely short result; anything the endpoint omits can still be added by
        // id via 手动添加.
        let models = fetch_openai_models(provider, api_key).await?;
        return Ok(models.into_iter().map(enrich_qwen_model).collect());
    }
    if crate::mimo::is_mimo(provider) {
        // MiMo's /models lists its ids but reports no modality, so overlay an
        // id-derived capability table (and the documented 1M context) onto each,
        // the same way Qwen's ids are enriched above.
        let models = fetch_openai_models(provider, api_key).await?;
        return Ok(models.into_iter().map(crate::mimo::enrich_mimo_model).collect());
    }
    if crate::minimax::is_minimax(provider) {
        // Same arrangement as Zhipu below: MiniMax documents no /models
        // endpoint, so the documented catalogue is the floor rather than a
        // fallback — try the endpoint, keep whatever it reports, and append the
        // documented ids it left out. `测试连接` is what validates the key.
        let fetched = fetch_openai_models(provider, api_key).await.unwrap_or_default();
        return Ok(crate::minimax::merge_catalogue(fetched));
    }
    if crate::moleapi::is_moleapi(provider) {
        // The relay's /models is the authority on what this key may call, but
        // it returns bare ids. Modalities and per-token rates live in the public
        // price list beside it, so overlay that — and drop the rows (speech,
        // reranking, moderation) that cannot take a chat turn.
        //
        // The price list is required, not best-effort. Without it the result is
        // six hundred ids with no way to tell a chat model from a reranker, and
        // — worse — the background price refresh would read the missing rates
        // as "no longer priced" and wipe the ones already saved. Both live on
        // the same host, so one answering while the other does not is rare.
        let (models, pricing) = tokio::join!(
            fetch_openai_models(provider, api_key),
            crate::moleapi::fetch_pricing(provider),
        );
        let models = models?;
        let pricing = pricing.map_err(|e| format!("MoleAPI price list (/api/pricing): {e}"))?;
        return Ok(crate::moleapi::enrich_models(models, &pricing));
    }
    if crate::stepfun::is_stepfun(provider) {
        // StepFun's `/models` answers, but the documented response is five chat
        // ids with no modality — it omits every audio, speech and image model the
        // key can actually call. So, as for Zhipu and MiniMax below, the
        // documented catalogue is the floor rather than the fallback: keep
        // whatever the endpoint reports (that is how a model newer than this
        // build still appears), enrich it, and append every documented id it
        // left out.
        let fetched = fetch_openai_models(provider, api_key).await.unwrap_or_default();
        return Ok(crate::stepfun::merge_catalogue(fetched));
    }
    if crate::zhipu::is_zhipu(provider) {
        // BigModel documents no /models endpoint — the path answers, but only
        // behind the platform's blanket auth gate, so whether it lists anything
        // is anyone's guess. The documented catalogue is therefore the floor
        // rather than a fallback: try the endpoint, keep whatever it reports
        // (that is how a model newer than this build appears), and append every
        // documented id it left out. A failure there is not surfaced as an
        // error, because an undocumented endpoint going quiet says nothing about
        // the key — 测试连接 is what validates that, against /chat/completions.
        let fetched = fetch_openai_models(provider, api_key).await.unwrap_or_default();
        return Ok(crate::zhipu::merge_catalogue(fetched));
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
            format!("{}. Is Ollama running at {}?", describe_reqwest_error(&e), ollama_root(provider))
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
        .header("Content-Type", "application/json");
    req = openai_auth(req, provider, api_key);
    if is_kimi_coding_endpoint {
        // Kimi Code's /coding endpoint gates access by User-Agent whitelist.
        // Pretend to be a whitelisted coding agent so ordinary API keys work.
        req = req.header("User-Agent", "KimiCLI/1.5");
    }
    let resp = req.json(&body).send().await.map_err(|e| describe_stream_error(&e))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    // A 200 can still be a refusal: MiniMax reports a bad key or a used-up
    // Token Plan window in `base_resp`, and "Connected" would be the wrong verdict.
    let failure = if status >= 400 {
        Some(friendly_error(status, &text))
    } else {
        serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|json| provider_error_in_body(&json))
    };
    if let Some(failure) = failure {
        return Err(format!(
            "{} [kind={}, base_url={}, model={}]",
            failure,
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
        .map_err(|e| describe_reqwest_error(&e))?;

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
                .map_err(|e| describe_reqwest_error(&e))?;

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

    // Same server tools as an ordinary chat: reading a paper is exactly where a
    // model is likely to want to look something up.
    let server_tools = crate::openrouter::server_tool_defs(provider, model);
    if !server_tools.is_empty() {
        body["tools"] = serde_json::json!(server_tools);
        body["max_tool_calls"] = serde_json::json!(crate::openrouter::max_tool_calls(provider));
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
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| describe_stream_error(&e))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    // Non-SSE lines, kept in case the "stream" is a plain JSON error body.
    let mut stray = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut usage_emitted = false;
    let mut trace = crate::openrouter::ServerToolTrace::default();

    'stream: while let Some(chunk) = stream.next().await {
        // Backend cancellation: if the user pressed stop, break out of the loop.
        // Dropping `stream`/`resp` on scope exit closes the HTTP connection so the
        // provider stops generating (and billing). Return the partial text.
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {}", describe_stream_error(&e)))?;
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
                            // Same as `stream_openai_compat`: an error sent as an
                            // event must not end the stream as an empty success.
                            if let Some(err) = provider_error_in_body(&json) {
                                if accumulated.is_empty() {
                                    return Err(err);
                                }
                                let notice = interrupted_notice(&err);
                                let _ = app.emit(event_name, serde_json::json!({"delta": &notice, "done": false}));
                                accumulated.push_str(&notice);
                                break 'stream;
                            }
                            if let Some(usage) = json.get("usage").filter(|v| !v.is_null()) {
                                if let Some(v) = usage["prompt_tokens"].as_u64() {
                                    input_tokens = v;
                                }
                                if let Some(v) = usage["completion_tokens"].as_u64() {
                                    output_tokens = v;
                                }
                                if let Some(v) = usage["prompt_cache_hit_tokens"]
                                    .as_u64()
                                    // OpenAI / Kimi / OpenRouter report them here…
                                    .or_else(|| {
                                        usage["prompt_tokens_details"]["cached_tokens"].as_u64()
                                    })
                                    // …and StepFun's worked examples put the same
                                    // figure at the top level instead. Reading only
                                    // the nested path would report every hit as zero.
                                    .or_else(|| usage["cached_tokens"].as_u64())
                                {
                                    // DeepSeek reports cache hits at the first path.
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
                                trace.absorb_usage(usage);
                            }
                            trace.absorb(&json["choices"][0]["delta"]);
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
                    } else {
                        keep_stray_line(&mut stray, &line);
                    }
                }
            }
        }
    }

    let cancelled = cancel.as_ref().is_some_and(|f| f.load(Ordering::SeqCst));
    if accumulated.is_empty() && !cancelled {
        if let Some(err) = stray_body_error(&stray, &buf) {
            return Err(err);
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
    emit_server_tool_trace(app, event_name, &trace);
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
    let msgs = prepare_deepseek_messages(provider, api_key, model, msgs).await?;
    if crate::stepfun::is_stepfun(provider) {
        crate::stepfun::check_messages(model, &msgs)?;
    }
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

    // The three providers that reason unless told otherwise need their controls
    // here too, not only on the streaming paths. This function serves the
    // one-shot jobs — summaries, translation, titles, abstracts, the arXiv digest
    // — none of which show a reasoning pane, so whatever the model thinks is
    // either billed and thrown away or, worse, rendered as part of the answer:
    // MiniMax returns its chain of thought inside `content` in `<think>` tags
    // unless `reasoning_split` is asked for, and only `apply_thinking` asks.
    //
    // `use_reasoning: false` throughout, because a one-shot job has no toggle to
    // read and none of these tasks wants deep thinking.
    if crate::zhipu::is_zhipu(provider) {
        crate::zhipu::apply_thinking(&mut body, model, false, None);
    }
    if crate::minimax::is_minimax(provider) {
        crate::minimax::apply_thinking(&mut body, model, false);
    }
    if crate::stepfun::is_stepfun(provider) {
        crate::stepfun::apply_reasoning(&mut body, model, false, None);
    }

    let is_kimi_coding_endpoint = provider.base_url.to_lowercase().contains("api.kimi.com");
    let mut req = client
        .post(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Content-Type", "application/json");
    req = openai_auth(req, provider, api_key);
    if is_kimi_coding_endpoint {
        req = req.header("User-Agent", "KimiCLI/1.5");
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| describe_reqwest_error(&e))?;

    let status = resp.status().as_u16();
    let text = match resp.text().await {
        Ok(text) => text,
        // The status alone is still the error, and says more than the read.
        Err(_) if status >= 400 => String::new(),
        // A body cut off by the timeout used to become an empty string here,
        // and then a misleading "Invalid JSON from API: EOF…" below.
        Err(e) => return Err(format!("读取响应失败: {}", describe_reqwest_error(&e))),
    };
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from API: {e}"))?;
    // MiniMax answers business errors — a used-up Token Plan window, a
    // throttle — with a 200 and a `base_resp`; gateways do the same with an
    // `error` object. Read them before anything goes looking for `choices`.
    if let Some(err) = provider_error_in_body(&json) {
        return Err(err);
    }

    let input_tokens = json["usage"]["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tokens = json["usage"]["completion_tokens"].as_u64().unwrap_or(0);
    let cache_hit_tokens = json["usage"]["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| json["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64())
        .or_else(|| json["usage"]["cached_tokens"].as_u64())
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

    let message = &json["choices"][0]["message"];
    let content = strip_leading_think(message["content"].as_str().unwrap_or(""));
    if !content.trim().is_empty() {
        return Ok(content.to_string());
    }
    // A thought-only reply counts as reasoning, whether the provider split it
    // out or inlined it in `<think>` tags that were just stripped.
    let mut reasoning = reply_reasoning(message);
    if reasoning.is_empty() {
        let raw = message["content"].as_str().unwrap_or("");
        if raw.trim_start().starts_with("<think>") {
            reasoning = raw.to_string();
        }
    }
    // kimi-for-coding answers in `reasoning_content` by default; the streaming
    // paths already treat that as the answer, so this one does too.
    if is_kimi_for_coding && !reasoning.trim().is_empty() {
        return Ok(reasoning);
    }
    Err(empty_reply_error(&json, &text, &reasoning))
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
    web_search: bool,
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
    let mut msgs = prepare_deepseek_messages(provider, api_key, model, msgs).await?;

    let is_stepfun = crate::stepfun::is_stepfun(provider);
    if is_stepfun {
        crate::stepfun::check_messages(model, &msgs)?;
    }
    // Pages fetched before the request goes out, for the one StepFun model that
    // has no built-in search tool. Collected here so they can be shown as
    // citations next to a server-run search's.
    let mut preflight_hits: Vec<crate::stepfun::SearchHit> = Vec::new();
    if web_search && is_stepfun && !crate::stepfun::supports_builtin_web_search(model) {
        if let Some(query) = crate::stepfun::query_from_messages(&msgs) {
            match crate::stepfun::search(provider, api_key, &query).await {
                Ok(hits) if !hits.is_empty() => {
                    // Appended, never prepended: the cached prefix every
                    // long-running task front-loads must stay byte-identical.
                    msgs.push(serde_json::json!({
                        "role": "system",
                        "content": crate::stepfun::search_context(&hits),
                    }));
                    preflight_hits = hits;
                }
                Ok(_) => {}
                // A search that fails should not take the answer down with it —
                // the model can still reply from what it knows.
                Err(e) => eprintln!("[stepfun] web search skipped: {e}"),
            }
        }
    }
    let msgs = msgs;

    let is_deepseek = provider.base_url.to_lowercase().contains("deepseek");
    let is_openrouter = provider.base_url.to_lowercase().contains("openrouter");
    let is_kimi = provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com");
    let is_kimi_k2 = is_kimi && model.starts_with("kimi-k2");
    let is_kimi_for_coding = is_kimi && model == "kimi-for-coding";

    let is_zhipu = crate::zhipu::is_zhipu(provider);
    let is_minimax = crate::minimax::is_minimax(provider);

    let mut body = serde_json::json!({
        "model": model, "messages": msgs, "stream": true,
        "stream_options": {"include_usage": true}
    });
    if is_stepfun {
        // StepFun documents no `stream_options` and does not need it: usage rides
        // every chunk unconditionally. Sending an unknown field to a strict
        // validator is a needless risk, so it is removed rather than left in.
        if let Some(obj) = body.as_object_mut() {
            obj.remove("stream_options");
        }
    }

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
        } else if is_qwen(provider) {
            // Qwen's compatible mode gates thinking with `enable_thinking`, not
            // OpenAI's `reasoning_effort` (which its strict validator rejects).
            body["enable_thinking"] = serde_json::json!(true);
        } else if crate::mimo::is_mimo(provider) {
            // MiMo gates deep thinking with `thinking: {type: enabled}` and
            // streams the reasoning back as `reasoning_content`, like DeepSeek.
            body["thinking"] = serde_json::json!({"type": "enabled"});
        } else {
            body["reasoning_effort"] = serde_json::json!(reasoning_effort.unwrap_or("high"));
        }
    }

    // GLM is the one provider here that thinks unless told not to, so its
    // controls are written in both directions — and outside the block above,
    // which only runs when reasoning is on. `apply_thinking` owns `thinking` and
    // `reasoning_effort` for GLM, including clearing the generic value the chain
    // may have just set.
    if is_zhipu {
        crate::zhipu::apply_thinking(&mut body, model, use_reasoning, reasoning_effort);
    }
    // MiniMax also thinks unless told not to — and, left alone, returns the
    // thinking inside `content`. `apply_thinking` handles both.
    if is_minimax {
        crate::minimax::apply_thinking(&mut body, model, use_reasoning);
    }
    // StepFun cannot be told to stop thinking at all, so "off" has to become its
    // cheapest setting rather than an omission — and the field has to be removed
    // outright for the models that have none. Both directions, hence outside the
    // block above. See `stepfun::apply_reasoning`.
    if is_stepfun {
        crate::stepfun::apply_reasoning(&mut body, model, use_reasoning, reasoning_effort);
        if provider.speech.enabled {
            crate::stepfun::apply_audio_output(
                &mut body,
                model,
                Some(provider.speech.voice.as_str()),
                true,
            );
        }
    }

    // OpenRouter's server-side tools ride along on every chat request. They are
    // run by OpenRouter mid-answer rather than handed back to us, so no client
    // loop is needed and nothing is billed unless the model actually reaches for
    // one — which is why they are attached rather than gated behind a toggle.
    let server_tools = crate::openrouter::server_tool_defs(provider, model);
    if !server_tools.is_empty() {
        body["tools"] = serde_json::json!(server_tools);
        body["max_tool_calls"] = serde_json::json!(crate::openrouter::max_tool_calls(provider));
    }

    // Qwen's native web search: unlike DeepSeek it stays on /chat/completions and
    // just takes an `enable_search` flag on the body. `forced_search` makes it
    // actually search rather than deciding on its own; `enable_source` returns
    // the pages it consulted.
    if web_search && is_qwen(provider) {
        body["enable_search"] = serde_json::json!(true);
        body["search_options"] = serde_json::json!({ "forced_search": true, "enable_source": true });
    }

    // MiMo's native web search is a tool the platform runs itself; unlike an
    // Argus tool it never comes back as a call, only as `annotations` the
    // `ServerToolTrace` below already reads. Append it to whatever `tools` the
    // body may already carry (none, on this plain-chat path, for MiMo).
    if web_search && crate::mimo::is_mimo(provider) {
        let mut tools = body["tools"].as_array().cloned().unwrap_or_default();
        tools.push(crate::mimo::web_search_tool());
        body["tools"] = serde_json::json!(tools);
    }

    // GLM's native web search is the same arrangement: a tool the platform runs
    // itself, reporting the pages it read in a top-level `web_search` array
    // rather than as a call for us to answer.
    if web_search && is_zhipu {
        let mut tools = body["tools"].as_array().cloned().unwrap_or_default();
        tools.push(crate::zhipu::web_search_tool());
        body["tools"] = serde_json::json!(tools);
    }

    // StepFun's built-in search, for the models that carry it. The flagship does
    // not, and was already served by the `/v1/search` preflight above — so the
    // two branches are mutually exclusive and a turn is never searched twice.
    if web_search && is_stepfun && crate::stepfun::supports_builtin_web_search(model) {
        let mut tools = body["tools"].as_array().cloned().unwrap_or_default();
        tools.push(crate::stepfun::web_search_tool());
        body["tools"] = serde_json::json!(tools);
    }

    let is_kimi_coding_endpoint = provider.base_url.to_lowercase().contains("api.kimi.com");
    let mut req = client
        .post(&url)
        .header("Content-Type", "application/json");
    req = openai_auth(req, provider, api_key);
    if is_kimi_coding_endpoint {
        req = req.header("User-Agent", "KimiCLI/1.5");
        // Kimi Code may return a gzipped SSE stream; ask for identity to keep it plain text.
        req = req.header("Accept-Encoding", "identity");
    }
    let resp = req
        .json(&body)
        .send()
        .await
        .map_err(|e| describe_stream_error(&e))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    // Non-SSE lines, kept in case the "stream" is a plain JSON error body.
    let mut stray = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut usage_emitted = false;
    let mut trace = crate::openrouter::ServerToolTrace::default();
    // Pages the client-side search read are citations too, even though they
    // never appear in the response.
    if !preflight_hits.is_empty() {
        for hit in &preflight_hits {
            trace.push_citation(&hit.url, Some(hit.title.as_str()));
        }
        trace.note_call("web_search", 1);
    }
    // Raw 24 kHz PCM from an end-to-end speech model, concatenated across chunks
    // and given a WAV header once the stream ends. Stays empty for every other
    // provider, and for StepFun with speech switched off.
    let mut audio_pcm: Vec<u8> = Vec::new();

    'stream: while let Some(chunk) = stream.next().await {
        // Backend cancellation: if the user pressed stop, break out of the loop.
        // Dropping `stream`/`resp` on scope exit closes the HTTP connection so the
        // provider stops generating (and billing). Return the partial text.
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {}", describe_stream_error(&e)))?;
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
                            // An error sent as an event — MiniMax's `base_resp`,
                            // OpenRouter's mid-stream `error` — used to be read
                            // as a chunk with no delta and end the stream "fine".
                            // Before anything is shown it is the call's error;
                            // after, see `interrupted_notice`.
                            if let Some(err) = provider_error_in_body(&json) {
                                if accumulated.is_empty() && audio_pcm.is_empty() {
                                    return Err(err);
                                }
                                let notice = interrupted_notice(&err);
                                let _ = app.emit(event_name, serde_json::json!({"delta": &notice, "done": false}));
                                accumulated.push_str(&notice);
                                break 'stream;
                            }
                            // Capture usage from the final usage chunk
                            if let Some(usage) = json.get("usage").filter(|v| !v.is_null()) {
                                if let Some(v) = usage["prompt_tokens"].as_u64() {
                                    input_tokens = v;
                                }
                                if let Some(v) = usage["completion_tokens"].as_u64() {
                                    output_tokens = v;
                                }
                                if let Some(v) = usage["prompt_cache_hit_tokens"]
                                    .as_u64()
                                    // OpenAI / Kimi / OpenRouter report them here…
                                    .or_else(|| {
                                        usage["prompt_tokens_details"]["cached_tokens"].as_u64()
                                    })
                                    // …and StepFun's worked examples put the same
                                    // figure at the top level instead. Reading only
                                    // the nested path would report every hit as zero.
                                    .or_else(|| usage["cached_tokens"].as_u64())
                                {
                                    // DeepSeek reports cache hits at the first path.
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
                                trace.absorb_usage(usage);
                            }
                            // Web pages the server-side search consulted, and any
                            // image it drew, both ride the delta alongside the text.
                            trace.absorb(&json["choices"][0]["delta"]);
                            // GLM hangs its search results off the chunk itself
                            // rather than the delta, so that level is read too.
                            if is_zhipu {
                                trace.absorb(&json);
                            }
                            // An end-to-end speech reply arrives as base64 PCM
                            // fragments, and its *text* comes back as the audio's
                            // transcript rather than as `content`. The API
                            // reference does not document `delta.audio` at all —
                            // only the audio guide's sample code reads it — so
                            // every field here is probed rather than assumed.
                            let audio = &json["choices"][0]["delta"]["audio"];
                            if let Some(b64) = audio["data"].as_str().filter(|s| !s.is_empty()) {
                                use base64::Engine;
                                if let Ok(bytes) =
                                    base64::engine::general_purpose::STANDARD.decode(b64)
                                {
                                    audio_pcm.extend_from_slice(&bytes);
                                }
                            }
                            let transcript_delta =
                                audio["transcript"].as_str().filter(|s| !s.is_empty());

                            // Main content delta
                            let content_delta = json["choices"][0]["delta"]["content"]
                                .as_str()
                                .filter(|s| !s.is_empty())
                                .or(transcript_delta);
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
                    } else {
                        keep_stray_line(&mut stray, &line);
                    }
                }
            }
        }
    }

    // A 200 whose body was a plain JSON error rather than events ends here with
    // nothing produced; say what it was instead of returning an empty answer.
    let cancelled = cancel.as_ref().is_some_and(|f| f.load(Ordering::SeqCst));
    if accumulated.is_empty() && audio_pcm.is_empty() && !cancelled {
        if let Some(err) = stray_body_error(&stray, &buf) {
            return Err(err);
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
    emit_server_tool_trace(app, event_name, &trace);
    // One event, at the end: the clip is only playable once it is complete and
    // wrapped, and a half-written WAV is worse than a slightly late one.
    if !audio_pcm.is_empty() {
        use base64::Engine;
        let wav = crate::stepfun::pcm_to_wav(&audio_pcm);
        let _ = app.emit(
            &format!("{event_name}-audio"),
            serde_json::json!({
                "audio": format!(
                    "data:audio/wav;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(&wav)
                ),
            }),
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
//   * images ride `input_image` blocks; a PDF part has no equivalent and is
//     dropped, and every non-user turn is flattened to text;
//   * the SSE stream carries typed events, not `choices[].delta`.
// Unsupported request fields are documented as silently ignored, so sending the
// OpenAI-shaped `reasoning.effort` is safe even where DeepSeek's own naming
// differs.

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

    // Images survive this path: DeepSeek's Responses API takes them as
    // `input_image` blocks, so a question about a figure keeps working with web
    // search on. The same limits guard runs first, on the chat-shaped array.
    let prepared = prepare_deepseek_messages(
        provider,
        api_key,
        model,
        messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": &m.content}))
            .collect(),
    )
    .await?;
    let (instructions, input) = crate::deepseek::to_responses_input(&prepared);

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

    let req = client
        .post(&url)
        .header("Content-Type", "application/json");
    let resp = openai_auth(req, provider, api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| describe_stream_error(&e))?;

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
        let bytes = chunk.map_err(|e| format!("Stream read error: {}", describe_stream_error(&e)))?;
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
                    // Ollama's native API has no video or audio block, and a PDF
                    // is not something a vision model can ingest — all three are
                    // dropped rather than mangled into the text.
                    ChatContentPart::VideoUrl { .. }
                    | ChatContentPart::InputAudio { .. }
                    | ChatContentPart::File { .. } => {}
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
        .map_err(|e| format!("{}. Is Ollama running at {}?", describe_reqwest_error(&e), ollama_root(provider)))?;

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
        .map_err(|e| format!("{}. Is Ollama running at {}?", describe_stream_error(&e), ollama_root(provider)))?;

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
        let bytes = chunk.map_err(|e| format!("Stream read error: {}", describe_stream_error(&e)))?;
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
        .map_err(|e| format!("{}. Is Ollama running at {}?", describe_reqwest_error(&e), ollama_root(provider)))?;

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
        .map_err(|e| format!("{}. Is Ollama running at {root}?", describe_reqwest_error(&e)))?;

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
        .map_err(|e| describe_reqwest_error(&e))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(friendly_error(status, &text));
    }
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("Invalid JSON from Anthropic: {e}"))?;
    if let Some(err) = provider_error_in_body(&json) {
        return Err(err);
    }

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
        .map_err(|e| describe_stream_error(&e))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    // Non-SSE lines, kept in case the "stream" is a plain JSON error body.
    let mut stray = String::new();
    let mut accumulated = String::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    // Anthropic reports cache hits separately from `input_tokens`.
    let mut cache_read: u64 = 0;

    'stream: while let Some(chunk) = stream.next().await {
        // Backend cancellation: if the user pressed stop, break out of the loop.
        // Dropping `stream`/`resp` on scope exit closes the HTTP connection so the
        // provider stops generating (and billing). Return the partial text.
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {}", describe_stream_error(&e)))?;
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
                            // `event: error` — Anthropic's mid-stream overload,
                            // for one — used to fall through to `_ => {}` and
                            // end the stream as an empty success.
                            if let Some(err) = provider_error_in_body(&json) {
                                if accumulated.is_empty() {
                                    return Err(err);
                                }
                                let notice = interrupted_notice(&err);
                                let _ = app.emit(event_name, serde_json::json!({"delta": &notice, "done": false}));
                                accumulated.push_str(&notice);
                                break 'stream;
                            }
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
                    } else {
                        keep_stray_line(&mut stray, &line);
                    }
                }
            }
        }
    }

    let cancelled = cancel.as_ref().is_some_and(|f| f.load(Ordering::SeqCst));
    if accumulated.is_empty() && !cancelled {
        if let Some(err) = stray_body_error(&stray, &buf) {
            return Err(err);
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

/// Announce what OpenRouter's server tools contributed, once the answer is
/// complete. Emitted only when something actually ran, so the ordinary answer
/// carries no extra event — and deliberately at the end rather than per delta,
/// since annotations arrive repeatedly and a citation strip that reshuffles
/// itself mid-answer is worse than one that appears when the answer is done.
fn emit_server_tool_trace(
    app: &tauri::AppHandle,
    event_name: &str,
    trace: &crate::openrouter::ServerToolTrace,
) {
    if trace.is_empty() {
        return;
    }
    let _ = app.emit(
        format!("{event_name}-servertools").as_str(),
        trace.to_payload(),
    );
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

    let req = client
        .get(&url)
        .timeout(REQUEST_TIMEOUT)
        .header("Content-Type", "application/json");
    let resp = openai_auth(req, provider, api_key)
        .send()
        .await
        .map_err(|e| describe_reqwest_error(&e))?;

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
    // The catalogue's own naming first; the hand-kept table only fills the gap.
    let param_billions = parse_param_billions(item).or_else(|| known_param_billions(id));
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

/// Sizes the catalogues do not carry but that the vendor has stated publicly.
///
/// Without this the UI falls back to its "assume a large model" placeholder and
/// renders `~100B`, which for a 1.6T model is not so much a rough estimate as a
/// wrong one. Keyed on the family rather than the exact id so dated snapshots
/// and variants (`-0813`, `-vision-exp`) inherit the figure instead of dropping
/// back to the placeholder.
///
/// Matched on the model id alone, so a DeepSeek model served through OpenRouter
/// (`deepseek/deepseek-v4-pro`) gets the same answer as the first-party one —
/// the parameter count is a fact about the model, not about who hosts it.
fn known_param_billions(model_id: &str) -> Option<f64> {
    let id = model_id.to_lowercase();
    // Gated on the vendor as well as the family: `v4-pro` is a generic enough
    // suffix that some unrelated model could carry it, and a confidently wrong
    // 1.6T would be worse than the honest `~100B` placeholder.
    if !id.contains("deepseek") {
        return None;
    }
    if id.contains("v4-pro") {
        return Some(1600.0);
    }
    if id.contains("v4-flash") {
        return Some(284.0);
    }
    None
}

/// Fill in a size the catalogue did not give us. Never overrides one it did:
/// a figure scanned out of the model's own name is first-hand, while this table
/// is maintained by hand and will go stale.
pub fn apply_known_param_size(model: &mut AiModel) {
    if model.param_billions.is_none() {
        model.param_billions = known_param_billions(&model.id);
    }
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

/// Capabilities inferred from a Qwen model id. QwenCloud's /models endpoint
/// reports no modalities, so the id is the only signal. `image_gen`/`video` are
/// labelled for display only — Argus does not drive those models (out of scope),
/// but the fetch dialog still shows what each id is.
fn qwen_capabilities(model_id: &str) -> Vec<String> {
    let id = model_id.to_lowercase();
    let mut caps = Vec::new();
    // Pure media-generation models: tag and stop — they are not chat/tool models.
    if id.contains("wan") || id.contains("-image") || id.contains("t2i") || id.contains("cogview") {
        add_capability(&mut caps, "image_gen");
        return caps;
    }
    if id.contains("t2v") || id.contains("i2v") || id.contains("happyhorse") {
        add_capability(&mut caps, "video");
        return caps;
    }
    // Vision: the -vl family, QvQ visual reasoning, and the omni multimodal line.
    if id.contains("-vl") || id.contains("vl-") || id.contains("qvq") || id.contains("omni") {
        add_capability(&mut caps, "vision");
    }
    // Audio understanding / speech.
    if id.contains("audio") || id.contains("omni") || id.contains("asr") || id.contains("tts") {
        add_capability(&mut caps, "audio");
    }
    // Reasoning / thinking lines.
    if id.contains("qwq") || id.contains("qvq") || id.contains("thinking") || id.contains("-r1") {
        add_capability(&mut caps, "reasoning");
    }
    // Every Qwen text/VL chat model carries OpenAI-style function calling; skip
    // the embedding models, which are not chat models.
    if id.contains("qwen") && !id.contains("embed") {
        add_capability(&mut caps, "tool_calling");
    }
    caps
}

/// Overlay id-derived Qwen capabilities onto a fetched model. Non-destructive:
/// capabilities are unioned with whatever `/models` reported.
///
/// Pricing is deliberately left untouched. QwenCloud's `/models` returns none,
/// the Token Plan bills in Credits rather than per-token CNY, and the 2026
/// flagship rates (e.g. qwen3.8-max at ¥12/¥36 per 1M) differ several-fold from
/// the classic tiers — so a name-guessed price would put a confidently wrong
/// number on the cost estimate. Users can type exact per-model prices in the UI
/// when they want a CNY figure; token counts and the cache-hit rate stay accurate.
fn enrich_qwen_model(mut m: AiModel) -> AiModel {
    for cap in qwen_capabilities(&m.id) {
        add_capability(&mut m.capabilities, &cap);
    }
    m
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
/// How long the shared client waits for a TCP + TLS connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long the shared client waits between two reads of a response body.
const READ_TIMEOUT: Duration = Duration::from_secs(180);

/// Process-wide shared client: reuses the connection pool (TCP + TLS sessions)
/// across requests instead of paying a fresh handshake per AI call, which
/// matters most for request bursts like batch analysis and embeddings.
pub(crate) fn build_client() -> Result<reqwest::Client, String> {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        // Per-read idle timeout: kills silently stalled connections without
        // capping total stream duration (long generations stay alive as long
        // as tokens keep arriving).
        .read_timeout(READ_TIMEOUT)
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
                // Anthropic has no video or audio block at all, so those go the
                // same way.
                ChatContentPart::VideoUrl { .. }
                | ChatContentPart::InputAudio { .. }
                | ChatContentPart::File { .. } => None,
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

/// Turn an HTTP error response into the message the user sees.
///
/// The headline is picked by status; the rest is what the provider said. When
/// the body is JSON with a message in it (`error.message`, a bare `error`
/// string, MiniMax's `base_resp.status_msg`, a top-level `message`), that
/// message is shown instead of a raw preview — it is far more readable — and a
/// MiniMax-coded body goes through [`crate::minimax::describe_code`], so the
/// code is explained in words and kept as ` (NNNN)` for [`classify_error`].
/// A raw 300-character preview is kept only when nothing could be extracted.
///
/// Every headline keeps its status in a form [`http_status_in`] reads.
pub(crate) fn friendly_error(status: u16, body: &str) -> String {
    let json = serde_json::from_str::<serde_json::Value>(body.trim()).ok();
    let message = json.as_ref().and_then(error_message_in);
    let code = json.as_ref().and_then(|j| minimax_code_in(j, message.as_deref()));
    let detail = match (&message, code) {
        (_, Some(code)) => crate::minimax::describe_code(code, message.as_deref().unwrap_or("")),
        (Some(m), None) => m.clone(),
        (None, None) => char_prefix(body, 300),
    };
    let extracted = message.is_some() || code.is_some();
    // A relay can wrap a permanent error in a 5xx — new-api answers "no channel
    // serves this model" as 503 `model_not_found`. The body's own name wins, so
    // it reads (and classifies) as the 404 it is rather than a busy server.
    let status = match json
        .as_ref()
        .and_then(|j| j.get("error")?.as_object())
        .and_then(embedded_status)
    {
        Some(named) if (500..=599).contains(&status) && (400..=499).contains(&named) => named,
        _ => status,
    };
    match status {
        // No preview of an unreadable body here: a 401 is self-explanatory, and
        // a raw page would only bury the advice.
        401 if extracted => format!(
            "Authentication failed (401). Check your API key in Settings → AI Providers. Response: {detail}"
        ),
        401 => "Authentication failed (401). Check your API key in Settings → AI Providers.".to_string(),
        // OpenRouter's moderated models refuse a flagged input with a 403. The
        // key is fine and the next request may pass, so it is worded — and
        // classified — as this one request's problem, not a permission error.
        403 if is_moderation_refusal(json.as_ref(), body) => format!(
            "内容被安全审核拦截（403）：这条输入被该模型的审核标记，可换用其它模型或跳过。Response: {detail}"
        ),
        403 => format!("Access denied (403). Your key may lack permission for this model. Response: {detail}"),
        404 => format!("Endpoint or model not found (404). Verify your API address and model ID. Response: {detail}"),
        // StepFun returns 402 when the account is out of credit. "Try again"
        // would be wrong advice, so it is named rather than left to the catch-all.
        402 => format!("余额不足（402）。请先充值或更换服务商。Response: {detail}"),
        // Two different 429s share the status and only the error identifier tells
        // them apart: an ordinary rate limit clears in seconds, while a credit cap
        // resets on the 1st of the month. Telling the user to wait a moment is
        // actively misleading for the second, so the body decides the wording.
        429 if body.contains("credit_limit_exceeded") => format!(
            "配额已用尽（429）。该项目的额度要到下个计费周期才会恢复，请调整额度或更换服务商。Response: {detail}"
        ),
        // MiniMax sends its throttles *and* its used-up Token Plan windows as a
        // 429; the code — already put into words in `detail` — is what says
        // which, so the headline stays neutral rather than saying "wait".
        429 if code.is_some() => format!("请求被拒绝（429）：{detail}"),
        429 if mentions_quota(&detail)
            || body.contains("insufficient_quota")
            || body.contains("exceeded_current_quota") =>
        {
            format!("额度已用尽（429），稍后重试无效，请充值、调整额度或更换服务商。Response: {detail}")
        }
        429 => format!("Rate limited (429). Please wait a moment and try again. Response: {detail}"),
        // StepFun's content-moderation refusal, on the request or the response.
        // Most OpenAI-compatible clients have no mapping for it.
        451 => format!("内容被安全策略拦截（451）。请调整提问或附件后重试。Response: {detail}"),
        // Overload (MiniMax's peak-hour 529, Anthropic's 529) and gateway
        // trouble: worth one more try a little later.
        502..=504 | 520..=529 => format!("服务端繁忙（{status}），通常稍后重试即可：{detail}"),
        // A plain server fault may or may not clear (Ollama answers 500 when a
        // model does not fit in memory), so no promise either way.
        500..=599 => format!("服务端出错（{status}）：{detail}"),
        _ => format!("API error {status}: {detail}"),
    }
}

/// A 403 that is a moderation refusal of this input rather than a key without
/// permission: OpenRouter's "… requires moderation … Your input was flagged".
fn is_moderation_refusal(json: Option<&serde_json::Value>, body: &str) -> bool {
    let lower = body.to_lowercase();
    lower.contains("requires moderation")
        || lower.contains("was flagged")
        || json.is_some_and(|j| {
            let meta = &j["error"]["metadata"];
            meta["reasons"].is_array() || meta["flagged_input"].is_string()
        })
}

/// Wording that means an account, key or plan has run out — not a throttle.
/// Shared by [`classify_error`] and the 429 headline in [`friendly_error`].
const QUOTA_WORDING: &[&str] = &[
    "usage limit exceeded",
    "用量上限",
    "额度已用尽",
    "配额已用尽",
    "余额不足",
    "credit_limit_exceeded",
    "insufficient_quota",
    "insufficient balance",
];

fn mentions_quota(text: &str) -> bool {
    let lower = text.to_lowercase();
    QUOTA_WORDING.iter().any(|k| lower.contains(k))
}

/// The provider's own words in a JSON error body, from wherever it put them:
/// `error.message` (OpenAI, Anthropic, MiniMax's HTTP envelope), a bare
/// `error` string (Ollama), `base_resp.status_msg` (MiniMax on a 200), or a
/// top-level `message` / `msg` / `detail`.
///
/// Capped at 500 characters: a validator that echoes a whole schema back
/// should not fill the screen. A MiniMax code cut off the end by the cap is
/// not lost — [`minimax_code_in`] reads it from the raw field, and
/// [`crate::minimax::describe_code`] puts it back.
fn error_message_in(json: &serde_json::Value) -> Option<String> {
    let text = |v: &serde_json::Value| {
        v.as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| char_prefix(s, 500))
    };
    let err = &json["error"];
    if let Some(msg) = text(&err["message"]) {
        // OpenRouter reports an upstream refusal as "Provider returned error"
        // and puts the upstream's actual reason in `metadata.raw`.
        if let Some(raw) = text(&err["metadata"]["raw"]) {
            let who = text(&err["metadata"]["provider_name"]).unwrap_or_else(|| "upstream".into());
            return Some(format!("{msg} — {who}: {}", char_prefix(&raw, 300)));
        }
        return Some(msg);
    }
    if let Some(msg) = text(err) {
        return Some(msg);
    }
    if let Some((_, msg)) = crate::minimax::base_resp_error(json) {
        if !msg.is_empty() {
            return Some(msg);
        }
    }
    ["message", "msg", "detail"]
        .iter()
        .find_map(|k| text(&json[*k]))
}

/// The MiniMax business code a JSON body carries: `base_resp.status_code`, or
/// the `(NNNN)` MiniMax ends its `error.message` with.
fn minimax_code_in(json: &serde_json::Value, message: Option<&str>) -> Option<u32> {
    if let Some((code, _)) = crate::minimax::base_resp_error(json) {
        return u32::try_from(code).ok();
    }
    json["error"]["message"]
        .as_str()
        .and_then(crate::minimax::trailing_code)
        .or_else(|| message.and_then(crate::minimax::trailing_code))
}

/// An error a provider reported *inside* a body that otherwise arrived as a
/// success — a non-streaming reply with HTTP 200, or one SSE chunk of a stream.
///
/// Two shapes are recognised:
///   * MiniMax's `base_resp` with a non-zero `status_code` (a number or a
///     numeric string) — it answers business errors that way with a 200 and no
///     `choices`, and `status_code: 0` rides every successful response;
///   * a top-level `error` *object* with a non-empty string `message` — the
///     OpenAI / Anthropic / OpenRouter shape, including MiniMax's
///     `{type:"error", error:{…}}` envelope. `"error": null`, an empty object
///     and a non-object are all ignored, so ordinary chunks never trip it.
///
/// The message keeps the markers [`classify_error`] reads: MiniMax's `(NNNN)`,
/// and the HTTP status when the error object names one (`code: 429`,
/// `http_code: "529"`, or an Anthropic-style `type` such as `overloaded_error`),
/// in which case it is worded exactly as [`friendly_error`] would word that
/// status.
pub(crate) fn provider_error_in_body(json: &serde_json::Value) -> Option<String> {
    body_error(json).map(|(_, msg)| msg)
}

/// [`provider_error_in_body`], plus the HTTP status the body itself names, for
/// the one caller that needs it ([`looks_like_tools_rejected`] in
/// [`stream_with_tools`]).
fn body_error(json: &serde_json::Value) -> Option<(Option<u16>, String)> {
    if let Some((code, msg)) = crate::minimax::base_resp_error(json) {
        let text = match u32::try_from(code) {
            Ok(code) => crate::minimax::describe_code(code, &msg),
            Err(_) => format!("服务商返回错误（错误码 {code}）：{msg}"),
        };
        return Some((None, text));
    }
    let err = json.get("error")?.as_object()?;
    let message = err
        .get("message")
        .and_then(|m| m.as_str())
        .map(str::trim)
        .filter(|m| !m.is_empty())?;
    let status = embedded_status(err);
    let text = match status {
        Some(status) => friendly_error(status, &json.to_string()),
        None => match crate::minimax::trailing_code(message) {
            Some(code) => crate::minimax::describe_code(code, &char_prefix(message, 500)),
            None => format!(
                "服务商返回错误：{}",
                error_message_in(json).unwrap_or_else(|| message.to_string())
            ),
        },
    };
    Some((status, text))
}

/// The HTTP status an in-body error object names, if any: a numeric
/// `http_code` / `status` / `code` in the 4xx–5xx range (as a number or a
/// numeric string), else a well-known error `type` / `code` name.
fn embedded_status(err: &serde_json::Map<String, serde_json::Value>) -> Option<u16> {
    for key in ["http_code", "status", "code"] {
        let Some(v) = err.get(key) else { continue };
        let n = v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()));
        if let Some(n) = n.filter(|n| (400..=599).contains(n)) {
            return Some(n as u16);
        }
    }
    for key in ["type", "code"] {
        let Some(name) = err.get(key).and_then(|v| v.as_str()) else { continue };
        let status = match name {
            // Anthropic, OpenAI and Moonshot (Kimi) spellings.
            "overloaded_error" => 529,
            "api_error" | "server_error" | "internal_error" | "internal_server_error" => 500,
            "rate_limit_error"
            | "rate_limit_exceeded"
            | "rate_limit_reached_error"
            | "engine_overloaded_error"
            | "insufficient_quota"
            | "exceeded_current_quota_error" => 429,
            "authentication_error" | "invalid_api_key" | "invalid_authentication_error" => 401,
            "permission_error" | "permission_denied_error" => 403,
            "not_found_error" | "model_not_found" | "resource_not_found_error" => 404,
            "request_too_large" => 413,
            "invalid_request_error" => 400,
            _ => continue,
        };
        return Some(status);
    }
    None
}

/// A failed `send()` or body read, in words.
///
/// For requests sent with [`REQUEST_TIMEOUT`]; streaming requests, which have
/// no total timeout, use [`describe_stream_error`]. Timeouts read `请求超时`
/// and everything else `Network error:`, both of which [`classify_error`]
/// treats as transient, and the error's source chain is spelled out — the
/// top-level text alone ("error sending request for url (…)") never says
/// whether it was DNS, TLS or a refused connection.
pub(crate) fn describe_reqwest_error(e: &reqwest::Error) -> String {
    describe_reqwest_error_within(e, Some(REQUEST_TIMEOUT))
}

/// [`describe_reqwest_error`] for requests with no total timeout (streams, and
/// the few one-shot calls sent without one): the only timeouts that can fire
/// are the client's connect and between-reads limits.
fn describe_stream_error(e: &reqwest::Error) -> String {
    describe_reqwest_error_within(e, None)
}

fn describe_reqwest_error_within(e: &reqwest::Error, total: Option<Duration>) -> String {
    let chain = error_chain(e);
    // Never left the machine: a malformed API address, or a key with a
    // character a header cannot carry. Not a network fault, and no retry will
    // fix it, so it is not worded as one.
    if e.is_builder() {
        return format!("请求无法发出，请检查 API 地址和密钥是否填写正确: {chain}");
    }
    if e.is_timeout() {
        if e.is_connect() {
            return format!(
                "请求超时（{} 秒内未能连上服务器）: {chain}",
                CONNECT_TIMEOUT.as_secs()
            );
        }
        return match total {
            Some(total) => format!("请求超时（{} 秒内未完成）: {chain}", total.as_secs()),
            None => format!(
                "请求超时（{} 秒内没有收到任何数据）: {chain}",
                READ_TIMEOUT.as_secs()
            ),
        };
    }
    if e.is_connect() {
        return format!("Network error: 无法连接到服务器 — {chain}");
    }
    format!("Network error: {chain}")
}

/// An error and its `source()` chain, joined with `: `, skipping a link whose
/// text the chain already contains (hyper and std often repeat each other).
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(inner) = source {
        let text = inner.to_string();
        if !text.is_empty() && !out.contains(&text) {
            out.push_str(": ");
            out.push_str(&text);
        }
        source = inner.source();
    }
    out
}

/// Drop a leading `<think>…</think>` block (after optional whitespace) from a
/// reply, along with the whitespace that follows it.
///
/// For the one-shot callers — the arXiv digest, translation, titles, section
/// outlines, canvas edge suggestions — none of which wants the chain of
/// thought: a model that inlines it (MiniMax without `reasoning_split`, many
/// open-weight models behind a generic gateway) would otherwise put it at the
/// top of a summary or into a parsed JSON reply. An opening tag with no close
/// is left alone rather than returning nothing: that is either a reply cut off
/// mid-thought or a literal `<think>` the answer talks about.
fn strip_leading_think(s: &str) -> &str {
    const OPEN: &str = "<think>";
    const CLOSE: &str = "</think>";
    let Some(rest) = s.trim_start().strip_prefix(OPEN) else {
        return s;
    };
    match rest.find(CLOSE) {
        Some(end) => rest[end + CLOSE.len()..].trim_start(),
        None => s,
    }
}

/// Why a successful non-streaming reply carried no text, as an error message.
///
/// Every message here is per-request ([`ErrorClass::Request`]): none contains
/// a status, a vendor code, or quota / network wording.
fn empty_reply_error(json: &serde_json::Value, text: &str, reasoning: &str) -> String {
    let choice = &json["choices"][0];
    let finish = choice["finish_reason"].as_str().unwrap_or("");
    if finish == "length" {
        let why = if reasoning.trim().is_empty() {
            ""
        } else {
            "（思考过程占满了输出长度）"
        };
        return format!(
            "输出被截断（finish_reason=length）：模型在写出正文之前就达到了输出长度上限{why}。可换用不思考的模型、缩短输入或调高输出长度后重试"
        );
    }
    let flagged = |v: &serde_json::Value| v.as_bool() == Some(true);
    if matches!(finish, "content_filter" | "content_filtered" | "sensitive")
        || flagged(&json["input_sensitive"])
        || flagged(&json["output_sensitive"])
    {
        let why = if finish.is_empty() {
            "输入/输出涉敏".to_string()
        } else {
            format!("finish_reason={finish}")
        };
        return format!("内容被安全策略拦截（{why}），模型没有返回正文。请调整输入后重试");
    }
    if !reasoning.trim().is_empty() {
        return "模型只返回了思考过程、没有正文。可换用不思考的模型或调高输出长度后重试".to_string();
    }
    format!("Unexpected response format from API: {}", char_prefix(text, 200))
}

/// What a non-streaming reply's message reasoned, under whichever field the
/// provider uses: `reasoning_content` (DeepSeek, Kimi, MiMo…), `reasoning`
/// (OpenRouter), `thinking`, or MiniMax's `reasoning_details[].text`.
fn reply_reasoning(message: &serde_json::Value) -> String {
    if let Some(r) = message["reasoning_content"]
        .as_str()
        .or_else(|| message["reasoning"].as_str())
        .or_else(|| message["thinking"].as_str())
        .filter(|s| !s.trim().is_empty())
    {
        return r.to_string();
    }
    message["reasoning_details"]
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default()
}

/// What is appended to a streamed answer the provider broke off with an error
/// event. Once text is on screen, failing the call would make the chat replace
/// the partial answer with the error; ending normally keeps it, still records
/// the usage that was billed, and says why it stops short.
fn interrupted_notice(err: &str) -> String {
    format!("\n\n（回答中断：{err}）")
}

/// Upper bound on the non-SSE text a stream keeps for [`stray_body_error`].
const STRAY_BODY_CAP: usize = 64 * 1024;

/// Keep a body line that is not an SSE field, in case the whole response turns
/// out to be a plain JSON error sent with a 200 instead of an event stream.
/// Bounded, and a no-op for every line a real event stream sends.
fn keep_stray_line(stray: &mut String, line: &str) {
    let t = line.trim();
    if t.is_empty() || stray.len() >= STRAY_BODY_CAP || t.starts_with(':') {
        return;
    }
    if ["data:", "event:", "id:", "retry:"].iter().any(|f| t.starts_with(f)) {
        return;
    }
    stray.push_str(line);
    stray.push('\n');
}

/// The error in a stream that ended without producing anything, when what it
/// sent was a JSON body rather than events: the kept stray lines plus whatever
/// was left unterminated in the buffer (also tried on its own, minus a
/// `data:` prefix, for a last event that never got its newline).
fn stray_body_error(stray: &str, tail: &str) -> Option<String> {
    let tail = tail.trim();
    let whole = format!("{stray}{tail}");
    let last_event = tail.strip_prefix("data:").unwrap_or("");
    // Bound to a local so the iterator is dropped before `whole` is.
    let found = [whole.as_str(), last_event].into_iter().find_map(|candidate| {
        let candidate = candidate.trim();
        if !candidate.starts_with('{') {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(candidate)
            .ok()
            .and_then(|json| provider_error_in_body(&json))
    });
    found
}

/// How a caller that can wait and send the request again should treat a failed
/// LLM call. Only batch jobs act on it (the arXiv digest); interactive callers
/// keep showing the message as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// Throttling, overload, timeouts, 5xx, dropped connections: the same
    /// request is likely to succeed if it is sent again a little later.
    Transient,
    /// Provider-wide and not going to clear by retrying soon: a bad key, an
    /// empty balance, a plan whose quota window is used up, a missing model.
    /// Every further request would fail the same way, so a batch should stop.
    Fatal,
    /// Specific to this one request: bad input, moderation, a malformed reply.
    Request,
}

/// Classify an error string produced by this module.
///
/// The messages are ours — [`friendly_error`], the network and body checks in
/// the request functions — so the markers read here are the ones they write:
/// a vendor code in parentheses (MiniMax appends one to every message, e.g.
/// `…请稍后重试 (2064)`), quota wording, the HTTP status, then network wording.
/// The order matters: a 429 that is a used-up plan window must be `Fatal`, not
/// the `Transient` an ordinary 429 is.
pub fn classify_error(msg: &str) -> ErrorClass {
    if let Some(class) = parenthesized_numbers(msg, 4)
        .into_iter()
        .rev()
        .find_map(crate::minimax::code_class)
    {
        return class;
    }

    let lower = msg.to_lowercase();
    if mentions_quota(msg) {
        return ErrorClass::Fatal;
    }

    // A moderation refusal is about this input, whatever status carried it
    // (StepFun's 451, OpenRouter's 403).
    if msg.contains("内容被安全") || lower.contains("requires moderation") {
        return ErrorClass::Request;
    }

    if let Some(status) = http_status_in(msg) {
        return match status {
            401..=404 => ErrorClass::Fatal,
            408 | 409 | 425 | 429 => ErrorClass::Transient,
            400..=499 => ErrorClass::Request,
            // "Not implemented" / "version not supported" will not change.
            501 | 505 => ErrorClass::Request,
            _ => ErrorClass::Transient,
        };
    }

    const TRANSIENT: &[&str] = &[
        "network error",
        "请求超时",
        "timed out",
        "overloaded",
        "rate limit",
        "服务器繁忙",
        "服务繁忙",
        "temporarily unavailable",
    ];
    if TRANSIENT.iter().any(|k| lower.contains(k)) {
        return ErrorClass::Transient;
    }
    ErrorClass::Request
}

/// Every run of exactly `digits` ASCII digits enclosed in `(…)` or `（…）`.
fn parenthesized_numbers(msg: &str, digits: usize) -> Vec<u32> {
    let chars: Vec<char> = msg.chars().collect();
    let mut out = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c != '(' && c != '（' {
            continue;
        }
        let run: String = chars[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        let close = chars.get(i + 1 + run.len());
        if run.len() == digits && matches!(close, Some(')') | Some('）')) {
            if let Ok(n) = run.parse() {
                out.push(n);
            }
        }
    }
    out
}

/// The HTTP status a message from [`friendly_error`] carries: `(429)`,
/// `（402）`, or `API error 529:`.
fn http_status_in(msg: &str) -> Option<u16> {
    let in_status_range = |n: u32| (400..=599).contains(&n);
    if let Some(n) = parenthesized_numbers(msg, 3).into_iter().find(|n| in_status_range(*n)) {
        return Some(n as u16);
    }
    for marker in ["API error ", "HTTP "] {
        if let Some(pos) = msg.find(marker) {
            let digits: String = msg[pos + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(n) = digits.parse::<u32>() {
                if digits.len() == 3 && in_status_range(n) {
                    return Some(n as u16);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod error_class_tests {
    use super::{classify_error, friendly_error, ErrorClass};

    #[test]
    fn the_minimax_peak_hour_529_is_transient() {
        let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"当前为整点高峰时段，服务器短暂繁忙，通常 1-5 分钟内恢复。请稍后重试 (2064)","http_code":"529"}}"#;
        assert_eq!(classify_error(&friendly_error(529, body)), ErrorClass::Transient);
    }

    #[test]
    fn a_used_up_token_plan_window_is_fatal_even_as_a_429() {
        let body = r#"{"type":"error","error":{"type":"rate_limit_error","message":"usage limit exceeded, 5-hour usage limit reached for Token Plan Plus (0/0 used), resets at 2026-05-15T15:00:00Z (2056)"}}"#;
        assert_eq!(classify_error(&friendly_error(429, body)), ErrorClass::Fatal);
        assert_eq!(
            classify_error("已达到 Token Plan 用量上限：请升级 Token Plan 套餐或购买积分补充用量。(2056)"),
            ErrorClass::Fatal
        );
    }

    #[test]
    fn statuses_map_to_the_expected_class() {
        for (status, class) in [
            (401, ErrorClass::Fatal),
            (402, ErrorClass::Fatal),
            (403, ErrorClass::Fatal),
            (404, ErrorClass::Fatal),
            (408, ErrorClass::Transient),
            (429, ErrorClass::Transient),
            (400, ErrorClass::Request),
            (422, ErrorClass::Request),
            (451, ErrorClass::Request),
            (500, ErrorClass::Transient),
            (502, ErrorClass::Transient),
            (503, ErrorClass::Transient),
            (504, ErrorClass::Transient),
            (529, ErrorClass::Transient),
        ] {
            assert_eq!(
                classify_error(&friendly_error(status, "{}")),
                class,
                "status {status}: {}",
                friendly_error(status, "{}")
            );
        }
        assert_eq!(
            classify_error(&friendly_error(429, r#"{"error":{"code":"credit_limit_exceeded"}}"#)),
            ErrorClass::Fatal
        );
    }

    #[test]
    fn minimax_codes_decide_before_anything_else() {
        assert_eq!(classify_error("请求频率超限 (1002)"), ErrorClass::Transient);
        assert_eq!(classify_error("已达到 Token Plan 速率限制 (2062)"), ErrorClass::Transient);
        assert_eq!(classify_error("invalid api key (2049)"), ErrorClass::Fatal);
        assert_eq!(classify_error("余额不足 (1008)"), ErrorClass::Fatal);
        assert_eq!(classify_error("input new_sensitive (1026)"), ErrorClass::Request);
        assert_eq!(classify_error("invalid params (2013)"), ErrorClass::Request);
        // A year in parentheses is not a code, and does not hide the status.
        assert_eq!(classify_error("Rate limited (429) since (2026)"), ErrorClass::Transient);
    }

    #[test]
    fn a_relay_wrapping_model_not_found_in_a_503_is_fatal() {
        let body = r#"{"error":{"message":"分组 default 下模型 x 无可用渠道（distributor）","type":"new_api_error","code":"model_not_found"}}"#;
        let msg = friendly_error(503, body);
        assert!(msg.contains("(404)"), "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Fatal);
        // A real overload keeps its own status.
        let overload = r#"{"type":"error","error":{"type":"overloaded_error","message":"busy","http_code":"529"}}"#;
        assert_eq!(classify_error(&friendly_error(529, overload)), ErrorClass::Transient);
    }

    #[test]
    fn a_plain_server_fault_promises_nothing_and_not_implemented_is_per_request() {
        let msg = friendly_error(500, r#"{"error":"model requires more system memory (5.5 GiB) than is available"}"#);
        assert!(msg.starts_with("服务端出错（500）"), "{msg}");
        assert!(!msg.contains("重试即可"), "{msg}");
        assert_eq!(classify_error(&friendly_error(501, "{}")), ErrorClass::Request);
    }

    #[test]
    fn a_moderation_403_fails_only_that_request() {
        let body = r#"{"error":{"code":403,"message":"openai/gpt-5 requires moderation on OpenAI. Your input was flagged for \"violence\". No credits were charged.","metadata":{"reasons":["violence"],"flagged_input":"..."}}}"#;
        let msg = friendly_error(403, body);
        assert!(msg.starts_with("内容被安全审核拦截（403）"), "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Request);
        // A genuine permission 403 still stops a batch.
        assert_eq!(classify_error(&friendly_error(403, r#"{"error":{"message":"no access"}}"#)), ErrorClass::Fatal);
    }

    #[test]
    fn network_failures_are_transient_and_the_rest_is_per_request() {
        assert_eq!(
            classify_error("Network error: error sending request for url (https://x/v1)"),
            ErrorClass::Transient
        );
        assert_eq!(classify_error("请求超时（120 秒内未完成）"), ErrorClass::Transient);
        assert_eq!(classify_error("Unexpected response format from API"), ErrorClass::Request);
        assert_eq!(classify_error("Invalid JSON from API: expected value"), ErrorClass::Request);
    }
}

#[cfg(test)]
mod provider_error_tests {
    use super::*;
    use serde_json::json;

    const PEAK_529: &str = r#"{"type":"error","error":{"type":"overloaded_error","message":"当前为整点高峰时段，服务器短暂繁忙，通常 1-5 分钟内恢复。请稍后重试 (2064)","http_code":"529"},"request_id":"abc"}"#;
    const WINDOW_429: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"usage limit exceeded, 5-hour usage limit reached for Token Plan Plus (0/0 used), resets at 2026-05-15T15:00:00Z (2056)"}}"#;

    // ── provider_error_in_body ──────────────────────────────────────────────

    #[test]
    fn a_base_resp_with_a_numeric_code_is_an_error() {
        let body = json!({
            "id": "x", "choices": null,
            "base_resp": {"status_code": 2056, "status_msg": "usage limit exceeded, resets at 2026-05-15T15:00:00Z"}
        });
        let err = provider_error_in_body(&body).expect("base_resp error");
        assert!(err.contains("usage limit exceeded"), "{err}");
        assert!(err.ends_with("(2056)"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Fatal);
    }

    #[test]
    fn a_base_resp_code_may_be_a_string_and_is_not_repeated() {
        let body = json!({"base_resp": {"status_code": "2064", "status_msg": "服务器繁忙，请稍后重试 (2064)"}});
        let err = provider_error_in_body(&body).expect("base_resp error");
        assert_eq!(err.matches("(2064)").count(), 1, "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);

        let body = json!({"base_resp": {"status_code": "1002", "status_msg": "rate limit exceeded(RPM)"}});
        let err = provider_error_in_body(&body).unwrap();
        assert!(err.contains("(1002)"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);
    }

    #[test]
    fn a_successful_base_resp_is_ignored() {
        for code in [json!(0), json!("0")] {
            let body = json!({
                "choices": [{"message": {"content": "hi"}}],
                "base_resp": {"status_code": code, "status_msg": ""}
            });
            assert_eq!(provider_error_in_body(&body), None);
        }
        // Not a number at all: not something to act on.
        assert_eq!(provider_error_in_body(&json!({"base_resp": {"status_code": "ok"}})), None);
    }

    #[test]
    fn an_error_object_with_a_message_is_an_error() {
        // OpenRouter's mid-stream shape: numeric code, a chunk around it.
        let chunk = json!({
            "id": "gen-1", "object": "chat.completion.chunk", "provider": "X",
            "error": {"code": 502, "message": "Provider disconnected"},
            "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "error"}]
        });
        let err = provider_error_in_body(&chunk).expect("error object");
        assert!(err.contains("（502）") && err.contains("Provider disconnected"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);

        // Anthropic's `event: error`: no number, but the type names the status.
        let event = json!({"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}});
        let err = provider_error_in_body(&event).unwrap();
        assert!(err.contains("（529）"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);

        // MiniMax's envelope on a 200.
        let err = provider_error_in_body(&serde_json::from_str(PEAK_529).unwrap()).unwrap();
        assert_eq!(classify_error(&err), ErrorClass::Transient);
        let err = provider_error_in_body(&serde_json::from_str(WINDOW_429).unwrap()).unwrap();
        assert_eq!(classify_error(&err), ErrorClass::Fatal);

        // Nothing to go on but the words: shown as is, per-request.
        let err = provider_error_in_body(&json!({"error": {"message": "something odd"}})).unwrap();
        assert!(err.contains("something odd"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Request);
    }

    #[test]
    fn ordinary_chunks_are_not_errors() {
        for body in [
            json!({"id": "c", "choices": [{"delta": {"content": "hi"}}], "error": null}),
            json!({"error": {}}),
            json!({"error": {"message": "   "}}),
            json!({"error": "a bare string is not the in-body shape"}),
            json!({"id": "gen-1", "provider": "OpenAI", "choices": [{"delta": {"content": "x"}}],
                   "usage": {"prompt_tokens": 1, "completion_tokens": 1, "cost": 0.0}}),
            json!({"type": "content_block_delta", "delta": {"type": "text_delta", "text": "x"}}),
            json!([1, 2, 3]),
        ] {
            assert_eq!(provider_error_in_body(&body), None, "{body}");
        }
    }

    // ── friendly_error ──────────────────────────────────────────────────────

    #[test]
    fn the_peak_hour_529_reads_as_a_busy_server_in_minimaxs_words() {
        let msg = friendly_error(529, PEAK_529);
        assert!(msg.starts_with("服务端繁忙（529），通常稍后重试即可："), "{msg}");
        assert!(msg.contains("当前为整点高峰时段"), "{msg}");
        assert_eq!(msg.matches("(2064)").count(), 1, "{msg}");
        // The provider's message replaces the raw JSON preview.
        assert!(!msg.contains("request_id"), "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Transient);
    }

    #[test]
    fn a_used_up_window_is_not_told_to_wait_a_moment() {
        let msg = friendly_error(429, WINDOW_429);
        assert!(msg.contains("（429）"), "{msg}");
        assert!(msg.contains("Token Plan 额度已用尽"), "{msg}");
        assert!(msg.contains("resets at 2026-05-15T15:00:00Z"), "{msg}");
        assert!(!msg.contains("Rate limited"), "{msg}");
        assert_eq!(msg.matches("(2056)").count(), 1, "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Fatal);

        // A Token Plan throttle on the same status stays transient.
        let throttle = r#"{"type":"error","error":{"type":"rate_limit_error","message":"已达到 Token Plan 速率限制 (2062)"}}"#;
        let msg = friendly_error(429, throttle);
        assert!(msg.contains("限流"), "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Transient);
    }

    #[test]
    fn a_plain_503_is_a_busy_server() {
        let msg = friendly_error(503, "Service Unavailable");
        assert_eq!(msg, "服务端繁忙（503），通常稍后重试即可：Service Unavailable");
        assert_eq!(classify_error(&msg), ErrorClass::Transient);
        for status in [500, 502, 504, 520] {
            let msg = friendly_error(status, "<html>bad gateway</html>");
            assert!(msg.contains(&format!("（{status}）")), "{msg}");
            assert_eq!(classify_error(&msg), ErrorClass::Transient);
        }
    }

    #[test]
    fn existing_headlines_and_their_markers_survive() {
        let m = friendly_error(401, "bad key");
        assert_eq!(m, "Authentication failed (401). Check your API key in Settings → AI Providers.");
        let m = friendly_error(401, r#"{"error":{"message":"invalid api key (2049)"}}"#);
        assert!(m.starts_with("Authentication failed (401)."), "{m}");
        assert!(m.contains("invalid api key"), "{m}");
        assert!(friendly_error(403, "x").contains("(403)"));
        assert!(friendly_error(404, "x").contains("(404)"));
        assert!(friendly_error(402, "x").contains("余额不足（402）"));
        assert!(friendly_error(451, "x").contains("（451）"));
        assert!(friendly_error(429, r#"{"error":{"code":"credit_limit_exceeded"}}"#)
            .starts_with("配额已用尽（429）"));
        let m = friendly_error(429, r#"{"error":{"message":"Rate limit reached for requests"}}"#);
        assert!(m.starts_with("Rate limited (429)."), "{m}");
        assert!(m.ends_with("Response: Rate limit reached for requests"), "{m}");
        assert_eq!(friendly_error(418, "teapot"), "API error 418: teapot");
    }

    #[test]
    fn the_providers_message_replaces_the_raw_preview() {
        let body = r#"{"error":{"message":"The model `x` does not exist","type":"invalid_request_error","param":null}}"#;
        assert_eq!(
            friendly_error(404, body),
            "Endpoint or model not found (404). Verify your API address and model ID. Response: The model `x` does not exist"
        );
        // A bare `error` string (Ollama) and a top-level `message` are read too.
        assert!(friendly_error(400, r#"{"error":"model not found"}"#).ends_with(": model not found"));
        assert!(friendly_error(400, r#"{"code":"x","message":"bad things"}"#).ends_with(": bad things"));
        // OpenRouter's upstream reason is kept alongside its generic wrapper.
        let body = r#"{"error":{"message":"Provider returned error","code":400,"metadata":{"raw":"context too long","provider_name":"Foo"}}}"#;
        assert!(friendly_error(400, body).contains("Provider returned error — Foo: context too long"));
        // OpenAI's out-of-credit 429 is not a throttle.
        let body = r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota","code":"insufficient_quota"}}"#;
        let m = friendly_error(429, body);
        assert!(m.starts_with("额度已用尽（429）"), "{m}");
        assert_eq!(classify_error(&m), ErrorClass::Fatal);
    }

    // ── MiniMax codes through the whole chain ───────────────────────────────

    #[test]
    fn minimax_codes_keep_their_class_once_described() {
        for (code, class) in [
            (2064, ErrorClass::Transient),
            (2062, ErrorClass::Transient),
            (1002, ErrorClass::Transient),
            (2056, ErrorClass::Fatal),
            (1008, ErrorClass::Fatal),
            (2049, ErrorClass::Fatal),
            (1026, ErrorClass::Request),
            (1027, ErrorClass::Request),
            (2013, ErrorClass::Request),
        ] {
            let body = json!({"base_resp": {"status_code": code, "status_msg": "x"}});
            let err = provider_error_in_body(&body).unwrap();
            assert_eq!(classify_error(&err), class, "{code}: {err}");
            // And as an HTTP error body, under a status that would say otherwise.
            let http = friendly_error(400, &body.to_string());
            assert_eq!(classify_error(&http), class, "{code}: {http}");
        }
    }

    // ── empty replies and <think> ───────────────────────────────────────────

    #[test]
    fn a_leading_think_block_is_stripped() {
        assert_eq!(strip_leading_think("<think>plan</think>\n\nAnswer"), "Answer");
        assert_eq!(strip_leading_think("  \n<think>a\nb</think>Answer"), "Answer");
        assert_eq!(strip_leading_think("Answer"), "Answer");
        // Not leading: part of the answer.
        assert_eq!(strip_leading_think("See <think>x</think>"), "See <think>x</think>");
        // Unterminated: left alone rather than emptied.
        assert_eq!(strip_leading_think("<think>cut off"), "<think>cut off");
        // Only a thought: nothing left, which the caller reports.
        assert_eq!(strip_leading_think("<think>only</think>  "), "");
    }

    #[test]
    fn empty_replies_explain_themselves_and_are_per_request() {
        let length = json!({"choices": [{"message": {"content": null, "reasoning_content": "long"}, "finish_reason": "length"}]});
        let filter = json!({"choices": [{"message": {"content": ""}, "finish_reason": "content_filter"}]});
        let sensitive = json!({"choices": [{"message": {"content": ""}, "finish_reason": "stop"}], "output_sensitive": true});
        let thought = json!({"choices": [{"message": {"content": "", "reasoning_content": "hmm"}, "finish_reason": "stop"}]});
        let odd = json!({"choices": [], "object": "chat.completion"});

        let cases = [
            (empty_reply_error(&length, &length.to_string(), "long"), "finish_reason=length"),
            (empty_reply_error(&filter, &filter.to_string(), ""), "安全策略"),
            (empty_reply_error(&sensitive, &sensitive.to_string(), ""), "安全策略"),
            (empty_reply_error(&thought, &thought.to_string(), "hmm"), "只返回了思考过程"),
            (empty_reply_error(&odd, &odd.to_string(), ""), "Unexpected response format from API: "),
        ];
        for (msg, needle) in cases {
            assert!(msg.contains(needle), "{msg}");
            assert_eq!(classify_error(&msg), ErrorClass::Request, "{msg}");
        }
    }

    #[test]
    fn reasoning_is_read_from_every_field_providers_use() {
        assert_eq!(reply_reasoning(&json!({"reasoning_content": "a"})), "a");
        assert_eq!(reply_reasoning(&json!({"reasoning": "b"})), "b");
        assert_eq!(
            reply_reasoning(&json!({"reasoning_details": [{"type": "reasoning.text", "text": "c"}, {"text": "d"}]})),
            "cd"
        );
        assert_eq!(reply_reasoning(&json!({"content": "x"})), "");
    }

    // ── network failures ────────────────────────────────────────────────────

    #[test]
    fn our_network_wordings_are_transient() {
        for msg in [
            "请求超时（120 秒内未完成）: error sending request for url (https://api.minimax.cn/v1/chat/completions)",
            "请求超时（30 秒内未能连上服务器）: error sending request for url (https://x/v1)",
            "请求超时（180 秒内没有收到任何数据）: error decoding response body",
            "Network error: 无法连接到服务器 — error sending request: tcp connect error: Connection refused (os error 61)",
            "读取响应失败: Network error: error decoding response body",
            "Stream read error: Network error: error decoding response body",
        ] {
            assert_eq!(classify_error(msg), ErrorClass::Transient, "{msg}");
        }
    }

    #[tokio::test]
    async fn a_timed_out_request_is_described_as_a_timeout() {
        // Accepts the connection and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            let (_sock, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(300))
            .build()
            .unwrap();
        let e = client.get(format!("http://{addr}/")).send().await.unwrap_err();
        let msg = describe_reqwest_error(&e);
        assert!(msg.starts_with("请求超时（120 秒内未完成）"), "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Transient);
        hold.abort();
    }

    #[tokio::test]
    async fn a_refused_connection_is_described_as_one() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let e = reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .unwrap_err();
        let msg = describe_reqwest_error(&e);
        assert!(msg.starts_with("Network error: 无法连接到服务器 — "), "{msg}");
        // The chain says why, not only "error sending request".
        assert!(msg.matches(": ").count() >= 1, "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Transient);
    }

    #[tokio::test]
    async fn a_malformed_address_is_not_a_network_fault() {
        let e = reqwest::Client::new().get("not a url").send().await.unwrap_err();
        let msg = describe_reqwest_error(&e);
        assert!(msg.starts_with("请求无法发出"), "{msg}");
        assert_eq!(classify_error(&msg), ErrorClass::Request);
    }

    // ── chat_openai_compat end to end, against a canned server ──────────────

    /// Serve one canned HTTP response on a loopback port and return the base
    /// URL. `declared_len` overrides Content-Length, to simulate a body cut off.
    async fn serve_once(status: &str, body: &str, declared_len: Option<usize>) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            declared_len.unwrap_or(body.len())
        );
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Read the whole request first, so closing never resets a send.
            let mut req = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&req);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if req.len() >= head_end + 4 + len {
                        break;
                    }
                }
            }
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.shutdown().await;
        });
        format!("http://{addr}/v1")
    }

    fn minimax_at(base_url: &str) -> AiProvider {
        serde_json::from_value(json!({
            "id": "mm", "name": "MiniMax", "kind": "minimax",
            "base_url": base_url, "created_at": ""
        }))
        .unwrap()
    }

    async fn ask(base_url: &str) -> Result<String, String> {
        let messages = vec![ChatMessage {
            role: "user".into(),
            content: ChatContent::Text("hi".into()),
        }];
        chat_openai_compat(&minimax_at(base_url), "k", "MiniMax-M2.7", &messages, "test").await
    }

    #[tokio::test]
    async fn a_base_resp_error_on_a_200_fails_the_call() {
        let body = r#"{"id":"1","choices":null,"base_resp":{"status_code":2056,"status_msg":"usage limit exceeded, 5-hour usage limit reached"}}"#;
        let err = ask(&serve_once("200 OK", body, None).await).await.unwrap_err();
        assert!(err.contains("(2056)"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Fatal);
    }

    #[tokio::test]
    async fn the_529_envelope_fails_the_call_as_transient() {
        let err = ask(&serve_once("529 Site Overloaded", PEAK_529, None).await).await.unwrap_err();
        assert!(err.starts_with("服务端繁忙（529）"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);
    }

    #[tokio::test]
    async fn an_inlined_chain_of_thought_is_dropped() {
        let body = json!({
            "choices": [{"message": {"content": "<think>let me see</think>\n\n{\"score\": 3}"}, "finish_reason": "stop"}],
            "base_resp": {"status_code": 0, "status_msg": ""}
        })
        .to_string();
        let answer = ask(&serve_once("200 OK", &body, None).await).await.unwrap();
        assert_eq!(answer, "{\"score\": 3}");
    }

    #[tokio::test]
    async fn an_empty_answer_is_an_error_not_an_empty_string() {
        let body = json!({"choices": [{"message": {"content": ""}, "finish_reason": "length"}]}).to_string();
        let err = ask(&serve_once("200 OK", &body, None).await).await.unwrap_err();
        assert!(err.contains("finish_reason=length"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Request);
    }

    #[tokio::test]
    async fn a_body_cut_off_is_a_network_failure_not_invalid_json() {
        let body = r#"{"choices":[{"message":{"content":"par"#;
        let err = ask(&serve_once("200 OK", body, Some(body.len() + 500)).await)
            .await
            .unwrap_err();
        assert!(err.starts_with("读取响应失败: "), "{err}");
        assert!(!err.contains("Invalid JSON"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);
    }

    // ── streams that were not streams ───────────────────────────────────────

    #[test]
    fn a_plain_json_error_body_is_found_after_a_stream_ends() {
        let mut stray = String::new();
        let body = "{\n  \"base_resp\": {\n    \"status_code\": 2062,\n    \"status_msg\": \"rate limit\"\n  }\n}";
        let mut lines: Vec<&str> = body.split('\n').collect();
        let tail = lines.pop().unwrap(); // the last line never got its newline
        for line in lines {
            keep_stray_line(&mut stray, line);
        }
        let err = stray_body_error(&stray, tail).expect("error body");
        assert!(err.contains("(2062)"), "{err}");
        assert_eq!(classify_error(&err), ErrorClass::Transient);

        // A final event that never got its newline.
        let err = stray_body_error("", r#"data: {"error":{"message":"Overloaded","type":"overloaded_error"}}"#);
        assert!(err.is_some());
    }

    #[test]
    fn a_real_event_stream_leaves_nothing_stray() {
        let mut stray = String::new();
        for line in [
            ": OPENROUTER PROCESSING",
            "event: message",
            "id: 7",
            "retry: 1000",
            r#"data: {"choices":[{"delta":{"content":"x"}}]}"#,
            "",
        ] {
            keep_stray_line(&mut stray, line);
        }
        assert!(stray.is_empty(), "{stray:?}");
        assert_eq!(stray_body_error(&stray, ""), None);
        // A successful plain JSON reply is not an error.
        assert_eq!(stray_body_error(r#"{"choices":[{"message":{"content":"x"}}]}"#, ""), None);
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
    /// What the model streamed as reasoning this round. Only replayed to the
    /// providers that ask for it — see [`replays_reasoning_with_tool_calls`].
    pub reasoning: String,
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

/// Whether a catalogue Argus can trust says this model takes no `tools`.
///
/// Only two catalogues are believed, because only two actually state it:
/// StepFun's, which Argus writes itself from the docs (`stepfun_capabilities`),
/// and OpenRouter's, derived from each model's `supported_parameters`. Every
/// other provider's capability list is a partial description — DeepSeek's says
/// `reasoning` and nothing about tools, yet every DeepSeek chat model calls
/// them — so reading a missing `tool_calling` there as "cannot" would lock
/// working models out of the agent.
///
/// An OpenRouter model with no capabilities recorded (added by hand, or before
/// the catalogue was enriched) is unknown, not incapable, and gets the benefit
/// of the doubt. If it really cannot, the provider says so on the first call and
/// [`looks_like_tools_rejected`] catches that.
pub fn model_declares_no_tools(provider: &AiProvider, model: &str) -> bool {
    if crate::stepfun::is_stepfun(provider) {
        return !crate::stepfun::stepfun_capabilities(model)
            .iter()
            .any(|c| c == "tool_calling");
    }
    if provider.kind == "openrouter" || provider.base_url.to_lowercase().contains("openrouter") {
        return !crate::openrouter::model_accepts_server_tools(provider, model);
    }
    false
}

/// Marks an error as "this model does not take tools", so a caller that has a
/// tool-free way to answer can tell it from every other failure.
///
/// A prefix on the message rather than an error type because the agent path
/// hands `String` errors through three layers already; the one caller that
/// cares strips it, and [`strip_tools_rejected`] keeps it from ever reaching
/// the screen.
pub const TOOLS_REJECTED_PREFIX: &str = "\u{1}tools-rejected\u{1}";

/// The message without the [`TOOLS_REJECTED_PREFIX`] marker, if it had one.
pub fn strip_tools_rejected(err: &str) -> &str {
    err.strip_prefix(TOOLS_REJECTED_PREFIX).unwrap_or(err)
}

/// Whether an error response is the provider refusing the `tools` field itself.
///
/// Deliberately narrow. Plenty of 400s mention tools without meaning "this
/// model cannot use them" — a replayed `tool` message with no matching
/// `tool_calls`, an external MCP server's malformed schema, a thinking model
/// missing its `reasoning_content`. Treating those as "no tools" would quietly
/// downgrade a working model and hide the real bug, so both halves are
/// required: something about tools, *and* something saying it is unsupported.
fn looks_like_tools_rejected(status: u16, body: &str) -> bool {
    if !matches!(status, 400 | 404 | 422) {
        return false;
    }
    let b = body.to_lowercase();
    let about_tools = b.contains("tool") || b.contains("function");
    let unsupported = [
        "not support",
        "unsupported",
        "does not support",
        "doesn't support",
        "no endpoints found that support",
        "enable-auto-tool-choice",
    ]
    .iter()
    .any(|k| b.contains(k));
    about_tools && unsupported
}

/// Whether this model's own reasoning must be sent back with its tool calls.
///
/// Kimi's thinking models reason across a multi-step tool exchange and expect
/// to see what they thought in the previous step; the docs require
/// `reasoning_content` to stay on each assistant turn that carried calls. Only
/// Kimi K2 is listed: other providers either ignore the field or, in
/// DeepSeek's reasoner's case, have rejected it on input, so it is not sent
/// anywhere it has not been asked for.
pub fn replays_reasoning_with_tool_calls(provider: &AiProvider, model: &str) -> bool {
    is_kimi_provider(provider) && model.starts_with("kimi-k2")
}

fn is_kimi_provider(provider: &AiProvider) -> bool {
    provider.kind == "kimi"
        || provider.base_url.to_lowercase().contains("moonshot.cn")
        || provider.base_url.to_lowercase().contains("api.kimi.com")
}

/// Whether a streamed `delta.tool_calls[]` entry is a call this loop must run.
///
/// A server-run tool arrives in the same array but is not ours: StepFun's
/// built-in search comes back as `type: "web_search"`, already answered, with the
/// pages it read in `function.results`. Handing that to the local tool runner
/// would make the loop try to execute `step_websearch`, fail, and burn a round
/// telling the model so — and because that entry carries no `index`, it would
/// also land in slot 0 and concatenate its name and arguments onto a genuine call
/// sitting there. `ServerToolTrace::absorb` has already taken the citations out
/// of it by this point, so dropping it here loses nothing.
///
/// Only an *explicitly* non-`function` type is skipped. A streamed continuation
/// fragment carries neither `id` nor `type` — just the next slice of
/// `function.arguments` — so treating a missing type as "not ours" would throw
/// away most of every real call.
fn is_local_tool_call(call: &serde_json::Value) -> bool {
    match call.get("type").and_then(|t| t.as_str()) {
        Some(t) => t == "function",
        None => true,
    }
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

    let req = client
        .post(&url)
        .header("Content-Type", "application/json");
    let resp = openai_auth(req, provider, api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| describe_stream_error(&e))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(friendly_error(status, &text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Invalid response: {e}"))?;
    // A refusal sent with a 200 is a failed ping, not a zero-hit one.
    if let Some(err) = provider_error_in_body(&json) {
        return Err(err);
    }
    let usage = &json["usage"];
    let input_tokens = usage["prompt_tokens"].as_u64().unwrap_or(0);
    let output_tokens = usage["completion_tokens"].as_u64().unwrap_or(0);
    let cache_hit_tokens = usage["prompt_cache_hit_tokens"]
        .as_u64()
        .or_else(|| usage["prompt_tokens_details"]["cached_tokens"].as_u64())
        .or_else(|| usage["cached_tokens"].as_u64())
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
    web_search: bool,
    // Whether this is the opening round of a user turn. Only the opening round
    // may run StepFun's billable `/v1/search` preflight: the agent loop appends
    // synthetic `user` messages of its own — the tool-budget notice and the
    // "here are the pages you asked to see" image turn — so "the last message is
    // from the user" is not enough to tell a fresh question from a continuation,
    // and searching on those would bill again with boilerplate as the query.
    first_round: bool,
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
    let is_kimi = is_kimi_provider(provider);
    let is_kimi_k2 = is_kimi && model.starts_with("kimi-k2");

    let is_zhipu = crate::zhipu::is_zhipu(provider);
    let is_minimax = crate::minimax::is_minimax(provider);

    // The agent replays its whole transcript each round, so page renders fed
    // back by `view_paper_page` pass through here too and get the same limits
    // check (and, when they are large, the same move to the Files API). Only
    // DeepSeek pays for the copy — everyone else keeps using the caller's slice.
    let prepared;
    let messages: &[serde_json::Value] = if is_deepseek(provider) {
        prepared =
            crate::deepseek::prepare_chat_messages(provider, api_key, model, messages.to_vec())
                .await?;
        &prepared
    } else {
        messages
    };

    let is_stepfun = crate::stepfun::is_stepfun(provider);
    if is_stepfun {
        crate::stepfun::check_messages(model, messages)?;
    }
    // The flagship has no built-in search tool, so Argus runs StepFun's own
    // `/v1/search` and writes the results in. `query_from_messages` returns None
    // unless the transcript ends on a user turn, which is exactly how the first
    // round of an agent turn is told from the rounds that answer a tool result —
    // without that, every round would run (and bill) another search.
    let searched;
    let mut preflight_hits: Vec<crate::stepfun::SearchHit> = Vec::new();
    let messages: &[serde_json::Value] =
        if first_round && web_search && is_stepfun && !crate::stepfun::supports_builtin_web_search(model) {
            match crate::stepfun::query_from_messages(messages) {
                Some(query) => match crate::stepfun::search(provider, api_key, &query).await {
                    Ok(hits) if !hits.is_empty() => {
                        let mut with_context = messages.to_vec();
                        with_context.push(serde_json::json!({
                            "role": "system",
                            "content": crate::stepfun::search_context(&hits),
                        }));
                        preflight_hits = hits;
                        searched = with_context;
                        &searched
                    }
                    Ok(_) => messages,
                    // A failed search must not take the whole agent round down.
                    Err(e) => {
                        eprintln!("[stepfun] web search skipped: {e}");
                        messages
                    }
                },
                None => messages,
            }
        } else {
            messages
        };

    let mut body = serde_json::json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    // Argus's own tools come back as calls this loop has to run; OpenRouter's
    // server tools are run by OpenRouter and never surface as a call, so the two
    // sets sit side by side in the same array without interfering.
    let openrouter_tools = crate::openrouter::server_tool_defs(provider, model);
    let mut server_tools = openrouter_tools.clone();
    // MiMo runs its `web_search` tool itself and reports what it consulted as
    // annotations, exactly like OpenRouter's server tools — so it joins the same
    // set here rather than surfacing as a call for the loop to run. The model can
    // both search the web and call the agent's tools in one turn.
    if web_search && crate::mimo::is_mimo(provider) {
        server_tools.push(crate::mimo::web_search_tool());
    }
    // GLM's built-in search behaves the same way, so it joins the same set.
    if web_search && is_zhipu {
        server_tools.push(crate::zhipu::web_search_tool());
    }
    // StepFun's too, for the models that carry it — the flagship was already
    // served by the `/v1/search` preflight above, so the two never both fire.
    if web_search && is_stepfun && crate::stepfun::supports_builtin_web_search(model) {
        server_tools.push(crate::stepfun::web_search_tool());
    }
    // An empty tool list must be omitted, not sent as `[]`: some gateways reject
    // `tools: []` outright, and it is how the loop says "no more tools".
    if !tools.is_empty() || !server_tools.is_empty() {
        let mut all: Vec<serde_json::Value> = tools.to_vec();
        all.extend(server_tools.iter().cloned());
        body["tools"] = serde_json::json!(all);
        // `tool_choice: auto` only when the model has a function it could pick;
        // with server tools alone there is nothing for it to choose between, and
        // the loop's "no more tools" signal must stay unambiguous.
        if !tools.is_empty() {
            body["tool_choice"] = serde_json::json!("auto");
            // GLM-5.3+ streams a call's arguments in fragments only when asked
            // to, and the docs pair `tool_stream` with `stream` for those
            // models. The accumulator below reads either shape, so this only
            // makes the call surface sooner.
            if is_zhipu && crate::zhipu::supports_tool_stream(model) {
                body["tool_stream"] = serde_json::json!(true);
            }
        }
    }
    // `max_tool_calls` is an OpenRouter extension — only OpenRouter gets it, so a
    // strict validator elsewhere (MiMo included) never sees an unknown field.
    if !openrouter_tools.is_empty() {
        body["max_tool_calls"] = serde_json::json!(crate::openrouter::max_tool_calls(provider));
    }
    // Qwen's native web search sits alongside the tools on the same body — the
    // model can both search the web and call the agent's tools in one turn.
    if web_search && is_qwen(provider) {
        body["enable_search"] = serde_json::json!(true);
        body["search_options"] = serde_json::json!({ "forced_search": true, "enable_source": true });
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
        } else if is_qwen(provider) {
            // Qwen gates thinking with `enable_thinking`, not `reasoning_effort`.
            body["enable_thinking"] = serde_json::json!(true);
        } else if crate::mimo::is_mimo(provider) {
            // MiMo gates thinking with `thinking: {type: enabled}`, streaming the
            // reasoning back as `reasoning_content` (already read below).
            body["thinking"] = serde_json::json!({"type": "enabled"});
        } else if !is_kimi {
            body["reasoning_effort"] = serde_json::json!(reasoning_effort.unwrap_or("high"));
        }
    }
    // Kimi K2.5 and later run only with thinking on and a fixed set of sampling
    // parameters, whether or not the user asked to see the reasoning. The
    // plain-chat path has always sent them; this one did not, so K2 was being
    // asked to run out of spec exactly where it was also being handed tools.
    if is_kimi_k2 {
        body["thinking"] = serde_json::json!({"type": "enabled"});
        body["temperature"] = serde_json::json!(1.0);
        body["top_p"] = serde_json::json!(0.95);
        body["n"] = serde_json::json!(1);
        body["presence_penalty"] = serde_json::json!(0.0);
        body["frequency_penalty"] = serde_json::json!(0.0);
    }
    // GLM thinks unless told otherwise, so both directions are written here
    // rather than only inside the block above. See `zhipu::apply_thinking`.
    if is_zhipu {
        crate::zhipu::apply_thinking(&mut body, model, use_reasoning, reasoning_effort);
    }
    if is_minimax {
        crate::minimax::apply_thinking(&mut body, model, use_reasoning);
    }
    // StepFun has no off switch either; see `stepfun::apply_reasoning`.
    //
    // Deliberately no `apply_audio_output` here, unlike the plain-chat path: an
    // agent answer is several rounds with tool calls in between, and asking each
    // of them to speak would bill audio for commentary nobody hears and split one
    // reply across several clips. Worse, with audio on the model's *text* arrives
    // as `audio.transcript` rather than `content`, which this loop accumulates —
    // so a spoken agent turn would come back looking empty.
    if is_stepfun {
        crate::stepfun::apply_reasoning(&mut body, model, use_reasoning, reasoning_effort);
        // StepFun documents no `stream_options`; usage rides every chunk anyway.
        if let Some(obj) = body.as_object_mut() {
            obj.remove("stream_options");
        }
    }

    let req = client
        .post(&url)
        .header("Content-Type", "application/json");
    let resp = openai_auth(req, provider, api_key)
        .json(&body)
        .send()
        .await
        .map_err(|e| describe_stream_error(&e))?;

    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        // Only a request that actually offered functions can have been refused
        // for them; the tool-free final turn of a spent budget never is.
        if !tools.is_empty() && looks_like_tools_rejected(status, &text) {
            return Err(format!("{TOOLS_REJECTED_PREFIX}{}", friendly_error(status, &text)));
        }
        return Err(friendly_error(status, &text));
    }

    let reasoning_event = format!("{event_name}-reasoning");
    let mut stream = resp.bytes_stream();
    let mut byte_buf: Vec<u8> = Vec::new();
    let mut buf = String::new();
    // Non-SSE lines, kept in case the "stream" is a plain JSON error body.
    let mut stray = String::new();
    let mut accumulated = String::new();
    let mut reasoning_text = String::new();
    // Tool calls keyed by the `index` the provider assigns, since fragments for
    // several concurrent calls interleave in the stream.
    let mut partial: std::collections::BTreeMap<u64, (String, String, String)> =
        std::collections::BTreeMap::new();
    let mut input_tokens: u64 = 0;
    let mut output_tokens: u64 = 0;
    let mut cache_hit_tokens: u64 = 0;
    let mut cost_usd: Option<f64> = None;
    let mut trace = crate::openrouter::ServerToolTrace::default();
    // Same as on the plain-chat path: a client-side search's pages are citations
    // even though nothing in the response mentions them.
    for hit in &preflight_hits {
        trace.push_citation(&hit.url, Some(hit.title.as_str()));
    }
    if !preflight_hits.is_empty() {
        trace.note_call("web_search", 1);
    }

    'outer: while let Some(chunk) = stream.next().await {
        if let Some(flag) = &cancel {
            if flag.load(Ordering::SeqCst) {
                break;
            }
        }
        let bytes = chunk.map_err(|e| format!("Stream read error: {}", describe_stream_error(&e)))?;
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
                keep_stray_line(&mut stray, &line);
                continue;
            };
            let data = data.trim_start();
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(json) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            // An error sent as an event (MiniMax's `base_resp`, OpenRouter's
            // mid-stream `error`) used to read as an empty round, which the
            // agent loop took for a finished answer with nothing in it. The
            // tools-rejected marker is applied under the same narrow test as
            // for an HTTP error, using the status the body itself names.
            if let Some((embedded, err)) = body_error(&json) {
                if !accumulated.is_empty() {
                    // Commentary already on screen: keep it and end the round as
                    // an answer. A tool call cut off mid-arguments is unusable.
                    let notice = interrupted_notice(&err);
                    let _ = app.emit(event_name, serde_json::json!({"delta": &notice, "done": false}));
                    accumulated.push_str(&notice);
                    partial.clear();
                    break 'outer;
                }
                if !tools.is_empty()
                    && embedded.is_some_and(|s| looks_like_tools_rejected(s, data))
                {
                    return Err(format!("{TOOLS_REJECTED_PREFIX}{err}"));
                }
                return Err(err);
            }

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
                    // StepFun puts it at the top level; see the note in
                    // `stream_openai_compat`.
                    .or_else(|| usage["cached_tokens"].as_u64())
                {
                    cache_hit_tokens = v;
                }
                if (is_openrouter || is_kimi) && cost_usd.is_none() {
                    cost_usd = usage_cost_usd(usage);
                }
                trace.absorb_usage(usage);
            }

            let delta = &json["choices"][0]["delta"];
            trace.absorb(delta);
            // GLM reports its search results on the chunk, not the delta.
            if is_zhipu {
                trace.absorb(&json);
            }

            // `audio.transcript` is the fallback, not the primary: an end-to-end
            // speech model puts its words there instead of in `content`. This
            // path does not request audio (see above), but a model configured to
            // speak by default would otherwise stream an answer this loop reads
            // as empty.
            if let Some(text) = delta["content"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| delta["audio"]["transcript"].as_str().filter(|s| !s.is_empty()))
            {
                accumulated.push_str(text);
                let _ = app.emit(event_name, serde_json::json!({"delta": text, "done": false}));
            }
            if let Some(r) = delta["reasoning_content"]
                .as_str()
                .or_else(|| delta["reasoning"].as_str())
                .or_else(|| delta["thinking"].as_str())
                .filter(|s| !s.is_empty())
            {
                reasoning_text.push_str(r);
                let _ = app.emit(
                    &reasoning_event,
                    serde_json::json!({"delta": r, "done": false}),
                );
            }

            if let Some(calls) = delta["tool_calls"].as_array() {
                for c in calls {
                    if !is_local_tool_call(c) {
                        continue;
                    }
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

    // A 200 whose body was a plain JSON error rather than events: nothing was
    // produced, and "no content, no calls" would read as a finished answer.
    let cancelled = cancel.as_ref().is_some_and(|f| f.load(Ordering::SeqCst));
    if accumulated.is_empty() && partial.is_empty() && !cancelled {
        if let Some(err) = stray_body_error(&stray, &buf) {
            return Err(err);
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
    //
    // The server-tool trace *is* emitted per round: unlike the cost strip it is
    // additive on the frontend, and a citation found in round one should not
    // wait for round five to appear.
    emit_server_tool_trace(app, event_name, &trace);

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
        reasoning: reasoning_text,
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
    fn deepseek_sizes_come_from_the_table_the_catalogue_lacks() {
        assert_eq!(known_param_billions("deepseek-v4-pro"), Some(1600.0));
        assert_eq!(known_param_billions("deepseek-v4-flash"), Some(284.0));
        // Variants inherit their family rather than dropping to the placeholder.
        assert_eq!(known_param_billions("deepseek-v4-pro-0813"), Some(1600.0));
        assert_eq!(
            known_param_billions("deepseek-v4-flash-vision-exp"),
            Some(284.0)
        );
        // Same model, served through OpenRouter.
        assert_eq!(
            known_param_billions("deepseek/deepseek-v4-pro"),
            Some(1600.0)
        );
    }

    /// The family suffixes are generic, so the vendor has to be there too.
    #[test]
    fn the_table_does_not_reach_past_deepseek() {
        assert_eq!(known_param_billions("acme/thing-v4-pro"), None);
        assert_eq!(known_param_billions("gpt-5.2"), None);
        assert_eq!(known_param_billions("deepseek-v3"), None);
    }

    /// A size read off the model's own name is first-hand; the hand-kept table
    /// is not allowed to overwrite it.
    #[test]
    fn a_catalogued_size_wins_over_the_table() {
        let mut model: AiModel = serde_json::from_value(serde_json::json!({
            "id": "deepseek-v4-pro",
            "display_name": "DeepSeek V4 Pro",
            "param_billions": 900.0,
        }))
        .unwrap();
        apply_known_param_size(&mut model);
        assert_eq!(model.param_billions, Some(900.0));

        let mut unknown: AiModel = serde_json::from_value(serde_json::json!({
            "id": "deepseek-v4-flash",
            "display_name": "DeepSeek V4 Flash",
        }))
        .unwrap();
        apply_known_param_size(&mut unknown);
        assert_eq!(unknown.param_billions, Some(284.0));
    }

    /// DeepSeek's `/models` returns bare ids, so the fetch path has nothing to
    /// scan and has to reach the table for the size to appear at all.
    #[test]
    fn a_bare_deepseek_id_still_gets_its_size() {
        let item = serde_json::json!({ "id": "deepseek-v4-pro", "object": "model" });
        assert_eq!(parse_model_item(&item).unwrap().param_billions, Some(1600.0));
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

    fn provider_of(kind: &str, base_url: &str, models: serde_json::Value) -> AiProvider {
        AiProvider {
            id: "p".into(),
            name: "P".into(),
            kind: kind.into(),
            base_url: base_url.into(),
            enabled: true,
            server_tools: Default::default(),
            speech: Default::default(),
            created_at: String::new(),
            models: serde_json::from_value(models).expect("AiModel fixtures"),
        }
    }

    /// Only the two catalogues that actually state tool support are believed.
    #[test]
    fn only_trusted_catalogues_can_say_a_model_takes_no_tools() {
        // StepFun: R1.5 is the documented exception; everything else calls tools.
        let stepfun = provider_of("stepfun", "https://api.stepfun.com/v1", serde_json::json!([]));
        assert!(model_declares_no_tools(&stepfun, "step-audio-r1.5"));
        assert!(!model_declares_no_tools(&stepfun, "step-3"));
        assert!(!model_declares_no_tools(&stepfun, "step-audio-2"));

        // OpenRouter: a recorded capability list without tools means no tools...
        let or = provider_of("openrouter", "https://openrouter.ai/api/v1", serde_json::json!([
            {"id": "text-only", "display_name": "t", "capabilities": ["vision"]},
            {"id": "tooly", "display_name": "t", "capabilities": ["tool_calling"]},
            {"id": "unknown", "display_name": "t", "capabilities": []},
        ]));
        assert!(model_declares_no_tools(&or, "text-only"));
        assert!(!model_declares_no_tools(&or, "tooly"));
        // ...but an empty list is unknown, not incapable.
        assert!(!model_declares_no_tools(&or, "unknown"));
        assert!(!model_declares_no_tools(&or, "not-in-catalogue"));

        // DeepSeek's list says nothing about tools, and every model calls them.
        let ds = provider_of("openai_compatible", "https://api.deepseek.com", serde_json::json!([
            {"id": "deepseek-chat", "display_name": "d", "capabilities": ["reasoning"]},
        ]));
        assert!(!model_declares_no_tools(&ds, "deepseek-chat"));
    }

    /// A refusal of the `tools` field is told apart from every other 400 that
    /// happens to mention tools — misreading one of those would quietly strip a
    /// working model of its tools and hide the real bug.
    #[test]
    fn a_tools_refusal_is_told_apart_from_other_tool_errors() {
        assert!(looks_like_tools_rejected(400, "tools is not supported with this model"));
        assert!(looks_like_tools_rejected(400, r#"{"error":"registry.ollama.ai/library/gemma does not support tools"}"#));
        assert!(looks_like_tools_rejected(404, "No endpoints found that support tool use."));
        assert!(looks_like_tools_rejected(400, r#""auto" tool choice requires --enable-auto-tool-choice"#));
        assert!(looks_like_tools_rejected(422, "Function calling is unsupported for this model"));

        // Broken history, not a refusal.
        assert!(!looks_like_tools_rejected(
            400,
            "messages with role 'tool' must be a response to a preceding message with 'tool_calls'"
        ));
        // An external server's bad schema, not a refusal.
        assert!(!looks_like_tools_rejected(400, "Invalid 'tools[3].function.name': string too long"));
        assert!(!looks_like_tools_rejected(400, "invalid api key"));
        // A server error is never a statement about the model.
        assert!(!looks_like_tools_rejected(500, "tool calling is not supported right now"));
    }

    #[test]
    fn the_refusal_marker_never_reaches_the_screen() {
        let marked = format!("{TOOLS_REJECTED_PREFIX}请求被拒绝");
        assert_eq!(strip_tools_rejected(&marked), "请求被拒绝");
        assert_eq!(strip_tools_rejected("普通错误"), "普通错误");
    }

    /// Reasoning goes back only where it was asked for.
    #[test]
    fn only_kimi_k2_gets_its_reasoning_replayed() {
        let kimi = provider_of("kimi", "https://api.moonshot.cn/v1", serde_json::json!([]));
        assert!(replays_reasoning_with_tool_calls(&kimi, "kimi-k2.6"));
        assert!(!replays_reasoning_with_tool_calls(&kimi, "moonshot-v1-8k"));
        let ds = provider_of("openai_compatible", "https://api.deepseek.com", serde_json::json!([]));
        assert!(!replays_reasoning_with_tool_calls(&ds, "deepseek-reasoner"));
    }

    /// The shapes the OpenAI-compatible providers actually stream. Dropping any
    /// of these would break tool calling for every provider, not just the new one.
    #[test]
    fn a_real_function_call_is_always_kept() {
        // Opening fragment, fully spelled out.
        assert!(is_local_tool_call(&serde_json::json!({
            "index": 0, "id": "call_1", "type": "function",
            "function": {"name": "view_paper_page", "arguments": ""}
        })));
        // Continuation fragments carry neither id nor type — only the next slice
        // of the arguments. These are the majority of what a stream delivers.
        assert!(is_local_tool_call(&serde_json::json!({
            "index": 0, "function": {"arguments": "{\"page\":"}
        })));
        assert!(is_local_tool_call(&serde_json::json!({"index": 1})));
        // A provider that sends the whole call in one chunk with no type at all.
        assert!(is_local_tool_call(&serde_json::json!({
            "id": "call_2", "function": {"name": "search", "arguments": "{}"}
        })));
    }

    /// StepFun's built-in search: already answered by the platform, no `index`,
    /// and carrying `results` rather than awaiting one.
    #[test]
    fn a_server_run_search_is_never_handed_to_the_local_runner() {
        assert!(!is_local_tool_call(&serde_json::json!({
            "id": "call_x", "type": "web_search",
            "function": {
                "name": "step_websearch",
                "arguments": "{\"keyword\": \"x\"}",
                "results": [{"index": 0, "url": "https://a.test", "title": "t", "summary": "s"}]
            }
        })));
        // Anything else a platform decides to run for itself is skipped too,
        // rather than being guessed at.
        assert!(!is_local_tool_call(&serde_json::json!({"type": "retrieval"})));
        assert!(!is_local_tool_call(&serde_json::json!({"type": "openrouter:web_search"})));
    }

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
