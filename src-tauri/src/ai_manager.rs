use std::collections::HashMap;
use std::path::Path;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use rand::RngCore;

use crate::models::{
    AiModel, AiProvider, AiProviderInfo, AiProviderInput, AiSettings, AiSettingsInfo,
};

// ── Hex helpers ───────────────────────────────────────────────────────────────

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn from_hex(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("Odd-length hex string".to_string());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

// ── Master key (random, stored per-library) ───────────────────────────────────

fn master_key_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join(".keymaster")
}

fn get_or_create_master_key(root: &str) -> Result<[u8; 32], String> {
    let path = master_key_path(root);
    if path.exists() {
        let hex = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let bytes = from_hex(hex.trim())?;
        if bytes.len() != 32 {
            return Err("Corrupt master key".to_string());
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(key)
    } else {
        let dir = Path::new(root).join(".argus");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let mut key = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut key);
        crate::fsutil::atomic_write_str(&path, &to_hex(&key)).map_err(|e| e.to_string())?;
        Ok(key)
    }
}

// ── Encrypted keys map (.argus/api_keys.json) ─────────────────────────────────

fn keys_map_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join("api_keys.json")
}

fn read_keys_map(root: &str) -> HashMap<String, String> {
    std::fs::read_to_string(keys_map_path(root))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_keys_map(root: &str, map: &HashMap<String, String>) -> Result<(), String> {
    let dir = Path::new(root).join(".argus");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let content = serde_json::to_string_pretty(map).map_err(|e| e.to_string())?;
    crate::fsutil::atomic_write_str(&keys_map_path(root), &content).map_err(|e| e.to_string())
}

// ── Key CRUD ──────────────────────────────────────────────────────────────────

/// Reserved key id for the easyScholar secret in the encrypted key store.
/// Namespaced with underscores so it never collides with a real provider UUID.
pub const EASYSCHOLAR_KEY_ID: &str = "__easyscholar__";

/// Reserved key id for the Semantic Scholar API key in the encrypted key store.
/// A key moves requests off the anonymous shared pool onto a private quota,
/// which is the single biggest lever against 429 rate-limit errors.
pub const SEMANTIC_SCHOLAR_KEY_ID: &str = "__semantic_scholar__";

pub fn save_api_key(root: &str, provider_id: &str, key: &str) -> Result<(), String> {
    let master = get_or_create_master_key(root)?;
    let cipher = Aes256Gcm::new_from_slice(&master).map_err(|e| e.to_string())?;

    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, key.as_bytes())
        .map_err(|e| format!("Encryption failed: {e}"))?;

    let encoded = format!("{}:{}", to_hex(&nonce_bytes), to_hex(&ciphertext));
    let mut map = read_keys_map(root);
    map.insert(provider_id.to_string(), encoded);
    write_keys_map(root, &map)
}

pub fn get_api_key(root: &str, provider_id: &str) -> Option<String> {
    let master = get_or_create_master_key(root).ok()?;
    let cipher = Aes256Gcm::new_from_slice(&master).ok()?;

    let map = read_keys_map(root);
    let encoded = map.get(provider_id)?;

    let (nonce_hex, ct_hex) = encoded.split_once(':')?;
    let nonce_bytes = from_hex(nonce_hex).ok()?;
    let ciphertext = from_hex(ct_hex).ok()?;
    if nonce_bytes.len() != 12 {
        return None;
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher.decrypt(nonce, ciphertext.as_ref()).ok()?;
    String::from_utf8(plaintext).ok().filter(|s| !s.is_empty())
}

pub fn has_api_key(root: &str, provider_id: &str) -> bool {
    get_api_key(root, provider_id).is_some()
}

pub fn delete_api_key(root: &str, provider_id: &str) {
    let mut map = read_keys_map(root);
    if map.remove(provider_id).is_some() {
        let _ = write_keys_map(root, &map);
    }
}

// ── AI settings persistence ───────────────────────────────────────────────────

pub fn read_ai_settings(root: &str) -> AiSettings {
    let path = Path::new(root).join(".argus").join("ai_providers.json");
    if !path.exists() {
        return AiSettings::default();
    }
    let mut settings: AiSettings = std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default();

    // Migration: older Kimi Code providers may have an empty model list because
    // the /coding endpoint has no public /models list. Backfill the built-in
    // model so the provider shows up in the model selector immediately.
    let mut changed = false;
    for p in &mut settings.providers {
        if p.models.is_empty() && is_kimi_code(&p.kind, &p.base_url) {
            p.models = crate::llm::kimi_known_models();
            changed = true;
        }
    }
    if changed {
        let _ = write_ai_settings(root, &settings);
    }
    settings
}

pub fn write_ai_settings(root: &str, settings: &AiSettings) -> Result<(), String> {
    let dir = Path::new(root).join(".argus");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("ai_providers.json");
    let content = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("Serialize AI settings: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write ai_providers.json: {e}"))
}

// ── Info conversion (strips keys) ────────────────────────────────────────────

pub fn to_info(root: &str, settings: &AiSettings) -> AiSettingsInfo {
    AiSettingsInfo {
        providers: settings
            .providers
            .iter()
            .map(|p| AiProviderInfo {
                id: p.id.clone(),
                name: p.name.clone(),
                kind: p.kind.clone(),
                base_url: p.base_url.clone(),
                enabled: p.enabled,
                has_key: has_api_key(root, &p.id),
                // Fill in sizes the provider's catalogue never carried. Done on
                // the way out rather than on save, so models added before the
                // table existed are right immediately instead of waiting for
                // the user to re-fetch the list.
                models: p
                    .models
                    .iter()
                    .cloned()
                    .map(|mut m| {
                        crate::llm::apply_known_param_size(&mut m);
                        m
                    })
                    .collect(),
                server_tools: p.server_tools.clone(),
            })
            .collect(),
        default_provider_id: settings.default_provider_id.clone(),
        default_model_id: settings.default_model_id.clone(),
    }
}

// ── CRUD ──────────────────────────────────────────────────────────────────────

fn is_kimi_code(kind: &str, base_url: &str) -> bool {
    kind == "kimi" || base_url.to_lowercase().contains("api.kimi.com")
}

pub fn add_provider(
    root: &str,
    input: AiProviderInput,
    api_key: &str,
) -> Result<AiProvider, String> {
    // Reject non-http(s) base URLs so a crafted provider can't exfiltrate the
    // API key via schemes like file:// or gopher://. Local servers (Ollama, LM
    // Studio on http://localhost) stay allowed.
    crate::net::validate_provider_url(&input.base_url)?;
    let mut settings = read_ai_settings(root);
    let id = uuid::Uuid::new_v4().to_string();
    let mut models = input.models;
    if models.is_empty() && is_kimi_code(&input.kind, &input.base_url) {
        models = crate::llm::kimi_known_models();
    }
    let provider = AiProvider {
        id: id.clone(),
        name: input.name,
        kind: input.kind,
        base_url: input.base_url,
        enabled: input.enabled,
        models,
        server_tools: input.server_tools.unwrap_or_default(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    if !api_key.is_empty() {
        save_api_key(root, &id, api_key)?;
    }
    settings.providers.push(provider.clone());
    write_ai_settings(root, &settings)?;
    Ok(provider)
}

pub fn update_provider(
    root: &str,
    input: AiProviderInput,
    api_key: Option<&str>,
) -> Result<(), String> {
    crate::net::validate_provider_url(&input.base_url)?;
    let id = input
        .id
        .as_deref()
        .ok_or("Provider id required for update")?;
    let mut settings = read_ai_settings(root);
    let p = settings
        .providers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Provider not found: {id}"))?;
    p.name = input.name;
    p.kind = input.kind.clone();
    p.base_url = input.base_url.clone();
    p.enabled = input.enabled;
    p.models = if input.models.is_empty() && is_kimi_code(&input.kind, &input.base_url) {
        crate::llm::kimi_known_models()
    } else {
        input.models
    };
    // Absent means "unchanged": the provider edit form does not carry the tool
    // switches, and a rename must not reset them.
    if let Some(tools) = input.server_tools {
        p.server_tools = tools;
    }
    if let Some(key) = api_key {
        if !key.is_empty() {
            save_api_key(root, id, key)?;
        }
    }
    write_ai_settings(root, &settings)
}

pub fn delete_provider(root: &str, id: &str) -> Result<(), String> {
    let mut settings = read_ai_settings(root);
    let before = settings.providers.len();
    settings.providers.retain(|p| p.id != id);
    if settings.providers.len() == before {
        return Err(format!("Provider not found: {id}"));
    }
    delete_api_key(root, id);
    if settings.default_provider_id.as_deref() == Some(id) {
        settings.default_provider_id = None;
        settings.default_model_id = None;
    }
    write_ai_settings(root, &settings)
}

pub fn set_provider_enabled(root: &str, id: &str, enabled: bool) -> Result<(), String> {
    let mut settings = read_ai_settings(root);
    let p = settings
        .providers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Provider not found: {id}"))?;
    p.enabled = enabled;
    write_ai_settings(root, &settings)
}

pub fn save_provider_models(root: &str, id: &str, models: Vec<AiModel>) -> Result<(), String> {
    let mut settings = read_ai_settings(root);
    let p = settings
        .providers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Provider not found: {id}"))?;
    p.models = models;
    write_ai_settings(root, &settings)
}

pub fn set_default_model(root: &str, provider_id: &str, model_id: &str) -> Result<(), String> {
    let mut settings = read_ai_settings(root);
    settings.default_provider_id = Some(provider_id.to_string());
    settings.default_model_id = Some(model_id.to_string());
    write_ai_settings(root, &settings)
}

// ── Resolve provider + key + model for a call ─────────────────────────────────

pub fn resolve_provider_model(
    root: &str,
    provider_id: Option<&str>,
    model_id: Option<&str>,
) -> Result<(AiProvider, String, String), String> {
    let settings = read_ai_settings(root);

    // Resolve provider + model as a *pair*. An explicit selection is honored only
    // when BOTH the provider and the model are present — a partial selection (e.g.
    // a per-task provider with no model) must not be mixed with the default
    // provider's model, which would send an unknown model id to that provider. In
    // every incomplete case fall back to the default provider+model as a unit.
    let (pid, mid) = match (provider_id, model_id) {
        (Some(p), Some(m)) if !p.is_empty() && !m.is_empty() => (p.to_string(), m.to_string()),
        _ => {
            let p = settings
                .default_provider_id
                .clone()
                .filter(|s| !s.is_empty())
                .ok_or("尚未配置 AI 服务商，请在「设置 → AI 服务」中添加。")?;
            let m = settings
                .default_model_id
                .clone()
                .filter(|s| !s.is_empty())
                .ok_or("尚未设置默认模型，请在「设置 → AI 服务」中把某个模型设为默认。")?;
            (p, m)
        }
    };

    let provider = settings
        .providers
        .iter()
        .find(|p| p.id == pid && p.enabled)
        .cloned()
        .ok_or_else(|| {
            provider_unavailable_reason(&settings, &pid)
                + "请在「设置 → AI 服务」中重新选择或重新启用后再试。"
        })?;

    // Ollama runs locally and is normally keyless, so an empty key is valid.
    let key = get_api_key(root, &pid)
        .or_else(|| (provider.kind == "ollama").then(String::new))
        .ok_or_else(|| {
            format!(
                "尚未为「{}」配置 API Key，请在「设置 → AI 服务」中填写。",
                provider.name
            )
        })?;

    Ok((provider, key, mid))
}

/// A short, human-facing reason a provider id can't be resolved right now. Prefers
/// the provider's display *name* over the opaque UUID, and distinguishes "disabled"
/// (a toggle the user can flip back) from "gone" (the id no longer exists — e.g. the
/// provider was deleted, or removed and re-added under a fresh id, leaving a stale
/// per-task selection behind). Ends with a comma so it composes into a longer
/// sentence.
fn provider_unavailable_reason(settings: &AiSettings, pid: &str) -> String {
    match settings.providers.iter().find(|p| p.id == pid) {
        Some(p) => format!("所选 AI 服务商「{}」已被停用，", p.name),
        None => "所选 AI 服务商已不存在（可能已被删除或重新添加），".to_string(),
    }
}

/// Resolve a provider/model like [`resolve_provider_model`], but when an
/// *explicitly requested* provider is missing or disabled, transparently fall back
/// to the configured default provider+model instead of failing. The fourth tuple
/// element is a human-facing notice describing the fallback (`None` when the request
/// resolved normally), so the caller can tell the user which model actually ran and
/// why — a stale per-task provider id is routine config drift, not a software fault,
/// and should keep the feature working rather than surface a scary error.
///
/// Flows that need a capability the default can't satisfy — embeddings above all,
/// where the default *chat* model is simply not a valid embedding model — must keep
/// calling [`resolve_provider_model`] directly.
pub fn resolve_provider_model_or_default(
    root: &str,
    provider_id: Option<&str>,
    model_id: Option<&str>,
) -> Result<(AiProvider, String, String, Option<String>), String> {
    match resolve_provider_model(root, provider_id, model_id) {
        Ok((provider, key, model)) => Ok((provider, key, model, None)),
        Err(primary_err) => {
            let requested_pid = provider_id.filter(|s| !s.is_empty());
            let has_explicit =
                requested_pid.is_some() && model_id.map(|s| !s.is_empty()).unwrap_or(false);
            let settings = read_ai_settings(root);

            // Only fall back when the requested provider is unusable *as a
            // selection* — missing or disabled. A provider that exists and is
            // enabled but fails for another reason (e.g. no API key configured) is
            // a fixable mistake the user should see, not silently paper over with
            // the default. Checking the provider list directly (rather than
            // sniffing the error text) also keeps `provider_unavailable_reason`'s
            // disabled-vs-deleted wording accurate.
            let stale = has_explicit
                && requested_pid
                    .map(|pid| !settings.providers.iter().any(|p| p.id == pid && p.enabled))
                    .unwrap_or(false);
            let default_pid = settings
                .default_provider_id
                .as_deref()
                .filter(|s| !s.is_empty());
            if !stale || requested_pid == default_pid {
                return Err(primary_err);
            }

            // Retry with the default provider+model as a unit. If even the default
            // is unusable, the requested-provider error names what the user actually
            // picked and is the more useful one to show.
            let (provider, key, model) =
                resolve_provider_model(root, None, None).map_err(|_| primary_err)?;
            let notice = format!(
                "{}已自动改用默认模型「{} · {}」。",
                provider_unavailable_reason(&settings, requested_pid.unwrap_or_default()),
                provider.name,
                model
            );
            Ok((provider, key, model, Some(notice)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AiProvider, AiSettings};

    fn tmp_root(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("argus-aimgr-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    /// Keyless (Ollama-kind) providers so the tests exercise the *selection*
    /// resolution without needing the encrypted key store.
    fn provider(id: &str, name: &str, enabled: bool) -> AiProvider {
        AiProvider {
            id: id.into(),
            name: name.into(),
            kind: "ollama".into(),
            base_url: "http://localhost".into(),
            enabled,
            models: vec![],
            server_tools: Default::default(),
            created_at: String::new(),
        }
    }

    fn write(root: &str, providers: Vec<AiProvider>, default: &str) {
        write_ai_settings(
            root,
            &AiSettings {
                providers,
                default_provider_id: Some(default.into()),
                default_model_id: Some("m-default".into()),
            },
        )
        .unwrap();
    }

    #[test]
    fn valid_explicit_selection_resolves_without_a_notice() {
        let root = tmp_root("valid");
        write(&root, vec![provider("p1", "主力", true)], "p1");

        let (prov, _key, model, notice) =
            resolve_provider_model_or_default(&root, Some("p1"), Some("m-x")).unwrap();
        assert_eq!(prov.id, "p1");
        assert_eq!(model, "m-x");
        assert!(notice.is_none(), "a working selection must not surface a notice");
    }

    #[test]
    fn a_stale_provider_falls_back_to_the_default_model() {
        let root = tmp_root("stale");
        write(&root, vec![provider("default", "默认", true)], "default");

        // "ghost" was configured for the task but no longer exists.
        let (prov, _key, model, notice) =
            resolve_provider_model_or_default(&root, Some("ghost"), Some("m-ghost")).unwrap();
        assert_eq!(prov.id, "default", "should fall back to the default provider");
        assert_eq!(model, "m-default", "should use the default model, not the stale one");
        let notice = notice.expect("a fallback notice should be surfaced");
        assert!(notice.contains("默认"), "notice names the model it used: {notice}");
    }

    #[test]
    fn a_disabled_provider_falls_back_and_says_so() {
        let root = tmp_root("disabled");
        write(
            &root,
            vec![provider("picked", "被停用的", false), provider("default", "默认", true)],
            "default",
        );

        let (prov, _key, model, notice) =
            resolve_provider_model_or_default(&root, Some("picked"), Some("m-picked")).unwrap();
        assert_eq!(prov.id, "default");
        assert_eq!(model, "m-default");
        assert!(notice.unwrap().contains("停用"), "disabled must be named as such");
    }

    #[test]
    fn a_stale_default_is_not_papered_over() {
        // When the default itself is the stale selection there is nothing to fall
        // back to — surface the friendly error instead of looping.
        let root = tmp_root("staledefault");
        write(&root, vec![provider("other", "其它", true)], "gone");

        let err = resolve_provider_model_or_default(&root, Some("gone"), Some("m")).unwrap_err();
        assert!(err.contains("不存在") || err.contains("停用"), "friendly error: {err}");
        assert!(!err.contains("ghost") && !err.contains("gone"), "no raw UUID leaks: {err}");
    }
}
