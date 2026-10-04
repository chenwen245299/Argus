//! Media generation that is not a conversation: text-to-image, image editing,
//! speech synthesis, transcription, sound design and music.
//!
//! These answer on their own routes rather than `/chat/completions`, and no two
//! providers spell them the same way — StepFun posts JSON to `/v1/images/…` and
//! multipart to `/v1/audio/transcriptions`, submits music as an async job and
//! polls it, and returns TTS as raw bytes with no envelope at all. So this module
//! is a *contract*, not an implementation: it says what a media task looks like
//! going in and coming out, and each provider's adapter does the talking.
//!
//! ## The form is data, not code
//!
//! Every provider exposes a different set of knobs — sizes, seeds, voices, CFG
//! scales, task types — and hard-coding one provider's set into the UI would mean
//! rewriting the UI for the next one. Instead an adapter *describes* its models
//! with [`MediaField`]s and the frontend renders the form from that description.
//! Adding a provider is then a new adapter plus two lines in [`capabilities`] and
//! [`run`]; the UI does not change at all.
//!
//! Values come back in [`MediaRequest::options`], keyed by the same
//! [`MediaField::key`] the adapter declared. The map is deliberately untyped: the
//! adapter that named a field is the only thing that has to understand it.
//!
//! ## What comes out
//!
//! A [`MediaArtifact`] is either bytes (as a data URI, which an `<img>` or
//! `<audio>` can show directly and `write_bytes_to_file` can save), a remote URL
//! the provider hosts, or plain text — transcription is a media task whose output
//! is words. One task can produce several.
//!
//! ## How to add a provider
//!
//! Adapters today: [`crate::stepfun_media`] (every kind) and
//! [`crate::minimax_media`] (speech). The checklist, in order:
//!
//!  1. **Adapter file** `<provider>_media.rs`, declared in `lib.rs` next to its
//!     chat module. It exposes `capabilities() -> Vec<MediaCapability>` and
//!     `async fn run(provider, api_key, req) -> Result<MediaResult, String>`.
//!  2. **Describe, don't hard-code.** Each model is a [`MediaModelSpec`] and each
//!     knob a [`MediaField`] (`select` / `number` / `Toggle` / `Text`) whose `key`
//!     is the name `run` reads back with [`opt_str`] / [`opt_f64`] / [`opt_bool`].
//!     A field's `default` must be what `run` does when the key is absent — the
//!     settings form seeds from it, but a request may also arrive with no options
//!     at all (read-aloud before the form was ever touched). A select's default
//!     must be one of its options. Offer a knob only for the models that accept it.
//!  3. **Declare [`MediaModelSpec::max_prompt_chars`]** — the longest main text one
//!     request accepts, in characters. The read-aloud player splits what it reads
//!     by it, so every speech model must declare it (a test enforces that).
//!     Enforce the same number in `run` with a message that says the count, so an
//!     over-long request fails before it is billed.
//!  4. **Wire the dispatch**: one `is_<provider>` arm in [`capabilities`] and one
//!     in [`run`] below. Detection is by `kind` or base URL, like the chat side.
//!  5. **Map errors through [`crate::llm::friendly_error`]** for HTTP failures. A
//!     provider that reports failures inside a 200 body (MiniMax's `base_resp`)
//!     must turn them into a message that still ends in the vendor code in
//!     parentheses, which `llm::classify_error` reads. Never put the API key in
//!     an error, a log line or an artifact.
//!  6. **Tests**: the request body for several option combinations, the response
//!     parser on a canned body, and one row in the `adapters()` helper of this
//!     file's test module, so the shared form rules (`adapters_declare_a_consistent_form`
//!     and friends) cover the new adapter too.
//!
//! The frontend then needs nothing: the studio form is generated from the
//! description, and 设置 → AI 随航 → 朗读 lists every speech model of every
//! provider that has an adapter and an API key, rendering its knobs the same way.
//! Synthesis goes through `run_media_task` with `kind: "speech"`.

use serde::{Deserialize, Serialize};

use crate::models::AiProvider;

/// A kind of media task, named for what it does rather than for any provider's
/// endpoint. The frontend groups the studio by these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    /// Text in, picture out.
    ImageGenerate,
    /// Picture (+ text) in, picture out.
    ImageEdit,
    /// Text in, speech out.
    Speech,
    /// Speech in, text out.
    Transcribe,
    /// Text in, composite audio out — voices, effects, ambience.
    AudioGenerate,
    /// Text or lyrics in, music out.
    Music,
}

/// How one option is entered.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    LongText,
    Select,
    Number,
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    /// What the option belongs to, for a long list the UI may split in two —
    /// a voice's language, so 朗读 settings can ask for the language first and
    /// then list only that language's voices. `None` for ungrouped options.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub group: Option<String>,
}

impl FieldOption {
    pub fn new(value: &str, label: &str) -> Self {
        Self {
            value: value.to_string(),
            label: label.to_string(),
            group: None,
        }
    }
    pub fn grouped(value: &str, label: &str, group: &str) -> Self {
        Self {
            group: Some(group.to_string()),
            ..Self::new(value, label)
        }
    }
    /// An option whose id is its own label — sizes, formats, sample rates.
    pub fn bare(value: &str) -> Self {
        Self::new(value, value)
    }
}

/// One control in the generated form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaField {
    /// The key this field's value arrives under in `MediaRequest::options`.
    pub key: String,
    pub label: String,
    pub kind: FieldKind,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub options: Vec<FieldOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    /// One line under the control, for a limit or a caveat worth knowing before
    /// the request is billed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl MediaField {
    pub fn new(key: &str, label: &str, kind: FieldKind) -> Self {
        Self {
            key: key.to_string(),
            label: label.to_string(),
            kind,
            options: Vec::new(),
            default: None,
            min: None,
            max: None,
            step: None,
            note: None,
        }
    }
    pub fn select(key: &str, label: &str, options: Vec<FieldOption>) -> Self {
        let mut f = Self::new(key, label, FieldKind::Select);
        f.options = options;
        f
    }
    pub fn number(key: &str, label: &str, min: f64, max: f64, step: f64) -> Self {
        let mut f = Self::new(key, label, FieldKind::Number);
        f.min = Some(min);
        f.max = Some(max);
        f.step = Some(step);
        f
    }
    pub fn with_default(mut self, v: serde_json::Value) -> Self {
        self.default = Some(v);
        self
    }
    pub fn with_note(mut self, note: &str) -> Self {
        self.note = Some(note.to_string());
        self
    }
}

/// One model a provider offers for one kind of task, with the knobs it takes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaModelSpec {
    pub id: String,
    pub display_name: String,
    /// Price, a retirement date, a limit — whatever the user should know before
    /// picking this model rather than after being billed by it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub fields: Vec<MediaField>,
    /// Accepted input media types, for the kinds that *can* take a file. Empty
    /// means the task never takes one.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub accepts: Vec<String>,
    /// Whether the file is mandatory.
    ///
    /// Separate from `accepts` because "can take a file" and "cannot run without
    /// one" are different questions: StepFun's music model reads a reference
    /// track for a cover or a backing track, but composes from a description
    /// alone — so gating the run button on `accepts` would make its main task
    /// unreachable. The adapter still enforces the real rule, which can depend on
    /// another field's value; this only decides whether the button greys out.
    #[serde(default)]
    pub file_required: bool,
    /// Whether the main text box is required (a prompt) or optional (an
    /// instruction beside an uploaded file).
    pub prompt_required: bool,
    /// Placeholder for the main text box.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_placeholder: Option<String>,
    /// The longest main text (`MediaRequest::prompt`) one request accepts,
    /// counted in characters rather than bytes. `None` means the adapter does not
    /// declare one.
    ///
    /// The read-aloud player cuts what it reads into chunks by this number, so a
    /// speech model needs it. It is the provider's *hard* limit, which is not
    /// always the comfortable one — MiniMax accepts nearly 10 000 characters but
    /// answers a request that long slowly — so the player is free to choose
    /// smaller chunks, never larger.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_prompt_chars: Option<u32>,
}

/// What one provider can do, for one kind of task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaCapability {
    pub kind: MediaKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub models: Vec<MediaModelSpec>,
}

/// A file handed to the task — an image to edit, audio to transcribe.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInput {
    pub name: String,
    /// A `data:<mime>;base64,…` URI. The frontend reads the file, so nothing here
    /// touches the filesystem.
    pub data_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRequest {
    pub provider_id: String,
    pub kind: MediaKind,
    pub model: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub inputs: Vec<MediaInput>,
    /// Field values, keyed by `MediaField::key`. Only the adapter that declared
    /// a field reads it back.
    #[serde(default)]
    pub options: serde_json::Map<String, serde_json::Value>,
}

/// One thing the task produced.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaArtifact {
    pub mime: String,
    /// Bytes, as a data URI an `<img>`/`<audio>` can show and
    /// `write_bytes_to_file` can save.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_url: Option<String>,
    /// A URL the provider hosts instead of returning bytes. These expire —
    /// StepFun's image URLs last 30 days, its TTS URLs 12 hours — so anything
    /// worth keeping has to be downloaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Text output: a transcription, or a caption beside a clip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Suggested filename for a save dialog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaResult {
    pub kind: MediaKind,
    pub provider_id: String,
    pub model: String,
    pub artifacts: Vec<MediaArtifact>,
}

// ── Dispatch ─────────────────────────────────────────────────────────────────

/// What this provider can do off the chat path. Empty for a provider with no
/// adapter, which is how the studio knows not to list it.
pub fn capabilities(provider: &AiProvider) -> Vec<MediaCapability> {
    if crate::stepfun::is_stepfun(provider) {
        return crate::stepfun_media::capabilities();
    }
    if crate::minimax::is_minimax(provider) {
        return crate::minimax_media::capabilities();
    }
    Vec::new()
}

pub async fn run(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<MediaResult, String> {
    if crate::stepfun::is_stepfun(provider) {
        return crate::stepfun_media::run(provider, api_key, req).await;
    }
    if crate::minimax::is_minimax(provider) {
        return crate::minimax_media::run(provider, api_key, req).await;
    }
    Err(format!("{} 暂不支持这类媒体生成。", provider.name))
}

/// Ceiling on an artifact downloaded from a provider's own host.
///
/// Generous enough for a full song at 48 kHz and far below anything that would
/// be a problem to hold in memory; it exists so a redirect to something huge
/// cannot be streamed into the process unbounded.
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

/// Fetch an artifact the provider hosts rather than returned inline.
///
/// Done here rather than with the webview's `fetch` for two reasons: a request
/// from `tauri://localhost` to a provider's CDN is subject to CORS and would
/// usually be blocked, and these URLs expire (StepFun's images in 30 days, its
/// TTS links in 12 hours), so downloading them is the whole point of the save
/// button. Goes through the same SSRF guard as every other backend fetch.
pub async fn fetch_artifact(url: &str) -> Result<Vec<u8>, String> {
    crate::net::validate_public_http_url(url)?;
    let resp = crate::llm::build_client()?
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载失败：{e}"))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        return Err(format!("下载失败，接口返回 {status}。链接可能已过期。"));
    }
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_ARTIFACT_BYTES {
            return Err("文件过大，已放弃下载。".to_string());
        }
    }
    let bytes = resp.bytes().await.map_err(|e| format!("下载失败：{e}"))?;
    if bytes.len() > MAX_ARTIFACT_BYTES {
        return Err("文件过大，已放弃下载。".to_string());
    }
    Ok(bytes.to_vec())
}

// ── Helpers shared by adapters ───────────────────────────────────────────────

/// Read one option as a string, treating an empty one as absent so a cleared
/// text box falls through to the provider's own default rather than sending `""`.
pub fn opt_str<'a>(req: &'a MediaRequest, key: &str) -> Option<&'a str> {
    req.options
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn opt_f64(req: &MediaRequest, key: &str) -> Option<f64> {
    req.options.get(key).and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_f64(),
        // A number input hands back a string when the user types into it.
        serde_json::Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

pub fn opt_bool(req: &MediaRequest, key: &str) -> Option<bool> {
    req.options.get(key).and_then(|v| match v {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// Split a `data:<mime>;base64,<payload>` URI. Adapters need both halves: the
/// media type for a multipart part's filename, the payload for the bytes.
pub fn split_data_url(uri: &str) -> Result<(String, Vec<u8>), String> {
    use base64::Engine;
    let rest = uri
        .strip_prefix("data:")
        .ok_or("附件不是 data URI，无法上传。")?;
    let (meta, payload) = rest.split_once(',').ok_or("附件的 data URI 缺少逗号分隔。")?;
    let mime = meta
        .split(';')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("application/octet-stream")
        .to_string();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|e| format!("附件的 base64 解码失败：{e}"))?;
    Ok((mime, bytes))
}

/// Wrap raw bytes as a data URI, which is how every artifact carrying bytes is
/// returned.
pub fn to_data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// A conventional extension for a media type, for the save dialog's default name.
pub fn ext_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mpeg" => "mp3",
        "audio/flac" => "flac",
        "audio/opus" | "audio/ogg" => "opus",
        "text/plain" => "txt",
        _ => "bin",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(options: serde_json::Value) -> MediaRequest {
        MediaRequest {
            provider_id: "p".into(),
            kind: MediaKind::Speech,
            model: "m".into(),
            prompt: String::new(),
            inputs: vec![],
            options: options.as_object().cloned().unwrap_or_default(),
        }
    }

    #[test]
    fn a_cleared_text_box_reads_as_absent() {
        let r = req(serde_json::json!({"voice": "  ", "instruction": "温柔一点"}));
        assert_eq!(opt_str(&r, "voice"), None);
        assert_eq!(opt_str(&r, "instruction"), Some("温柔一点"));
        assert_eq!(opt_str(&r, "missing"), None);
    }

    /// An HTML number input hands back a string, and a checkbox bound through a
    /// generic form can too. Reading only the JSON type would drop both.
    #[test]
    fn numbers_and_toggles_survive_arriving_as_strings() {
        let r = req(serde_json::json!({"speed": "1.5", "seed": 42, "hd": "true", "off": false}));
        assert_eq!(opt_f64(&r, "speed"), Some(1.5));
        assert_eq!(opt_f64(&r, "seed"), Some(42.0));
        assert_eq!(opt_bool(&r, "hd"), Some(true));
        assert_eq!(opt_bool(&r, "off"), Some(false));
        assert_eq!(opt_f64(&r, "hd"), None);
    }

    #[test]
    fn data_urls_round_trip() {
        let url = to_data_url("audio/wav", b"RIFF....");
        assert!(url.starts_with("data:audio/wav;base64,"));
        let (mime, bytes) = split_data_url(&url).unwrap();
        assert_eq!(mime, "audio/wav");
        assert_eq!(bytes, b"RIFF....");

        // A plain URL is not something an adapter can upload.
        assert!(split_data_url("https://a.test/x.png").is_err());
    }

    #[test]
    fn extensions_are_known_for_what_the_adapters_produce() {
        assert_eq!(ext_for("image/png"), "png");
        assert_eq!(ext_for("audio/mpeg"), "mp3");
        assert_eq!(ext_for("audio/wav"), "wav");
        assert_eq!(ext_for("application/x-unheard-of"), "bin");
    }

    // ── Dispatch and the rules every adapter's form must follow ──────────────

    fn provider(name: &str, kind: &str, base_url: &str) -> AiProvider {
        serde_json::from_value(serde_json::json!({
            "id": format!("id-{kind}"), "name": name, "kind": kind,
            "base_url": base_url, "created_at": ""
        }))
        .unwrap()
    }

    /// One row per adapter. **Add the new provider here** (step 6 of "How to add
    /// a provider"): every test below then holds it to the same form rules.
    fn adapters() -> Vec<AiProvider> {
        vec![
            provider("StepFun", "stepfun", "https://api.stepfun.com/v1"),
            provider("MiniMax", "minimax", "https://api.minimax.cn/v1"),
        ]
    }

    fn speech_models(p: &AiProvider) -> Vec<MediaModelSpec> {
        capabilities(p)
            .into_iter()
            .filter(|c| c.kind == MediaKind::Speech)
            .flat_map(|c| c.models)
            .collect()
    }

    #[test]
    fn the_right_adapter_answers_for_each_provider() {
        let step = capabilities(&provider("StepFun", "stepfun", "https://api.stepfun.com/v1"));
        let mini = capabilities(&provider("MiniMax", "minimax", "https://api.minimax.cn/v1"));
        // StepFun does all six kinds; MiniMax is speech only.
        assert_eq!(step.len(), 6);
        assert_eq!(mini.len(), 1);
        assert_eq!(mini[0].kind, MediaKind::Speech);
        let ids = |caps: &[MediaCapability]| -> Vec<String> {
            caps.iter()
                .filter(|c| c.kind == MediaKind::Speech)
                .flat_map(|c| c.models.iter().map(|m| m.id.clone()))
                .collect()
        };
        assert!(ids(&step).iter().all(|id| !id.starts_with("speech-")));
        assert!(ids(&mini).iter().all(|id| id.starts_with("speech-")));

        // Detected by base URL as well as by kind, like the chat side: a custom
        // provider that points at MiniMax is MiniMax.
        let by_url = capabilities(&provider("我的", "openai_compatible", "https://api.minimaxi.com/v1"));
        assert_eq!(by_url.len(), 1);
        let by_url = capabilities(&provider("我的", "openai_compatible", "https://api.stepfun.com/v1"));
        assert_eq!(by_url.len(), 6);
    }

    #[test]
    fn a_provider_with_no_adapter_offers_nothing() {
        for p in [
            provider("DeepSeek", "openai_compatible", "https://api.deepseek.com/v1"),
            provider("Claude", "anthropic", "https://api.anthropic.com"),
            provider("Ollama", "openai_compatible", "http://localhost:11434/v1"),
        ] {
            assert!(capabilities(&p).is_empty(), "{}", p.name);
        }
    }

    /// `run` must reach the same adapter `capabilities` did. Without a network:
    /// an over-long text is refused by the adapter itself, and each adapter names
    /// its own limit in the refusal.
    #[tokio::test]
    async fn run_reaches_the_adapter_that_listed_the_model() {
        let long = "x".repeat(20_000);
        let ask = |p: &AiProvider, model: &str| {
            let r = MediaRequest {
                provider_id: p.id.clone(),
                kind: MediaKind::Speech,
                model: model.into(),
                prompt: long.clone(),
                inputs: vec![],
                options: Default::default(),
            };
            let p = p.clone();
            async move { run(&p, "k", &r).await }
        };

        let step = provider("StepFun", "stepfun", "https://api.stepfun.com/v1");
        let err = ask(&step, "step-tts-mini").await.unwrap_err();
        assert!(err.contains("1000"), "{err}");

        let mini = provider("MiniMax", "minimax", "https://api.minimax.cn/v1");
        let err = ask(&mini, "speech-2.8-turbo").await.unwrap_err();
        assert!(err.contains("9999"), "{err}");

        // No adapter: a clear refusal that names the provider.
        let other = provider("DeepSeek", "openai_compatible", "https://api.deepseek.com/v1");
        let err = ask(&other, "deepseek-chat").await.unwrap_err();
        assert!(err.contains("DeepSeek") && err.contains("暂不支持"), "{err}");
    }

    /// The studio and the read-aloud settings both render a form from this
    /// description with no knowledge of any provider, so a malformed field is not
    /// a compile error anywhere — it is a blank or broken control. These are the
    /// rules that keep it from happening.
    #[test]
    fn adapters_declare_a_consistent_form() {
        for p in adapters() {
            let caps = capabilities(&p);
            assert!(!caps.is_empty(), "{} lists nothing", p.name);
            for cap in &caps {
                assert!(!cap.label.is_empty());
                assert!(!cap.models.is_empty(), "{} {:?}: no models", p.name, cap.kind);
                for m in &cap.models {
                    let who = format!("{} {:?} {}", p.name, cap.kind, m.id);
                    assert!(!m.id.trim().is_empty() && !m.display_name.is_empty(), "{who}");

                    let mut keys = std::collections::BTreeSet::new();
                    for f in &m.fields {
                        assert!(!f.key.is_empty() && !f.label.is_empty(), "{who}: {f:?}");
                        assert!(keys.insert(f.key.clone()), "{who}: duplicate field {}", f.key);
                        match f.kind {
                            FieldKind::Select => {
                                assert!(!f.options.is_empty(), "{who}: select {} has no options", f.key);
                                let mut values = std::collections::BTreeSet::new();
                                for o in &f.options {
                                    assert!(!o.label.is_empty(), "{who}: {} option {:?}", f.key, o.value);
                                    assert!(values.insert(o.value.clone()), "{who}: {} repeats {:?}", f.key, o.value);
                                }
                                // A default that is not an option renders as a
                                // select with nothing selected.
                                if let Some(d) = &f.default {
                                    let d = d.as_str().unwrap_or_default();
                                    assert!(values.contains(d), "{who}: {} default {d:?} not in options", f.key);
                                }
                            }
                            FieldKind::Number => {
                                let (lo, hi) = (f.min.unwrap(), f.max.unwrap());
                                assert!(lo < hi, "{who}: {} range {lo}..{hi}", f.key);
                                assert!(f.step.unwrap() > 0.0, "{who}: {} step", f.key);
                                if let Some(d) = f.default.as_ref().and_then(|d| d.as_f64()) {
                                    assert!((lo..=hi).contains(&d), "{who}: {} default {d} outside {lo}..{hi}", f.key);
                                }
                            }
                            _ => assert!(f.options.is_empty(), "{who}: {} is not a select", f.key),
                        }
                    }
                }
            }
        }
    }

    /// The read-aloud player chunks by this number; a speech model without one
    /// would be sent whatever the player guessed.
    #[test]
    fn every_speech_model_declares_how_much_text_one_request_takes() {
        for p in adapters() {
            let models = speech_models(&p);
            assert!(!models.is_empty(), "{} has no speech model", p.name);
            for m in &models {
                let n = m.max_prompt_chars.unwrap_or(0);
                assert!(n >= 100, "{} {}: max_prompt_chars {:?}", p.name, m.id, m.max_prompt_chars);
                assert!(m.prompt_required, "{} {}: speech without text", p.name, m.id);
                assert!(m.accepts.is_empty() && !m.file_required);
            }
        }
        // Where a limit was already stated in a placeholder, the two agree.
        let step = speech_models(&provider("StepFun", "stepfun", "https://api.stepfun.com/v1"));
        assert!(step.iter().all(|m| m.max_prompt_chars == Some(1000)));
        let mini = speech_models(&provider("MiniMax", "minimax", "https://api.minimax.cn/v1"));
        assert!(mini.iter().all(|m| m.max_prompt_chars == Some(9999)));
    }

    /// The field is optional in both directions on the wire: absent when an
    /// adapter does not declare one, camelCase when it does.
    #[test]
    fn max_prompt_chars_travels_as_an_optional_camel_case_key() {
        let spec = |n: Option<u32>| MediaModelSpec {
            id: "m".into(),
            display_name: "M".into(),
            note: None,
            fields: vec![],
            accepts: vec![],
            file_required: false,
            prompt_required: true,
            prompt_placeholder: None,
            max_prompt_chars: n,
        };
        let with = serde_json::to_value(spec(Some(1000))).unwrap();
        assert_eq!(with["maxPromptChars"], 1000);
        let without = serde_json::to_value(spec(None)).unwrap();
        assert!(without.get("maxPromptChars").is_none());

        // And an older description without the key still parses.
        let old: MediaModelSpec = serde_json::from_value(serde_json::json!({
            "id": "m", "displayName": "M", "promptRequired": true
        }))
        .unwrap();
        assert_eq!(old.max_prompt_chars, None);
    }

    /// Nothing an adapter offers may be a format a player cannot open — the one
    /// rule about *values* that is the same for every provider.
    #[test]
    fn no_adapter_offers_headerless_pcm_as_an_output() {
        for p in adapters() {
            for m in speech_models(&p) {
                for f in &m.fields {
                    for o in &f.options {
                        let v = o.value.to_lowercase();
                        assert!(
                            !(f.key.contains("format") && (v == "pcm" || v.starts_with("pcmu"))),
                            "{} {}: {} offers {}",
                            p.name, m.id, f.key, o.value
                        );
                    }
                }
            }
        }
    }
}
