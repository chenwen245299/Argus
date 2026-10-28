//! NUS SoCLaaS uses the standard Chat Completions and Embeddings endpoints.
//! Keep the public aliases from /models; backend model identities are private
//! and must not be inferred from an alias or substituted into a request.
//!
//! API/quickstart reference supplied by the user (2026-09-02):
//! https://dochub.comp.nus.edu.sg/cf/guides/soclaas/api
//! https://dochub.comp.nus.edu.sg/cf/guides/soclaas/quickstart
//! Efforts, prices and modalities come from the authenticated /models catalogue;
//! aliases can change their upstream model and must never have fixed effort rules.

use crate::models::{AiModel, AiProvider};
use serde_json::Value;

pub fn is_soclaas(provider: &AiProvider) -> bool {
    provider.kind == "soclaas"
        || provider.base_url.to_ascii_lowercase().contains("soclaas")
        || provider.name.to_ascii_lowercase().contains("soclaas")
        || provider.name.to_ascii_lowercase().contains("soclass")
}

/// SoCLaaS adds its display metadata under `soclaas`; standard fields remain
/// valid fallbacks. Never alter the model id or manufacture a catalogue.
pub fn catalogue_item(item: &Value) -> Value {
    let mut result = item.clone();
    for (nested, standard) in [("display_name", "name"), ("description", "description")] {
        if let Some(text) = item["soclaas"][nested]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            result[standard] = Value::String(text.to_string());
        }
    }
    if let Some(length) = [
        "context_length",
        "context_window",
        "max_context_length",
        "max_context_tokens",
        "max_model_len",
    ]
    .iter()
    .find_map(|key| item[*key].as_u64().filter(|n| *n > 0))
    {
        result["context_length"] = length.into();
    }
    result
}

/// Enrich a standard model without changing its public id. A missing effort
/// declaration means no adjustable reasoning, rather than an invented range.
pub fn enrich_model(model: &mut AiModel, item: &Value) {
    model.reasoning_efforts = Some(
        item["reasoning"]["supported_efforts"]
            .as_array()
            .map(|values| {
                let mut efforts = Vec::new();
                for value in values {
                    if let Some(value) = value.as_str().map(str::trim).filter(|s| !s.is_empty()) {
                        if !efforts.iter().any(|s| s == value) {
                            efforts.push(value.to_string());
                        }
                    }
                }
                efforts
            })
            .unwrap_or_default(),
    );
    let mut add = |cap: &str| {
        if !model.capabilities.iter().any(|s| s == cap) {
            model.capabilities.push(cap.into());
        }
    };
    if model
        .reasoning_efforts
        .as_ref()
        .is_some_and(|v| v.iter().any(|s| s != "none"))
    {
        add("reasoning");
    }
    if let Some(caps) = item["soclaas"]["capabilities"].as_array() {
        for cap in caps.iter().filter_map(Value::as_str) {
            match cap {
                "chat" => add("chat"),
                "embedding" | "embeddings" => add("embedding"),
                "audio-transcription" | "audio" => add("audio"),
                "tool_calling" | "tools" => add("tool_calling"),
                _ => {}
            }
        }
    }
    // The gateway's explicit input modalities outrank name-based guesses.
    if let Some(modalities) = item["soclaas"]["input_modalities"].as_array() {
        model.capabilities.retain(|c| c != "vision");
        if modalities
            .iter()
            .filter_map(Value::as_str)
            .any(|s| s == "vision" || s == "image")
        {
            model.capabilities.push("vision".into());
        }
    }
    let price = |key: &str| {
        item["soclaas"][key]
            .as_f64()
            .filter(|p| p.is_finite() && *p >= 0.0)
            .map(|p| p / 1_000_000.0)
    };
    if let Some(p) = price("input_microdollars_per_million_tokens") {
        model.input_price_usd_per_million = Some(p);
    }
    if let Some(p) = price("output_microdollars_per_million_tokens") {
        model.output_price_usd_per_million = Some(p);
    }
    // Token prices alone cannot make a billed transcription model free.
    model.is_free = model.input_price_usd_per_million == Some(0.0)
        && model.output_price_usd_per_million == Some(0.0)
        && item["soclaas"]["audio_microdollars_per_minute"]
            .as_f64()
            .unwrap_or(0.0)
            == 0.0;
}

fn effort_rank(effort: &str) -> i32 {
    match effort {
        "none" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        "max" => 6,
        _ => 7,
    }
}

/// Preserve an exact choice; otherwise pick the closest declared level, with
/// ties going up. Off uses none only when declared, or the lowest valid level.
pub fn resolve_effort<'a>(
    supported: &'a [String],
    enabled: bool,
    requested: &str,
) -> Option<&'a str> {
    if !enabled {
        return supported
            .iter()
            .min_by_key(|s| effort_rank(s))
            .map(String::as_str);
    }
    let thinking: Vec<_> = supported.iter().filter(|s| s.as_str() != "none").collect();
    thinking
        .iter()
        .find(|s| s.as_str() == requested)
        .map(|s| s.as_str())
        .or_else(|| {
            thinking
                .iter()
                .min_by_key(|s| {
                    let rank = effort_rank(s);
                    ((rank - effort_rank(requested)).abs(), -rank)
                })
                .map(|s| s.as_str())
        })
}

pub fn apply_reasoning(
    body: &mut Value,
    provider: &AiProvider,
    model: &str,
    enabled: bool,
    effort: Option<&str>,
) {
    // Clear values from generic OpenAI/model-name branches. Unknown metadata
    // leaves the gateway's default intact until the catalogue refresh succeeds.
    if let Some(obj) = body.as_object_mut() {
        for key in [
            "reasoning",
            "reasoning_effort",
            "thinking",
            "enable_thinking",
        ] {
            obj.remove(key);
        }
    }
    let supported = provider
        .models
        .iter()
        .find(|m| m.id == model)
        .and_then(|m| m.reasoning_efforts.as_deref())
        .unwrap_or_default();
    if let Some(effective) = resolve_effort(supported, enabled, effort.unwrap_or("high")) {
        body["reasoning_effort"] = Value::String(effective.to_string());
    }
}

/// Catalogue-owned fields can refresh without overwriting user labels,
/// capabilities, enabled flags, custom CNY prices or endpoint preferences.
pub fn refresh_model(saved: &mut AiModel, fresh: &AiModel) -> bool {
    let changed = saved.reasoning_efforts != fresh.reasoning_efforts
        || saved.context_length != fresh.context_length;
    saved.reasoning_efforts = fresh.reasoning_efforts.clone();
    saved.context_length = fresh.context_length;
    changed
}

/// Public aliases can be repointed by operators. Repair only an explicit
/// effort-validation refusal that lists the accepted values, and change no
/// other parameter. The caller allows one retry before any stream is consumed.
pub fn repair_reasoning_rejection(body: &mut Value, error_body: &str) -> bool {
    let Ok(error) = serde_json::from_str::<Value>(error_body) else {
        return false;
    };
    let message = error["error"]
        .as_str()
        .or_else(|| error["error"]["message"].as_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !(message.contains("reasoning effort") || message.contains("reasoning_effort"))
        || !(message.contains("unexpected")
            || message.contains("unsupported")
            || message.contains("invalid"))
    {
        return false;
    }
    let Some((_, values)) = message
        .split_once("supported types are")
        .or_else(|| message.split_once("supported values are"))
    else {
        return false;
    };
    // Recognize only complete, explicit effort tokens from the validation list.
    let supported: Vec<String> = values
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| {
            matches!(
                *s,
                "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max"
            )
        })
        .map(str::to_string)
        .collect();
    let Some(current) = body["reasoning_effort"].as_str() else {
        return false;
    };
    if supported.iter().any(|s| s == current) {
        return false;
    }
    let Some(replacement) = resolve_effort(&supported, current != "none", current) else {
        return false;
    };
    body["reasoning_effort"] = Value::String(replacement.to_string());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn provider(kind: &str, name: &str, url: &str) -> AiProvider {
        serde_json::from_value(json!({
            "id": "test", "kind": kind, "name": name, "base_url": url,
            "models": [], "created_at": "2026-10-13",
        }))
        .unwrap()
    }

    #[test]
    fn recognizes_new_and_existing_generic_configs() {
        assert!(is_soclaas(&provider(
            "soclaas",
            "Research",
            "https://relay.example/v1"
        )));
        assert!(is_soclaas(&provider(
            "openai_compatible",
            "Custom",
            "https://soclaas-api.comp.nus.edu.sg/v1"
        )));
        assert!(is_soclaas(&provider(
            "openai_compatible",
            "SoCLaaS",
            "https://relay.example/v1"
        )));
        assert!(!is_soclaas(&provider(
            "openai_compatible",
            "OpenAI",
            "https://api.openai.com/v1"
        )));
    }

    #[test]
    fn catalogue_preserves_aliases_and_standard_fields() {
        let item = json!({
            "id": "internal-chat", "name": "Fallback", "context_window": 65536,
            "capabilities": ["vision", "tool_calling"],
            "soclaas": {"display_name": " Research Chat ", "description": " Model description "},
        });
        let mapped = catalogue_item(&item);
        assert_eq!(mapped["id"], "internal-chat");
        assert_eq!(mapped["name"], "Research Chat");
        assert_eq!(mapped["description"], "Model description");
        assert_eq!(mapped["context_length"], 65536);
        assert_eq!(mapped["capabilities"], item["capabilities"]);
        assert_eq!(
            catalogue_item(
                &json!({"id": "alias", "name": "Fallback", "soclaas": {"display_name": " "}})
            )["name"],
            "Fallback"
        );
    }

    #[test]
    fn context_aliases_are_respected_without_zero_windows() {
        for key in [
            "context_length",
            "context_window",
            "max_context_tokens",
            "max_context_length",
            "max_model_len",
        ] {
            let mut item = json!({"id": "alias"});
            item[key] = json!(65536);
            assert_eq!(catalogue_item(&item)["context_length"], 65536);
        }
        assert_eq!(
            catalogue_item(&json!({"id": "alias", "context_length": 0, "max_model_len": 32768}))
                ["context_length"],
            32768
        );
    }

    fn with_efforts(model: &str, efforts: &[&str]) -> AiProvider {
        let mut p = provider("soclaas", "NUS", "https://soclaas-api.comp.nus.edu.sg/v1");
        p.models = serde_json::from_value(json!([{
            "id": model, "display_name": model, "reasoning_efforts": efforts,
        }]))
        .unwrap();
        p
    }

    #[test]
    fn discovered_efforts_follow_the_current_alias_without_changing_tools() {
        for (model, efforts) in [
            ("x-test-1", vec!["max", "high", "low"]),
            ("x-test-2", vec!["max", "high", "low", "none"]),
            ("x-test-3", vec!["xhigh", "medium", "low", "none"]),
            ("x-test-4", vec!["max", "xhigh", "high", "medium", "low"]),
            ("qwen3.8:27b", vec!["xhigh", "medium", "low"]),
            ("coder", vec!["xhigh", "medium", "low"]),
        ] {
            let p = with_efforts(model, &efforts);
            for effort in efforts.iter().filter(|s| **s != "none") {
                let mut body = json!({"model": model, "tools": [{"type": "function"}], "stream_options": {"include_usage": true}});
                let original = body.clone();
                apply_reasoning(&mut body, &p, model, true, Some(effort));
                assert_eq!(body["reasoning_effort"], *effort);
                assert_eq!(body["tools"], original["tools"]);
                assert_eq!(body["stream_options"], original["stream_options"]);
                assert_eq!(body["model"], model);
            }
            let mut body = json!({});
            apply_reasoning(&mut body, &p, model, false, None);
            assert_eq!(
                body["reasoning_effort"],
                if efforts.contains(&"none") {
                    "none"
                } else {
                    "low"
                }
            );
        }
        // The same public alias used to accept xhigh; no fixed rule survives.
        let mut body = json!({});
        apply_reasoning(
            &mut body,
            &with_efforts("x-test-1", &["max", "high", "low"]),
            "x-test-1",
            true,
            Some("xhigh"),
        );
        assert_eq!(body["reasoning_effort"], "max");
    }

    #[test]
    fn unsupported_saved_preferences_select_the_nearest_valid_effort() {
        for (efforts, wanted, expected) in [
            (vec!["max", "high", "low"], "medium", "high"),
            (vec!["xhigh", "medium", "low"], "high", "xhigh"),
            (
                vec!["max", "xhigh", "high", "medium", "low"],
                "xhigh",
                "xhigh",
            ),
            (vec!["high", "low"], "max", "high"),
        ] {
            let supported: Vec<String> = efforts.into_iter().map(str::to_string).collect();
            assert_eq!(resolve_effort(&supported, true, wanted), Some(expected));
        }
    }

    #[test]
    fn unknown_and_nonreasoning_models_never_get_invented_parameters() {
        for p in [
            provider("soclaas", "NUS", "https://example.test/v1"),
            with_efforts("plain", &[]),
        ] {
            let mut body = json!({"reasoning_effort": "high", "thinking": {"type": "enabled"}, "reasoning": {}, "enable_thinking": true});
            apply_reasoning(&mut body, &p, "plain", true, None);
            assert_eq!(body, json!({}));
            apply_reasoning(&mut body, &p, "plain", false, None);
            assert_eq!(body, json!({}));
        }
    }

    #[test]
    fn old_settings_still_deserialize_and_discovered_metadata_round_trips() {
        let mut p = with_efforts("alias", &["max", "high", "low"]);
        let encoded = serde_json::to_value(&p).unwrap();
        let restored: AiProvider = serde_json::from_value(encoded).unwrap();
        assert_eq!(
            restored.models[0].reasoning_efforts,
            p.models[0].reasoning_efforts
        );
        let mut old = serde_json::to_value(&p.models[0]).unwrap();
        old.as_object_mut().unwrap().remove("reasoning_efforts");
        p.models[0] = serde_json::from_value(old).unwrap();
        assert_eq!(p.models[0].reasoning_efforts, None);
    }

    #[test]
    fn background_refresh_preserves_personal_edits_but_updates_alias_facts() {
        let mut saved = with_efforts("alias", &["xhigh", "medium", "low"])
            .models
            .remove(0);
        saved.display_name = "My label".into();
        saved.capabilities = vec!["tool_calling".into()];
        saved.input_price_per_million = Some(2.0);
        let mut fresh = with_efforts("alias", &["max", "high", "low"])
            .models
            .remove(0);
        fresh.context_length = Some(1048576);
        assert!(refresh_model(&mut saved, &fresh));
        assert_eq!(saved.reasoning_efforts, fresh.reasoning_efforts);
        assert_eq!(saved.context_length, Some(1048576));
        assert_eq!(saved.display_name, "My label");
        assert_eq!(saved.capabilities, vec!["tool_calling"]);
        assert_eq!(saved.input_price_per_million, Some(2.0));
        assert!(!refresh_model(&mut saved, &fresh));
    }

    #[test]
    fn an_alias_can_follow_its_explicit_effort_validation() {
        let mut body = json!({"model": "operator-alias", "reasoning_effort": "high", "tools": [{"type": "function"}]});
        let error = json!({"error": "Unexpected reasoning effort high. Supported types are xhigh (default), medium, and low."}).to_string();
        assert!(repair_reasoning_rejection(&mut body, &error));
        assert_eq!(body["reasoning_effort"], "xhigh");
        assert_eq!(body["model"], "operator-alias");
        assert_eq!(body["tools"], json!([{"type": "function"}]));
        assert!(!repair_reasoning_rejection(&mut body, &error));
        body["reasoning_effort"] = json!("none");
        assert!(repair_reasoning_rejection(&mut body, &error));
        assert_eq!(body["reasoning_effort"], "low");
        body["reasoning_effort"] = json!("xhigh");
        assert!(repair_reasoning_rejection(&mut body, &json!({"error": {"message": "Unsupported reasoning_effort xhigh. Supported values are low, medium, high."}}).to_string()));
        assert_eq!(body["reasoning_effort"], "high");
    }

    #[test]
    fn unrelated_refusals_and_unsupported_guesses_are_not_retried() {
        for message in [
            "quota exceeded",
            "tools are unsupported",
            "invalid model",
            "Unsupported reasoning effort",
            "Invalid reasoning effort high. Supported types are unknown, auto.",
        ] {
            let mut body = json!({"reasoning_effort": "high"});
            assert!(
                !repair_reasoning_rejection(&mut body, &json!({"error": message}).to_string()),
                "{message}"
            );
            assert_eq!(body["reasoning_effort"], "high");
        }
    }
}
