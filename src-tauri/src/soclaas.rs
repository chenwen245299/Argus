//! NUS SoCLaaS uses the standard Chat Completions and Embeddings endpoints.
//! Keep the public aliases from /models; backend model identities are private
//! and must not be inferred from an alias or substituted into a request.
//!
//! API/quickstart reference supplied by the user (2026-09-02):
//! https://dochub.comp.nus.edu.sg/cf/guides/soclaas/api
//! https://dochub.comp.nus.edu.sg/cf/guides/soclaas/quickstart
//! The x-test-1 effort range comes from its explicit API validation response:
//! xhigh (default), medium, low. It does not accept high or none.

use crate::models::AiProvider;
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

/// The app's high/medium/low control describes relative effort. x-test-1's
/// highest setting is xhigh; when thinking is hidden it still needs its lowest
/// valid setting. Other models use the documented OpenAI-style none switch.
pub fn apply_reasoning(body: &mut Value, model: &str, enabled: bool, effort: Option<&str>) {
    let requested = if enabled {
        effort.unwrap_or("high")
    } else {
        "none"
    };
    let effective = if model.eq_ignore_ascii_case("x-test-1") {
        match requested {
            "none" | "low" => "low",
            "medium" => "medium",
            _ => "xhigh",
        }
    } else {
        requested
    };
    body["reasoning_effort"] = Value::String(effective.to_string());
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
    let supported: Vec<_> = values.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    let Some(current) = body["reasoning_effort"].as_str() else {
        return false;
    };
    if supported.contains(&current) {
        return false;
    }
    let replacement = match current {
        "high" if supported.contains(&"xhigh") => "xhigh",
        "xhigh" if supported.contains(&"high") => "high",
        "none" if supported.contains(&"low") => "low",
        _ => return false,
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

    #[test]
    fn x_test_effort_is_valid_in_both_directions_and_preserves_tools() {
        for (enabled, requested, expected) in [
            (true, Some("low"), "low"),
            (true, Some("medium"), "medium"),
            (true, Some("high"), "xhigh"),
            (true, None, "xhigh"),
            (false, Some("high"), "low"),
        ] {
            let mut body = json!({"model": "x-test-1", "tools": [{"type": "function"}], "stream_options": {"include_usage": true}});
            let original = body.clone();
            apply_reasoning(&mut body, "x-test-1", enabled, requested);
            assert_eq!(body["reasoning_effort"], expected);
            assert_eq!(body["tools"], original["tools"]);
            assert_eq!(body["stream_options"], original["stream_options"]);
            assert_eq!(body["model"], "x-test-1");
        }
    }

    #[test]
    fn ordinary_models_use_the_documented_none_switch() {
        for model in ["gemma4:26b", "qwen3.5:9b", "llama3.1:8b", "other-alias"] {
            let mut body = json!({});
            apply_reasoning(&mut body, model, false, None);
            assert_eq!(body["reasoning_effort"], "none");
            apply_reasoning(&mut body, model, true, Some("medium"));
            assert_eq!(body["reasoning_effort"], "medium");
            apply_reasoning(&mut body, model, true, Some("high"));
            assert_eq!(body["reasoning_effort"], "high");
        }
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
            "Invalid reasoning effort high. Supported types are none, low, medium.",
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
