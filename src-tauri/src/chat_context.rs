//! Model context is separate from the visible, full-fidelity conversation.
//! Inspired by DeepSeek Harness token-meter / compaction-basic (MIT): latest
//! prompt sample plus surface delta, 80% pressure, 16% retained balanced tail.
use crate::{llm, models::AiProvider};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    sync::{atomic::AtomicBool, Arc},
};

pub fn estimate_text(text: &str) -> usize {
    let (mut ascii, mut wide) = (0usize, 0usize);
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1
        } else {
            wide += 1
        }
    }
    wide + ascii.div_ceil(4)
}

fn content_tokens(content: &Value) -> usize {
    match content {
        Value::String(s) => estimate_text(s),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p["type"].as_str() {
                Some("text") => estimate_text(p["text"].as_str().unwrap_or_default()) + 4,
                Some("image_url") => {
                    if p["image_url"]["detail"] == "low" {
                        85
                    } else {
                        2048
                    }
                }
                Some("file") => 4096,
                Some("input_audio" | "video_url") => 8192,
                _ => 256,
            })
            .sum(),
        _ => 0,
    }
}

pub fn message_tokens(message: &Value) -> usize {
    4 + content_tokens(&message["content"])
        + message
            .get("tool_calls")
            .map_or(0, |c| estimate_text(&c.to_string()))
        + message["reasoning_content"]
            .as_str()
            .map_or(0, estimate_text)
}

#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub window: usize,
    pub capacity_estimated: bool,
    pub reserve: usize,
    pub threshold: usize,
    pub retain: usize,
    pub summary: usize,
}
impl Policy {
    pub fn for_model(provider: &AiProvider, model: &str) -> Self {
        let declared = provider
            .models
            .iter()
            .find(|m| m.id == model)
            .and_then(|m| m.context_length)
            .filter(|n| *n > 0);
        let window = declared.unwrap_or(128_000) as usize;
        // Scale reservations to small models too; never inherit a 65K reserve
        // into an 8K window. Leave both answer space and compaction headroom.
        let reserve = (window / 10).clamp(1, 16_384);
        Self {
            window,
            capacity_estimated: declared.is_none(),
            reserve,
            threshold: (window * 4 / 5).min(window.saturating_sub(reserve * 2)),
            retain: window.saturating_sub(reserve) * 16 / 100,
            summary: (window / 32).clamp(1, 4096),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptSample {
    pub estimated_tokens: usize,
    pub input_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used_tokens: usize,
    pub context_window: usize,
    pub remaining_tokens: usize,
    pub system_tokens: usize,
    pub tools_tokens: usize,
    pub message_tokens: usize,
    pub reserved_tokens: usize,
    pub threshold_tokens: usize,
    pub estimated: bool,
    pub capacity_estimated: bool,
    pub media_estimated: bool,
    pub can_compact: bool,
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub system_fingerprint: String,
}

impl ContextUsage {
    pub fn project_from(&mut self, previous: &Self) {
        if self.provider_id == previous.provider_id
            && self.model_id == previous.model_id
            && self.context_window == previous.context_window
            && self.system_tokens == previous.system_tokens
            && !self.system_fingerprint.is_empty()
            && self.system_fingerprint == previous.system_fingerprint
            && (self.tools_tokens == 0) == (previous.tools_tokens == 0)
        {
            // Hover does not start MCP processes. Keep the last live schema
            // price until another request measures the complete tool surface.
            self.tools_tokens = previous.tools_tokens;
            self.used_tokens = previous
                .used_tokens
                .saturating_add(self.message_tokens)
                .saturating_sub(previous.message_tokens);
            self.remaining_tokens = self.context_window.saturating_sub(self.used_tokens);
            self.estimated = true;
        }
    }
}

pub fn measure(
    messages: &[Value],
    tools: &[Value],
    policy: Policy,
    provider: &AiProvider,
    model: &str,
    sample: Option<&PromptSample>,
) -> ContextUsage {
    let system_tokens = messages
        .iter()
        .filter(|m| m["role"] == "system")
        .map(message_tokens)
        .sum();
    let conversation_tokens = messages
        .iter()
        .filter(|m| m["role"] != "system")
        .map(message_tokens)
        .sum();
    let tools_tokens = if tools.is_empty() {
        0
    } else {
        estimate_text(&json!(tools).to_string()) + 4
    };
    let estimated_tokens = system_tokens + conversation_tokens + tools_tokens;
    let used_tokens = sample.map_or(estimated_tokens, |s| {
        s.input_tokens
            .saturating_add(estimated_tokens)
            .saturating_sub(s.estimated_tokens)
    });
    ContextUsage {
        used_tokens,
        context_window: policy.window,
        remaining_tokens: policy.window.saturating_sub(used_tokens),
        system_tokens,
        tools_tokens,
        message_tokens: conversation_tokens,
        reserved_tokens: policy.reserve,
        threshold_tokens: policy.threshold,
        estimated: sample.is_none()
            || sample.is_some_and(|s| s.estimated_tokens != estimated_tokens),
        capacity_estimated: policy.capacity_estimated,
        media_estimated: messages.iter().any(|m| {
            m["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] != "text"))
        }),
        can_compact: compact_range(messages, policy.retain).is_some_and(|(start, end)| {
            messages[start..end]
                .iter()
                .map(message_tokens)
                .sum::<usize>()
                >= 256
        }),
        provider_id: provider.id.clone(),
        model_id: model.into(),
        system_fingerprint: {
            // A same-length prompt edit must invalidate the old calibration.
            // Stable FNV-1a identifies content without persisting prompt text.
            let mut hash = 0xcbf29ce484222325u64;
            for message in messages.iter().filter(|m| m["role"] == "system") {
                for byte in message.to_string().bytes().chain(std::iter::once(0)) {
                    hash = (hash ^ byte as u64).wrapping_mul(0x100000001b3);
                }
            }
            format!("{hash:016x}")
        },
    }
}

/// Whole assistant-call/result groups must stay on the same side of a cut.
/// Closed steps of an oversized turn may compact; an unfinished pair may not.
pub fn compact_range(messages: &[Value], retain: usize) -> Option<(usize, usize)> {
    let first = messages.iter().position(|m| m["role"] != "system")?;
    let mut pending = HashSet::new();
    let mut boundaries = Vec::new();
    for (i, m) in messages.iter().enumerate().skip(first) {
        if let Some(calls) = m["tool_calls"].as_array() {
            for call in calls {
                pending.insert(call["id"].as_str()?.to_string());
            }
        }
        if m["role"] == "tool" && !pending.remove(m["tool_call_id"].as_str()?) {
            return None;
        }
        if pending.is_empty() && i + 1 < messages.len() {
            boundaries.push(i + 1);
        }
    }
    if !pending.is_empty() {
        return None;
    }
    let mut tokens = 0;
    let mut keep = messages.len();
    for (i, m) in messages.iter().enumerate().rev() {
        tokens += message_tokens(m);
        keep = i;
        if tokens >= retain {
            break;
        }
    }
    let end = boundaries
        .into_iter()
        .filter(|i| *i <= keep && *i > first)
        .last()?;
    Some((first, end))
}

fn checkpoint(summary: &str) -> Value {
    json!({"role":"user", "content":format!(
        "Earlier conversation checkpoint. Use it as background; continue from the following messages. \
         Preserve the user's constraints and distinguish verified evidence from hypotheses.\n\n<compacted-summary>\n{summary}\n</compacted-summary>")})
}

pub fn is_context_rejection(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    [
        "(400)",
        "(413)",
        "(422)",
        "api error 400",
        "api error 413",
        "api error 422",
    ]
    .iter()
    .any(|s| lower.contains(s))
        && [
            "context_length_exceeded",
            "maximum context length",
            "context window exceeded",
            "too many input tokens",
            "上下文长度超出",
        ]
        .iter()
        .any(|s| lower.contains(s))
}

/// For histories already bigger than the selected model, summarize bounded
/// text batches instead of sending another oversized request. Never tokenize
/// base64: retain attachment identities and label omitted media explicitly.
fn transcript_text(messages: &[Value]) -> String {
    messages.iter().map(|m| {
        let content = if let Some(s) = m["content"].as_str() { s.to_string() }
        else { m["content"].as_array().map(|parts| parts.iter().map(|p| {
            if p["type"] == "text" { p["text"].as_str().unwrap_or_default().to_string() }
            else { format!("[Earlier {} attachment; pixels/audio omitted. Filename: {}. \
                Preserve only observations already recorded in the conversation.]",
                p["type"].as_str().unwrap_or("media"), p["file"]["filename"].as_str().unwrap_or("unnamed")) }
        }).collect::<Vec<_>>().join("\n")).unwrap_or_default() };
        format!("{}: {}\n{}", m["role"].as_str().unwrap_or("message"), content,
            m.get("tool_calls").map_or(String::new(), Value::to_string))
    }).collect::<Vec<_>>().join("\n\n")
}

fn text_chunks(text: &str, cap: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut part = String::new();
    let mut units = 0;
    for c in text.chars() {
        let cost = if c.is_ascii() { 1 } else { 4 };
        if units + cost > cap * 4 && !part.is_empty() {
            chunks.push(std::mem::take(&mut part));
            units = 0;
        }
        part.push(c);
        units += cost;
    }
    if !part.is_empty() {
        chunks.push(part);
    }
    chunks
}

fn commit_summary(
    messages: &mut Vec<Value>,
    start: usize,
    end: usize,
    summary: &str,
    limit: usize,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<(), String> {
    if summary.trim().is_empty() {
        return Err("Compaction returned an empty summary.".into());
    }
    if estimate_text(summary) > limit * 2 {
        return Err("Compaction summary exceeded its budget.".into());
    }
    let before: usize = messages[start..end].iter().map(message_tokens).sum();
    let replacement = checkpoint(summary.trim());
    if message_tokens(&replacement) >= before {
        return Err("Compaction did not reduce context size.".into());
    }
    if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err("cancelled".into());
    }
    messages.splice(start..end, [replacement]);
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedContext {
    pub messages: Vec<Value>,
    pub usage: ContextUsage,
    pub compacted: bool,
    pub summary: Option<String>,
}

pub async fn compact(
    messages: &mut Vec<Value>,
    tools: &[Value],
    policy: Policy,
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    event: &str,
    app: &tauri::AppHandle,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<Option<String>, String> {
    use tauri::Emitter;
    let Some((start, end)) = compact_range(messages, policy.retain) else {
        return Ok(None);
    };
    let before: usize = messages[start..end].iter().map(message_tokens).sum();
    // Avoid a model call for tiny ranges whose summary framing costs more.
    if before < 256 {
        return Ok(None);
    }
    let _ = app.emit(
        format!("{event}-context-status").as_str(),
        json!({"phase":"compacting"}),
    );
    let result: Result<String, String> = async {
        let systems = messages[..start].to_vec();
        let system_size: usize = systems.iter().map(message_tokens).sum();
        let tool_size = if tools.is_empty() { 0 } else { estimate_text(&json!(tools).to_string()) + 4 };
        let input_cap = policy.window.saturating_sub(system_size + tool_size + policy.summary * 2 + policy.reserve + 512);
        if input_cap < 256 { return Err("System instructions leave no room to summarize this conversation.".into()); }
        let native_input = before <= input_cap;
        let batches = if native_input { vec![messages[start..end].to_vec()] }
            else { text_chunks(&transcript_text(&messages[start..end]), input_cap).into_iter()
                .map(|text| vec![json!({"role":"user", "content":text})]).collect() };
        let mut summary = String::new();
        for batch in batches {
            let mut input = systems.clone();
            if !summary.is_empty() { input.push(checkpoint(&summary)); }
            input.extend(batch);
            input.push(json!({"role":"user", "content":format!(
                "Produce a compact research conversation checkpoint, within {} tokens. \
                Record goals and latest corrections; established facts and exact quantitative details; \
                paper titles/slugs, citations, page numbers and useful tool evidence; decisions and constraints; \
                unresolved questions and the next action. Keep uncertainty and failed/interrupted work explicit. \
                Merge an earlier checkpoint with new evidence, removing stale conclusions. \
                Use the conversation's language. Summarize only: do not answer its questions or run tools.", policy.summary)}));
            summary = crate::cancel::until_cancelled(cancel.as_ref(), llm::compact_completion(
                provider, api_key, model, &input, tools, policy.summary + policy.reserve, &format!("{event}-compact-{}", uuid::Uuid::new_v4()), app, cancel.clone()))
                .await.ok_or("cancelled")??;
            if summary.trim().is_empty() { return Err("Compaction returned an empty summary.".into()); }
            if estimate_text(&summary) > policy.summary * 2 { return Err("Compaction summary exceeded its budget.".into()); }
        }
        // Commit only after a successful, useful, uncancelled summary.
        commit_summary(messages, start, end, &summary, policy.summary, cancel.as_ref())?;
        Ok(summary)
    }.await;
    let _ = app.emit(
        format!("{event}-context-status").as_str(),
        match &result {
            Ok(_) => json!({"phase":"compacted"}),
            Err(e) => json!({"phase":"error", "error":e}),
        },
    );
    result.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn msg(role: &str, text: &str) -> Value {
        json!({"role":role,"content":text})
    }
    fn provider(window: Option<u64>) -> AiProvider {
        serde_json::from_value(json!({"id":"p","name":"test","kind":"openai_compatible",
            "base_url":"https://example.test/v1","enabled":true,"created_at":"2026-10-15",
            "models":[{"id":"m","display_name":"test","context_length":window}]}))
        .unwrap()
    }
    #[test]
    fn balances_parallel_calls_and_retains_question() {
        let mut m = vec![
            msg("system", "instructions"),
            msg("user", "old question"),
            json!({"role":"assistant","content":"","tool_calls":[{"id":"a"},{"id":"b"}]}),
            json!({"role":"tool","tool_call_id":"a","content":"result"}),
            json!({"role":"tool","tool_call_id":"b","content":"result"}),
            msg("assistant", "answer"),
            msg("user", "current question"),
        ];
        assert_eq!(compact_range(&m, 1), Some((1, 6)));
        m.pop();
        // A long single turn can compact its closed earlier tool steps.
        assert_eq!(compact_range(&m, 1), Some((1, 5)));
        m.push(json!({"role":"tool","tool_call_id":"orphan","content":"bad"}));
        assert_eq!(compact_range(&m, 1), None);
    }
    #[test]
    fn scripts_and_media_do_not_count_base64_as_tokens() {
        assert_eq!(estimate_text("abcdefgh中文"), 4);
        let image = |n| json!({"role":"user","content":[{"type":"image_url","image_url":{"url":"a".repeat(n)}}]});
        assert_eq!(
            message_tokens(&image(10)),
            message_tokens(&image(1_000_000))
        );
        let chunks = text_chunks("中🦀abcdef", 2);
        assert_eq!(chunks.concat(), "中🦀abcdef");
        assert!(chunks.iter().all(|s| estimate_text(s) <= 2));
    }
    #[test]
    fn sample_tracks_surface_delta_and_saturates() {
        let p = provider(Some(8192));
        let policy = Policy::for_model(&p, "m");
        let mut messages = vec![msg("system", "instructions"), msg("user", "question")];
        let tools =
            vec![json!({"type":"function","function":{"name":"find_papers","parameters":{}}})];
        let initial = measure(&messages, &tools, policy, &p, "m", None);
        let sample = PromptSample {
            estimated_tokens: initial.used_tokens,
            input_tokens: 900,
        };
        assert_eq!(
            measure(&messages, &tools, policy, &p, "m", Some(&sample)).used_tokens,
            900
        );
        messages.push(msg("assistant", &"a".repeat(400)));
        let after = measure(&messages, &tools, policy, &p, "m", Some(&sample));
        assert_eq!(after.used_tokens, 1004);
        assert_eq!(after.remaining_tokens, 8192 - 1004);
        assert!(after.tools_tokens > 0 && after.system_tokens > 0);
        let under = PromptSample {
            estimated_tokens: 1_000_000,
            input_tokens: 1,
        };
        assert_eq!(
            measure(&messages, &tools, policy, &p, "m", Some(&under)).used_tokens,
            0
        );
    }
    #[test]
    fn capacity_policy_scales_and_unknown_is_labelled() {
        for window in [8192, 128_000, 1_000_000] {
            let policy = Policy::for_model(&provider(Some(window)), "m");
            assert!(policy.retain < policy.threshold);
            assert!(policy.threshold + policy.reserve <= policy.window);
            assert_eq!(policy.threshold, policy.window * 4 / 5);
            assert!(!policy.capacity_estimated);
        }
        let unknown = Policy::for_model(&provider(None), "m");
        assert_eq!(unknown.window, 128_000);
        assert!(unknown.capacity_estimated);
    }
    #[test]
    fn checkpoint_commit_is_atomic_and_preserves_the_tail() {
        let original = vec![
            msg("system", "immutable instructions"),
            msg("user", &"old ".repeat(1000)),
            msg("user", "current task"),
        ];
        for summary in ["", &"x".repeat(10000)] {
            let mut m = original.clone();
            assert!(commit_summary(&mut m, 1, 2, summary, 1000, None).is_err());
            assert_eq!(m, original);
        }
        let cancel = Arc::new(AtomicBool::new(true));
        let mut m = original.clone();
        assert_eq!(
            commit_summary(&mut m, 1, 2, "goal and citations", 1000, Some(&cancel)).unwrap_err(),
            "cancelled"
        );
        assert_eq!(m, original);
        commit_summary(&mut m, 1, 2, "goal and citations", 1000, None).unwrap();
        assert_eq!(m[0], original[0]);
        assert_eq!(m[2], original[2]);
        assert!(m[1]["content"]
            .as_str()
            .unwrap()
            .contains("<compacted-summary>"));
        let mut tiny = vec![msg("user", "a")];
        let old = tiny.clone();
        assert!(commit_summary(&mut tiny, 0, 1, "larger summary", 1000, None).is_err());
        assert_eq!(tiny, old);
    }
    #[test]
    fn only_explicit_http_overflow_qualifies_for_recovery() {
        assert!(is_context_rejection(
            "API error 400: maximum context length is 8192"
        ));
        assert!(is_context_rejection("context_length_exceeded (422)"));
        assert!(!is_context_rejection("invalid max_tokens (400)"));
        assert!(!is_context_rejection(
            "context window exceeded during an SSE stream"
        ));
        assert!(!is_context_rejection("maximum context length (429)"));
    }
    #[test]
    fn idle_projection_keeps_last_live_schemas_but_never_another_route() {
        let p = provider(Some(8192));
        let policy = Policy::for_model(&p, "m");
        let messages = vec![msg("system", "instructions"), msg("user", "question")];
        let mut old = measure(
            &messages,
            &[json!({"external":"tools"})],
            policy,
            &p,
            "m",
            None,
        );
        old.used_tokens = 1000;
        old.tools_tokens = 500;
        let mut next = measure(
            &messages,
            &[json!({"builtin":"only"})],
            policy,
            &p,
            "m",
            None,
        );
        next.project_from(&old);
        assert_eq!(next.used_tokens, 1000);
        assert_eq!(next.tools_tokens, 500);
        next.model_id = "other".into();
        next.used_tokens = 50;
        next.project_from(&old);
        assert_eq!(next.used_tokens, 50);
    }

    #[test]
    fn same_size_system_edits_do_not_reuse_old_prompt_calibration() {
        let p = provider(Some(8192));
        let policy = Policy::for_model(&p, "m");
        let mut old = measure(&[msg("system", "original"), msg("user", "question")], &[], policy, &p, "m", None);
        old.used_tokens = 1000;
        let mut next = measure(&[msg("system", "modified"), msg("user", "question")], &[], policy, &p, "m", None);
        assert_eq!(next.system_tokens, old.system_tokens);
        let fresh = next.used_tokens;
        next.project_from(&old);
        assert_eq!(next.used_tokens, fresh);
        // Old saved records have no content fingerprint and must be remeasured.
        old.system_fingerprint.clear();
        next.project_from(&old);
        assert_eq!(next.used_tokens, fresh);
    }
}
