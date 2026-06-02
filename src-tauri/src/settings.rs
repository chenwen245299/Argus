use std::path::Path;

use crate::models::AppSettings;

fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    let metadata_prompt = settings.metadata_ai_prompt.trim();
    if metadata_prompt.is_empty() || metadata_prompt.contains("a concise paper abstract") {
        settings.metadata_ai_prompt = crate::models::default_metadata_ai_prompt();
    }

    let prompt = settings.ai_summary_prompt.trim();
    if prompt.is_empty() || crate::models::is_legacy_ai_summary_prompt(prompt) {
        settings.ai_summary_prompt = crate::models::default_ai_summary_prompt();
    }

    let abstract_prompt = settings.abstract_ai_prompt.trim();
    if abstract_prompt.is_empty() || crate::models::is_legacy_abstract_ai_prompt(abstract_prompt) {
        settings.abstract_ai_prompt = crate::models::default_abstract_ai_prompt();
    }

    if settings.translate_ai_prompt.trim().is_empty() {
        settings.translate_ai_prompt = crate::models::default_translate_ai_prompt();
    }

    settings
}

/// Read AppSettings from `app_settings` key in `.argus/config.json`.
/// Returns defaults if the key is absent or the file is missing.
pub fn read_settings(root: &str) -> AppSettings {
    let path = Path::new(root).join(".argus").join("config.json");
    if !path.exists() {
        return AppSettings::default();
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return AppSettings::default(),
    };
    let config: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return AppSettings::default(),
    };
    match config.get("app_settings") {
        Some(v) => normalize_settings(serde_json::from_value(v.clone()).unwrap_or_default()),
        None => AppSettings::default(),
    }
}

/// Write AppSettings into `app_settings` key in `.argus/config.json`.
/// Preserves all other keys in the file (e.g. `version`, `created_at`).
pub fn write_settings(root: &str, settings: &AppSettings) -> Result<(), String> {
    let path = Path::new(root).join(".argus").join("config.json");
    let settings = normalize_settings(settings.clone());

    let mut config: serde_json::Value = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|c| serde_json::from_str(&c).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    config["app_settings"] =
        serde_json::to_value(&settings).map_err(|e| format!("Serialize AppSettings: {e}"))?;

    let content =
        serde_json::to_string_pretty(&config).map_err(|e| format!("Serialize config.json: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Write config.json: {e}"))
}
