use std::path::Path;

use crate::models::AppSettings;

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GuidedReadingPreset {
    pub id: String,
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GuidedReadingSettings {
    pub presets: Vec<GuidedReadingPreset>,
    pub default_id: String,
}

impl Default for GuidedReadingSettings {
    fn default() -> Self {
        Self { presets: vec![], default_id: "builtin".into() }
    }
}

impl GuidedReadingSettings {
    fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::HashSet::new();
        for preset in &self.presets {
            if preset.id.trim().is_empty() || preset.id == "builtin"
                || preset.name.trim().is_empty() || preset.prompt.trim().is_empty()
                || !ids.insert(&preset.id)
            {
                return Err("Each reading preset needs a unique id, a name and a nonempty prompt.".into());
            }
        }
        if self.default_id != "builtin" && !ids.contains(&self.default_id) {
            return Err("The default reading preset does not exist.".into());
        }
        Ok(())
    }
}

/// Kept separately so saving general settings in another window cannot overwrite
/// the preset collection with an older AppSettings snapshot.
pub fn read_guided_reading_settings(root: &str) -> GuidedReadingSettings {
    let path = Path::new(root).join(".argus").join("guided_reading.json");
    let mut settings: GuidedReadingSettings = std::fs::read_to_string(path).ok()
        .and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default();
    let mut ids = std::collections::HashSet::new();
    settings.presets.retain(|p| !p.id.trim().is_empty() && p.id != "builtin"
        && !p.name.trim().is_empty() && !p.prompt.trim().is_empty() && ids.insert(p.id.clone()));
    if settings.default_id != "builtin" && !ids.contains(&settings.default_id) {
        settings.default_id = "builtin".into();
    }
    settings
}

pub fn write_guided_reading_settings(root: &str, settings: &GuidedReadingSettings) -> Result<(), String> {
    settings.validate()?;
    let path = Path::new(root).join(".argus").join("guided_reading.json");
    let text = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    crate::fsutil::atomic_write_str(&path, &text).map_err(|e| e.to_string())
}

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
    fn reading_presets_persist_exact_text_and_do_not_touch_general_settings() {
        let dir = std::env::temp_dir().join(format!("argus-guides-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".argus")).unwrap();
        let root = dir.to_str().unwrap();
        assert_eq!(read_guided_reading_settings(root).default_id, "builtin");
        write_settings(root, &AppSettings::default()).unwrap();
        let config = std::fs::read(dir.join(".argus/config.json")).unwrap();
        let settings = GuidedReadingSettings {
            presets: vec![GuidedReadingPreset { id: "math".into(), name: "公式推导".into(), prompt: "逐式解释。\n\n保留 $x$。  ".into() }],
            default_id: "math".into(),
        };
        write_guided_reading_settings(root, &settings).unwrap();
        let loaded = read_guided_reading_settings(root);
        assert_eq!(loaded.default_id, "math");
        assert_eq!(loaded.presets[0].prompt, settings.presets[0].prompt);
        assert_eq!(std::fs::read(dir.join(".argus/config.json")).unwrap(), config);
        let invalid = GuidedReadingSettings { default_id: "deleted".into(), ..settings.clone() };
        assert!(write_guided_reading_settings(root, &invalid).is_err());
        assert_eq!(read_guided_reading_settings(root).default_id, "math");
        let duplicate = GuidedReadingSettings { presets: vec![settings.presets[0].clone(), settings.presets[0].clone()], ..settings };
        assert!(write_guided_reading_settings(root, &duplicate).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reading_presets_repair_missing_defaults_without_losing_valid_prompts() {
        let dir = std::env::temp_dir().join(format!("argus-guides-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join(".argus")).unwrap();
        std::fs::write(dir.join(".argus/guided_reading.json"), r#"{"defaultId":"gone","presets":[{"id":"ok","name":"Guide","prompt":"exact"},{"id":"empty","name":"No prompt"},{"id":"ok","name":"Duplicate","prompt":"other"}]}"#).unwrap();
        let loaded = read_guided_reading_settings(dir.to_str().unwrap());
        assert_eq!(loaded.default_id, "builtin");
        assert_eq!(loaded.presets.len(), 1);
        assert_eq!(loaded.presets[0].prompt, "exact");
        std::fs::remove_dir_all(dir).unwrap();
    }

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
