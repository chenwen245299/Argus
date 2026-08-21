//! Xiaomi MiMo (`api.xiaomimimo.com`). OpenAI-compatible, so it rides the generic
//! `/chat/completions` path in `llm.rs`; this module holds only what is specific
//! to it:
//!   * detection (`is_mimo`),
//!   * its built-in `web_search` tool, which the platform runs itself and whose
//!     results come back as `annotations` — the same shape `ServerToolTrace`
//!     already reads for OpenRouter, so nothing new is needed to surface them,
//!   * id-derived capabilities, since its `/models` endpoint reports bare ids
//!     with no modality.
//!
//! Two more MiMo details are handled at the call sites in `llm.rs` rather than
//! here, because they change the request body / headers directly:
//!   * auth — the platform documents an `api-key:` header; it also accepts the
//!     standard `Authorization: Bearer` (its OpenAI-SDK compatibility requires
//!     it), so `openai_auth` sends both,
//!   * thinking — gated with `thinking: {"type": "enabled"}`, which streams back
//!     `reasoning_content` exactly like DeepSeek's.
//!
//! Reference: <https://mimo.mi.com/docs/zh-CN/quick-start/summary/welcome>

use crate::models::{AiModel, AiProvider};

pub fn is_mimo(provider: &AiProvider) -> bool {
    provider.kind == "mimo" || provider.base_url.to_lowercase().contains("xiaomimimo")
}

/// MiMo's built-in web search, expressed as a tool the platform runs for itself.
///
/// Unlike an Argus tool, this one never comes back as a call the agent loop has
/// to answer: MiMo searches mid-answer and reports what it consulted as
/// `annotations` on the message. That is why it can sit in the `tools` array of
/// an ordinary request, next to whatever local tools are already there, without
/// a client loop around it — the same arrangement OpenRouter's server tools use.
///
/// Left at MiMo's defaults (the model decides when a search is warranted) rather
/// than forcing one: the agent loop re-sends this tool every round, so forcing
/// would search on each round regardless of need. Turning the toggle off drops
/// the tool entirely, which is the real on/off signal.
pub fn web_search_tool() -> serde_json::Value {
    serde_json::json!({ "type": "web_search" })
}

/// Capabilities inferred from a MiMo model id. The `/models` endpoint returns
/// bare ids with no modality, so the id is the only signal.
///
/// The line splits in two: `mimo-v2.5` alone is the omni multimodal model (sees
/// images / audio / video), while the `-pro` and `-pro-ultraspeed` ids are the
/// text-only reasoning models. Every chat id reasons and carries OpenAI-style
/// function calling; the `-asr` / `-tts` speech ids are neither chat nor tool
/// models and are tagged `audio` only.
pub fn mimo_capabilities(model_id: &str) -> Vec<String> {
    let id = model_id.to_lowercase();
    let mut caps: Vec<String> = Vec::new();
    let add = |caps: &mut Vec<String>, cap: &str| {
        if !caps.iter().any(|c| c == cap) {
            caps.push(cap.to_string());
        }
    };

    // Speech models are not chat/tool models — tag and stop.
    if id.contains("asr") || id.contains("tts") {
        add(&mut caps, "audio");
        return caps;
    }

    // The omni model (`mimo-v2.5`, no `-pro` reasoning suffix) sees images/audio.
    if id.contains("mimo") && !id.contains("pro") && !id.contains("ultraspeed") {
        add(&mut caps, "vision");
        add(&mut caps, "audio");
    }
    // Every MiMo chat model reasons and supports function calling.
    add(&mut caps, "reasoning");
    add(&mut caps, "tool_calling");
    caps
}

/// Parameter count (in billions) inferred from a MiMo model id. Published by
/// Xiaomi rather than by the `/models` endpoint: MiMo-V2.5 is ~310B, the V2.5-Pro
/// line (including Pro-UltraSpeed) ~1T. `None` for the speech models, whose size
/// Xiaomi does not quote.
fn mimo_param_billions(model_id: &str) -> Option<f64> {
    let id = model_id.to_lowercase();
    if id.contains("asr") || id.contains("tts") {
        return None;
    }
    if id.contains("pro") {
        Some(1000.0)
    } else if id.contains("mimo") {
        Some(310.0)
    } else {
        None
    }
}

/// Overlay id-derived MiMo facts onto a fetched model. Non-destructive:
/// capabilities are unioned with whatever `/models` reported, and the 1M context
/// window (documented for every MiMo chat model) and parameter count are filled
/// in only when the endpoint gave none. Pricing is left untouched — like Qwen,
/// the platform's `/models` returns none and a name-guessed figure would be a
/// confidently wrong cost estimate; users can type exact per-model prices in the
/// UI.
pub fn enrich_mimo_model(mut m: AiModel) -> AiModel {
    let caps = mimo_capabilities(&m.id);
    let is_speech = caps.iter().any(|c| c == "audio") && caps.len() == 1;
    for cap in caps {
        if !m.capabilities.iter().any(|c| *c == cap) {
            m.capabilities.push(cap);
        }
    }
    if m.context_length.is_none() && !is_speech {
        m.context_length = Some(1_000_000);
    }
    if m.param_billions.is_none() {
        m.param_billions = mimo_param_billions(&m.id);
    }
    m
}
