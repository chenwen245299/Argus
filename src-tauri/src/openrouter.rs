//! OpenRouter's server-side tools, and the traces they leave in a response.
//!
//! These are not Argus's tools. A `function` tool comes back to us as a call we
//! have to run and answer; an `openrouter:*` tool is run by OpenRouter, fed back
//! to the model, and the stream simply continues — the client never sees a turn
//! boundary. That is why they can be offered on an ordinary chat request without
//! an agent loop around it, and why they are attached by default: a model that
//! never needs one is billed nothing for having had the option.
//!
//! What the response carries afterwards is documented only in pieces, so this
//! module reads what is specified and ignores the rest:
//!   * web search results surface as `url_citation` annotations on the message;
//!   * generated images surface as `images` on the message;
//!   * `usage.server_tool_use` counts what actually ran.
//!
//! Reference: <https://openrouter.ai/docs/guides/features/server-tools>

use std::collections::BTreeMap;

use serde::Serialize;

use crate::models::AiProvider;

pub fn is_openrouter(provider: &AiProvider) -> bool {
    provider.kind == "openrouter" || provider.base_url.to_lowercase().contains("openrouter")
}

/// Whether attaching server tools to `model` is safe.
///
/// A model that cannot emit a tool call cannot reach for a server tool either,
/// and OpenRouter answers such a request with an error rather than ignoring the
/// field — so a model the catalogue records as text-only keeps exactly the
/// request it had before this feature existed. A model with no capabilities on
/// record (hand-added, or fetched before they were) is given the tools: most of
/// the catalogue supports them, and refusing by default would quietly disable
/// the feature for anyone who typed a model id in by hand.
pub fn model_accepts_server_tools(provider: &AiProvider, model: &str) -> bool {
    match provider.models.iter().find(|m| m.id == model) {
        Some(entry) if !entry.capabilities.is_empty() => entry.capabilities.iter().any(|c| {
            let c = c.to_lowercase();
            c.contains("tool") || c.contains("function")
        }),
        _ => true,
    }
}

/// The `tools` entries to attach to an OpenRouter request.
///
/// Empty for every other provider, for a model that cannot use them, and for an
/// OpenRouter provider whose tools the user has all switched off — callers treat
/// an empty list as "add nothing" rather than sending `tools: []`, which some
/// gateways reject.
pub fn server_tool_defs(provider: &AiProvider, model: &str) -> Vec<serde_json::Value> {
    if !is_openrouter(provider) || !model_accepts_server_tools(provider, model) {
        return Vec::new();
    }
    let cfg = &provider.server_tools;
    let mut tools = Vec::new();

    if cfg.web_search {
        let mut params = serde_json::Map::new();
        if let Some(n) = cfg.web_search_max_results {
            params.insert("max_results".into(), serde_json::json!(n));
        }
        tools.push(tool("openrouter:web_search", params));
    }
    if cfg.web_fetch {
        tools.push(tool("openrouter:web_fetch", serde_json::Map::new()));
    }
    if cfg.datetime {
        let mut params = serde_json::Map::new();
        if let Some(tz) = cfg.timezone.as_ref().filter(|s| !s.trim().is_empty()) {
            params.insert("timezone".into(), serde_json::json!(tz));
        }
        tools.push(tool("openrouter:datetime", params));
    }
    if cfg.image_generation {
        let mut params = serde_json::Map::new();
        if let Some(model) = cfg.image_model.as_ref().filter(|s| !s.trim().is_empty()) {
            params.insert("model".into(), serde_json::json!(model));
        }
        tools.push(tool("openrouter:image_generation", params));
    }
    tools
}

/// `parameters` is omitted entirely when empty so each tool falls through to
/// OpenRouter's own defaults rather than to a hard-coded copy of them.
fn tool(kind: &str, params: serde_json::Map<String, serde_json::Value>) -> serde_json::Value {
    let mut value = serde_json::json!({ "type": kind });
    if !params.is_empty() {
        value["parameters"] = serde_json::Value::Object(params);
    }
    value
}

/// Step budget for one request, clamped to what OpenRouter accepts.
pub fn max_tool_calls(provider: &AiProvider) -> u32 {
    provider.server_tools.max_tool_calls.clamp(1, 30)
}

// ── What the tools left behind ───────────────────────────────────────────────

/// One page the model consulted, as reported by a `url_citation` annotation.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Citation {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Everything the server tools contributed to one answer.
///
/// Accumulated across a stream and emitted once at the end: annotations arrive
/// piecemeal and often repeat, and a citation strip that reshuffles itself
/// while the answer is still being written is worse than one that appears when
/// the answer is done.
#[derive(Debug, Default, Clone)]
pub struct ServerToolTrace {
    citations: Vec<Citation>,
    images: Vec<String>,
    /// Per-tool call counts, from `usage.server_tool_use`.
    calls: BTreeMap<String, u64>,
}

impl ServerToolTrace {
    pub fn is_empty(&self) -> bool {
        self.citations.is_empty() && self.images.is_empty() && self.calls.is_empty()
    }

    /// Read a streaming `choices[].delta` or a whole `message` — both carry the
    /// same `annotations` / `images` fields.
    pub fn absorb(&mut self, node: &serde_json::Value) {
        if let Some(annotations) = node.get("annotations").and_then(|a| a.as_array()) {
            for annotation in annotations {
                // `{"type":"url_citation","url_citation":{"url":…,"title":…}}`,
                // with the fields sometimes hoisted onto the annotation itself.
                let body = annotation.get("url_citation").unwrap_or(annotation);
                let Some(url) = body.get("url").and_then(|u| u.as_str()) else {
                    continue;
                };
                if url.is_empty() || self.citations.iter().any(|c| c.url == url) {
                    continue;
                }
                self.citations.push(Citation {
                    url: url.to_string(),
                    title: body
                        .get("title")
                        .and_then(|t| t.as_str())
                        .filter(|t| !t.is_empty())
                        .map(str::to_string),
                });
            }
        }

        if let Some(images) = node.get("images").and_then(|i| i.as_array()) {
            for image in images {
                // Either `{"image_url":{"url":…}}` or the bare URL.
                let url = image
                    .get("image_url")
                    .and_then(|i| i.get("url"))
                    .and_then(|u| u.as_str())
                    .or_else(|| image.get("url").and_then(|u| u.as_str()))
                    .or_else(|| image.as_str());
                if let Some(url) = url.filter(|u| !u.is_empty()) {
                    if !self.images.iter().any(|existing| existing == url) {
                        self.images.push(url.to_string());
                    }
                }
            }
        }
    }

    /// Read `usage.server_tool_use`, which counts what actually ran.
    pub fn absorb_usage(&mut self, usage: &serde_json::Value) {
        let Some(counts) = usage.get("server_tool_use").and_then(|u| u.as_object()) else {
            return;
        };
        for (name, value) in counts {
            let Some(n) = value.as_u64().filter(|n| *n > 0) else {
                continue;
            };
            // `web_search_requests` -> `web_search`, so the UI can name the tool
            // rather than the counter.
            let tool = name.strip_suffix("_requests").unwrap_or(name);
            let entry = self.calls.entry(tool.to_string()).or_insert(0);
            *entry = (*entry).max(n);
        }
    }

    pub fn to_payload(&self) -> serde_json::Value {
        serde_json::json!({
            "citations": self.citations,
            "images": self.images,
            "calls": self.calls,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ServerTools;

    fn provider(kind: &str, url: &str, tools: ServerTools) -> AiProvider {
        AiProvider {
            id: "p1".into(),
            name: "P".into(),
            kind: kind.into(),
            base_url: url.into(),
            enabled: true,
            models: vec![],
            server_tools: tools,
            created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    #[test]
    fn only_openrouter_gets_server_tools() {
        let p = provider(
            "openai_compatible",
            "https://api.deepseek.com/v1",
            ServerTools::default(),
        );
        assert!(server_tool_defs(&p, "openai/gpt-5.2").is_empty());
    }

    #[test]
    fn all_four_tools_are_attached_by_default() {
        let p = provider(
            "openrouter",
            "https://openrouter.ai/api/v1",
            ServerTools::default(),
        );
        let types: Vec<String> = server_tool_defs(&p, "openai/gpt-5.2")
            .iter()
            .map(|t| t["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            types,
            vec![
                "openrouter:web_search",
                "openrouter:web_fetch",
                "openrouter:datetime",
                "openrouter:image_generation",
            ]
        );
        // No `parameters` block on a default config, so OpenRouter's own
        // defaults apply rather than a stale copy of them.
        assert!(server_tool_defs(&p, "openai/gpt-5.2").iter().all(|t| t.get("parameters").is_none()));
    }

    #[test]
    fn switching_everything_off_sends_nothing() {
        let p = provider(
            "openrouter",
            "https://openrouter.ai/api/v1",
            ServerTools {
                web_search: false,
                web_fetch: false,
                datetime: false,
                image_generation: false,
                ..ServerTools::default()
            },
        );
        assert!(server_tool_defs(&p, "openai/gpt-5.2").is_empty());
    }

    #[test]
    fn configured_parameters_reach_the_right_tool() {
        let p = provider(
            "openrouter",
            "https://openrouter.ai/api/v1",
            ServerTools {
                web_search_max_results: Some(3),
                timezone: Some("Asia/Shanghai".into()),
                image_model: Some("openai/gpt-5-image".into()),
                ..ServerTools::default()
            },
        );
        let defs = server_tool_defs(&p, "openai/gpt-5.2");
        assert_eq!(defs[0]["parameters"]["max_results"], 3);
        assert!(defs[1].get("parameters").is_none());
        assert_eq!(defs[2]["parameters"]["timezone"], "Asia/Shanghai");
        assert_eq!(defs[3]["parameters"]["model"], "openai/gpt-5-image");
    }

    #[test]
    fn the_step_budget_stays_inside_what_openrouter_accepts() {
        let mut tools = ServerTools::default();
        tools.max_tool_calls = 500;
        let p = provider("openrouter", "https://openrouter.ai/api/v1", tools);
        assert_eq!(max_tool_calls(&p), 30);

        let mut tools = ServerTools::default();
        tools.max_tool_calls = 0;
        let p = provider("openrouter", "https://openrouter.ai/api/v1", tools);
        assert_eq!(max_tool_calls(&p), 1);
    }

    /// A model the catalogue records as text-only must keep the exact request it
    /// had before server tools existed — OpenRouter rejects `tools` outright
    /// rather than ignoring it.
    #[test]
    fn a_text_only_model_is_left_alone() {
        let mut p = provider(
            "openrouter",
            "https://openrouter.ai/api/v1",
            ServerTools::default(),
        );
        p.models = vec![
            serde_json::from_value(serde_json::json!({
                "id": "some/text-only",
                "display_name": "Text only",
                "capabilities": ["reasoning"],
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "id": "some/with-tools",
                "display_name": "With tools",
                "capabilities": ["reasoning", "tool_calling"],
            }))
            .unwrap(),
        ];
        assert!(server_tool_defs(&p, "some/text-only").is_empty());
        assert!(!server_tool_defs(&p, "some/with-tools").is_empty());
        // Not in the catalogue at all: given the tools rather than silently
        // opted out, since most of OpenRouter's models support them.
        assert!(!server_tool_defs(&p, "some/hand-typed").is_empty());
    }

    #[test]
    fn citations_are_collected_once_however_often_they_repeat() {
        let mut trace = ServerToolTrace::default();
        let delta = serde_json::json!({
            "annotations": [
                {"type": "url_citation", "url_citation": {"url": "https://a.test", "title": "A"}},
                {"type": "url_citation", "url_citation": {"url": "https://a.test", "title": "A"}},
                {"type": "url_citation", "url_citation": {"url": "https://b.test"}},
            ]
        });
        trace.absorb(&delta);
        trace.absorb(&delta);
        let payload = trace.to_payload();
        assert_eq!(payload["citations"].as_array().unwrap().len(), 2);
        assert_eq!(payload["citations"][0]["title"], "A");
        // A citation without a title omits the field rather than sending "".
        assert!(payload["citations"][1].get("title").is_none());
    }

    #[test]
    fn images_are_read_from_either_shape() {
        let mut trace = ServerToolTrace::default();
        trace.absorb(&serde_json::json!({
            "images": [
                {"type": "image_url", "image_url": {"url": "https://img.test/a.png"}},
                "https://img.test/b.png",
            ]
        }));
        let payload = trace.to_payload();
        assert_eq!(
            payload["images"],
            serde_json::json!(["https://img.test/a.png", "https://img.test/b.png"])
        );
    }

    /// Usage arrives cumulatively, so the counter must settle on the highest
    /// figure seen rather than adding each report to the last.
    #[test]
    fn tool_call_counts_do_not_double_up() {
        let mut trace = ServerToolTrace::default();
        trace.absorb_usage(&serde_json::json!({"server_tool_use": {"web_search_requests": 2}}));
        trace.absorb_usage(&serde_json::json!({"server_tool_use": {"web_search_requests": 3}}));
        assert_eq!(trace.to_payload()["calls"]["web_search"], 3);
    }

    #[test]
    fn an_answer_that_used_no_tool_leaves_no_trace() {
        let mut trace = ServerToolTrace::default();
        trace.absorb(&serde_json::json!({"content": "hello"}));
        trace.absorb_usage(&serde_json::json!({"prompt_tokens": 10}));
        assert!(trace.is_empty());
    }
}
