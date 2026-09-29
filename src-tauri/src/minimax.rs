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

/// Whether a model always reasons, so thinking must not be switched off: the
/// M2.x line accepts `disabled` but ignores it, and MiniMax-M3.1-Flash-Preview
/// (a Token Plan model) rejects it outright with a 400 — "requires adaptive
/// thinking … (2013)". Leaving the field out means `adaptive` for both.
fn thinking_is_mandatory(model_id: &str) -> bool {
    let id = model_id.to_lowercase();
    id.starts_with("minimax-m2") || id.starts_with("abab") || id.starts_with("minimax-m3.1-flash")
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
        // The M2.x line always reasons. Per MiniMax's docs it accepts
        // `disabled` and simply ignores it (the documented 400 for `disabled`
        // is MiniMax-M3.1-Flash-Preview's, not M2.x's), so sending it would
        // only pretend to turn thinking off. The honest thing is to send
        // nothing and let it think — the answer pane still separates the
        // reasoning out, thanks to the split above.
        if let Some(obj) = body.as_object_mut() {
            obj.remove("thinking");
        }
    } else {
        body["thinking"] = serde_json::json!({"type": "disabled"});
    }
}

/// What a MiniMax business code means for a caller that could retry.
///
/// MiniMax appends its code to every error message in parentheses —
/// `…请稍后重试 (2064)` — on the HTTP error envelope and in `base_resp` alike,
/// and the code is the only thing that tells a one-minute throttle from a
/// five-hour quota window. `None` for codes not listed here (and for anything
/// that is not a MiniMax code at all), so the caller falls back to the status.
///
/// Reference: <https://platform.minimax.cn/docs/api-reference/errorcode>
pub fn code_class(code: u32) -> Option<crate::llm::ErrorClass> {
    use crate::llm::ErrorClass::*;
    Some(match code {
        // Unknown / timeout / RPM-TPM limit / internal / system error /
        // connection limit / rate-growth limit / Token Plan rate limit /
        // peak-hour overload ("通常 1-5 分钟内恢复").
        1000 | 1001 | 1002 | 1013 | 1024 | 1033 | 1041 | 2045 | 2062 | 2064 => Transient,
        // Not authorised / insufficient balance / quota exhausted / invalid key /
        // Token Plan window used up / model not on this plan.
        1004 | 1008 | 1028 | 1030 | 2049 | 2056 | 2061 => Fatal,
        // Sensitive input or output / token limit / invisible characters /
        // invalid parameters.
        1026 | 1027 | 1039 | 1042 | 2013 => Request,
        _ => return None,
    })
}

/// Whether a MiniMax code means "slow down": an RPM/TPM or connection limit, a
/// rate-growth limit, the Token Plan's rate limit, the peak-hour overload. None
/// of these is ever the request's own fault — unlike the other transient codes
/// (unknown, timeout, internal and system errors), which might be.
pub fn is_throttle_code(code: u32) -> bool {
    matches!(code, 1002 | 1041 | 2045 | 2062 | 2064)
}

// ── Pacing a batch ────────────────────────────────────────────────────────────
//
// The arXiv analysis sends hundreds of small requests back to back, which is
// exactly the burst MiniMax throttles. Two published limits apply:
//
//   * A Token Plan subscription key is limited by how much runs at once: the
//     plan's FAQ puts peak-hour capacity at "约 3-4 个 Agent" on Plus, 4–5 on
//     Max and 6–7 on Ultra, and says a throttle "通常约 1 分钟恢复". Four in
//     flight fits even Plus.
//   * Every key is held to the model's requests per minute — 200 for M3, 500
//     for the M2 line. Ten requests in flight at about three seconds each is
//     already 200 a minute on M3.
//
// References: <https://platform.minimax.cn/docs/token-plan/faq>,
// <https://platform.minimax.io/docs/guides/rate-limits>

/// Most requests a batch keeps in flight on a Token Plan key.
pub const PLAN_MAX_IN_FLIGHT: usize = 4;

/// A Token Plan subscription key (`sk-cp-…`), as opposed to a pay-as-you-go
/// one. MiniMax keeps the two apart; neither works in the other's place.
pub fn is_plan_key(api_key: &str) -> bool {
    api_key.trim().starts_with("sk-cp-")
}

/// A model's published requests-per-minute limit. Ids the table does not list
/// — M3 and anything newer — get M3's, the stricter of the two.
fn published_rpm(model_id: &str) -> u64 {
    if model_id.to_lowercase().starts_with("minimax-m2") {
        500
    } else {
        200
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatchPacing {
    /// Most requests in flight at once; `None` leaves the user's setting alone.
    pub max_in_flight: Option<usize>,
    /// Least time between two request starts: the published RPM with a quarter
    /// kept back for whatever else is using the key meanwhile.
    pub min_interval: std::time::Duration,
}

pub fn batch_pacing(api_key: &str, model_id: &str) -> BatchPacing {
    let per_minute = published_rpm(model_id) * 3 / 4;
    BatchPacing {
        max_in_flight: is_plan_key(api_key).then_some(PLAN_MAX_IN_FLIGHT),
        min_interval: std::time::Duration::from_millis(60_000 / per_minute),
    }
}

/// The provider's message for a MiniMax business code, led by what it means in
/// plain words.
///
/// MiniMax's own text is kept — it carries the specifics, such as when a Token
/// Plan window resets — and the code stays on the end as ` (NNNN)`, which is
/// the marker `llm::classify_error` reads. Nothing in the added wording may
/// carry a marker of its own: the code has to be what decides.
pub fn describe_code(code: u32, msg: &str) -> String {
    let msg = msg.trim();
    let label = match code {
        2056 => Some("Token Plan 额度已用尽，需等当前用量窗口（5 小时 / 每周）重置，或升级套餐"),
        2062 => Some("Token Plan 请求过于频繁，已被限流，请稍后重试"),
        1002 => Some("请求过于频繁（RPM/TPM 超限），已被限流，请稍后重试"),
        2045 => Some("请求量增长过快，已被限流，请稍后重试"),
        1041 => Some("连接数超限，已被限流，请稍后重试"),
        2064 => Some("整点高峰时段服务器繁忙"),
        1008 => Some("账户余额不足，请充值"),
        1004 | 2049 => Some("API Key 无效或未授权，请在 设置 → AI 供应商 中检查密钥"),
        1026 => Some("输入内容涉敏，被安全策略拦截"),
        1027 => Some("输出内容涉敏，被安全策略拦截"),
        2013 => Some("请求参数错误"),
        _ => None,
    };
    // MiniMax already ends most messages with the code; say it once.
    let suffix = if msg.contains(&format!("({code})")) || msg.contains(&format!("（{code}）")) {
        String::new()
    } else {
        format!(" ({code})")
    };
    match (label, msg.is_empty()) {
        (Some(label), false) => format!("{label}：{msg}{suffix}"),
        (Some(label), true) => format!("{label}{suffix}"),
        (None, false) => format!("{msg}{suffix}"),
        (None, true) => format!("MiniMax 返回错误{suffix}"),
    }
}

/// The MiniMax code a message ends with — `…请稍后重试 (2064)` — when it is one
/// [`code_class`] knows. Only a *trailing* code counts, which is where MiniMax
/// writes it; a four-digit number in parentheses mid-sentence (a year, say) in
/// another provider's message is left alone.
pub fn trailing_code(msg: &str) -> Option<u32> {
    let t = msg.trim_end();
    let (body, close) = if let Some(b) = t.strip_suffix(')') {
        (b, ')')
    } else {
        (t.strip_suffix('）')?, '）')
    };
    let open = body.rfind(['(', '（'])?;
    let open_char = body[open..].chars().next()?;
    // `( … ）` mixes are not MiniMax's; insist on a matching pair.
    if (open_char == '(') != (close == ')') {
        return None;
    }
    let digits = &body[open + open_char.len_utf8()..];
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let code: u32 = digits.parse().ok()?;
    code_class(code).map(|_| code)
}

/// A business error in MiniMax's `base_resp` envelope — which it can send with
/// an HTTP 200 and no `choices` — as `(code, status_msg)`. `None` for
/// `status_code: 0` (success, which rides every normal response), for a missing
/// envelope, and for a code that is not a number. The code may be spelled as a
/// number or as a numeric string.
pub fn base_resp_error(json: &serde_json::Value) -> Option<(i64, String)> {
    let resp = json.get("base_resp")?.as_object()?;
    let raw = resp.get("status_code")?;
    let code = raw
        .as_i64()
        .or_else(|| raw.as_str().and_then(|s| s.trim().parse().ok()))?;
    if code == 0 {
        return None;
    }
    let msg = resp
        .get("status_msg")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    Some((code, msg))
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
    fn thinking_is_never_disabled_on_the_flash_preview() {
        let mut body = serde_json::json!({"thinking": {"type": "disabled"}});
        apply_thinking(&mut body, "MiniMax-M3.1-Flash-Preview", false);
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
    fn a_code_is_explained_and_kept_once_on_the_end() {
        let m = describe_code(2064, "当前为整点高峰时段，服务器短暂繁忙。请稍后重试 (2064)");
        assert!(m.starts_with("整点高峰时段服务器繁忙："), "{m}");
        assert_eq!(m.matches("2064").count(), 1, "{m}");

        let m = describe_code(2056, "usage limit exceeded");
        assert!(m.contains("Token Plan 额度已用尽"), "{m}");
        assert!(m.ends_with("usage limit exceeded (2056)"), "{m}");

        // Full-width parentheses count as already carrying the code.
        assert_eq!(describe_code(9999, "奇怪的错误（9999）"), "奇怪的错误（9999）");
        // Unknown code: MiniMax's words, code appended.
        assert_eq!(describe_code(9999, "odd"), "odd (9999)");
        // No words at all still says something.
        assert_eq!(describe_code(1008, ""), "账户余额不足，请充值 (1008)");
        assert_eq!(describe_code(9999, " "), "MiniMax 返回错误 (9999)");
    }

    #[test]
    fn only_a_trailing_known_code_is_read() {
        assert_eq!(trailing_code("请稍后重试 (2064)"), Some(2064));
        assert_eq!(trailing_code("已达到 Token Plan 用量上限。(2056)  "), Some(2056));
        assert_eq!(trailing_code("限流（2062）"), Some(2062));
        // Not at the end, not four digits, not a code MiniMax documents, or
        // a mismatched pair.
        assert_eq!(trailing_code("(2064) then more"), None);
        assert_eq!(trailing_code("status (429)"), None);
        assert_eq!(trailing_code("published (2026)"), None);
        assert_eq!(trailing_code("mixed (2064）"), None);
        assert_eq!(trailing_code(""), None);
    }

    #[test]
    fn only_slow_down_codes_are_throttles() {
        for code in [1002, 1041, 2045, 2062, 2064] {
            assert!(is_throttle_code(code), "{code}");
            assert_eq!(code_class(code), Some(crate::llm::ErrorClass::Transient), "{code}");
        }
        // A used-up plan window, a timeout, an internal error, no balance.
        for code in [2056, 1001, 1013, 1008] {
            assert!(!is_throttle_code(code), "{code}");
        }
    }

    #[test]
    fn a_plan_key_is_held_to_four_in_flight_and_every_key_to_the_models_rpm() {
        use std::time::Duration;
        let plan = batch_pacing(" sk-cp-abc", "MiniMax-M3");
        assert_eq!(plan.max_in_flight, Some(PLAN_MAX_IN_FLIGHT));
        // 200 RPM, three quarters of it: 150 a minute.
        assert_eq!(plan.min_interval, Duration::from_millis(400));

        let pay_as_you_go = batch_pacing("sk-api-abc", "MiniMax-M2.7-highspeed");
        assert_eq!(pay_as_you_go.max_in_flight, None);
        assert_eq!(pay_as_you_go.min_interval, Duration::from_millis(160));

        // An id the published table does not list gets the stricter limit.
        assert_eq!(
            batch_pacing("sk-api-abc", "MiniMax-M3.1-Flash-Preview").min_interval,
            Duration::from_millis(400)
        );
    }

    #[test]
    fn base_resp_is_an_error_only_when_its_code_is_not_zero() {
        let err = serde_json::json!({"base_resp": {"status_code": 1002, "status_msg": " rpm "}});
        assert_eq!(base_resp_error(&err), Some((1002, "rpm".to_string())));
        let err = serde_json::json!({"base_resp": {"status_code": "2056"}});
        assert_eq!(base_resp_error(&err), Some((2056, String::new())));
        for ok in [
            serde_json::json!({"base_resp": {"status_code": 0, "status_msg": "success"}}),
            serde_json::json!({"base_resp": {"status_code": "0"}}),
            serde_json::json!({"base_resp": null}),
            serde_json::json!({"choices": []}),
        ] {
            assert_eq!(base_resp_error(&ok), None, "{ok}");
        }
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
