//! MiniMax (`api.minimax.cn`, `api.minimax.io`). OpenAI-compatible, so it rides
//! the generic `/chat/completions` path in `llm.rs`; this module holds only what
//! is specific to it:
//!   * detection (`is_minimax`),
//!   * its thinking controls (`apply_thinking`), which differ from every other
//!     provider here in two ways — see below,
//!   * id-derived capabilities plus a documented catalogue, since the platform
//!     has no documented `/models` endpoint.
//!
//! Auth is the plain `Authorization: Bearer` this path already sends, and images
//! ride the standard `image_url` block. Video is the one thing MiniMax reads
//! that nothing else here does: a `video_url` block on the same message, which
//! is why `ChatContentPart` gained that variant.
//!
//! ## Thinking is on by default, and it leaks into the answer
//!
//! Two separate hazards, both handled in `apply_thinking`:
//!
//! 1. `thinking` defaults to on when the field is omitted, so — as with GLM —
//!    staying silent leaves the model reasoning after the user turned reasoning
//!    off. The "on" value is spelled `adaptive` rather than `enabled`, and the
//!    M2.x line cannot be turned off at all.
//! 2. With `reasoning_split` at its default of `false`, the thinking comes back
//!    inside `content`, wrapped in `<think>` tags, and would be rendered as part
//!    of the answer. Asking for the split moves it to `reasoning_content`, which
//!    the streaming loops in `llm.rs` already read and show in their own pane.
//!    So the flag is set on every request, not only when reasoning is on.
//!
//! ## What is not here
//!
//! MiniMax's speech (`speech-2.8-*`), image (`image-01`) and video generation
//! (`MiniMax-H3`) models answer on their own endpoints, not on
//! `/chat/completions`, and audio is not an input the chat endpoint accepts at
//! all. None of them can reply in a conversation, so the catalogue leaves them
//! out rather than offering the user a model that cannot answer.
//!
//! Reference: <https://platform.minimaxi.com/docs/api-reference/text-openai-api>

use crate::models::{AiModel, AiProvider};

pub fn is_minimax(provider: &AiProvider) -> bool {
    let url = provider.base_url.to_lowercase();
    provider.kind == "minimax" || url.contains("minimax")
}

/// Whether a model id belongs to the M2.x line, which always reasons: its
/// `thinking.type` refuses `disabled`.
fn thinking_is_mandatory(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.starts_with("minimax-m2") || id.starts_with("abab")
}

/// Capabilities inferred from a MiniMax model id.
///
/// Every chat model in the line reasons and carries OpenAI-style function
/// calling. Vision and video are a property of the generation rather than of the
/// individual id: M2 and up read images, and M3 adds video.
pub fn minimax_capabilities(model_id: &str) -> Vec<String> {
    let id = model_id.to_lowercase();
    let mut caps: Vec<String> = Vec::new();
    let add = |caps: &mut Vec<String>, cap: &str| {
        if !caps.iter().any(|c| c == cap) {
            caps.push(cap.to_string());
        }
    };

    // Non-chat lines first. These ride their own endpoints and are not in the
    // catalogue, but an id typed by hand still deserves an honest label.
    if id.starts_with("speech") || id.contains("-t2a") || id.contains("asr") {
        add(&mut caps, "audio");
        return caps;
    }
    if id.starts_with("image-") {
        add(&mut caps, "image_gen");
        return caps;
    }
    if id.starts_with("video-") || id.starts_with("minimax-hailuo") || id.starts_with("minimax-h") {
        add(&mut caps, "video");
        return caps;
    }
    if id.contains("embo") || id.contains("embedding") {
        add(&mut caps, "embedding");
        return caps;
    }

    // The VL line was the older explicitly-visual model; from M2 on, sight is
    // part of the base model.
    if id.contains("-vl") || minimax_generation(&id).is_some_and(|g| g >= 2.0) {
        add(&mut caps, "vision");
    }
    // Video input arrived with M3.
    if minimax_generation(&id).is_some_and(|g| g >= 3.0) {
        add(&mut caps, "video");
    }
    add(&mut caps, "reasoning");
    add(&mut caps, "tool_calling");
    caps
}

/// The generation number in a MiniMax id: `minimax-m2.7-highspeed` -> 2.7,
/// `MiniMax-M3` -> 3. `None` for ids that are not shaped that way.
fn minimax_generation(model_id: &str) -> Option<f64> {
    let id = model_id.to_lowercase();
    let rest = id.strip_prefix("minimax-m")?;
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.trim_end_matches('.').parse().ok()
}

/// Overlay id-derived facts onto a model. Non-destructive: capabilities are
/// unioned with whatever the caller already had, and context length is filled in
/// only when it was unknown.
///
/// Pricing is left untouched, as for Qwen, MiMo and Zhipu: the platform quotes
/// per-model rates that a name guess would get confidently wrong, and a wrong
/// number on a cost estimate is worse than no number.
pub fn enrich_minimax_model(mut m: AiModel) -> AiModel {
    for cap in minimax_capabilities(&m.id) {
        if !m.capabilities.contains(&cap) {
            m.capabilities.push(cap);
        }
    }
    if m.context_length.is_none() {
        if let Some(known) = known_models()
            .into_iter()
            .find(|k| k.id.eq_ignore_ascii_case(&m.id))
        {
            m.context_length = known.context_length;
            if m.param_billions.is_none() {
                m.param_billions = known.param_billions;
            }
        }
    }
    m
}

fn model(id: &str, ctx: u64, params: Option<f64>) -> AiModel {
    AiModel {
        id: id.to_string(),
        display_name: id.to_string(),
        capabilities: minimax_capabilities(id),
        context_length: Some(ctx),
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
        param_billions: params,
        is_free: false,
        discount_percent: None,
        discount_windows: vec![],
    }
}

/// The documented catalogue of models the OpenAI-compatible endpoint serves.
///
/// Ids are spelled with MiniMax's own capitalisation, because that is what the
/// `model` field is matched against.
pub fn known_models() -> Vec<AiModel> {
    vec![
        // M3 is the multimodal one: text, images and video in, 1M of context.
        model("MiniMax-M3", 1_000_000, Some(428.0)),
        model("MiniMax-M2.7", 204_800, None),
        model("MiniMax-M2.7-highspeed", 204_800, None),
        model("MiniMax-M2.5", 204_800, None),
        model("MiniMax-M2.5-highspeed", 204_800, None),
        model("MiniMax-M2.1", 204_800, None),
        model("MiniMax-M2.1-highspeed", 204_800, None),
        model("MiniMax-M2", 204_800, None),
    ]
}

/// Fold whatever the (undocumented) `/models` endpoint returned together with
/// the documented catalogue — the same arrangement as Zhipu's, and for the same
/// reason: ids the endpoint reports are kept and enriched, so a model released
/// after this build still appears, and every documented id it omitted is added.
pub fn merge_catalogue(fetched: Vec<AiModel>) -> Vec<AiModel> {
    let mut out: Vec<AiModel> = fetched.into_iter().map(enrich_minimax_model).collect();
    for m in known_models() {
        if !out.iter().any(|e| e.id.eq_ignore_ascii_case(&m.id)) {
            out.push(m);
        }
    }
    out
}

/// Write MiniMax's thinking controls onto a request body — in both directions,
/// plus the split that keeps the thinking out of the answer text.
///
/// This function owns `thinking` and `reasoning_split` for this provider, and
/// clears the `reasoning_effort` the generic branch in `llm.rs` may have set:
/// MiniMax has no such field and its validator does not know it.
///
/// See the module docs for why "off" has to be said out loud, and why the split
/// is asked for on every request rather than only when reasoning is on.
pub fn apply_thinking(body: &mut serde_json::Value, model_id: &str, use_reasoning: bool) {
    if let Some(obj) = body.as_object_mut() {
        obj.remove("reasoning_effort");
    }

    // Always: without this the chain of thought arrives inside `content`, in
    // `<think>` tags, and lands in the answer the user reads.
    body["reasoning_split"] = serde_json::json!(true);

    if use_reasoning {
        body["thinking"] = serde_json::json!({"type": "adaptive"});
    } else if thinking_is_mandatory(model_id) {
        // The M2.x line always reasons. Sending `disabled` is a 400, so the
        // honest thing is to send nothing and let it think — the answer pane
        // still separates the reasoning out, thanks to the split above.
        if let Some(obj) = body.as_object_mut() {
            obj.remove("thinking");
        }
    } else {
        body["thinking"] = serde_json::json!({"type": "disabled"});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations_are_read_off_the_id() {
        assert_eq!(minimax_generation("MiniMax-M3"), Some(3.0));
        assert_eq!(minimax_generation("MiniMax-M2.7-highspeed"), Some(2.7));
        assert_eq!(minimax_generation("minimax-m2"), Some(2.0));
        assert_eq!(minimax_generation("speech-2.8-hd"), None);
    }

    #[test]
    fn video_starts_at_m3_but_sight_starts_at_m2() {
        let m3 = minimax_capabilities("MiniMax-M3");
        assert!(m3.iter().any(|c| c == "vision"));
        assert!(m3.iter().any(|c| c == "video"));

        let m27 = minimax_capabilities("MiniMax-M2.7");
        assert!(m27.iter().any(|c| c == "vision"));
        assert!(!m27.iter().any(|c| c == "video"));

        // Every chat model reasons and calls tools.
        for id in ["MiniMax-M3", "MiniMax-M2", "MiniMax-M2.5-highspeed"] {
            let caps = minimax_capabilities(id);
            assert!(caps.iter().any(|c| c == "reasoning"), "{id}");
            assert!(caps.iter().any(|c| c == "tool_calling"), "{id}");
        }
    }

    #[test]
    fn non_chat_lines_get_only_their_own_capability() {
        assert_eq!(minimax_capabilities("speech-2.8-hd"), vec!["audio"]);
        assert_eq!(minimax_capabilities("image-01"), vec!["image_gen"]);
        assert_eq!(minimax_capabilities("MiniMax-Hailuo-02"), vec!["video"]);
    }

    #[test]
    fn the_split_is_asked_for_whether_or_not_reasoning_is_on() {
        let mut body = serde_json::json!({});
        apply_thinking(&mut body, "MiniMax-M3", true);
        assert_eq!(body["reasoning_split"], true);
        assert_eq!(body["thinking"]["type"], "adaptive");

        let mut body = serde_json::json!({});
        apply_thinking(&mut body, "MiniMax-M3", false);
        assert_eq!(body["reasoning_split"], true);
        assert_eq!(body["thinking"]["type"], "disabled");
    }

    #[test]
    fn the_m2_line_is_never_told_to_stop_thinking() {
        let mut body = serde_json::json!({"thinking": {"type": "adaptive"}});
        apply_thinking(&mut body, "MiniMax-M2.7", false);
        assert!(body.get("thinking").is_none());
        assert_eq!(body["reasoning_split"], true);
    }

    #[test]
    fn a_stray_reasoning_effort_never_reaches_minimax() {
        let mut body = serde_json::json!({"reasoning_effort": "high"});
        apply_thinking(&mut body, "MiniMax-M3", true);
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn catalogue_is_the_floor_not_the_fallback() {
        let fetched = vec![model("MiniMax-M9", 1, None)];
        let merged = merge_catalogue(fetched);
        assert!(merged.iter().any(|m| m.id == "MiniMax-M9"));
        assert!(merged.iter().any(|m| m.id == "MiniMax-M3"));
        // A case-different duplicate from the endpoint wins over the catalogue
        // rather than showing up twice.
        let merged = merge_catalogue(vec![model("minimax-m3", 123, None)]);
        assert_eq!(
            merged
                .iter()
                .filter(|m| m.id.eq_ignore_ascii_case("minimax-m3"))
                .count(),
            1
        );
    }
}
