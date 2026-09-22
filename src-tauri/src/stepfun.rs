//! StepFun / 阶跃星辰 (`api.stepfun.com`). OpenAI-compatible, so it rides the
//! generic `/chat/completions` path in `llm.rs`; this module holds only what is
//! specific to it:
//!   * detection (`is_stepfun`),
//!   * its reasoning control (`apply_reasoning`), which is unlike every other
//!     provider here — see below,
//!   * its two web-search mechanisms (`web_search_tool` for the models that
//!     carry the built-in tool, `search` for the flagship that does not),
//!   * end-to-end speech: audio in (`input_audio` blocks) and audio out
//!     (`modalities` + `audio`), plus the PCM→WAV wrapping a streamed reply
//!     needs,
//!   * id-derived capabilities plus a documented catalogue, since `/v1/models`
//!     reports five bare ids and nothing else.
//!
//! Auth is the plain `Authorization: Bearer` every other provider on this path
//! uses, and images ride the standard `{"type":"image_url", …}` content block.
//!
//! ## Reasoning cannot be turned off
//!
//! Everywhere else "reasoning off" means either "send nothing" (DeepSeek,
//! OpenRouter) or "say `disabled` out loud" (GLM-4.x, MiniMax). StepFun offers
//! neither: `reasoning_effort` accepts `low` / `medium` / `high` and there is no
//! `none`, no boolean, and no `thinking` object anywhere in its API. Its
//! reasoning models emit `reasoning` even when no reasoning parameter is sent at
//! all. So `apply_reasoning` maps the toggle the only way the platform allows —
//! off becomes `low`, which is also what the docs' own migration advice says —
//! and strips the field entirely from the models that have no reasoning to
//! control, whose validator would reject it.
//!
//! `step-3.5-flash-2603` is the one model that takes only `low` and `high`;
//! sending it `medium`, which is Argus's middle setting, is a 400.
//!
//! ## Two different web searches
//!
//! Most models carry a built-in `web_search` tool the platform runs itself,
//! reporting what it read inside a `tool_calls` entry whose `type` is
//! `web_search` (not `function`) and whose `function.results` array holds the
//! pages. That shape matters twice: `openrouter::ServerToolTrace` has to read it
//! to show citations, and the agent loop has to *skip* it, because a tool call
//! the platform already answered must not be handed to the local tool runner.
//!
//! The flagship `step-5-preview` does not carry that tool at all. For it the
//! docs prescribe calling `POST /v1/search` and writing the results into
//! `messages` — which is what `search` and `search_context` are for. The two
//! mechanisms report their hits under different field names (`summary` vs
//! `snippet`, 0-based `index` vs 1-based `position`), so neither struct is
//! reused for the other.
//!
//! ## What is not here
//!
//! TTS, ASR, image generation, audio/music generation and the realtime voice
//! socket all answer on their own routes rather than `/chat/completions`. They
//! are catalogued (so the model list is honest about what the key can reach) but
//! not driven from this module.
//!
//! References:
//! <https://platform.stepfun.com/docs/zh/api-reference/chat/chat-completion-create>,
//! <https://platform.stepfun.com/docs/zh/guides/developer/audio-chat>,
//! <https://platform.stepfun.com/docs/zh/guides/developer/web-search>

use serde::Deserialize;

use crate::models::{AiModel, AiProvider};

pub fn is_stepfun(provider: &AiProvider) -> bool {
    let url = provider.base_url.to_lowercase();
    provider.kind == "stepfun" || url.contains("stepfun")
}

// ── Capabilities ─────────────────────────────────────────────────────────────

/// Whether a model id reads images (and, for every one of them, video too).
///
/// Three ids, and no pattern that generalises: the two flagships plus the legacy
/// turbo-vision model. The `-flash` text models are deliberately excluded — the
/// quickstart says outright that `step-3.5-flash` refuses image and video input,
/// so tagging the whole `flash` line would light up an attach button that only
/// produces a 400.
fn reads_images(id: &str) -> bool {
    id.starts_with("step-5") || id.starts_with("step-3.7-flash") || id.contains("vision")
}

/// Whether a model id takes an `input_audio` block on the chat path.
///
/// Two traps here, and the ids are spelled two different ways — `step-audio-2`
/// is hyphenated, `stepaudio-2.5-chat` is not — so a single prefix will not do.
///
///   * `step-1o-audio` *speaks* but cannot *listen* in context ("目前不支持上下文
///     中传入 wav"), so it is excluded while still being tagged `audio` by
///     [`stepfun_capabilities`] for the picker.
///   * `stepaudio-2.5-chat` is the mirror image: the model list says it "支持单次
///     提交语音请求、流式文本输出回复内容", so it hears but answers only in text.
///     (A Step Plan summary table calls it "文本输入、文本返回"; the model list is
///     the more specific of the two, and erring towards allowing it costs a 400
///     at worst, where erring the other way silently removes a working feature.)
pub fn accepts_audio_input(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.starts_with("step-audio-2")
        || id.starts_with("step-audio-r1")
        // The unhyphenated line: `stepaudio-3-chat-preview`, `stepaudio-2.5-chat`.
        // Only the `-chat` members — realtime rides a websocket, and tts/asr are
        // not chat models at all.
        || (id.starts_with("stepaudio-") && id.contains("-chat"))
}

/// Whether a model can reply in speech, i.e. whether `modalities` / `audio` mean
/// anything to it.
///
/// The end-to-end voice models only. `stepaudio-2.5-chat` is the explicit
/// counter-example — it hears audio but answers in text, and asking it for audio
/// is documented as an error rather than a silent downgrade.
pub fn supports_audio_output(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.starts_with("step-audio-2") || id.starts_with("step-audio-r1") || id == "step-1o-audio"
}

/// Refuse an attachment the chosen model cannot take, before the request leaves
/// the machine.
///
/// The composer already warns when the selected model lacks the capability, but
/// that warning is advisory — the user can send anyway, and the model list can be
/// edited by hand. Without this the request comes back as a bare 400 naming an
/// unknown content block, which says nothing about what to do next.
///
/// Only `user` turns are checked: an assistant turn replaying an earlier
/// attachment is history, not a new request, and `step-3.5-flash` reading its own
/// past transcript should not be blocked over it.
pub fn check_messages(model_id: &str, messages: &[serde_json::Value]) -> Result<(), String> {
    let sees = reads_images(&model_id.to_lowercase());
    let hears = accepts_audio_input(model_id);
    for msg in messages {
        if msg.get("role").and_then(|r| r.as_str()) != Some("user") {
            continue;
        }
        let Some(parts) = msg.get("content").and_then(|c| c.as_array()) else {
            continue;
        };
        for part in parts {
            match part.get("type").and_then(|t| t.as_str()) {
                Some("image_url") if !sees => {
                    return Err(format!(
                        "{model_id} 不支持图片输入。请改用 step-5-preview、step-3.7-flash \
                         或 step-1o-turbo-vision，或移除图片附件。"
                    ));
                }
                Some("video_url") if !sees => {
                    return Err(format!(
                        "{model_id} 不支持视频输入。请改用 step-5-preview、step-3.7-flash \
                         或 step-1o-turbo-vision，或移除视频附件。"
                    ));
                }
                Some("input_audio") if !hears => {
                    return Err(format!(
                        "{model_id} 不支持音频输入。请改用 step-audio-2、step-audio-2-mini、\
                         step-audio-r1.5 或 stepaudio-3-chat-preview，或移除音频附件。\
                         （step-1o-audio 能说不能听。）"
                    ));
                }
                // No StepFun model takes a document block on the chat path —
                // `text`, `image_url`, `video_url` and `input_audio` are the only
                // four it documents. Its document understanding goes through the
                // Files API (upload, poll, fetch the extracted text, paste it in),
                // which Argus does not drive, so this is a dead end rather than a
                // model-choice problem and the message says so.
                Some("file") => {
                    return Err(
                        "阶跃星辰的对话接口不支持直接上传 PDF 等文档。\
                         请移除文档附件——论文正文可以用左侧的「全文」上下文带进来，\
                         或改用 OpenRouter 等支持内联 PDF 的服务商。"
                            .to_string(),
                    );
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Whether a model reasons, i.e. whether `reasoning_effort` is meaningful.
fn reasons(id: &str) -> bool {
    id.starts_with("step-5")
        || id.starts_with("step-3.7-flash")
        || id.starts_with("step-3.5-flash")
        || id.starts_with("step-audio-r1")
        || id.starts_with("step-router")
}

/// Whether a model id names a speech line that answers on its own route rather
/// than `/chat/completions` — TTS, ASR, and the audio/music generators.
fn is_offline_speech_route(id: &str) -> bool {
    id.contains("-tts") || id.contains("-asr") || id.contains("-gen") || id.contains("-music")
}

/// Capabilities inferred from a StepFun model id.
///
/// `/v1/models` documents five ids with no modality field, and omits every audio,
/// speech and image model the key can actually call — so the id is the only
/// signal, for the catalogue below and for anything a future endpoint returns.
pub fn stepfun_capabilities(model_id: &str) -> Vec<String> {
    let id = model_id.to_lowercase();
    let mut caps: Vec<String> = Vec::new();
    let add = |caps: &mut Vec<String>, cap: &str| {
        if !caps.iter().any(|c| c == cap) {
            caps.push(cap.to_string());
        }
    };

    // Non-chat lines first — each rides its own endpoint, and none of them takes
    // tools or reasoning. Tagged and returned so the catalogue can name what the
    // key reaches without offering them as a conversation partner.
    if is_offline_speech_route(&id) && !id.contains("chat") {
        add(&mut caps, "audio");
        return caps;
    }
    if id.contains("image") || id.starts_with("step-2x") {
        add(&mut caps, "image_gen");
        return caps;
    }

    // Every remaining id is a chat model.
    if id.contains("audio") {
        // Both directions collapse to one tag, as `video` does for MiniMax: the
        // picker's job is to say "this model deals in sound", and the two
        // narrower questions have their own predicates above.
        add(&mut caps, "audio");
    }
    if reads_images(&id) {
        add(&mut caps, "vision");
        // Every StepFun model that reads a still also reads a clip — the docs
        // never split the two.
        add(&mut caps, "video");
    }
    if reasons(&id) {
        add(&mut caps, "reasoning");
    }
    // Every chat model carries OpenAI-style function calling except R1.5, which
    // the audio guide names as the one exception.
    if !id.starts_with("step-audio-r1") {
        add(&mut caps, "tool_calling");
    }
    caps
}

// ── Reasoning ────────────────────────────────────────────────────────────────

/// Whether a model id accepts only `low` / `high`, with no middle setting.
fn effort_is_two_way(model_id: &str) -> bool {
    model_id.to_lowercase().starts_with("step-3.5-flash-2603")
}

/// Write StepFun's reasoning control onto a request body — in both directions.
///
/// This function owns `reasoning_effort` for this provider and clears the
/// `thinking` / `enable_thinking` a neighbouring branch in `llm.rs` may have set
/// for a differently-shaped provider: StepFun has neither field and its
/// validator does not know them.
///
/// See the module docs for why "off" is spelled `low` rather than omitted.
pub fn apply_reasoning(
    body: &mut serde_json::Value,
    model_id: &str,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("thinking");
        obj.remove("enable_thinking");
        if !reasons(&model_id.to_lowercase()) {
            // A speech or vision-only model has no reasoning to dial; the field
            // is not merely useless to it, it is unknown.
            obj.remove("reasoning_effort");
            return;
        }
    }

    let wanted = if use_reasoning {
        reasoning_effort.unwrap_or("high")
    } else {
        // The cheapest setting the platform offers. Not "off" — there is no off.
        "low"
    };
    let effort = match (wanted, effort_is_two_way(model_id)) {
        // 2603 knows only the two ends; the middle rounds up so "medium" still
        // means "think more than the floor".
        ("medium", true) => "high",
        (w, _) => w,
    };
    body["reasoning_effort"] = serde_json::json!(effort);
}

// ── Audio in / out ───────────────────────────────────────────────────────────

/// The four voices the `step-audio-2` family documents by name. `step-1o-audio`
/// and `step-audio-r1.5` read theirs from `GET /v1/audio/voices` instead, so a
/// user-supplied id is passed through untouched.
pub const DEFAULT_VOICE: &str = "wenrounansheng";

/// Ask for a spoken reply.
///
/// `format` is not a preference but a consequence of `stream`: the platform
/// documents `wav` for non-streaming and raw `pcm` for streaming, and pairing
/// them the other way round is not something the schema rejects — it simply
/// fails. Callers therefore never choose it.
pub fn apply_audio_output(
    body: &mut serde_json::Value,
    model_id: &str,
    voice: Option<&str>,
    stream: bool,
) {
    if !supports_audio_output(model_id) {
        return;
    }
    body["modalities"] = serde_json::json!(["text", "audio"]);
    body["audio"] = serde_json::json!({
        "voice": voice.filter(|v| !v.is_empty()).unwrap_or(DEFAULT_VOICE),
        "format": if stream { "pcm" } else { "wav" },
    });
}

/// Sample rate of the PCM a streamed audio reply arrives as: 24 kHz, mono,
/// 16-bit little-endian signed, with no container.
const PCM_SAMPLE_RATE: u32 = 24_000;
const PCM_CHANNELS: u16 = 1;
const PCM_BITS: u16 = 16;

/// Wrap raw streamed PCM in the 44-byte RIFF/WAVE header that makes it playable.
///
/// The stream hands back headerless samples, so without this the accumulated
/// bytes are not a file any player will open. The constants are the ones the
/// platform documents for its streaming format; they are not negotiable and not
/// reported per-response, which is why they are compiled in.
pub fn pcm_to_wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = PCM_SAMPLE_RATE * PCM_CHANNELS as u32 * PCM_BITS as u32 / 8;
    let block_align = PCM_CHANNELS * PCM_BITS / 8;

    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&PCM_CHANNELS.to_le_bytes());
    out.extend_from_slice(&PCM_SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&PCM_BITS.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

// ── Web search ───────────────────────────────────────────────────────────────

/// Whether a model carries the built-in `web_search` tool.
///
/// Stated as an exclusion list rather than an inclusion one because the docs
/// phrase it that way ("`step-3.7-flash` 等仍支持该工具的模型" — the models that
/// *still* support it), so a model released after this build is assumed to carry
/// it until it is known not to. The two that do not: the flagship, which the
/// docs name three separate times, and the Step Plan router, which answers
/// `unsupported_content_type`.
pub fn supports_builtin_web_search(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    // R1.5 takes no `tools` array at all — the built-in search rides that same
    // field, so offering it here would 400 the whole request, not just skip the
    // search. Kept in step with `stepfun_capabilities`, which likewise refuses to
    // tag it `tool_calling`.
    !(id.starts_with("step-5") || id.starts_with("step-router") || id.starts_with("step-audio-r1"))
}

/// StepFun's built-in web search, expressed as a tool the platform runs itself.
///
/// Like MiMo's and GLM's, this never comes back as a call the agent loop has to
/// answer: the platform searches mid-answer and reports what it read inside the
/// `tool_calls` entry described in the module docs. `function.description` is the
/// only configurable part and steers *whether* the model searches, so it is left
/// at the docs' own wording rather than forced — the agent loop re-sends this
/// tool every round, and forcing would search on each one.
pub fn web_search_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "web_search",
        "function": { "description": "搜索互联网上的公开信息，用于回答时效性问题或需要外部事实佐证的问题。" }
    })
}

/// One hit from `POST /v1/search`.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchHit {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub title: String,
    /// The short extract. The standalone endpoint also returns a much longer
    /// `content`, which is deliberately not read: it is whole-page text, and
    /// several of them would crowd out the conversation it is meant to support.
    #[serde(default)]
    pub snippet: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<SearchHit>,
}

/// How many pages to fetch for one turn. The endpoint allows 1–20 and bills per
/// call rather than per hit, so the ceiling is the context the results occupy,
/// not the price.
const SEARCH_HITS: u32 = 5;

/// Run StepFun's standalone web search.
///
/// This is the documented substitute for the built-in tool on `step-5-preview`.
/// It is a separate billable call (0.04 元 each), so it runs once per user turn —
/// never once per agent round; see `llm.rs` for where that is decided.
pub async fn search(
    provider: &AiProvider,
    api_key: &str,
    query: &str,
) -> Result<Vec<SearchHit>, String> {
    let url = format!("{}/search", provider.base_url.trim_end_matches('/'));
    let client = crate::llm::build_client()?;
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json; charset=utf-8")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&serde_json::json!({ "query": query, "n": SEARCH_HITS }))
        .send()
        .await
        .map_err(|e| format!("StepFun 网页搜索请求失败：{e}"))?;

    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(format!("StepFun 网页搜索返回 {status}：{text}"));
    }
    let parsed: SearchResponse = serde_json::from_str(&text)
        .map_err(|e| format!("StepFun 网页搜索返回了无法解析的内容：{e}"))?;
    Ok(parsed
        .results
        .into_iter()
        .filter(|h| !h.url.is_empty())
        .collect())
}

/// Turn search hits into the context block that is appended to the request.
///
/// Appended rather than prepended on purpose: every long-running task in Argus
/// front-loads an identical paper-context system message so the provider's prompt
/// cache can be reused, and inserting anything before it would invalidate that
/// prefix on every single turn.
pub fn search_context(hits: &[SearchHit]) -> String {
    let mut out = String::from(
        "以下是刚刚检索到的网页结果，请优先据此回答，并在引用时标注来源编号；\
         若检索结果不足以判断，请明确说明。\n",
    );
    for (i, hit) in hits.iter().enumerate() {
        out.push_str(&format!(
            "\n[{}] {}\n{}\n{}\n",
            i + 1,
            hit.title,
            hit.url,
            hit.snippet
        ));
    }
    out
}

/// The text the search should be run on: the last thing the user actually said.
///
/// Returns `None` when the final message is not a user turn — which, on the agent
/// path, is exactly how a follow-up round is told apart from the start of a turn.
/// A round that is answering a tool result must not trigger a second billable
/// search.
pub fn query_from_messages(messages: &[serde_json::Value]) -> Option<String> {
    let last = messages.last()?;
    if last.get("role").and_then(|r| r.as_str()) != Some("user") {
        return None;
    }
    let content = last.get("content")?;
    let text = match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    // The endpoint takes a search keyword, not an essay. A long paste is cut
    // rather than refused, because the opening sentences carry the question.
    Some(trimmed.chars().take(300).collect())
}

// ── Catalogue ────────────────────────────────────────────────────────────────

struct Spec {
    id: &'static str,
    ctx: Option<u64>,
    /// CNY per 1M tokens: (input cache-miss, input cache-hit, output).
    price: Option<(f64, f64, f64)>,
    params: Option<f64>,
    free: bool,
}

/// The documented catalogue of models this key can reach.
///
/// Wider than `/v1/models`, which lists five chat ids and omits every audio,
/// speech and image model — so this is the floor rather than the fallback.
///
/// Prices are the published CNY per-1M-token rates. The speech models billed per
/// 万字符 or per hour (TTS, ASR) and the per-image generators carry no token
/// price: quoting one would make the cost estimate confidently wrong, which is
/// worse than leaving it blank.
///
/// Two ids are deliberately absent. `step-2x-large` and `step-image-edit-2` go
/// offline on 2026-10-10 together with the three `/v1/images/*` routes, and the
/// 2026-07-08 retirements (`step-1-8k`, `step-1v-32k`, `step-2-16k`, `step-3`, …)
/// no longer answer at all.
fn specs() -> Vec<Spec> {
    vec![
        // ── Chat / multimodal ────────────────────────────────────────────────
        Spec { id: "step-5-preview",       ctx: Some(1_000_000), price: Some((7.0, 0.35, 20.0)), params: None, free: false },
        Spec { id: "step-3.7-flash",       ctx: Some(262_144),   price: Some((1.35, 0.27, 8.1)), params: Some(198.0), free: false },
        Spec { id: "step-3.5-flash",       ctx: Some(262_144),   price: Some((0.7, 0.14, 2.1)),  params: Some(196.0), free: false },
        Spec { id: "step-3.5-flash-2603",  ctx: Some(262_144),   price: Some((0.7, 0.14, 2.1)),  params: Some(196.0), free: false },
        Spec { id: "step-1o-turbo-vision", ctx: Some(32_768),    price: Some((2.5, 0.5, 8.0)),   params: None, free: false },
        // ── End-to-end speech, on /chat/completions ──────────────────────────
        Spec { id: "stepaudio-3-chat-preview", ctx: None, price: None,                    params: None, free: true },
        Spec { id: "stepaudio-2.5-chat",       ctx: None, price: Some((10.0, 2.0, 25.0)), params: None, free: false },
        Spec { id: "step-audio-2",             ctx: None, price: Some((10.0, 2.0, 70.0)), params: None, free: false },
        Spec { id: "step-audio-2-mini",        ctx: None, price: None,                    params: None, free: false },
        Spec { id: "step-audio-r1.5",          ctx: None, price: Some((10.0, 2.0, 105.0)), params: None, free: false },
        Spec { id: "step-1o-audio",            ctx: None, price: Some((25.0, 5.0, 60.0)), params: None, free: false },
        // ── Speech models on their own routes ────────────────────────────────
        // Catalogued so the model list is honest about what the key reaches;
        // they are tagged `audio` only and never offered as a chat partner.
        Spec { id: "stepaudio-3-tts",        ctx: None, price: None, params: None, free: false },
        Spec { id: "stepaudio-2.5-tts",      ctx: None, price: None, params: None, free: false },
        Spec { id: "step-tts-2",             ctx: None, price: None, params: None, free: false },
        Spec { id: "step-tts-mini",          ctx: None, price: None, params: None, free: false },
        Spec { id: "stepaudio-3-asr-max",    ctx: None, price: None, params: None, free: false },
        Spec { id: "stepaudio-2.5-asr",      ctx: None, price: None, params: None, free: false },
        Spec { id: "stepaudio-2-asr-pro",    ctx: None, price: None, params: None, free: false },
        Spec { id: "stepaudio-3-gen-preview",   ctx: None, price: None, params: None, free: true },
        Spec { id: "stepaudio-3-music-preview", ctx: None, price: None, params: None, free: true },
    ]
}

fn model_from(spec: &Spec) -> AiModel {
    let (input, cache_hit, output) = match spec.price {
        Some((i, c, o)) => (Some(i), Some(c), Some(o)),
        None => (None, None, None),
    };
    AiModel {
        id: spec.id.to_string(),
        display_name: spec.id.to_string(),
        capabilities: stepfun_capabilities(spec.id),
        context_length: spec.ctx,
        enabled: true,
        input_price_per_million: input,
        output_price_per_million: output,
        peak_pricing: false,
        peak_input_price_per_million: None,
        peak_output_price_per_million: None,
        cache_hit_input_price_per_million: cache_hit,
        input_price_usd_per_million: None,
        output_price_usd_per_million: None,
        provider_order: vec![],
        param_billions: spec.params,
        is_free: spec.free,
        discount_percent: None,
        discount_windows: vec![],
    }
}

pub fn known_models() -> Vec<AiModel> {
    specs().iter().map(model_from).collect()
}

/// Overlay catalogue facts onto a model the endpoint reported.
///
/// Non-destructive: capabilities are unioned with whatever the caller already
/// had, and context, parameter count and prices are filled in only where they
/// were unknown — so a rate the user edited by hand survives a refresh.
pub fn enrich_stepfun_model(mut m: AiModel) -> AiModel {
    for cap in stepfun_capabilities(&m.id) {
        if !m.capabilities.contains(&cap) {
            m.capabilities.push(cap);
        }
    }
    if let Some(known) = known_models()
        .into_iter()
        .find(|k| k.id.eq_ignore_ascii_case(&m.id))
    {
        if m.context_length.is_none() {
            m.context_length = known.context_length;
        }
        if m.param_billions.is_none() {
            m.param_billions = known.param_billions;
        }
        if m.input_price_per_million.is_none() {
            m.input_price_per_million = known.input_price_per_million;
        }
        if m.output_price_per_million.is_none() {
            m.output_price_per_million = known.output_price_per_million;
        }
        if m.cache_hit_input_price_per_million.is_none() {
            m.cache_hit_input_price_per_million = known.cache_hit_input_price_per_million;
        }
    }
    m
}

/// Fold whatever `/v1/models` returned together with the documented catalogue —
/// the same arrangement as Zhipu's and MiniMax's, and for the same reason: ids
/// the endpoint reports are kept and enriched, so a model released after this
/// build still appears, and every documented id it omitted is added.
pub fn merge_catalogue(fetched: Vec<AiModel>) -> Vec<AiModel> {
    let mut out: Vec<AiModel> = fetched.into_iter().map(enrich_stepfun_model).collect();
    for m in known_models() {
        if !out.iter().any(|e| e.id.eq_ignore_ascii_case(&m.id)) {
            out.push(m);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(kind: &str, url: &str) -> AiProvider {
        AiProvider {
            id: "p".into(),
            name: "StepFun".into(),
            kind: kind.into(),
            base_url: url.into(),
            enabled: true,
            models: vec![],
            server_tools: crate::models::ServerTools::default(),
            speech: crate::models::SpeechOutput::default(),
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn detection_takes_the_kind_or_the_host() {
        assert!(is_stepfun(&provider("stepfun", "https://example.test/v1")));
        assert!(is_stepfun(&provider(
            "openai_compatible",
            "https://api.stepfun.com/v1"
        )));
        assert!(!is_stepfun(&provider(
            "openai_compatible",
            "https://api.deepseek.com/v1"
        )));
    }

    #[test]
    fn only_the_documented_models_read_pictures() {
        for id in ["step-5-preview", "step-3.7-flash", "step-1o-turbo-vision"] {
            let caps = stepfun_capabilities(id);
            assert!(caps.iter().any(|c| c == "vision"), "{id}");
            // Everything here that reads a still also reads a clip.
            assert!(caps.iter().any(|c| c == "video"), "{id}");
        }
        // The text-only flash line refuses both, and tagging it would offer an
        // attach button that can only 400.
        for id in ["step-3.5-flash", "step-3.5-flash-2603"] {
            let caps = stepfun_capabilities(id);
            assert!(!caps.iter().any(|c| c == "vision"), "{id}");
            assert!(!caps.iter().any(|c| c == "video"), "{id}");
        }
    }

    #[test]
    fn speaking_and_listening_are_separate_questions() {
        // Speaks but cannot be sent audio.
        assert!(supports_audio_output("step-1o-audio"));
        assert!(!accepts_audio_input("step-1o-audio"));
        // Hears but answers in text — asking either for audio output is a
        // documented error. Note the two spellings of the same brand.
        assert!(accepts_audio_input("stepaudio-3-chat-preview"));
        assert!(accepts_audio_input("stepaudio-2.5-chat"));
        assert!(!supports_audio_output("stepaudio-2.5-chat"));
        assert!(!supports_audio_output("stepaudio-3-chat-preview"));
        // The websocket and non-chat speech lines are not chat models at all.
        for id in ["stepaudio-2.5-realtime", "stepaudio-3-tts", "stepaudio-2.5-asr"] {
            assert!(!accepts_audio_input(id), "{id}");
        }
        // Both directions.
        for id in ["step-audio-2", "step-audio-2-mini", "step-audio-r1.5"] {
            assert!(accepts_audio_input(id), "{id}");
            assert!(supports_audio_output(id), "{id}");
        }
    }

    #[test]
    fn r1_5_is_the_one_chat_model_without_tools() {
        assert!(!stepfun_capabilities("step-audio-r1.5")
            .iter()
            .any(|c| c == "tool_calling"));
        for id in ["step-5-preview", "step-audio-2", "step-1o-turbo-vision"] {
            assert!(
                stepfun_capabilities(id).iter().any(|c| c == "tool_calling"),
                "{id}"
            );
        }
    }

    /// `stores/ai.ts` hides a model from every chat picker when *all* of its
    /// capabilities are non-chat (`embedding`, `image_gen`, `video`, `audio`).
    /// That filter only works if this module keeps both halves of the bargain:
    /// a speech-only line must carry `audio` and nothing else, and a model that
    /// actually converses in sound must carry something else too.
    #[test]
    fn speech_routes_are_tagged_but_not_offered_as_chat() {
        const NON_CHAT: [&str; 4] = ["embedding", "image_gen", "video", "audio"];

        // Own endpoint, cannot take a chat turn — must be filtered out.
        for id in [
            "stepaudio-3-tts",
            "step-tts-mini",
            "stepaudio-2.5-asr",
            "stepaudio-3-music-preview",
            "stepaudio-3-gen-preview",
        ] {
            assert_eq!(stepfun_capabilities(id), vec!["audio"], "{id}");
        }
        assert_eq!(stepfun_capabilities("step-image-edit-2"), vec!["image_gen"]);

        // Converses in sound — must survive the filter.
        for id in [
            "step-audio-2",
            "step-audio-2-mini",
            "step-audio-r1.5",
            "stepaudio-2.5-chat",
            "stepaudio-3-chat-preview",
            "step-1o-audio",
        ] {
            let caps = stepfun_capabilities(id);
            assert!(caps.iter().any(|c| c == "audio"), "{id} should be tagged audio");
            assert!(
                caps.iter().any(|c| !NON_CHAT.contains(&c.as_str())),
                "{id} would be hidden from every chat picker: {caps:?}"
            );
        }
    }

    #[test]
    fn reasoning_off_is_low_because_there_is_no_off() {
        let mut body = serde_json::json!({});
        apply_reasoning(&mut body, "step-5-preview", false, None);
        assert_eq!(body["reasoning_effort"], "low");

        let mut body = serde_json::json!({});
        apply_reasoning(&mut body, "step-5-preview", true, Some("medium"));
        assert_eq!(body["reasoning_effort"], "medium");
    }

    #[test]
    fn the_2603_variant_never_sees_medium() {
        let mut body = serde_json::json!({});
        apply_reasoning(&mut body, "step-3.5-flash-2603", true, Some("medium"));
        assert_eq!(body["reasoning_effort"], "high");
        // Its two real settings pass through untouched.
        for effort in ["low", "high"] {
            let mut body = serde_json::json!({});
            apply_reasoning(&mut body, "step-3.5-flash-2603", true, Some(effort));
            assert_eq!(body["reasoning_effort"], effort);
        }
    }

    #[test]
    fn a_model_that_cannot_reason_is_sent_no_effort_at_all() {
        let mut body = serde_json::json!({"reasoning_effort": "high"});
        apply_reasoning(&mut body, "step-1o-turbo-vision", true, Some("high"));
        assert!(body.get("reasoning_effort").is_none());

        let mut body = serde_json::json!({"thinking": {"type": "enabled"}});
        apply_reasoning(&mut body, "step-audio-2", false, None);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn a_stray_thinking_object_never_reaches_stepfun() {
        let mut body = serde_json::json!({"thinking": {"type": "enabled"}, "enable_thinking": true});
        apply_reasoning(&mut body, "step-5-preview", true, Some("high"));
        assert!(body.get("thinking").is_none());
        assert!(body.get("enable_thinking").is_none());
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn the_flagship_is_the_one_without_the_built_in_search() {
        assert!(!supports_builtin_web_search("step-5-preview"));
        assert!(!supports_builtin_web_search("step-router-v1"));
        assert!(supports_builtin_web_search("step-3.7-flash"));
        assert!(supports_builtin_web_search("step-audio-2"));
    }

    /// The built-in search rides the `tools` array, and R1.5 takes no tools at
    /// all — offering it would 400 the whole request, not just skip the search.
    /// The two predicates must therefore agree.
    #[test]
    fn a_model_without_tools_is_never_offered_the_search_tool() {
        assert!(!supports_builtin_web_search("step-audio-r1.5"));
        assert!(!stepfun_capabilities("step-audio-r1.5")
            .iter()
            .any(|c| c == "tool_calling"));
    }

    #[test]
    fn audio_output_format_follows_the_stream_flag() {
        let mut body = serde_json::json!({});
        apply_audio_output(&mut body, "step-audio-2", None, true);
        assert_eq!(body["audio"]["format"], "pcm");
        assert_eq!(body["audio"]["voice"], DEFAULT_VOICE);
        assert_eq!(body["modalities"], serde_json::json!(["text", "audio"]));

        let mut body = serde_json::json!({});
        apply_audio_output(&mut body, "step-audio-2", Some("qingchunshaonv"), false);
        assert_eq!(body["audio"]["format"], "wav");
        assert_eq!(body["audio"]["voice"], "qingchunshaonv");

        // A model that cannot speak is left entirely alone — `stepaudio-2.5-chat`
        // errors outright when `modalities` names audio.
        let mut body = serde_json::json!({});
        apply_audio_output(&mut body, "stepaudio-2.5-chat", None, true);
        assert!(body.get("modalities").is_none());
        assert!(body.get("audio").is_none());
    }

    #[test]
    fn streamed_pcm_gets_a_playable_header() {
        let wav = pcm_to_wav(&[1, 2, 3, 4]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(wav.len(), 44 + 4);
        // 24 kHz, mono, 16-bit — the format the platform streams.
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 24_000);
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(wav[34..36].try_into().unwrap()), 16);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
    }

    #[test]
    fn the_search_query_comes_only_from_a_fresh_user_turn() {
        let msgs = vec![
            serde_json::json!({"role": "system", "content": "…"}),
            serde_json::json!({"role": "user", "content": "上海最高的楼是哪栋？"}),
        ];
        assert_eq!(
            query_from_messages(&msgs).as_deref(),
            Some("上海最高的楼是哪栋？")
        );

        // Multipart: only the text parts are searchable.
        let msgs = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}},
                {"type": "text", "text": "这是什么？"}
            ]
        })];
        assert_eq!(query_from_messages(&msgs).as_deref(), Some("这是什么？"));

        // A round answering a tool result must not trigger a second billable
        // search — this is how the agent loop tells the rounds apart.
        let msgs = vec![
            serde_json::json!({"role": "user", "content": "找一下"}),
            serde_json::json!({"role": "assistant", "content": ""}),
            serde_json::json!({"role": "tool", "content": "…", "tool_call_id": "c1"}),
        ];
        assert!(query_from_messages(&msgs).is_none());
    }

    #[test]
    fn an_attachment_the_model_cannot_take_is_refused_before_the_request() {
        let audio = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "input_audio", "input_audio": {"data": "data:audio/wav;base64,AA"}}]
        })];
        // The model that speaks but cannot listen is the one worth naming.
        let err = check_messages("step-1o-audio", &audio).unwrap_err();
        assert!(err.contains("音频"), "{err}");
        assert!(check_messages("step-audio-2", &audio).is_ok());

        let picture = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}}]
        })];
        assert!(check_messages("step-3.5-flash", &picture).is_err());
        assert!(check_messages("step-5-preview", &picture).is_ok());

        let clip = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "video_url", "video_url": {"url": "https://x.test/a.mp4"}}]
        })];
        assert!(check_messages("step-3.5-flash", &clip).is_err());
        assert!(check_messages("step-3.7-flash", &clip).is_ok());
    }

    /// No StepFun model reads a document block, so the early-out for a model that
    /// both sees and hears must not skip the check entirely.
    #[test]
    fn a_pdf_is_refused_even_by_the_most_capable_model() {
        let pdf = vec![serde_json::json!({
            "role": "user",
            "content": [{"type": "file", "file": {"filename": "a.pdf", "file_data": "data:application/pdf;base64,AA"}}]
        })];
        for id in ["step-5-preview", "step-audio-2", "step-3.5-flash"] {
            let err = check_messages(id, &pdf).unwrap_err();
            assert!(err.contains("文档"), "{id}: {err}");
        }
    }

    #[test]
    fn replayed_history_is_not_re_checked() {
        // An assistant turn quoting an earlier attachment is history, not a new
        // request; blocking on it would make a transcript unreplayable the moment
        // the user switched models.
        let msgs = vec![serde_json::json!({
            "role": "assistant",
            "content": [{"type": "image_url", "image_url": {"url": "data:image/png;base64,AA"}}]
        })];
        assert!(check_messages("step-3.5-flash", &msgs).is_ok());
        // A plain text turn is never in the way either.
        let msgs = vec![serde_json::json!({"role": "user", "content": "你好"})];
        assert!(check_messages("step-3.5-flash", &msgs).is_ok());
    }

    #[test]
    fn catalogue_is_the_floor_not_the_fallback() {
        let fetched = vec![model_from(&Spec {
            id: "step-9-preview",
            ctx: Some(1),
            price: None,
            params: None,
            free: false,
        })];
        let merged = merge_catalogue(fetched);
        // A model newer than this build survives...
        assert!(merged.iter().any(|m| m.id == "step-9-preview"));
        // ...and every documented id the five-item endpoint omitted is added.
        assert!(merged.iter().any(|m| m.id == "step-audio-2"));
        assert!(merged.iter().any(|m| m.id == "step-5-preview"));
        // The image models retiring on 2026-10-10 are not shipped.
        assert!(!merged.iter().any(|m| m.id.contains("image")));
        assert!(!merged.iter().any(|m| m.id == "step-2x-large"));
        // The 2026-07-08 retirements are gone too.
        assert!(!merged.iter().any(|m| m.id == "step-1-8k"));
    }

    #[test]
    fn a_case_different_duplicate_does_not_double_up() {
        let merged = merge_catalogue(vec![model_from(&Spec {
            id: "STEP-5-PREVIEW",
            ctx: None,
            price: None,
            params: None,
            free: false,
        })]);
        assert_eq!(
            merged
                .iter()
                .filter(|m| m.id.eq_ignore_ascii_case("step-5-preview"))
                .count(),
            1
        );
    }

    #[test]
    fn enrichment_fills_gaps_without_overwriting() {
        let mut m = model_from(&Spec {
            id: "step-5-preview",
            ctx: None,
            price: None,
            params: None,
            free: false,
        });
        // A rate the user edited by hand must survive a refresh.
        m.input_price_per_million = Some(1.0);
        let m = enrich_stepfun_model(m);
        assert_eq!(m.input_price_per_million, Some(1.0));
        assert_eq!(m.output_price_per_million, Some(20.0));
        assert_eq!(m.cache_hit_input_price_per_million, Some(0.35));
        assert_eq!(m.context_length, Some(1_000_000));
    }
}
