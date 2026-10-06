//! MiMo's built-in-voice speech synthesis, behind the shared media contract.
//!
//! Despite using `/chat/completions`, this is not a conversation: the text to
//! speak belongs in an **assistant** message. Audio is returned as base64 in
//! `choices[0].message.audio.data`. No chat sampling/thinking parameters belong
//! here. Non-streaming MP3/WAV clips fit the reader's chunk/prefetch/cache flow;
//! headerless PCM is never handed to an HTML audio element.
//!
//! The current preset-voice model is `mimo-v2.5-tts`. Voice design and cloning
//! require different inputs and are not advertised as preset-voice models.
//! Numeric synthesis speed is not documented, so no `speed` field is offered;
//! the reader's playback-rate control still works without another API request.
//!
//! Verified 2026-10 against:
//! <https://mimo.mi.com/docs/en-US/api/audio/tts>
//! <https://mimo.mi.com/docs/en-US/quick-start/usage-guide/audio/speech-synthesis-v2.5>

use base64::Engine;

use crate::media::{
    opt_str, to_data_url, FieldOption, MediaArtifact, MediaCapability, MediaField, MediaKind,
    MediaModelSpec, MediaRequest, MediaResult,
};
use crate::models::AiProvider;

const TTS_MODEL: &str = "mimo-v2.5-tts";
// An app-side ceiling for short, responsive clips, not a claimed vendor hard
// limit (the current API reference does not publish a character limit).
const TTS_MAX_CHARS: u32 = 1_000;
const DEFAULT_VOICE: &str = "Mia";
const DEFAULT_FORMAT: &str = "mp3";
const FORMATS: [&str; 2] = ["mp3", "wav"];
const MAX_AUDIO_BYTES: usize = 64 * 1024 * 1024;

// Language groups feed the existing language-first voice picker. Explicit IDs
// keep the voice stable across clusters; mimo_default varies with the region.
const VOICES: &[(&str, &str, &str)] = &[
    ("Mia", "英语", "Mia（女声）"),
    ("Chloe", "英语", "Chloe（女声）"),
    ("Milo", "英语", "Milo（男声）"),
    ("Dean", "英语", "Dean（男声）"),
    ("冰糖", "普通话", "冰糖（女声）"),
    ("茉莉", "普通话", "茉莉（女声）"),
    ("苏打", "普通话", "苏打（男声）"),
    ("白桦", "普通话", "白桦（男声）"),
    ("mimo_default", "自动", "MiMo 默认音色"),
];

pub fn capabilities() -> Vec<MediaCapability> {
    vec![MediaCapability {
        kind: MediaKind::Speech,
        label: "语音合成".into(),
        note: None,
        models: vec![MediaModelSpec {
            id: TTS_MODEL.into(),
            display_name: "MiMo V2.5 TTS".into(),
            note: None,
            fields: vec![
                MediaField::select(
                    "voice",
                    "音色",
                    VOICES
                        .iter()
                        .map(|(id, lang, label)| FieldOption::grouped(id, label, lang))
                        .collect(),
                )
                .with_default(serde_json::json!(DEFAULT_VOICE)),
                MediaField::select(
                    "format",
                    "输出格式",
                    FORMATS.iter().map(|f| FieldOption::bare(f)).collect(),
                )
                .with_default(serde_json::json!(DEFAULT_FORMAT)),
            ],
            accepts: vec![],
            file_required: false,
            prompt_required: true,
            prompt_placeholder: Some("输入要朗读的原文，每段最多 1000 字符".into()),
            max_prompt_chars: Some(TTS_MAX_CHARS),
        }],
    }]
}

fn speech_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        return base.into();
    }
    // Also accept the Anthropic address used by the same provider's chat setup.
    let base = base
        .strip_suffix("/anthropic/v1/messages")
        .or_else(|| base.strip_suffix("/anthropic/v1"))
        .or_else(|| base.strip_suffix("/anthropic"))
        .or_else(|| base.strip_suffix("/messages"))
        .unwrap_or(base);
    let after_scheme = base.split_once("://").map_or(base, |(_, rest)| rest);
    if after_scheme.contains('/') {
        format!("{base}/chat/completions")
    } else {
        format!("{base}/v1/chat/completions")
    }
}

fn voice_for(req: &MediaRequest) -> &str {
    opt_str(req, "voice")
        .filter(|v| VOICES.iter().any(|(id, _, _)| id == v))
        .unwrap_or(DEFAULT_VOICE)
}

fn format_for(req: &MediaRequest) -> &str {
    opt_str(req, "format")
        .filter(|f| FORMATS.contains(f))
        .unwrap_or(DEFAULT_FORMAT)
}

fn validate_text(text: &str) -> Result<&str, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("请先输入要朗读的文字。".into());
    }
    let count = text.chars().count();
    if count > TTS_MAX_CHARS as usize {
        return Err(format!(
            "MiMo 朗读每段最多 {TTS_MAX_CHARS} 字符，当前为 {count}，请分段后重试。"
        ));
    }
    Ok(text)
}

fn speech_body(req: &MediaRequest, text: &str) -> serde_json::Value {
    serde_json::json!({
        "model": req.model,
        "messages": [{ "role": "assistant", "content": text }],
        "audio": { "format": format_for(req), "voice": voice_for(req) },
        "stream": false,
    })
}

fn artifact_from_response(
    value: &serde_json::Value,
    format: &str,
) -> Result<MediaArtifact, String> {
    if let Some(error) = value.get("error").filter(|e| !e.is_null()) {
        let status = error
            .get("status")
            .or_else(|| error.get("code"))
            .and_then(|v| v.as_u64())
            .filter(|s| (400..=599).contains(s))
            .unwrap_or(400) as u16;
        return Err(crate::llm::friendly_error(status, &value.to_string()));
    }
    let choice = value
        .get("choices")
        .and_then(|c| c.get(0))
        .ok_or("MiMo 语音合成没有返回音频。")?;
    match choice.get("finish_reason").and_then(|v| v.as_str()) {
        Some("content_filter") => {
            return Err("语音内容被安全审核拦截（403），请换一段文字或模型。".into())
        }
        Some("length") => return Err("MiMo 语音生成被截断，请缩短每段文字后重试。".into()),
        _ => {}
    }
    let data = choice
        .pointer("/message/audio/data")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .ok_or("MiMo 语音合成没有返回音频。")?;
    if data.len() > MAX_AUDIO_BYTES.div_ceil(3) * 4 {
        return Err("MiMo 返回的音频过大，已放弃加载。".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "MiMo 返回的音频 base64 数据不完整或无法解码。")?;
    let (mime, ext, valid) = match format {
        "wav" => (
            "audio/wav",
            "wav",
            bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE"),
        ),
        _ => (
            "audio/mpeg",
            "mp3",
            bytes.starts_with(b"ID3")
                || (bytes.first() == Some(&0xff) && bytes.get(1).is_some_and(|b| b & 0xe0 == 0xe0)),
        ),
    };
    if !valid {
        return Err("MiMo 返回的音频不是请求的 MP3/WAV 格式，无法播放。".into());
    }
    Ok(MediaArtifact {
        mime: mime.into(),
        data_url: Some(to_data_url(mime, &bytes)),
        url: None,
        text: None,
        filename: Some(format!("{TTS_MODEL}.{ext}")),
    })
}

pub async fn run(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<MediaResult, String> {
    if req.kind != MediaKind::Speech {
        return Err("MiMo 目前只接入了语音合成。".into());
    }
    if req.model != TTS_MODEL {
        return Err(format!(
            "「{}」不是已接入的 MiMo 朗读模型，请选择 MiMo V2.5 TTS。",
            req.model
        ));
    }
    if !req.inputs.is_empty() {
        return Err("MiMo 内置音色朗读只接受文字，请移除附件。".into());
    }
    let text = validate_text(&req.prompt)?;
    let request = crate::llm::build_client()?
        .post(speech_url(&provider.base_url))
        .header("Content-Type", "application/json");
    // The same auth as MiMo chat: api-key for Xiaomi, Bearer for compatible relays.
    let resp = crate::llm::openai_auth(request, provider, api_key)
        .json(&speech_body(req, text))
        .send()
        .await
        .map_err(|e| crate::llm::describe_reqwest_error(&e))?;
    let status = resp.status().as_u16();
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败：{}", crate::llm::describe_reqwest_error(&e)))?;
    let redact = |error: String| {
        if api_key.is_empty() {
            error
        } else {
            error.replace(api_key, "[redacted]")
        }
    };
    if status >= 400 {
        return Err(redact(crate::llm::friendly_error(status, &raw)));
    }
    // JSON/base64 can be large even for a short WAV clip. Decode off the async
    // runtime, returning only the artifact to the command's task.
    let format = format_for(req).to_owned();
    let artifact = tokio::task::spawn_blocking(move || {
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| format!("MiMo 语音响应无法解析：{e}"))?;
        artifact_from_response(&value, &format)
    })
    .await
    .map_err(|e| format!("MiMo 音频处理失败：{e}"))?
    .map_err(redact)?;
    Ok(MediaResult {
        kind: req.kind,
        provider_id: provider.id.clone(),
        model: req.model.clone(),
        artifacts: vec![artifact],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(options: serde_json::Value) -> MediaRequest {
        MediaRequest {
            provider_id: "mimo".into(),
            kind: MediaKind::Speech,
            model: TTS_MODEL.into(),
            prompt: "Hello. 你好。".into(),
            inputs: vec![],
            options: options.as_object().unwrap().clone(),
        }
    }
    fn provider(base: &str) -> AiProvider {
        serde_json::from_value(serde_json::json!({ "id": "mimo", "name": "MiMo", "kind": "mimo", "base_url": base, "created_at": "" })).unwrap()
    }
    fn reply(bytes: &[u8]) -> serde_json::Value {
        serde_json::json!({ "choices": [{ "finish_reason": "stop", "message": { "audio": { "data": base64::engine::general_purpose::STANDARD.encode(bytes) } } }] })
    }

    #[test]
    fn preset_voices_and_formats_are_playable_and_match_the_form_defaults() {
        let model = &capabilities()[0].models[0];
        assert_eq!(model.id, TTS_MODEL);
        assert_eq!(model.fields.len(), 2);
        assert_eq!(
            model.fields[0].default,
            Some(serde_json::json!(DEFAULT_VOICE))
        );
        assert_eq!(
            model.fields[1].default,
            Some(serde_json::json!(DEFAULT_FORMAT))
        );
        let voices = &model.fields[0].options;
        assert_eq!(voices.len(), 9);
        assert_eq!(
            voices
                .iter()
                .filter(|v| v.group.as_deref() == Some("英语"))
                .count(),
            4
        );
        assert_eq!(
            voices
                .iter()
                .filter(|v| v.group.as_deref() == Some("普通话"))
                .count(),
            4
        );
        assert!(voices.iter().any(|v| v.value == "mimo_default"));
        assert_eq!(model.max_prompt_chars, Some(TTS_MAX_CHARS));
    }

    #[test]
    fn the_exact_text_is_an_assistant_message_with_no_chat_or_rewrite_parameters() {
        let r = req(serde_json::json!({}));
        assert_eq!(
            speech_body(&r, &r.prompt),
            serde_json::json!({
                "model": TTS_MODEL, "messages": [{"role": "assistant", "content": r.prompt}],
                "audio": {"format": "mp3", "voice": "Mia"}, "stream": false,
            })
        );
        for voice in VOICES {
            let r = req(
                serde_json::json!({"voice": voice.0, "format": "wav", "speed": 1.5, "optimize_text_preview": true}),
            );
            let body = speech_body(&r, &r.prompt);
            assert_eq!(
                body["audio"],
                serde_json::json!({"voice": voice.0, "format": "wav"})
            );
            assert!(body.get("temperature").is_none() && body.get("thinking").is_none());
        }
    }

    #[test]
    fn stale_voice_ids_and_headerless_formats_fall_back_to_the_defaults() {
        for options in [
            serde_json::json!({"voice": "English_Graceful_Lady", "format": "pcm16"}),
            serde_json::json!({"voice": "", "format": ""}),
            serde_json::json!({"voice": null, "format": 42}),
        ] {
            let r = req(options);
            assert_eq!(voice_for(&r), DEFAULT_VOICE);
            assert_eq!(format_for(&r), DEFAULT_FORMAT);
        }
    }

    #[test]
    fn base_urls_keep_the_configured_host_and_relay_prefix() {
        for (base, expected) in [
            (
                "https://api.xiaomimimo.com/v1/",
                "https://api.xiaomimimo.com/v1/chat/completions",
            ),
            (
                " https://api.xiaomimimo.com ",
                "https://api.xiaomimimo.com/v1/chat/completions",
            ),
            (
                "https://token-plan-cn.xiaomimimo.com/v1",
                "https://token-plan-cn.xiaomimimo.com/v1/chat/completions",
            ),
            (
                "https://api.xiaomimimo.com/anthropic",
                "https://api.xiaomimimo.com/v1/chat/completions",
            ),
            (
                "https://api.xiaomimimo.com/anthropic/v1/messages",
                "https://api.xiaomimimo.com/v1/chat/completions",
            ),
            (
                "https://relay.test/proxy/v1",
                "https://relay.test/proxy/v1/chat/completions",
            ),
            (
                "https://relay.test/proxy/v1/chat/completions",
                "https://relay.test/proxy/v1/chat/completions",
            ),
        ] {
            assert_eq!(speech_url(base), expected);
        }
    }

    #[test]
    fn text_limit_counts_code_points_and_refuses_empty_text() {
        assert!(validate_text(" \n\t").is_err());
        let text = "字".repeat(TTS_MAX_CHARS as usize);
        assert!(validate_text(&text).is_ok());
        assert!(validate_text(&(text + "😀")).unwrap_err().contains("1001"));
    }

    #[test]
    fn mp3_and_wav_become_data_urls_with_matching_filenames_and_media_types() {
        for (format, mime, bytes) in [
            ("mp3", "audio/mpeg", b"ID3\x04test-audio".as_slice()),
            ("mp3", "audio/mpeg", b"\xff\xfbtest-audio".as_slice()),
            (
                "wav",
                "audio/wav",
                b"RIFF\x24\0\0\0WAVEfmt test-audio".as_slice(),
            ),
        ] {
            let artifact = artifact_from_response(&reply(bytes), format).unwrap();
            assert_eq!(artifact.mime, mime);
            assert_eq!(artifact.data_url, Some(to_data_url(mime, bytes)));
            assert_eq!(artifact.filename, Some(format!("{TTS_MODEL}.{format}")));
            assert!(artifact.url.is_none() && artifact.text.is_none());
        }
    }

    #[test]
    fn absent_broken_and_truncated_audio_are_errors_without_echoing_the_payload() {
        for value in [
            serde_json::json!({}),
            serde_json::json!({"choices": []}),
            serde_json::json!({"choices": [{"message": {"audio": {"data": "private-invalid-payload"}}}]}),
            serde_json::json!({"choices": [{"message": {"audio": {"data": ""}}}]}),
            reply(b""),
            reply(b"not audio"),
        ] {
            let error = artifact_from_response(&value, "mp3").unwrap_err();
            assert!(!error.contains("private-invalid-payload"));
        }
        for reason in ["content_filter", "length"] {
            let mut value = reply(b"ID3\x04audio");
            value["choices"][0]["finish_reason"] = serde_json::json!(reason);
            assert!(artifact_from_response(&value, "mp3").is_err());
        }
        assert!(artifact_from_response(&reply(b"ID3\x04audio"), "wav").is_err());
        assert!(artifact_from_response(
            &serde_json::json!({"error":{"code":429,"message":"rate limit"}}),
            "mp3"
        )
        .unwrap_err()
        .contains("429"));
    }

    // A local HTTP endpoint checks the entire dispatch, header, wire body and decode.
    async fn serve_once(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            if key.eq_ignore_ascii_case("content-length") {
                                value.trim().parse::<usize>().ok()
                            } else {
                                None
                            }
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            sock.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{addr}/v1"), handle)
    }

    #[tokio::test]
    async fn dispatch_posts_the_documented_request_and_returns_playable_audio() {
        let bytes = b"ID3\x04test-audio";
        let (base, seen) = serve_once("200 OK", &reply(bytes).to_string()).await;
        let r = req(serde_json::json!({"voice": "冰糖"}));
        let result = crate::media::run(&provider(&base), "sk-mock", &r)
            .await
            .unwrap();
        assert_eq!(result.provider_id, "mimo");
        assert_eq!(result.model, TTS_MODEL);
        assert_eq!(
            result.artifacts[0].data_url,
            Some(to_data_url("audio/mpeg", bytes))
        );
        let request = seen.await.unwrap();
        let (headers, body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("POST /v1/chat/completions HTTP/1.1"));
        assert!(headers.to_lowercase().contains("api-key: sk-mock"));
        assert!(headers
            .to_lowercase()
            .contains("authorization: bearer sk-mock"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap(),
            speech_body(&r, &r.prompt)
        );
    }

    #[tokio::test]
    async fn http_and_json_errors_keep_retry_markers_and_redact_the_key() {
        for (status, body, marker) in [
            (
                "429 Too Many Requests",
                r#"{"error":{"message":"rate limit sk-private"}}"#,
                "429",
            ),
            (
                "401 Unauthorized",
                r#"{"error":{"message":"invalid api key sk-private"}}"#,
                "401",
            ),
            (
                "200 OK",
                r#"{"error":{"code":429,"message":"rate limit sk-private"}}"#,
                "429",
            ),
            ("200 OK", "not JSON", "解析"),
            ("200 OK", r#"{"choices":[]}"#, "没有返回音频"),
        ] {
            let (base, seen) = serve_once(status, body).await;
            let error = run(&provider(&base), "sk-private", &req(serde_json::json!({})))
                .await
                .unwrap_err();
            seen.await.unwrap();
            assert!(error.contains(marker), "{error}");
            assert!(!error.contains("sk-private"), "{error}");
        }
    }

    #[tokio::test]
    async fn invalid_requests_are_refused_before_any_network_request() {
        let p = provider("http://127.0.0.1:9/v1");
        let mut r = req(serde_json::json!({}));
        r.prompt = "x".repeat(TTS_MAX_CHARS as usize + 1);
        assert!(run(&p, "k", &r).await.unwrap_err().contains("1000"));
        r.prompt = "Hello.".into();
        r.model = "mimo-v2.5-pro".into();
        assert!(run(&p, "k", &r).await.unwrap_err().contains("不是已接入"));
        r.model = TTS_MODEL.into();
        r.kind = MediaKind::Music;
        assert!(run(&p, "k", &r)
            .await
            .unwrap_err()
            .contains("只接入了语音合成"));
    }
}
