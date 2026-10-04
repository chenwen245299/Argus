use std::path::Path;

use crate::models::AppSettings;

fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    if !settings.usd_to_cny_rate.is_finite() || settings.usd_to_cny_rate <= 0.0 {
        settings.usd_to_cny_rate = crate::models::default_usd_to_cny_rate();
    }

    let metadata_prompt = settings.metadata_ai_prompt.trim();
    if metadata_prompt.is_empty() || crate::models::is_legacy_metadata_ai_prompt(metadata_prompt) {
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

    if settings.sections_ai_prompt.trim().is_empty() {
        settings.sections_ai_prompt = crate::models::default_sections_ai_prompt();
    }

    // A cleared dropdown can come back as "" rather than null. Read-aloud tells
    // "not configured" by `None`, so a blank id has to become one — otherwise the
    // 朗读 button would call a provider named "".
    for id in [&mut settings.speech_provider_id, &mut settings.speech_model_id] {
        if id.as_deref().is_some_and(|s| s.trim().is_empty()) {
            *id = None;
        } else if let Some(s) = id.as_mut() {
            *s = s.trim().to_string();
        }
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
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| format!("Write config.json: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_speech_id_is_not_configured() {
        let mut s = AppSettings::default();
        s.speech_provider_id = Some("  ".into());
        s.speech_model_id = Some(" speech-2.8-turbo ".into());
        let n = normalize_settings(s);
        assert_eq!(n.speech_provider_id, None);
        // Ids are trimmed, not rewritten.
        assert_eq!(n.speech_model_id.as_deref(), Some("speech-2.8-turbo"));
    }

    /// Through the real file path: an old `config.json` with other settings in
    /// it keeps them and gains the speech defaults.
    #[test]
    fn an_old_config_file_reads_with_speech_defaults() {
        let dir = std::env::temp_dir().join(format!("argus-speech-settings-{}", std::process::id()));
        let argus = dir.join(".argus");
        std::fs::create_dir_all(&argus).unwrap();
        std::fs::write(
            argus.join("config.json"),
            r#"{"version":"1","app_settings":{"appearance":"forest","extraction_default":"lopdf"}}"#,
        )
        .unwrap();
        let s = read_settings(dir.to_str().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(s.appearance, "forest");
        assert!(s.speech_skip_citations);
        assert_eq!(s.speech_provider_id, None);
        assert!(s.speech_options.is_empty());
    }
}
