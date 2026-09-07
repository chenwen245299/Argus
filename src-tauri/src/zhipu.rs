//! 智谱 BigModel / Z.AI (`open.bigmodel.cn`, `api.z.ai`). OpenAI-compatible, so
//! it rides the generic `/chat/completions` path in `llm.rs`; this module holds
//! only what is specific to it:
//!   * detection (`is_zhipu`),
//!   * its thinking controls (`apply_thinking`), which are the one place GLM
//!     differs from every other provider here — see below,
//!   * its built-in `web_search` tool, which the platform runs itself and whose
//!     results come back in a top-level `web_search` array,
//!   * id-derived capabilities plus a documented catalogue, since the platform
//!     has no documented `/models` endpoint.
//!
//! Auth is the plain `Authorization: Bearer` every other provider on this path
//! uses, and images ride the standard `{"type":"image_url", …}` content block —
//! so the vision models (GLM-5.3-Flash, the GLM-*V line) need no special casing
//! beyond being tagged `vision` so the UI offers the attach button.
//!
//! ## Thinking is on by default
//!
//! Everywhere else in `llm.rs`, "reasoning off" means "send nothing". GLM is the
//! exception: `thinking.type` defaults to `enabled`, so staying silent leaves the
//! model thinking (and billing for it) after the user turned the toggle off.
//! `apply_thinking` therefore owns both directions, and is the only thing that
//! writes `thinking` / `reasoning_effort` for this provider.
//!
//! The two GLM generations want different words for "less thinking":
//!   * GLM-5.x cannot be told to stop — `thinking.type` only accepts `enabled` —
//!     so the docs' own migration note applies: keep it enabled and drop
//!     `reasoning_effort` to `low`.
//!   * GLM-4.x accepts `thinking: {"type": "disabled"}` and does not take
//!     `reasoning_effort`, so that field is removed rather than sent.
//!
//! Reference: <https://docs.bigmodel.cn/api-reference/模型-api/对话补全>

use crate::models::AiModel;
use crate::models::AiProvider;

pub fn is_zhipu(provider: &AiProvider) -> bool {
    let url = provider.base_url.to_lowercase();
    provider.kind == "zhipu"
        || url.contains("bigmodel")
        || url.contains("zhipu")
        // Z.AI is the same platform's international endpoint, same API shape.
        // Matched with the `api.` prefix so an unrelated host that merely ends
        // in "z.ai" cannot claim it.
        || url.contains("api.z.ai")
}

/// The numeric version in a GLM id: `glm-4.6v-flash` -> 4.6, `glm-5.3-flash` ->
/// 5.3, `glm-5v-turbo` -> 5. `None` for ids that are not versioned this way
/// (`embedding-3`, `autoglm-phone`, a user's custom alias).
fn glm_version(model_id: &str) -> Option<f64> {
    let rest = model_id.to_lowercase();
    let rest = rest.strip_prefix("glm-")?.to_string();
    let digits: String = rest
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.trim_end_matches('.').parse().ok()
}

/// Whether a GLM id names a vision model.
///
/// The line is marked by a `v` glued to the version — `glm-4v`, `glm-4.1v`,
/// `glm-4.6v`, `glm-5v` — plus three ids that break the pattern: GLM-5.3-Flash
/// (a Flash-priced omni model that reads images, video and files), GLM-OCR, and
/// AutoGLM-Phone, which drives a phone screen.
fn is_vision_id(id: &str) -> bool {
    if let Some(rest) = id.strip_prefix("glm-") {
        let ver_len = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .count();
        if ver_len > 0 && rest[ver_len..].starts_with('v') {
            return true;
        }
    }
    id.starts_with("glm-5.3-flash") || id.contains("ocr") || id.contains("autoglm")
}

/// Capabilities inferred from a model id. The platform documents no `/models`
/// endpoint, so the id is the only signal — for the catalogue below and for
/// anything an undocumented endpoint happens to return.
pub fn zhipu_capabilities(model_id: &str) -> Vec<String> {
    let id = model_id.to_lowercase();
    let mut caps: Vec<String> = Vec::new();
    let add = |caps: &mut Vec<String>, cap: &str| {
        if !caps.iter().any(|c| c == cap) {
            caps.push(cap.to_string());
        }
    };

    // Non-chat lines first — each of these rides its own endpoint, and none of
    // them takes tools or thinking.
    if id.starts_with("embedding") || id.contains("embedding") {
        add(&mut caps, "embedding");
        return caps;
    }
    if id.contains("asr") || id.contains("tts") || id.contains("voice") || id.contains("realtime") {
        add(&mut caps, "audio");
        return caps;
    }
    if id.contains("cogview") || id == "glm-image" {
        add(&mut caps, "image_gen");
        return caps;
    }
    if id.contains("cogvideo") || id.starts_with("vidu") {
        add(&mut caps, "video");
        return caps;
    }

    if is_vision_id(&id) {
        add(&mut caps, "vision");
    }
    // GLM-4.5 was the generation that made thinking a first-class mode; every
    // model from there on reasons, as does the explicitly-named 4.1V-Thinking.
    if id.contains("thinking") || glm_version(&id).is_some_and(|v| v >= 4.5) {
        add(&mut caps, "reasoning");
    }
    // Every GLM chat model carries OpenAI-style function calling.
    add(&mut caps, "tool_calling");
    caps
}

/// Overlay id-derived facts onto a model. Non-destructive: capabilities are
/// unioned with whatever the caller already had, and context length is filled in
/// only when it was unknown.
///
/// Pricing is left untouched, as for Qwen and MiMo: the platform quotes tiered
/// per-model rates that a name guess would get confidently wrong, and a wrong
/// number on a cost estimate is worse than no number. Users can type exact
/// per-model prices in the UI; token counts stay accurate either way.
pub fn enrich_zhipu_model(mut m: AiModel) -> AiModel {
    for cap in zhipu_capabilities(&m.id) {
        if !m.capabilities.contains(&cap) {
            m.capabilities.push(cap);
        }
    }
    if m.context_length.is_none() {
        if let Some(known) = known_models().into_iter().find(|k| k.id == m.id) {
            m.context_length = known.context_length;
            if m.param_billions.is_none() {
                m.param_billions = known.param_billions;
            }
        }
    }
    m
}

fn model(id: &str, name: &str, ctx: u64, params: Option<f64>) -> AiModel {
    AiModel {
        id: id.to_string(),
        display_name: name.to_string(),
        capabilities: zhipu_capabilities(id),
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

/// The documented catalogue: every GLM model reachable through
/// `/chat/completions` (plus the two embedding models, which the RAG index can
/// use through the OpenAI-compatible `/embeddings`).
///
/// The image / video / speech lines are deliberately absent — they answer on
/// their own endpoints, so listing them here would only offer the user models
/// that cannot reply in a chat.
///
/// Parameter counts are given only where the vendor published them (the
/// open-weight releases); the closed models keep `None`, which the UI shows as
/// unknown rather than as a guess.
pub fn known_models() -> Vec<AiModel> {
    vec![
        // ── Text ──────────────────────────────────────────────────────────
        model("glm-5.3", "GLM-5.3", 1_000_000, None),
        model("glm-5.2", "GLM-5.2", 1_000_000, None),
        model("glm-5.1", "GLM-5.1", 200_000, None),
        model("glm-5", "GLM-5", 200_000, None),
        model("glm-5-turbo", "GLM-5-Turbo", 200_000, None),
        model("glm-4.7", "GLM-4.7", 200_000, None),
        model("glm-4.7-flashx", "GLM-4.7-FlashX", 200_000, None),
        model("glm-4.7-flash", "GLM-4.7-Flash", 200_000, None),
        model("glm-4.6", "GLM-4.6", 200_000, Some(357.0)),
        model("glm-4.5-air", "GLM-4.5-Air", 128_000, Some(106.0)),
        model("glm-4.5-airx", "GLM-4.5-AirX", 128_000, Some(106.0)),
        model("glm-4.5-flash", "GLM-4.5-Flash", 128_000, None),
        model("glm-4-long", "GLM-4-Long", 1_000_000, None),
        // ── Vision ────────────────────────────────────────────────────────
        // GLM-5.3-Flash sits with the VLMs despite the plain name: it reads
        // images, video and files at roughly a tenth of GLM-5.3's price.
        model("glm-5.3-flash", "GLM-5.3-Flash", 1_000_000, None),
        model("glm-5v-turbo", "GLM-5V-Turbo", 200_000, None),
        model("glm-4.6v", "GLM-4.6V", 128_000, Some(106.0)),
        model("glm-4.6v-flashx", "GLM-4.6V-FlashX", 128_000, Some(9.0)),
        model("glm-4.6v-flash", "GLM-4.6V-Flash", 128_000, None),
        model("glm-4.1v-thinking-flashx", "GLM-4.1V-Thinking-FlashX", 64_000, None),
        model("glm-4.1v-thinking-flash", "GLM-4.1V-Thinking-Flash", 64_000, None),
        model("glm-ocr", "GLM-OCR", 128_000, None),
        model("autoglm-phone", "AutoGLM-Phone", 20_000, None),
        // ── Embedding ─────────────────────────────────────────────────────
        model("embedding-3", "Embedding-3", 8_192, None),
        model("embedding-2", "Embedding-2", 8_192, None),
    ]
}

/// Fold whatever the (undocumented) `/models` endpoint returned together with
/// the documented catalogue.
///
/// The endpoint is not in the API reference and may answer with anything or
/// nothing, so the catalogue is the floor rather than the fallback: ids the
/// endpoint reports are enriched and kept — that is how a model released after
/// this build still shows up — and every documented id it omitted is appended.
pub fn merge_catalogue(fetched: Vec<AiModel>) -> Vec<AiModel> {
    let mut out: Vec<AiModel> = fetched.into_iter().map(enrich_zhipu_model).collect();
    for m in known_models() {
        if !out.iter().any(|e| e.id.eq_ignore_ascii_case(&m.id)) {
            out.push(m);
        }
    }
    out
}

/// GLM's built-in web search, expressed as a tool the platform runs for itself.
///
/// Like MiMo's, this never comes back as a call the agent loop has to answer:
/// the platform searches mid-answer and reports the pages in a top-level
/// `web_search` array, which `ServerToolTrace` reads. So it can sit in the
/// `tools` array next to whatever local tools are already there.
///
/// `search_engine` is pinned to `search_std` — the ¥0.01/次 basic engine —
/// rather than left to the default, because the Pro engines cost three to five
/// times as much per query and the toggle gives no way to say which one.
/// `search_result: true` is what makes the sources come back at all.
pub fn web_search_tool() -> serde_json::Value {
    serde_json::json!({
        "type": "web_search",
        "web_search": {
            "enable": true,
            "search_engine": "search_std",
            "search_result": true,
            "count": 10,
            "search_recency_filter": "noLimit",
            "content_size": "high"
        }
    })
}

/// Whether a GLM id takes `tool_stream`, which the platform introduced with
/// GLM-5.3 and documents as the companion to `stream` for those models: with it,
/// a function call's arguments arrive in fragments instead of in one piece at
/// the end. The agent loop's accumulator reads either shape, so this only makes
/// the call surface sooner — but an older id's validator does not know the
/// field, so it is asked for by version.
pub fn supports_tool_stream(model_id: &str) -> bool {
    glm_version(&model_id.to_lowercase()).is_some_and(|v| v >= 5.3)
}

/// GLM's `reasoning_effort`, from the app's three-level control. The platform
/// grades deep thinking as `low` / `high` / `max` (default `max`).
fn effort_for(level: Option<&str>) -> &'static str {
    match level.unwrap_or("high") {
        "low" | "minimal" | "none" => "low",
        "medium" => "high",
        _ => "max",
    }
}

/// Write GLM's thinking controls onto a request body — in both directions.
///
/// This function owns `thinking` and `reasoning_effort` for this provider: it
/// sets what applies and removes what does not, so a value left behind by the
/// generic branch in `llm.rs` cannot reach a GLM model that would reject it
/// (the 4.x line does not take `reasoning_effort`).
///
/// See the module docs for why "off" has to be said out loud here.
pub fn apply_thinking(
    body: &mut serde_json::Value,
    model_id: &str,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
) {
    let id = model_id.to_lowercase();
    let version = glm_version(&id);
    let graded = version.is_some_and(|v| v >= 5.0);

    let remove = |body: &mut serde_json::Value, key: &str| {
        if let Some(obj) = body.as_object_mut() {
            obj.remove(key);
        }
    };

    if use_reasoning {
        body["thinking"] = serde_json::json!({"type": "enabled"});
        if graded {
            body["reasoning_effort"] = serde_json::json!(effort_for(reasoning_effort));
        } else {
            // GLM-4.x has one thinking mode, and its validator has no field for
            // grading it.
            remove(body, "reasoning_effort");
        }
        return;
    }

    if graded {
        // GLM-5.x always thinks; the documented way down is the lowest grade.
        remove(body, "thinking");
        body["reasoning_effort"] = serde_json::json!("low");
    } else if version.is_some() {
        body["thinking"] = serde_json::json!({"type": "disabled"});
        remove(body, "reasoning_effort");
    } else {
        // An id we cannot place (a custom alias, a model newer than this build):
        // say nothing rather than risk a field its validator rejects.
        remove(body, "thinking");
        remove(body, "reasoning_effort");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_read_off_the_id() {
        assert_eq!(glm_version("glm-5.3-flash"), Some(5.3));
        assert_eq!(glm_version("glm-4.6v"), Some(4.6));
        assert_eq!(glm_version("glm-5v-turbo"), Some(5.0));
        assert_eq!(glm_version("glm-4v-flash"), Some(4.0));
        assert_eq!(glm_version("glm-4.1v-thinking-flash"), Some(4.1));
        assert_eq!(glm_version("embedding-3"), None);
        assert_eq!(glm_version("glm-image"), None);
    }

    #[test]
    fn vision_line_is_recognised() {
        for id in [
            "glm-5.3-flash",
            "glm-5v-turbo",
            "glm-4.6v",
            "glm-4.6v-flash",
            "glm-4v-flash",
            "glm-4.1v-thinking-flashx",
            "glm-ocr",
            "autoglm-phone",
        ] {
            assert!(zhipu_capabilities(id).iter().any(|c| c == "vision"), "{id}");
        }
        for id in ["glm-5.3", "glm-4.6", "glm-4.5-flash", "glm-4-long"] {
            assert!(!zhipu_capabilities(id).iter().any(|c| c == "vision"), "{id}");
        }
    }

    #[test]
    fn non_chat_lines_get_only_their_own_capability() {
        assert_eq!(zhipu_capabilities("embedding-3"), vec!["embedding"]);
        assert_eq!(zhipu_capabilities("glm-asr-2512"), vec!["audio"]);
        assert_eq!(zhipu_capabilities("glm-tts"), vec!["audio"]);
        assert_eq!(zhipu_capabilities("cogview-4"), vec!["image_gen"]);
        assert_eq!(zhipu_capabilities("cogvideox-3"), vec!["video"]);
    }

    #[test]
    fn reasoning_starts_at_4_5() {
        assert!(zhipu_capabilities("glm-5.3").iter().any(|c| c == "reasoning"));
        assert!(zhipu_capabilities("glm-4.5-air").iter().any(|c| c == "reasoning"));
        assert!(zhipu_capabilities("glm-4.1v-thinking-flash")
            .iter()
            .any(|c| c == "reasoning"));
        assert!(!zhipu_capabilities("glm-4-long").iter().any(|c| c == "reasoning"));
    }

    #[test]
    fn thinking_on_grades_only_the_5_line() {
        let mut body = serde_json::json!({"model": "glm-5.3-flash"});
        apply_thinking(&mut body, "glm-5.3-flash", true, Some("medium"));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");

        // A stray reasoning_effort from the generic branch must not survive onto
        // a 4.x model.
        let mut body = serde_json::json!({"reasoning_effort": "high"});
        apply_thinking(&mut body, "glm-4.6v", true, Some("high"));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn thinking_off_is_said_out_loud() {
        // 5.x cannot be silenced, so it is graded down instead.
        let mut body = serde_json::json!({});
        apply_thinking(&mut body, "glm-5.3", false, None);
        assert!(body.get("thinking").is_none());
        assert_eq!(body["reasoning_effort"], "low");

        // 4.x takes the explicit off switch.
        let mut body = serde_json::json!({});
        apply_thinking(&mut body, "glm-4.6", false, None);
        assert_eq!(body["thinking"]["type"], "disabled");

        // An unplaceable id gets neither field.
        let mut body = serde_json::json!({"thinking": {"type": "enabled"}});
        apply_thinking(&mut body, "my-custom-alias", false, None);
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn catalogue_is_the_floor_not_the_fallback() {
        let fetched = vec![AiModel {
            id: "glm-9-preview".to_string(),
            display_name: "glm-9-preview".to_string(),
            capabilities: vec![],
            context_length: None,
            ..model("x", "x", 1, None)
        }];
        let merged = merge_catalogue(fetched);
        // The endpoint's own id survives, enriched.
        let fresh = merged.iter().find(|m| m.id == "glm-9-preview").unwrap();
        assert!(fresh.capabilities.iter().any(|c| c == "reasoning"));
        // And nothing documented was lost.
        assert!(merged.iter().any(|m| m.id == "glm-5.3-flash"));
        assert_eq!(
            merged.iter().filter(|m| m.id == "glm-5.3-flash").count(),
            1
        );
    }

    #[test]
    fn duplicate_ids_from_the_endpoint_win_over_the_catalogue() {
        let fetched = vec![model("glm-5.3-flash", "Custom name", 999, None)];
        let merged = merge_catalogue(fetched);
        let hits: Vec<_> = merged.iter().filter(|m| m.id == "glm-5.3-flash").collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].display_name, "Custom name");
    }
}
