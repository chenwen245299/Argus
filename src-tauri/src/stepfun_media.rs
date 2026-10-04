//! StepFun's media routes: the things that are not a conversation.
//!
//! Six tasks on five wire shapes, which is why the generic contract in
//! [`crate::media`] describes forms rather than prescribing one request body:
//!
//!   * image generation / editing — JSON to `/v1/images/generations` and
//!     `/v1/images/edits`, answering with a URL *or* base64 depending on a flag;
//!   * speech — JSON to `/v1/audio/speech`, answering with **raw bytes and no
//!     envelope at all**, so the media type has to be read off the request;
//!   * transcription — `multipart/form-data` to `/v1/audio/transcriptions`;
//!   * sound design — JSON to `/v1/audio/generate`, with roles and script lines
//!     rather than a single prompt;
//!   * music — an async job: `POST /v1/audio/music/submit` hands back a
//!     `task_id`, and `/v1/audio/music/query` is polled until it is done. Its
//!     audio inputs are bare base64 with **no `data:` prefix**, unlike every
//!     other endpoint here.
//!
//! ## The image routes are dying
//!
//! `step-2x-large`, `step-image-edit-2` and all three `/v1/images/*` paths go
//! offline on **2026-10-10**. They are still offered, because they still work and
//! withholding a working feature helps nobody — but every image model carries the
//! date in its note, so the studio says so before the user builds a habit on it.
//!
//! References:
//! <https://platform.stepfun.com/docs/zh/api-reference/images/image>,
//! <https://platform.stepfun.com/docs/zh/api-reference/audio/create-audio>,
//! <https://platform.stepfun.com/docs/zh/api-reference/audio/transcriptions>,
//! <https://platform.stepfun.com/docs/zh/api-reference/audio/generate>,
//! <https://platform.stepfun.com/docs/zh/api-reference/audio/music>

use serde::Deserialize;

use crate::media::{
    ext_for, opt_bool, opt_f64, opt_str, split_data_url, to_data_url, FieldKind, FieldOption,
    MediaArtifact, MediaCapability, MediaField, MediaKind, MediaModelSpec, MediaRequest,
    MediaResult,
};
use crate::models::AiProvider;

/// The date the three image routes stop serving, repeated in every image model's
/// note so it is visible at the point of choosing one.
const IMAGE_OFFLINE: &str = "2026-10-10 下线，届时文生图/图生图/改图接口一并停服";

/// The longest text one `/v1/audio/speech` request takes, in characters. Declared
/// as `max_prompt_chars` on every TTS model (the read-aloud player chunks by it)
/// and enforced in [`synthesize`] — one constant, so the two cannot drift.
const TTS_MAX_CHARS: u32 = 1000;

fn base(provider: &AiProvider) -> String {
    provider.base_url.trim_end_matches('/').to_string()
}

// ── What StepFun offers ──────────────────────────────────────────────────────

/// The official voice list, for the TTS form.
///
/// Hard-coded because no public endpoint enumerates it: `GET /v1/audio/voices`
/// returns the caller's own *cloned* voices and is empty on a fresh key, and
/// `/v1/audio/system_voices` only answers for `step-tts-2`. The field stays free
/// text underneath, so a cloned voice id can still be typed in.
fn voices() -> Vec<FieldOption> {
    [
        ("wenrounansheng", "温柔男声"),
        ("cixingnansheng", "磁性男声"),
        ("zixinnansheng", "自信男声"),
        ("yuanqinansheng", "元气男声"),
        ("boyinnansheng", "播音男声"),
        ("shenchennanyin", "深沉男音"),
        ("ruyananshi", "儒雅男士"),
        ("wenrougongzi", "温柔公子"),
        ("zhengpaiqingnian", "正派青年"),
        ("qingniandaxuesheng", "青年大学生"),
        ("qingchunshaonv", "清纯少女"),
        ("yuanqishaonv", "元气少女"),
        ("jilingshaonv", "机灵少女"),
        ("tianmeinvsheng", "甜美女声"),
        ("ruanmengnvsheng", "软萌女声"),
        ("wenrounvsheng", "温柔女声"),
        ("jingdiannvsheng", "经典女声"),
        ("qinqienvsheng", "亲切女声"),
        ("youyanvsheng", "优雅女声"),
        ("wenroushunv", "温柔熟女"),
        ("linjiajiejie", "邻家姐姐"),
        ("linjiameimei", "邻家妹妹"),
        ("zhixingjiejie", "知性姐姐"),
        ("shuangkuaijiejie", "爽快姐姐"),
        ("wenjingxuejie", "文静学姐"),
        ("lengyanyujie", "冷艳御姐"),
        ("elegantgentle-female", "气质温婉"),
        ("livelybreezy-female", "活力轻快"),
    ]
    .into_iter()
    .map(|(v, l)| FieldOption::new(v, l))
    .collect()
}

fn tts_fields(model: &str) -> Vec<MediaField> {
    let mut fields = vec![
        MediaField::select("voice", "音色", voices())
            .with_default(serde_json::json!("wenrounansheng")),
        // A dropdown cannot take a voice id that is not in it, and the cloned
        // ones never are — no public endpoint enumerates them
        // (`GET /v1/audio/voices` returns only the caller's own clones, and is
        // empty on a fresh key). So the list stays a list and this overrides it,
        // the same arrangement the provider's speech settings panel uses.
        MediaField::new("voice_custom", "自定义音色 ID", FieldKind::Text)
            .with_note("填了就用这个，覆盖上面的选择；复刻音色的 ID 在阶跃星辰控制台查"),
        // `pcm` is deliberately absent: the endpoint returns it headerless, so it
        // is not a file any player will open, and the sample rate is picked right
        // below — one fixed RIFF header could not describe every choice. It only
        // exists for streaming, which the studio does not do.
        MediaField::select(
            "response_format",
            "输出格式",
            ["mp3", "wav", "flac", "opus"]
                .iter()
                .map(|f| FieldOption::bare(f))
                .collect(),
        )
        .with_default(serde_json::json!("mp3")),
        MediaField::number("speed", "语速", 0.5, 2.0, 0.1).with_default(serde_json::json!(1.0)),
        MediaField::number("volume", "音量", 0.1, 2.0, 0.1).with_default(serde_json::json!(1.0)),
        MediaField::select(
            "sample_rate",
            "采样率",
            ["8000", "16000", "22050", "24000", "48000"]
                .iter()
                .map(|f| FieldOption::bare(f))
                .collect(),
        )
        .with_default(serde_json::json!("24000")),
    ];
    // `instruction` is the newer models' way of setting a global tone, and the
    // two older ones reject it outright.
    if model.starts_with("stepaudio-") {
        let cap = if model.starts_with("stepaudio-3") { 500 } else { 200 };
        fields.insert(
            // After the voice pair, before the output knobs.
            2,
            MediaField::new("instruction", "整体风格", FieldKind::LongText)
                .with_note(&format!("自然语言描述整段的情绪与人设，最多 {cap} 字"),
            ),
        );
    }
    fields
}

fn tts_model(id: &str, name: &str, note: &str) -> MediaModelSpec {
    MediaModelSpec {
        id: id.to_string(),
        display_name: name.to_string(),
        note: Some(note.to_string()),
        fields: tts_fields(id),
        accepts: vec![],
        file_required: false,
        prompt_required: true,
        prompt_placeholder: Some(format!("要合成的文本，最多 {TTS_MAX_CHARS} 字符")),
        max_prompt_chars: Some(TTS_MAX_CHARS),
    }
}

fn image_fields(edit2: bool) -> Vec<MediaField> {
    // step-image-edit-2 states its sizes as **height x width**, the other way
    // round from step-2x-large. Each model therefore gets its own list rather
    // than a shared one that would be silently transposed for one of them.
    let sizes: Vec<FieldOption> = if edit2 {
        ["1024x1024", "768x1360", "896x1184", "1360x768", "1184x896"]
            .iter()
            .map(|s| FieldOption::bare(s))
            .collect()
    } else {
        ["1024x1024", "512x512", "768x768", "256x256", "1280x800", "800x1280"]
            .iter()
            .map(|s| FieldOption::bare(s))
            .collect()
    };
    let mut fields = vec![
        MediaField::select("size", if edit2 { "尺寸（高×宽）" } else { "尺寸" }, sizes)
            .with_default(serde_json::json!("1024x1024")),
        MediaField::number("steps", "迭代步数", 1.0, 50.0, 1.0)
            .with_default(serde_json::json!(if edit2 { 8 } else { 50 })),
        MediaField::number("cfg_scale", "提示词贴合度", 1.0, 10.0, 0.5)
            .with_default(serde_json::json!(if edit2 { 1.0 } else { 6.0 })),
        MediaField::number("seed", "随机种子", 0.0, 2_147_483_647.0, 1.0)
            .with_note("留空则每次都不一样"),
    ];
    if edit2 {
        fields.push(
            MediaField::new("negative_prompt", "反向提示词", FieldKind::Text)
                .with_note("不希望出现的内容，最多 512 字符"),
        );
        fields.push(
            MediaField::new("text_mode", "文字增强", FieldKind::Toggle)
                .with_note("画面里要写字时打开"),
        );
    }
    fields
}

pub fn capabilities() -> Vec<MediaCapability> {
    vec![
        MediaCapability {
            kind: MediaKind::ImageGenerate,
            label: "文生图".to_string(),
            note: Some(format!("阶跃星辰的图像模型将于 {IMAGE_OFFLINE}")),
            models: vec![
                MediaModelSpec {
                    id: "step-image-edit-2".to_string(),
                    display_name: "Step Image Edit 2".to_string(),
                    note: Some(format!("0.02 元/张，1~2 秒出图；{IMAGE_OFFLINE}")),
                    fields: image_fields(true),
                    accepts: vec![],
                    file_required: false,
                    prompt_required: true,
                    prompt_placeholder: Some("描述想要的画面，最多 512 字符".to_string()),
                    max_prompt_chars: Some(512),
                },
                MediaModelSpec {
                    id: "step-2x-large".to_string(),
                    display_name: "Step 2X Large".to_string(),
                    note: Some(format!("0.1 元/张；{IMAGE_OFFLINE}")),
                    fields: image_fields(false),
                    accepts: vec![],
                    file_required: false,
                    prompt_required: true,
                    prompt_placeholder: Some("描述想要的画面，最多 512 字符".to_string()),
                    max_prompt_chars: Some(512),
                },
            ],
        },
        MediaCapability {
            kind: MediaKind::ImageEdit,
            label: "图片编辑".to_string(),
            note: Some(format!("阶跃星辰的图像模型将于 {IMAGE_OFFLINE}")),
            models: vec![MediaModelSpec {
                id: "step-image-edit-2".to_string(),
                display_name: "Step Image Edit 2".to_string(),
                note: Some(format!("0.02 元/张，输出尺寸跟随原图；{IMAGE_OFFLINE}")),
                fields: vec![
                    MediaField::number("steps", "迭代步数", 1.0, 50.0, 1.0)
                        .with_default(serde_json::json!(8)),
                    MediaField::number("cfg_scale", "提示词贴合度", 1.0, 10.0, 0.5)
                        .with_default(serde_json::json!(1.0)),
                    MediaField::number("seed", "随机种子", 0.0, 2_147_483_647.0, 1.0)
                        .with_note("留空则每次都不一样"),
                ],
                accepts: vec!["image/png".into(), "image/jpeg".into(), "image/webp".into()],
                file_required: true,
                prompt_required: true,
                prompt_placeholder: Some("要怎么改这张图，最多 512 字符".to_string()),
                max_prompt_chars: Some(512),
            }],
        },
        MediaCapability {
            kind: MediaKind::Speech,
            label: "语音合成".to_string(),
            note: None,
            models: vec![
                tts_model("stepaudio-3-tts", "StepAudio 3 TTS", "2.5 元/万字符，表现力最强"),
                tts_model("stepaudio-2.5-tts", "StepAudio 2.5 TTS", "5.8 元/万字符，语境理解"),
                tts_model("step-tts-2", "Step TTS 2", "2.8 元/万字符"),
                tts_model("step-tts-mini", "Step TTS Mini", "0.9 元/万字符，最便宜"),
            ],
        },
        MediaCapability {
            kind: MediaKind::Transcribe,
            label: "语音识别".to_string(),
            note: None,
            models: vec![
                MediaModelSpec {
                    id: "stepaudio-2.5-asr".to_string(),
                    display_name: "StepAudio 2.5 ASR".to_string(),
                    note: Some("0.15 元/小时，5 分钟音频 1 秒出结果".to_string()),
                    fields: vec![],
                    accepts: vec![
                        "audio/mpeg".into(),
                        "audio/wav".into(),
                        "audio/ogg".into(),
                    ],
                    file_required: true,
                    prompt_required: false,
                    prompt_placeholder: Some("可留空；填入热词（逗号分隔）可提升专有名词识别".to_string()),
                    max_prompt_chars: None,
                },
                MediaModelSpec {
                    id: "step-asr".to_string(),
                    display_name: "Step ASR".to_string(),
                    note: Some("0.9 元/小时，上一代模型".to_string()),
                    fields: vec![],
                    accepts: vec![
                        "audio/mpeg".into(),
                        "audio/wav".into(),
                        "audio/ogg".into(),
                    ],
                    file_required: true,
                    prompt_required: false,
                    prompt_placeholder: Some("可留空；填入热词（逗号分隔）可提升专有名词识别".to_string()),
                    max_prompt_chars: None,
                },
            ],
        },
        MediaCapability {
            kind: MediaKind::AudioGenerate,
            label: "音频生成".to_string(),
            note: Some("限时免费".to_string()),
            models: vec![MediaModelSpec {
                id: "stepaudio-3-gen-preview".to_string(),
                display_name: "StepAudio 3 Gen".to_string(),
                note: Some("限时免费；人声、音效、环境音和配乐一起生成".to_string()),
                fields: vec![
                    MediaField::new("roles", "角色设定", FieldKind::LongText)
                        .with_note("一行一个角色，如「小林：三十岁男性，声音低沉」，合计最多 500 字符"),
                    MediaField::new("scripts", "台词脚本", FieldKind::LongText)
                        .with_note("一行一句，如「小林：你终于来了」，合计最多 1000 字符"),
                ],
                accepts: vec![],
                file_required: false,
                prompt_required: true,
                prompt_placeholder: Some("整体描述：场景、氛围、音效与配乐，最多 500 字符".to_string()),
                max_prompt_chars: Some(500),
            }],
        },
        MediaCapability {
            kind: MediaKind::Music,
            label: "音乐生成".to_string(),
            note: Some("限时免费；异步生成，通常要等一会儿".to_string()),
            models: vec![MediaModelSpec {
                id: "stepaudio-3-music-preview".to_string(),
                display_name: "StepAudio 3 Music".to_string(),
                note: Some("限时免费".to_string()),
                fields: vec![
                    MediaField::select(
                        "task",
                        "任务",
                        vec![
                            FieldOption::new("text_to_music", "按描述作曲"),
                            FieldOption::new("music_cover", "翻唱既有歌曲"),
                            FieldOption::new("vocal_to_music", "为干声配乐"),
                        ],
                    )
                    .with_default(serde_json::json!("text_to_music")),
                    MediaField::new("lyrics", "歌词", FieldKind::LongText)
                        .with_note("留空则由模型自拟；纯音乐可忽略"),
                    MediaField::new("instrumental", "纯音乐（无人声）", FieldKind::Toggle)
                        .with_note("仅「按描述作曲」有效"),
                ],
                accepts: vec!["audio/mpeg".into(), "audio/wav".into()],
                // Only the cover / backing-track tasks need one, and which task is
                // selected is not known here — so the run button stays enabled and
                // `generate_music` refuses with a sentence naming the task.
                file_required: false,
                prompt_required: true,
                prompt_placeholder: Some("曲风、情绪、乐器、节奏……".to_string()),
                max_prompt_chars: None,
            }],
        },
    ]
}

// ── Running a task ───────────────────────────────────────────────────────────

pub async fn run(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<MediaResult, String> {
    let artifacts = match req.kind {
        MediaKind::ImageGenerate => generate_image(provider, api_key, req).await?,
        MediaKind::ImageEdit => edit_image(provider, api_key, req).await?,
        MediaKind::Speech => synthesize(provider, api_key, req).await?,
        MediaKind::Transcribe => transcribe(provider, api_key, req).await?,
        MediaKind::AudioGenerate => generate_audio(provider, api_key, req).await?,
        MediaKind::Music => generate_music(provider, api_key, req).await?,
    };
    Ok(MediaResult {
        kind: req.kind,
        provider_id: provider.id.clone(),
        model: req.model.clone(),
        artifacts,
    })
}

/// One POST with a JSON body, returning the parsed response or a message that
/// says which endpoint refused and why.
async fn post_json(
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let client = crate::llm::build_client()?;
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(body)
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(crate::llm::friendly_error(status, &text));
    }
    serde_json::from_str(&text).map_err(|e| format!("返回内容无法解析：{e}"))
}

// ── Images ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ImageResponse {
    #[serde(default)]
    data: Vec<ImageDatum>,
}

#[derive(Deserialize)]
struct ImageDatum {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    seed: Option<i64>,
}

/// Turn the image envelope into artifacts.
///
/// `b64_json` is always asked for rather than `url`: StepFun's image URLs expire
/// in 30 days, and a picture that vanishes from a saved result is worse than a
/// slightly larger payload.
fn image_artifacts(value: serde_json::Value, model: &str) -> Result<Vec<MediaArtifact>, String> {
    let parsed: ImageResponse =
        serde_json::from_value(value).map_err(|e| format!("图片返回内容无法解析：{e}"))?;
    if parsed.data.is_empty() {
        return Err("接口没有返回任何图片。".to_string());
    }
    let mut out = Vec::new();
    for (i, datum) in parsed.data.iter().enumerate() {
        // The moderator's verdict rides alongside the (missing) image, so say so
        // rather than reporting an empty success.
        if datum.finish_reason.as_deref() == Some("content_filtered") {
            return Err("这张图被内容安全策略拦截了，换个描述再试。".to_string());
        }
        let seed = datum.seed.map(|s| format!("seed {s}"));
        if let Some(b64) = datum.b64_json.as_deref().filter(|s| !s.is_empty()) {
            out.push(MediaArtifact {
                mime: "image/png".into(),
                data_url: Some(format!("data:image/png;base64,{b64}")),
                url: None,
                text: seed,
                filename: Some(format!("{model}-{}.png", i + 1)),
            });
        } else if let Some(url) = datum.url.as_deref().filter(|s| !s.is_empty()) {
            out.push(MediaArtifact {
                mime: "image/png".into(),
                data_url: None,
                url: Some(url.to_string()),
                text: seed,
                filename: Some(format!("{model}-{}.png", i + 1)),
            });
        }
    }
    if out.is_empty() {
        return Err("接口返回了空的图片数据。".to_string());
    }
    Ok(out)
}

/// The knobs both image routes share.
fn apply_image_options(body: &mut serde_json::Value, req: &MediaRequest) {
    if let Some(v) = opt_f64(req, "steps") {
        body["steps"] = serde_json::json!(v.round() as i64);
    }
    if let Some(v) = opt_f64(req, "cfg_scale") {
        body["cfg_scale"] = serde_json::json!(v);
    }
    if let Some(v) = opt_f64(req, "seed") {
        body["seed"] = serde_json::json!(v.round() as i64);
    }
    if let Some(v) = opt_str(req, "negative_prompt") {
        body["negative_prompt"] = serde_json::json!(v);
    }
    if opt_bool(req, "text_mode") == Some(true) {
        body["text_mode"] = serde_json::json!(true);
    }
}

async fn generate_image(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    if req.prompt.trim().is_empty() {
        return Err("请先写一句描述。".to_string());
    }
    let mut body = serde_json::json!({
        "model": req.model,
        "prompt": req.prompt.trim(),
        // Both image models document `n` as 1 only, so it is not a form field.
        "n": 1,
        "response_format": "b64_json",
    });
    if let Some(size) = opt_str(req, "size") {
        body["size"] = serde_json::json!(size);
    }
    apply_image_options(&mut body, req);

    let value = post_json(
        &format!("{}/images/generations", base(provider)),
        api_key,
        &body,
    )
    .await?;
    image_artifacts(value, &req.model)
}

async fn edit_image(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    let source = req
        .inputs
        .first()
        .ok_or("请先选一张要编辑的图片。")?;
    if req.prompt.trim().is_empty() {
        return Err("请说明要怎么改这张图。".to_string());
    }
    // Unlike `/images/generations`, this route is multipart/form-data with the
    // picture as a file field — every documented example uses `-F image=@…`.
    // Posting the same thing as JSON is what a 400 looks like.
    let (mime, bytes) = split_data_url(&source.data_url)?;
    let mut fields: Vec<(&str, String)> = vec![
        ("model", req.model.clone()),
        ("prompt", req.prompt.trim().to_string()),
        // Bytes rather than a URL: StepFun's image URLs expire in 30 days, and a
        // picture that vanishes from a saved result is worse than a big payload.
        ("response_format", "b64_json".to_string()),
    ];
    if let Some(v) = opt_f64(req, "steps") {
        fields.push(("steps", (v.round() as i64).to_string()));
    }
    if let Some(v) = opt_f64(req, "cfg_scale") {
        fields.push(("cfg_scale", v.to_string()));
    }
    if let Some(v) = opt_f64(req, "seed") {
        fields.push(("seed", (v.round() as i64).to_string()));
    }
    // `size` is documented as ignored on this route — the result matches the
    // input — so it is neither offered in the form nor sent.
    let (content_type, form) = multipart_body(&fields, "image", &source.name, &mime, &bytes);

    let client = crate::llm::build_client()?;
    let resp = client
        .post(format!("{}/images/edits", base(provider)))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", content_type)
        .body(form)
        .send()
        .await
        .map_err(|e| format!("图片编辑请求失败：{e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(crate::llm::friendly_error(status, &text));
    }
    let value: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("返回内容无法解析：{e}"))?;
    image_artifacts(value, &req.model)
}

// ── Speech ───────────────────────────────────────────────────────────────────

/// The media type for each `response_format`, needed because the endpoint answers
/// with bare bytes and no `Content-Type` worth trusting.
fn speech_mime(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "opus" => "audio/opus",
        // `pcm` is not offered (see `tts_fields`) precisely because it has no
        // container; if one ever reaches here it is labelled honestly rather than
        // dressed up as a wav a player would fail to open.
        "pcm" => "application/octet-stream",
        _ => "audio/mpeg",
    }
}

async fn synthesize(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    let input = req.prompt.trim();
    if input.is_empty() {
        return Err("请先输入要合成的文本。".to_string());
    }
    // Counted in characters rather than bytes, which is also how StepFun counts
    // (and bills) them. Refusing here beats a 400 after the text was typed.
    if input.chars().count() > TTS_MAX_CHARS as usize {
        return Err(format!(
            "文本有 {} 个字符，超过了 {TTS_MAX_CHARS} 的上限。请分段合成。",
            input.chars().count()
        ));
    }
    let format = opt_str(req, "response_format").unwrap_or("mp3");
    let mut body = serde_json::json!({
        "model": req.model,
        "input": input,
        // The free-text override wins when it is filled in; see `tts_fields`.
        "voice": opt_str(req, "voice_custom")
            .or_else(|| opt_str(req, "voice"))
            .unwrap_or("wenrounansheng"),
        "response_format": format,
    });
    if let Some(v) = opt_f64(req, "speed") {
        body["speed"] = serde_json::json!(v);
    }
    if let Some(v) = opt_f64(req, "volume") {
        body["volume"] = serde_json::json!(v);
    }
    if let Some(v) = opt_f64(req, "sample_rate") {
        body["sample_rate"] = serde_json::json!(v.round() as i64);
    }
    // `instruction` is rejected by the two older models, so it is only offered —
    // and only sent — for the stepaudio line.
    if let Some(v) = opt_str(req, "instruction") {
        if req.model.starts_with("stepaudio-") {
            body["instruction"] = serde_json::json!(v);
        }
    }

    let client = crate::llm::build_client()?;
    let resp = client
        .post(format!("{}/audio/speech", base(provider)))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("语音合成请求失败：{e}"))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(crate::llm::friendly_error(status, &text));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("读取音频失败：{e}"))?;
    if bytes.is_empty() {
        return Err("接口返回了空音频。".to_string());
    }
    let mime = speech_mime(format);
    Ok(vec![MediaArtifact {
        mime: mime.to_string(),
        data_url: Some(to_data_url(mime, &bytes)),
        url: None,
        text: None,
        filename: Some(format!("{}.{}", req.model, ext_for(mime))),
    }])
}

// ── Transcription ────────────────────────────────────────────────────────────

/// Build a `multipart/form-data` body by hand: some text fields plus one file.
///
/// Returns the `Content-Type` header (which carries the boundary) and the bytes.
/// Mirrors `deepseek::upload_file` rather than pulling in reqwest's `multipart`
/// feature, which this project deliberately does not enable.
fn multipart_body(
    fields: &[(&str, String)],
    file_field: &str,
    filename: &str,
    mime: &str,
    bytes: &[u8],
) -> (String, Vec<u8>) {
    use rand::RngCore;
    let boundary = format!("----ArgusFormBoundary{:016x}", rand::rngs::OsRng.next_u64());
    // A quote or newline in a filename would otherwise break out of the header.
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"").replace(['\r', '\n'], "_");

    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"{file_field}\"; filename=\"{}\"\r\n",
            escape(filename)
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[derive(Deserialize)]
struct TranscriptionResponse {
    #[serde(default)]
    text: String,
}

async fn transcribe(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    let source = req.inputs.first().ok_or("请先选一个音频文件。")?;
    let (mime, bytes) = split_data_url(&source.data_url)?;

    // The prompt box doubles as a hot-word list here: the endpoint takes them as
    // a JSON array string, which is not something to make the user type.
    let hotwords: Vec<&str> = req
        .prompt
        .split([',', '，', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    // Assembled by hand rather than through reqwest's `multipart` feature, the
    // same way `deepseek::upload_file` does it — the payload is three fields, and
    // hand-rolling keeps the dependency set exactly as it was.
    let mut fields: Vec<(&str, String)> = vec![
        ("model", req.model.clone()),
        // Documented as required, and `json` is the shape parsed below.
        ("response_format", "json".to_string()),
    ];
    if !hotwords.is_empty() {
        fields.push((
            "hotwords",
            serde_json::to_string(&hotwords).unwrap_or_default(),
        ));
    }
    let (content_type, body) = multipart_body(&fields, "file", &source.name, &mime, &bytes);

    let client = crate::llm::build_client()?;
    let resp = client
        .post(format!("{}/audio/transcriptions", base(provider)))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", content_type)
        .body(body)
        .send()
        .await
        .map_err(|e| format!("语音识别请求失败：{e}"))?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if status >= 400 {
        return Err(crate::llm::friendly_error(status, &text));
    }
    let parsed: TranscriptionResponse =
        serde_json::from_str(&text).map_err(|e| format!("识别结果无法解析：{e}"))?;
    if parsed.text.trim().is_empty() {
        return Err("没有识别到任何语音内容。".to_string());
    }
    Ok(vec![MediaArtifact {
        mime: "text/plain".into(),
        data_url: None,
        url: None,
        text: Some(parsed.text),
        filename: Some(format!("{}.txt", source.name)),
    }])
}

// ── Sound design ─────────────────────────────────────────────────────────────

/// Split a textarea into trimmed, non-empty lines.
fn lines_of(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    })
    .unwrap_or_default()
}

/// Split `名字：描述` on the first colon, accepting either width of it.
///
/// The form asks for one role or line per row because a nested editor would be
/// far more UI than this is worth; the endpoint wants objects, so the colon is
/// the separator between the two halves.
fn split_label(line: &str) -> Option<(String, String)> {
    let idx = line.find(['：', ':'])?;
    let (head, tail) = line.split_at(idx);
    let tail = tail.trim_start_matches(['：', ':']).trim();
    let head = head.trim();
    if head.is_empty() || tail.is_empty() {
        return None;
    }
    Some((head.to_string(), tail.to_string()))
}

/// `roles` as the endpoint defines it: `[{name, description}]`.
///
/// A row with no colon is not a usable role — there is nothing to call the voice
/// — so it is dropped rather than sent as a nameless one.
fn role_objects(raw: Option<&str>) -> Vec<serde_json::Value> {
    lines_of(raw)
        .iter()
        .filter_map(|l| split_label(l))
        .map(|(name, description)| serde_json::json!({ "name": name, "description": description }))
        .collect()
}

/// `scripts` as the endpoint defines it: `[{speaker?, text}]`.
///
/// `speaker` is optional and must be omitted for a pure sound effect or a music
/// cue, which the docs write wrapped in `[]` — so a bracketed row keeps its
/// brackets and gets no speaker, and a row with no colon is treated the same way.
fn script_objects(raw: Option<&str>, roles: &[serde_json::Value]) -> Vec<serde_json::Value> {
    let known: Vec<&str> = roles
        .iter()
        .filter_map(|r| r.get("name").and_then(|n| n.as_str()))
        .collect();
    lines_of(raw)
        .iter()
        .map(|line| {
            if line.starts_with('[') {
                return serde_json::json!({ "text": line });
            }
            match split_label(line) {
                // Only a declared role becomes a speaker. A line of dialogue that
                // happens to contain a colon ("旁白说：走吧") would otherwise be
                // split into a speaker the model was never told about.
                Some((speaker, text)) if known.contains(&speaker.as_str()) => {
                    serde_json::json!({ "speaker": speaker, "text": text })
                }
                _ => serde_json::json!({ "text": line }),
            }
        })
        .collect()
}

/// POST a JSON body to a route that answers with raw audio bytes rather than an
/// envelope — `/v1/audio/speech` and `/v1/audio/generate` both do, because their
/// `stream_format` defaults to `audio`.
async fn post_for_audio(
    url: &str,
    api_key: &str,
    body: &serde_json::Value,
) -> Result<Vec<u8>, String> {
    let client = crate::llm::build_client()?;
    let resp = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(body)
        .send()
        .await
        .map_err(|e| format!("请求失败：{e}"))?;
    let status = resp.status().as_u16();
    if status >= 400 {
        let text = resp.text().await.unwrap_or_default();
        return Err(crate::llm::friendly_error(status, &text));
    }
    let bytes = resp.bytes().await.map_err(|e| format!("读取音频失败：{e}"))?;
    if bytes.is_empty() {
        return Err("接口返回了空音频。".to_string());
    }
    Ok(bytes.to_vec())
}

async fn generate_audio(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    let instruction = req.prompt.trim();
    if instruction.is_empty() {
        return Err("请先描述想要的音频。".to_string());
    }
    if instruction.chars().count() > 500 {
        return Err("整体描述超过 500 字符，接口会拒绝。请精简一些。".to_string());
    }
    if opt_str(req, "roles").map_or(0, |r| r.chars().count()) > 500 {
        return Err("角色设定合计超过 500 字符，接口会拒绝。".to_string());
    }
    if opt_str(req, "scripts").map_or(0, |s| s.chars().count()) > 1000 {
        return Err("台词脚本合计超过 1000 字符，接口会拒绝。".to_string());
    }
    let roles = role_objects(opt_str(req, "roles"));
    let scripts = script_objects(opt_str(req, "scripts"), &roles);
    // `scripts` or `instruction` — at least one is required; `roles` is optional.
    // The instruction is already known non-empty, so nothing more to check.

    let mut body = serde_json::json!({
        "model": req.model,
        "task": "text_to_audio",
        "instruction": instruction,
        // Asked for explicitly so the bytes coming back have a media type this
        // code can name. Left unset, the route still returns audio — but nothing
        // in the response says which container it chose.
        "response_format": "mp3",
    });
    if !roles.is_empty() {
        body["roles"] = serde_json::json!(roles);
    }
    if !scripts.is_empty() {
        body["scripts"] = serde_json::json!(scripts);
    }

    // `stream_format` defaults to `audio`, so this answers with the file itself,
    // not with JSON.
    let bytes = post_for_audio(&format!("{}/audio/generate", base(provider)), api_key, &body).await?;
    Ok(vec![MediaArtifact {
        mime: "audio/mpeg".into(),
        data_url: Some(to_data_url("audio/mpeg", &bytes)),
        url: None,
        text: None,
        filename: Some(format!("{}.mp3", req.model)),
    }])
}

/// Pull an audio payload out of a response whose exact envelope is not pinned
/// down by the docs.
///
/// Both the sound-design and music routes have been observed describing their
/// result as base64 under one of several keys, or as a URL. Rather than guess one
/// and break on the others, the known spellings are tried in turn — and a URL is
/// accepted as-is, since the frontend can play and download it either way.
fn audio_from_value(
    value: &serde_json::Value,
    model: &str,
    fallback_mime: &str,
) -> Option<MediaArtifact> {
    let root = value.get("data").unwrap_or(value);
    for key in ["audio", "audio_base64", "b64_json", "audio_data"] {
        if let Some(b64) = root.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            // Some fields already carry the `data:` prefix; others are bare.
            let data_url = if b64.starts_with("data:") {
                b64.to_string()
            } else {
                format!("data:{fallback_mime};base64,{b64}")
            };
            return Some(MediaArtifact {
                mime: fallback_mime.to_string(),
                data_url: Some(data_url),
                url: None,
                text: None,
                filename: Some(format!("{model}.{}", ext_for(fallback_mime))),
            });
        }
    }
    for key in ["audio_url", "url", "song_url", "output_url"] {
        if let Some(url) = root.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
            return Some(MediaArtifact {
                mime: fallback_mime.to_string(),
                data_url: None,
                url: Some(url.to_string()),
                text: None,
                filename: Some(format!("{model}.{}", ext_for(fallback_mime))),
            });
        }
    }
    None
}

// ── Music ────────────────────────────────────────────────────────────────────

/// How long to keep polling a music job. Generation is minutes, not seconds, and
/// a job that has not finished by then has almost certainly failed in a way the
/// query endpoint is not reporting.
const MUSIC_POLL_ATTEMPTS: u32 = 60;
const MUSIC_POLL_INTERVAL_SECS: u64 = 5;

async fn generate_music(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    let task = opt_str(req, "task").unwrap_or("text_to_music");
    let prompt = req.prompt.trim();
    if prompt.is_empty() {
        return Err("请先描述想要的音乐。".to_string());
    }

    let instrumental = task == "text_to_music" && opt_bool(req, "instrumental") == Some(true);
    let lyrics = opt_str(req, "lyrics");
    // The docs' own requirement table: lyrics are mandatory for the two tasks
    // that follow an existing melody, and forbidden alongside `instrumental`.
    if (task == "music_cover" || task == "vocal_to_music") && lyrics.is_none() {
        return Err("翻唱和干声配乐必须提供歌词。".to_string());
    }
    if instrumental && lyrics.is_some() {
        return Err("纯音乐不能同时提供歌词，请清空歌词或关闭「纯音乐」。".to_string());
    }

    // `model_id` not `model`, and `caption` not `prompt` — this route spells both
    // differently from every other one here.
    let mut body = serde_json::json!({
        "model_id": req.model,
        "task": task,
        "caption": prompt,
        // Pinned so the bytes coming back have a media type this code can name;
        // the route would otherwise default to wav.
        "response_format": "mp3",
    });
    if let Some(lyrics) = lyrics {
        body["lyrics"] = serde_json::json!(lyrics);
    }
    if instrumental {
        body["instrumental"] = serde_json::json!(true);
    }
    // The reference audio for a cover or a backing track. This route takes bare
    // base64 with **no `data:` prefix**, unlike everything else here.
    if task == "music_cover" || task == "vocal_to_music" {
        let source = req
            .inputs
            .first()
            .ok_or("这个任务需要先选一个参考音频。")?;
        use base64::Engine;
        let (_, bytes) = split_data_url(&source.data_url)?;
        let bare = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let field = if task == "music_cover" { "song_audio" } else { "vocal_audio" };
        body[field] = serde_json::json!(bare);
    }

    let submitted = post_json(
        &format!("{}/audio/music/submit", base(provider)),
        api_key,
        &body,
    )
    .await?;
    let task_id = submitted
        .get("task_id")
        .or_else(|| submitted.get("data").and_then(|d| d.get("task_id")))
        .and_then(|v| v.as_str())
        .ok_or("提交成功但没有拿到任务 ID，无法查询结果。")?
        .to_string();

    for _ in 0..MUSIC_POLL_ATTEMPTS {
        tokio::time::sleep(std::time::Duration::from_secs(MUSIC_POLL_INTERVAL_SECS)).await;
        let queried = post_json(
            &format!("{}/audio/music/query", base(provider)),
            api_key,
            &serde_json::json!({ "task_id": task_id }),
        )
        .await?;
        let status = queried
            .get("status")
            .or_else(|| queried.get("data").and_then(|d| d.get("status")))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_uppercase();
        match status.as_str() {
            "SUCCESS" | "SUCCEEDED" | "COMPLETED" => {
                return audio_from_value(&queried, &req.model, "audio/mpeg")
                    .ok_or_else(|| "任务完成了，但返回里没有音频。".to_string())
                    .map(|a| vec![a]);
            }
            "FAILED" | "FAILURE" | "ERROR" => {
                // `FAILED` is a business outcome delivered over HTTP 200, and the
                // reason is nested under `error` with the stage that produced it.
                let err = queried.get("error");
                let msg = err
                    .and_then(|e| e.get("message"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("接口没有说明原因");
                let stage = err
                    .and_then(|e| e.get("stage"))
                    .and_then(|v| v.as_str())
                    .map(|s| format!("（{s} 阶段）"))
                    .unwrap_or_default();
                return Err(format!("音乐生成失败{stage}：{msg}"));
            }
            _ => continue,
        }
    }
    Err(format!(
        "等了 {} 分钟还没出结果，先停在这里。任务 ID {task_id} 仍在阶跃星辰那边，可以稍后到控制台查。",
        MUSIC_POLL_ATTEMPTS as u64 * MUSIC_POLL_INTERVAL_SECS / 60
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(kind: MediaKind, model: &str, prompt: &str, options: serde_json::Value) -> MediaRequest {
        MediaRequest {
            provider_id: "p".into(),
            kind,
            model: model.into(),
            prompt: prompt.into(),
            inputs: vec![],
            options: options.as_object().cloned().unwrap_or_default(),
        }
    }

    /// The studio renders its form straight from this JSON, so the wire names
    /// have to be exactly what `src/types/index.ts` declares. A serde rename
    /// drifting from the TS interface would not fail to compile on either side —
    /// the form would just render blank controls.
    #[test]
    fn the_wire_shape_is_camel_case_as_the_frontend_expects() {
        let json = serde_json::to_value(capabilities()).unwrap();
        let caps = json.as_array().unwrap();

        let speech = caps
            .iter()
            .find(|c| c["kind"] == "speech")
            .expect("speech capability");
        assert!(speech["label"].is_string());
        let model = &speech["models"][0];
        for key in ["id", "displayName", "promptRequired", "promptPlaceholder", "note", "fields"] {
            assert!(!model[key].is_null(), "models[0].{key} missing or null");
        }
        assert!(model["promptRequired"].is_boolean());

        let voice = model["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == "voice")
            .expect("voice field");
        assert_eq!(voice["kind"], "select");
        assert!(voice["options"].as_array().unwrap().len() > 10);
        assert_eq!(voice["options"][0]["value"], "wenrounansheng");
        assert!(voice["options"][0]["label"].is_string());
        assert_eq!(voice["default"], "wenrounansheng");

        // Field kinds must be the snake_case literals the template switches on.
        let kinds: std::collections::BTreeSet<String> = caps
            .iter()
            .flat_map(|c| c["models"].as_array().unwrap())
            .flat_map(|m| m["fields"].as_array().cloned().unwrap_or_default())
            .map(|f| f["kind"].as_str().unwrap_or_default().to_string())
            .collect();
        for k in &kinds {
            assert!(
                ["text", "long_text", "select", "number", "toggle"].contains(&k.as_str()),
                "unknown field kind {k}"
            );
        }

        // And the task kinds must be the snake_case literals `MediaKind` declares
        // in TypeScript, since the sidebar keys its icons off them.
        let task_kinds: Vec<&str> = caps.iter().map(|c| c["kind"].as_str().unwrap()).collect();
        assert_eq!(
            task_kinds,
            vec![
                "image_generate",
                "image_edit",
                "speech",
                "transcribe",
                "audio_generate",
                "music"
            ]
        );

        // `fileRequired` is what greys out the run button, and it is a different
        // question from `accepts`: music reads a reference track for two of its
        // three tasks but composes from a description alone, so requiring one
        // would make its main task unreachable.
        let music = caps.iter().find(|c| c["kind"] == "music").unwrap();
        assert!(!music["models"][0]["accepts"].as_array().unwrap().is_empty());
        assert_eq!(music["models"][0]["fileRequired"], false);
        let edit = caps.iter().find(|c| c["kind"] == "image_edit").unwrap();
        assert_eq!(edit["models"][0]["fileRequired"], true);
        let tr = caps.iter().find(|c| c["kind"] == "transcribe").unwrap();
        assert_eq!(tr["models"][0]["fileRequired"], true);

        // `accepts` drives the file picker for the two kinds that need an upload.
        let transcribe = caps.iter().find(|c| c["kind"] == "transcribe").unwrap();
        assert!(!transcribe["models"][0]["accepts"]
            .as_array()
            .unwrap()
            .is_empty());
        // …and is omitted entirely where nothing is uploaded, which the frontend
        // reads as "no file needed".
        assert!(speech["models"][0]["accepts"].is_null());
    }

    /// Writes the exact JSON the studio receives to a file, so the UI harness
    /// renders the real thing rather than a hand-written approximation.
    /// Ignored by default — it is a tool, not an assertion.
    #[test]
    #[ignore]
    fn dump_capabilities_for_the_harness() {
        let json = serde_json::to_string_pretty(&capabilities()).unwrap();
        let path = std::env::var("ARGUS_CAPS_DUMP").unwrap_or_else(|_| "/tmp/caps.json".into());
        std::fs::write(&path, json).unwrap();
        eprintln!("wrote {path}");
    }

    #[test]
    fn every_capability_offers_at_least_one_model() {
        let caps = capabilities();
        assert_eq!(caps.len(), 6, "all six task kinds are offered");
        for cap in &caps {
            assert!(!cap.models.is_empty(), "{:?} has no models", cap.kind);
            for m in &cap.models {
                assert!(!m.id.is_empty());
                assert!(!m.display_name.is_empty());
            }
        }
    }

    /// The image routes stop serving on a known date. Anyone picking one should
    /// see that before they build a habit on it.
    #[test]
    fn the_dying_image_models_say_so() {
        for cap in capabilities()
            .iter()
            .filter(|c| matches!(c.kind, MediaKind::ImageGenerate | MediaKind::ImageEdit))
        {
            for m in &cap.models {
                assert!(
                    m.note.as_deref().is_some_and(|n| n.contains("2026-10-10")),
                    "{} does not mention its retirement",
                    m.id
                );
            }
        }
    }

    /// `instruction` is a 400 on the two older TTS models, so it must not even be
    /// offered for them.
    #[test]
    fn instruction_is_offered_only_where_it_is_accepted() {
        let has_instruction =
            |id: &str| tts_fields(id).iter().any(|f| f.key == "instruction");
        assert!(has_instruction("stepaudio-3-tts"));
        assert!(has_instruction("stepaudio-2.5-tts"));
        assert!(!has_instruction("step-tts-2"));
        assert!(!has_instruction("step-tts-mini"));
    }

    /// The two image models state their sizes in opposite orders, so sharing one
    /// list would silently transpose half of them.
    #[test]
    fn each_image_model_gets_its_own_size_list() {
        let edit2: Vec<String> = image_fields(true)
            .into_iter()
            .find(|f| f.key == "size")
            .unwrap()
            .options
            .into_iter()
            .map(|o| o.value)
            .collect();
        let large: Vec<String> = image_fields(false)
            .into_iter()
            .find(|f| f.key == "size")
            .unwrap()
            .options
            .into_iter()
            .map(|o| o.value)
            .collect();
        assert!(edit2.contains(&"768x1360".to_string()));
        assert!(!large.contains(&"768x1360".to_string()));
        assert!(large.contains(&"1280x800".to_string()));
        assert!(!edit2.contains(&"1280x800".to_string()));
    }

    #[test]
    fn a_moderated_image_is_reported_rather_than_returned_empty() {
        let value = serde_json::json!({
            "data": [{"finish_reason": "content_filtered"}]
        });
        let err = image_artifacts(value, "step-image-edit-2").unwrap_err();
        assert!(err.contains("安全策略"), "{err}");
    }

    #[test]
    fn an_image_comes_back_as_a_data_url_with_its_seed() {
        let value = serde_json::json!({
            "data": [{"b64_json": "QUJD", "finish_reason": "success", "seed": 7}]
        });
        let arts = image_artifacts(value, "step-2x-large").unwrap();
        assert_eq!(arts.len(), 1);
        assert_eq!(arts[0].data_url.as_deref(), Some("data:image/png;base64,QUJD"));
        assert_eq!(arts[0].text.as_deref(), Some("seed 7"));
        assert_eq!(arts[0].filename.as_deref(), Some("step-2x-large-1.png"));
    }

    #[test]
    fn speech_format_decides_the_media_type() {
        assert_eq!(speech_mime("mp3"), "audio/mpeg");
        assert_eq!(speech_mime("wav"), "audio/wav");
        assert_eq!(speech_mime("flac"), "audio/flac");
        // Raw PCM has no container, so it is labelled honestly rather than
        // dressed up as a wav a player would fail to open. It is not offered as
        // a choice either — see `pcm_is_not_offered_as_a_tts_format`.
        assert_eq!(speech_mime("pcm"), "application/octet-stream");
        assert_eq!(speech_mime("anything-else"), "audio/mpeg");
    }

    #[tokio::test]
    async fn over_long_speech_is_refused_before_it_is_billed() {
        let provider = AiProvider {
            id: "p".into(),
            name: "StepFun".into(),
            kind: "stepfun".into(),
            base_url: "https://api.stepfun.com/v1".into(),
            enabled: true,
            models: vec![],
            server_tools: Default::default(),
            speech: Default::default(),
            created_at: String::new(),
        };
        // Characters, not bytes — 1001 Chinese characters is over the limit even
        // though a byte count would say 3003.
        let long = "字".repeat(1001);
        let r = req(MediaKind::Speech, "step-tts-mini", &long, serde_json::json!({}));
        let err = synthesize(&provider, "k", &r).await.unwrap_err();
        assert!(err.contains("1000"), "{err}");

        let r = req(MediaKind::Speech, "step-tts-mini", "   ", serde_json::json!({}));
        assert!(synthesize(&provider, "k", &r).await.is_err());
    }

    #[test]
    fn a_textarea_becomes_the_array_the_endpoint_wants() {
        assert_eq!(
            lines_of(Some("小林：低沉男声\n\n  阿May：清脆女声  \n")),
            vec!["小林：低沉男声", "阿May：清脆女声"]
        );
        assert!(lines_of(None).is_empty());
        assert!(lines_of(Some("  \n \n")).is_empty());
    }

    /// `roles` is `[{name, description}]`, not a list of strings. Sending strings
    /// is a well-formed request the endpoint rejects.
    #[test]
    fn roles_become_name_description_objects() {
        let roles = role_objects(Some("小林：三十岁男性，声音低沉\n阿May: 清脆女声\n没有冒号的一行"));
        assert_eq!(roles.len(), 2, "a row with no colon names no voice and is dropped");
        assert_eq!(roles[0]["name"], "小林");
        assert_eq!(roles[0]["description"], "三十岁男性，声音低沉");
        // A half-width colon works too — people type both.
        assert_eq!(roles[1]["name"], "阿May");
        assert_eq!(roles[1]["description"], "清脆女声");
    }

    /// `scripts` is `[{speaker?, text}]`, and `speaker` must be omitted for a
    /// sound effect or a music cue.
    #[test]
    fn scripts_attach_a_speaker_only_when_the_role_was_declared() {
        let roles = role_objects(Some("小林：低沉男声"));
        let scripts = script_objects(
            Some("[远处传来雷声]\n小林：你终于来了\n旁白说：这里有个冒号\n单纯一句话"),
            &roles,
        );
        assert_eq!(scripts.len(), 4);
        // A bracketed cue keeps its brackets and gets no speaker.
        assert_eq!(scripts[0]["text"], "[远处传来雷声]");
        assert!(scripts[0].get("speaker").is_none());
        // A declared role becomes the speaker.
        assert_eq!(scripts[1]["speaker"], "小林");
        assert_eq!(scripts[1]["text"], "你终于来了");
        // A colon inside ordinary dialogue must NOT invent an undeclared speaker.
        assert!(scripts[2].get("speaker").is_none());
        assert_eq!(scripts[2]["text"], "旁白说：这里有个冒号");
        assert!(scripts[3].get("speaker").is_none());
    }

    /// A cloned voice id is not in the dropdown and never can be, so the free
    /// text field has to win when it is filled in.
    #[test]
    fn a_custom_voice_id_overrides_the_dropdown() {
        let fields = tts_fields("stepaudio-3-tts");
        assert!(fields.iter().any(|f| f.key == "voice_custom"));
        // The dropdown's own note must not promise something it cannot do.
        let voice = fields.iter().find(|f| f.key == "voice").unwrap();
        assert!(voice.note.is_none(), "{:?}", voice.note);

        let with_custom = req(
            MediaKind::Speech,
            "stepaudio-3-tts",
            "你好",
            serde_json::json!({"voice": "wenrounansheng", "voice_custom": "my-cloned-voice"}),
        );
        assert_eq!(
            opt_str(&with_custom, "voice_custom").or_else(|| opt_str(&with_custom, "voice")),
            Some("my-cloned-voice")
        );
        // Blank override falls back to the dropdown rather than sending "".
        let blank = req(
            MediaKind::Speech,
            "stepaudio-3-tts",
            "你好",
            serde_json::json!({"voice": "qingchunshaonv", "voice_custom": "   "}),
        );
        assert_eq!(
            opt_str(&blank, "voice_custom").or_else(|| opt_str(&blank, "voice")),
            Some("qingchunshaonv")
        );
    }

    #[test]
    fn pcm_is_not_offered_as_a_tts_format() {
        // Headerless PCM is not a file a player opens, and the sample rate is
        // user-selectable so no single RIFF header would fit.
        for model in ["stepaudio-3-tts", "step-tts-mini"] {
            let formats: Vec<String> = tts_fields(model)
                .into_iter()
                .find(|f| f.key == "response_format")
                .unwrap()
                .options
                .into_iter()
                .map(|o| o.value)
                .collect();
            assert!(!formats.contains(&"pcm".to_string()), "{model}: {formats:?}");
            assert!(formats.contains(&"mp3".to_string()));
        }
        assert_eq!(speech_mime("pcm"), "application/octet-stream");
    }

    /// The docs' own requirement table. Each of these is a request the endpoint
    /// rejects, so refusing locally turns a 400 into a sentence.
    #[tokio::test]
    async fn music_enforces_the_documented_lyrics_rules() {
        let provider = AiProvider {
            id: "p".into(),
            name: "StepFun".into(),
            kind: "stepfun".into(),
            base_url: "https://api.stepfun.com/v1".into(),
            enabled: true,
            models: vec![],
            server_tools: Default::default(),
            speech: Default::default(),
            created_at: String::new(),
        };
        let m = "stepaudio-3-music-preview";

        // Lyrics are mandatory for the two tasks that follow an existing melody…
        let mut r = req(MediaKind::Music, m, "民谣", serde_json::json!({"task": "music_cover"}));
        r.inputs = vec![crate::media::MediaInput {
            name: "a.mp3".into(),
            data_url: "data:audio/mpeg;base64,QUJD".into(),
        }];
        let err = generate_music(&provider, "k", &r).await.unwrap_err();
        assert!(err.contains("歌词"), "{err}");

        // …and forbidden alongside `instrumental`.
        let r = req(
            MediaKind::Music,
            m,
            "轻快钢琴",
            serde_json::json!({"task": "text_to_music", "instrumental": true, "lyrics": "啦啦啦"}),
        );
        let err = generate_music(&provider, "k", &r).await.unwrap_err();
        assert!(err.contains("纯音乐"), "{err}");

        // An empty caption is refused before anything is billed.
        let r = req(MediaKind::Music, m, "  ", serde_json::json!({}));
        assert!(generate_music(&provider, "k", &r).await.is_err());
    }

    #[test]
    fn an_audio_payload_is_found_under_any_documented_spelling() {
        // Bare base64 under `audio`.
        let a = audio_from_value(
            &serde_json::json!({"audio": "QUJD"}),
            "m",
            "audio/mpeg",
        )
        .unwrap();
        assert_eq!(a.data_url.as_deref(), Some("data:audio/mpeg;base64,QUJD"));
        // Already-prefixed payload is not double-wrapped.
        let a = audio_from_value(
            &serde_json::json!({"data": {"audio": "data:audio/wav;base64,QUJD"}}),
            "m",
            "audio/wav",
        )
        .unwrap();
        assert_eq!(a.data_url.as_deref(), Some("data:audio/wav;base64,QUJD"));
        // A hosted URL instead of bytes.
        let a = audio_from_value(
            &serde_json::json!({"data": {"audio_url": "https://a.test/x.mp3"}}),
            "m",
            "audio/mpeg",
        )
        .unwrap();
        assert_eq!(a.url.as_deref(), Some("https://a.test/x.mp3"));
        assert!(a.data_url.is_none());
        // Nothing usable at all.
        assert!(audio_from_value(&serde_json::json!({"status": "PENDING"}), "m", "audio/mpeg").is_none());
    }
}
