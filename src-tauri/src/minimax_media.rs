//! MiniMax's media routes: today, speech synthesis (T2A v2).
//!
//! The chat side of MiniMax lives in [`crate::minimax`]; its speech, image and
//! video models answer on routes of their own, which is why they are an adapter
//! under the generic contract in [`crate::media`] rather than part of the chat
//! path. Only speech is wired up so far — it is what read-aloud needs — and the
//! others would slot in as further arms of [`run`] and [`capabilities`].
//!
//! ## `POST {base_url}/t2a_v2`
//!
//! JSON in, JSON out, with three habits worth knowing before touching this:
//!
//!   * **Failures arrive as HTTP 200.** The verdict is `base_resp.status_code`
//!     (0 = fine), the same envelope the chat endpoint uses, so
//!     [`crate::minimax::base_resp_error`] reads it. Only a throttle at the
//!     gateway or a dead key at the edge is a real 4xx/5xx, and those go through
//!     [`crate::llm::friendly_error`] like every other provider's.
//!   * **The audio is hex text, not base64** (`data.audio`), in whatever
//!     container `audio_setting.format` asked for. `data` itself may be `null`.
//!     `output_format: "url"` would return a link valid for 24 hours instead, but
//!     a link that expires is the wrong thing to hand a player that may sit
//!     paused, so bytes are always asked for.
//!   * **The limit is on characters, and a hard one**: "less than 10 000". Over
//!     3 000 the docs recommend streaming, which this adapter does not do — the
//!     read-aloud player sends small chunks and plays them as they arrive, so the
//!     work streaming would save is already done by chunking.
//!
//! ## Which values reach the wire
//!
//! The form's values are untyped and may be stale: they were saved against
//! whichever model was selected before, possibly another provider's. So nothing
//! is forwarded on trust. Numbers are clamped to the documented range rather than
//! refused (a read should not fail because a slider was dragged past an end),
//! `emotion` is sent only when the model documents it, a `voice` that is not in
//! the curated list falls back to the default (a leftover StepFun id would be a
//! 2013 here), and `language_boost` is checked against the options offered.
//!
//! References (all verified against the live pages, 2026-10):
//! <https://platform.minimax.cn/docs/api-reference/speech-t2a-http>,
//! <https://platform.minimax.cn/docs/faq/system-voice-id>,
//! <https://platform.minimax.cn/docs/api-reference/errorcode>,
//! <https://platform.minimax.cn/docs/guides/pricing-paygo>

use crate::media::{
    ext_for, opt_bool, opt_f64, opt_str, to_data_url, FieldKind, FieldOption, MediaArtifact,
    MediaCapability, MediaField, MediaKind, MediaModelSpec, MediaRequest, MediaResult,
};
use crate::models::AiProvider;

/// Longest text one request takes, in characters. The docs say "小于 10000", so
/// the last accepted length is 9 999 — one short is cheap insurance against an
/// off-by-one in a limit that costs a billed round trip to discover. Declared as
/// `max_prompt_chars` on every model and enforced in [`validate_text`].
const T2A_MAX_CHARS: u32 = 9_999;

/// Past this the docs recommend streaming; mentioned in the placeholder because a
/// non-streaming request this long is slow, not wrong.
const T2A_COMFORTABLE_CHARS: u32 = 3_000;

/// What the read-aloud button speaks with until the user picks something else.
///
/// An English voice, because the reader of this app is working through papers
/// that are mostly English: an English-native voice gets the prosody of technical
/// prose right where a Mandarin voice reading English can sound transliterated.
/// Chinese text still reads correctly, since [`DEFAULT_LANGUAGE_BOOST`] is `auto`
/// and the model decides per request. The same id heads the docs' own list of
/// current English voices, so it is not one of the older roster's strays.
const DEFAULT_VOICE: &str = "English_Graceful_Lady";

/// `language_boost` default. `auto` lets the model decide from the text, which is
/// the right call for a reader that meets English and Chinese on the same day.
const DEFAULT_LANGUAGE_BOOST: &str = "auto";

/// Sent as `Chinese,Yue` for a Cantonese voice under `auto`: the docs say those
/// voices "need" it, and a user who picked one has already said what they want.
const CANTONESE_BOOST: &str = "Chinese,Yue";

/// Default for `text_normalization`. Off in the API, on here: it improves how
/// numbers, dates and units are read at the cost of slightly more latency, and
/// a paper is mostly numbers, percentages and years.
const DEFAULT_TEXT_NORMALIZATION: bool = true;

const SPEED_RANGE: (f64, f64) = (0.5, 2.0);
/// The docs say `(0, 10]` — exclusive at zero — so the floor is the form's own
/// minimum of 0.1 rather than something arbitrarily close to zero.
const VOL_RANGE: (f64, f64) = (0.1, 10.0);
const PITCH_RANGE: (i64, i64) = (-12, 12);

/// Output containers offered. `pcm` and the two `pcmu_*` are deliberately absent
/// (headerless, or 8 kHz telephony), and `opus` is Ogg, which WebKit's `<audio>`
/// does not reliably play — the same reasoning as StepFun's list.
const FORMATS: [&str; 3] = ["mp3", "wav", "flac"];
const DEFAULT_FORMAT: &str = "mp3";

fn mime_for(format: &str) -> &'static str {
    match format {
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        _ => "audio/mpeg",
    }
}

// ── What MiniMax offers ──────────────────────────────────────────────────────

/// A curated slice of the official system voices, for the TTS form.
///
/// Hard-coded, like StepFun's, because the full roster is 327 voices in 27
/// languages — too many for a dropdown — and the only API that enumerates them
/// (`POST /v1/get_voice`) is account-scoped and answers per key. What is here
/// was checked against two official pages: the roster at `/faq/system-voice-id`
/// (every id below except the three marked) and the "latest system voices" list
/// in the T2A reference itself, which is where those three come from — newer
/// than the roster, so a roster-only check would call them invalid.
///
/// Ordered for this app's reader: English first (the default heads the list),
/// then Mandarin, then the two other languages a researcher is likely to want.
/// Labels carry the voice's own display name; the gloss after it is a translation
/// of that name or, for the two the docs describe (沉稳高管, 新闻女声), of their
/// description. Nothing is claimed about a timbre the docs do not state.
///
/// The field stays free text underneath (`voice_custom`), so a cloned or designed
/// voice id — which is never in any list — can still be typed in.
const VOICES: &[(&str, &str)] = &[
    ("English_Graceful_Lady", "英语 · Graceful Lady（优雅女声）"),
    ("English_Trustworthy_Man", "英语 · Trustworthy Man（可信赖男声）"),
    // The next three are in the T2A reference's list of current voices but not
    // yet in the roster page.
    ("English_Insightful_Speaker", "英语 · Insightful Speaker（睿智演讲者）"),
    ("English_Persuasive_Man", "英语 · Persuasive Man（有说服力的男声）"),
    ("English_radiant_girl", "英语 · Radiant Girl（明朗女声）"),
    ("English_Diligent_Man", "英语 · Diligent Man（勤勉男声）"),
    ("English_Gentle-voiced_man", "英语 · Gentle-voiced man（温和男声）"),
    ("Chinese (Mandarin)_Reliable_Executive", "普通话 · 沉稳高管（中年男声）"),
    ("Chinese (Mandarin)_News_Anchor", "普通话 · 新闻女声（播音腔）"),
    ("Chinese (Mandarin)_Male_Announcer", "普通话 · 播报男声"),
    ("Chinese (Mandarin)_Radio_Host", "普通话 · 电台男主播"),
    ("Chinese (Mandarin)_Gentleman", "普通话 · 温润男声"),
    ("Chinese (Mandarin)_Lyrical_Voice", "普通话 · 抒情男声"),
    ("Chinese (Mandarin)_Sweet_Lady", "普通话 · 甜美女声"),
    ("Chinese (Mandarin)_Gentle_Senior", "普通话 · 温柔学姐"),
    ("male-qn-jingying", "普通话 · 精英青年音色"),
    ("female-chengshu", "普通话 · 成熟女性音色"),
    ("Cantonese_GentleLady", "粤语 · 温柔女声"),
    ("Japanese_IntellectualSenior", "日语 · Intellectual Senior（知性前辈）"),
];

fn voices() -> Vec<FieldOption> {
    VOICES
        .iter()
        .map(|(id, label)| FieldOption::new(id, label))
        .collect()
}

/// The `language_boost` values offered: the languages this app's reader is
/// likely to meet, out of the 40 the API accepts. `auto` is the API's own value
/// for "decide from the text", not a sentinel of ours.
const LANGUAGE_BOOSTS: &[(&str, &str)] = &[
    ("auto", "自动判断"),
    ("Chinese", "中文"),
    ("Chinese,Yue", "粤语"),
    ("English", "英语"),
    ("Japanese", "日语"),
    ("Korean", "韩语"),
    ("French", "法语"),
    ("German", "德语"),
    ("Spanish", "西班牙语"),
    ("Russian", "俄语"),
    ("Portuguese", "葡萄牙语"),
    ("Italian", "意大利语"),
];

/// The `emotion` values every speech model documents, and the value the form
/// uses for "leave it to the model" — which the API has no spelling for, because
/// the way to ask for it is to omit the field.
const EMOTION_AUTO: &str = "auto";
const EMOTIONS: &[(&str, &str)] = &[
    ("happy", "高兴"),
    ("sad", "悲伤"),
    ("angry", "愤怒"),
    ("fearful", "害怕"),
    ("disgusted", "厌恶"),
    ("surprised", "惊讶"),
    ("calm", "中性"),
];
/// Two further emotions that only the 2.6 line takes: "仅对 speech-2.6-turbo,
/// speech-2.6-hd 模型生效". The 2.8 models are documented as not supporting
/// `whisper`, and `fluent` is listed with it, so neither is offered there.
const EMOTIONS_2_6_ONLY: &[(&str, &str)] = &[("fluent", "生动"), ("whisper", "低语")];

/// The model lines whose `emotion` the docs list as working. A model outside
/// them (a future id this build has never heard of) is sent no emotion at all,
/// not a guess.
fn supports_emotion(model: &str) -> bool {
    ["speech-2.8", "speech-2.6", "speech-02", "speech-01"]
        .iter()
        .any(|line| model.starts_with(line))
}

fn emotions_for(model: &str) -> Vec<(&'static str, &'static str)> {
    if !supports_emotion(model) {
        return Vec::new();
    }
    let mut all: Vec<_> = EMOTIONS.to_vec();
    if model.starts_with("speech-2.6") {
        all.extend_from_slice(EMOTIONS_2_6_ONLY);
    }
    all
}

fn tts_fields(model: &str) -> Vec<MediaField> {
    let mut fields = vec![
        MediaField::select("voice", "音色", voices())
            .with_default(serde_json::json!(DEFAULT_VOICE)),
        // A dropdown cannot take a voice id that is not in it, and the cloned and
        // designed ones never are. So the list stays a list and this overrides it,
        // the arrangement StepFun's form uses too.
        MediaField::new("voice_custom", "自定义音色 ID", FieldKind::Text).with_note(
            "填了就用这个，覆盖上面的选择；复刻或文生音色的 ID 在 MiniMax 控制台查，复刻音色需先正式合成一次才可用",
        ),
        MediaField::number("speed", "语速", SPEED_RANGE.0, SPEED_RANGE.1, 0.1)
            .with_default(serde_json::json!(1.0)),
        MediaField::number("vol", "音量", VOL_RANGE.0, VOL_RANGE.1, 0.1)
            .with_default(serde_json::json!(1.0)),
        MediaField::number("pitch", "语调", PITCH_RANGE.0 as f64, PITCH_RANGE.1 as f64, 1.0)
            .with_default(serde_json::json!(0))
            .with_note("半音为单位，0 为原音色"),
    ];

    let emotions = emotions_for(model);
    if !emotions.is_empty() {
        let mut options = vec![FieldOption::new(EMOTION_AUTO, "自动（按文本判断）")];
        options.extend(emotions.iter().map(|(v, l)| FieldOption::new(v, l)));
        fields.push(
            MediaField::select("emotion", "情绪", options)
                .with_default(serde_json::json!(EMOTION_AUTO))
                .with_note("一般无需手动指定，模型会按文本自动匹配"),
        );
    }

    fields.push(
        MediaField::select(
            "language_boost",
            "语种增强",
            LANGUAGE_BOOSTS
                .iter()
                .map(|(v, l)| FieldOption::new(v, l))
                .collect(),
        )
        .with_default(serde_json::json!(DEFAULT_LANGUAGE_BOOST))
        .with_note("增强对指定语种的识别；选「自动判断」让模型自己看文本。粤语音色会自动按粤语处理"),
    );
    fields.push(
        MediaField::new("text_normalization", "文本规范化", FieldKind::Toggle)
            .with_default(serde_json::json!(DEFAULT_TEXT_NORMALIZATION))
            .with_note("规范数字、日期、单位的读法，会略增延迟；论文里数字多，默认打开"),
    );
    fields.push(
        MediaField::select(
            "format",
            "输出格式",
            FORMATS.iter().map(|f| FieldOption::bare(f)).collect(),
        )
        .with_default(serde_json::json!(DEFAULT_FORMAT)),
    );
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
        prompt_placeholder: Some(format!(
            "要合成的文本，最多 {T2A_MAX_CHARS} 字符（超过 {T2A_COMFORTABLE_CHARS} 字符会比较慢）"
        )),
        max_prompt_chars: Some(T2A_MAX_CHARS),
    }
}

/// Models offered, newest first. `speech-01-*` is in the API's enum but missing
/// from the published price list, so it is left out rather than offered without
/// saying what it costs.
///
/// Prices are the pay-as-you-go ones on the platform's pricing page (元 per 万
/// characters); a plan or resource pack may price them differently.
pub fn capabilities() -> Vec<MediaCapability> {
    vec![MediaCapability {
        kind: MediaKind::Speech,
        label: "语音合成".to_string(),
        note: Some(
            "按字符计费：1 个汉字算 2 个字符，英文字母、数字、标点、空格各算 1 个。下列价格为官方按量计费价"
                .to_string(),
        ),
        models: vec![
            tts_model(
                "speech-2.8-turbo",
                "Speech 2.8 Turbo",
                "2 元/万字符；速度更快、成本更低，朗读首选。文本里可插入 (laughs) 等语气词标签",
            ),
            tts_model(
                "speech-2.8-hd",
                "Speech 2.8 HD",
                "3.5 元/万字符；音质与表现力最好。文本里可插入 (laughs) 等语气词标签",
            ),
            tts_model(
                "speech-2.6-turbo",
                "Speech 2.6 Turbo",
                "2 元/万字符；上一代，情绪多「生动」「低语」两种",
            ),
            tts_model(
                "speech-2.6-hd",
                "Speech 2.6 HD",
                "3.5 元/万字符；上一代，情绪多「生动」「低语」两种",
            ),
            tts_model("speech-02-turbo", "Speech 02 Turbo", "2 元/万字符；更早一代"),
            tts_model("speech-02-hd", "Speech 02 HD", "3.5 元/万字符；更早一代"),
        ],
    }]
}

// ── Running a task ───────────────────────────────────────────────────────────

pub async fn run(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<MediaResult, String> {
    let artifacts = match req.kind {
        MediaKind::Speech => synthesize(provider, api_key, req).await?,
        _ => return Err("MiniMax 目前只接入了语音合成。".to_string()),
    };
    Ok(MediaResult {
        kind: req.kind,
        provider_id: provider.id.clone(),
        model: req.model.clone(),
        artifacts,
    })
}

/// The T2A URL for a provider's `base_url`.
///
/// The preset is `https://api.minimax.cn/v1`, but people paste what they have:
/// the global host's `…/v1`, a trailing slash, a bare host, or the Anthropic-
/// compatible address (`…/anthropic`) a coding tool asked for. Those all mean the
/// same service, so they all land on `/v1/t2a_v2`. A base with some other path is
/// a relay's and is left exactly as given.
fn t2a_url(base_url: &str) -> String {
    let base = base_url.trim().trim_end_matches('/');
    let base = base.strip_suffix("/anthropic").unwrap_or(base);
    let after_scheme = base.split_once("://").map_or(base, |(_, rest)| rest);
    let has_path = after_scheme.contains('/');
    if has_path {
        format!("{base}/t2a_v2")
    } else {
        format!("{base}/v1/t2a_v2")
    }
}

/// Refuse what the API would refuse, in a sentence, before anything is billed.
/// Counted in characters rather than bytes: 9 999 Chinese characters are 30 kB.
fn validate_text(text: &str) -> Result<&str, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("请先输入要合成的文本。".to_string());
    }
    let n = text.chars().count();
    if n > T2A_MAX_CHARS as usize {
        return Err(format!(
            "文本有 {n} 个字符，超过了 {T2A_MAX_CHARS} 的上限。请分段合成。"
        ));
    }
    Ok(text)
}

fn clamp_round(v: f64, (lo, hi): (f64, f64)) -> f64 {
    // Two decimals: a slider hands back 1.2000000000000002, which is noise in a
    // request body and in a log of one.
    ((v.clamp(lo, hi)) * 100.0).round() / 100.0
}

/// The voice id this request speaks with: the free-text override, else the
/// dropdown, else the default. A dropdown value that is not in [`VOICES`] is not
/// trusted — see the module docs — so a voice from another provider's form (or
/// from a roster that has since changed) cannot reach the API as an id MiniMax
/// has never heard of.
fn voice_id(req: &MediaRequest) -> String {
    if let Some(custom) = opt_str(req, "voice_custom") {
        return custom.to_string();
    }
    match opt_str(req, "voice") {
        Some(v) if VOICES.iter().any(|(id, _)| *id == v) => v.to_string(),
        _ => DEFAULT_VOICE.to_string(),
    }
}

/// The `emotion` to send, if any. `auto`, blank, an unknown word, and an emotion
/// this model does not take all mean "omit it" — a stale value should make a read
/// plainer, never make it fail.
fn emotion_for(req: &MediaRequest) -> Option<&str> {
    let wanted = opt_str(req, "emotion").filter(|e| *e != EMOTION_AUTO)?;
    emotions_for(&req.model)
        .iter()
        .any(|(v, _)| *v == wanted)
        .then_some(wanted)
}

fn language_boost_for(req: &MediaRequest, voice: &str) -> String {
    let chosen = opt_str(req, "language_boost")
        .filter(|b| LANGUAGE_BOOSTS.iter().any(|(v, _)| v == b))
        .unwrap_or(DEFAULT_LANGUAGE_BOOST);
    if chosen == DEFAULT_LANGUAGE_BOOST && voice.starts_with("Cantonese_") {
        return CANTONESE_BOOST.to_string();
    }
    chosen.to_string()
}

fn format_for(req: &MediaRequest) -> &'static str {
    let wanted = opt_str(req, "format").unwrap_or(DEFAULT_FORMAT);
    FORMATS
        .iter()
        .copied()
        .find(|f| *f == wanted)
        .unwrap_or(DEFAULT_FORMAT)
}

/// The JSON body of one T2A request. Pure, so every option combination can be
/// checked without a network.
///
/// Only values the user (or the form's defaults) actually set are written;
/// anything left out takes MiniMax's own default, which is the same number the
/// form displays.
fn t2a_body(req: &MediaRequest, text: &str) -> serde_json::Value {
    let voice = voice_id(req);

    let mut voice_setting = serde_json::json!({
        "voice_id": voice,
        "text_normalization":
            opt_bool(req, "text_normalization").unwrap_or(DEFAULT_TEXT_NORMALIZATION),
    });
    // `is_finite`: a string option "NaN" or "inf" parses as a float, and neither
    // survives being clamped into something meaningful.
    if let Some(v) = opt_f64(req, "speed").filter(|v| v.is_finite()) {
        voice_setting["speed"] = serde_json::json!(clamp_round(v, SPEED_RANGE));
    }
    if let Some(v) = opt_f64(req, "vol").filter(|v| v.is_finite()) {
        voice_setting["vol"] = serde_json::json!(clamp_round(v, VOL_RANGE));
    }
    if let Some(v) = opt_f64(req, "pitch").filter(|v| v.is_finite()) {
        // Integer-only on the wire: 1.5 is a 400, not a rounding.
        let semitones = (v.round() as i64).clamp(PITCH_RANGE.0, PITCH_RANGE.1);
        voice_setting["pitch"] = serde_json::json!(semitones);
    }
    if let Some(emotion) = emotion_for(req) {
        voice_setting["emotion"] = serde_json::json!(emotion);
    }

    serde_json::json!({
        "model": req.model,
        "text": text,
        "stream": false,
        // Bytes, not the 24-hour link: see the module docs.
        "output_format": "hex",
        "voice_setting": voice_setting,
        "audio_setting": { "format": format_for(req) },
        "language_boost": language_boost_for(req, &voice),
    })
}

/// MiniMax's message for a `base_resp` code, as the user sees it.
///
/// [`crate::minimax::describe_code`] already puts the chat-side codes into
/// words and keeps the code on the end as ` (NNNN)`, which is the marker
/// `llm::classify_error` reads — so this only adds the codes whose meaning is
/// specific to speech, and hands the rest through. Two of those differ from the
/// chat reading: **1039** is "Token limit" in the shared error table but the TPM
/// rate limit on this route, and a text full of invisible characters (**1042**)
/// is a PDF-copy problem here, not a prompt one.
///
/// The shared tables are left alone on purpose: `code_class(1039)` is `Request`,
/// which is right for the chat endpoint's `max_tokens` and is what the arXiv
/// batch reads. Nothing batch-like calls this adapter.
fn speech_error(code: i64, msg: &str) -> String {
    let Ok(code) = u32::try_from(code) else {
        return format!("语音合成失败（错误码 {code}）：{msg}");
    };
    let label = match code {
        1039 => Some("语音合成请求过于频繁（TPM 超限），已被限流，请稍后重试"),
        1042 => Some("文本里的不可见字符或非法字符超过 10%（从 PDF 复制的文字常带控制字符），请换一段再试"),
        2042 => Some("没有权限使用这个音色 ID（只能用自己创建的音色）"),
        20132 => Some("音色 ID 有误，请检查「自定义音色 ID」是否填对"),
        2061 => Some("当前套餐不包含这个语音模型，请换一个模型或改用按量计费的 Key"),
        _ => None,
    };
    let msg = msg.trim();
    match label {
        Some(label) if msg.is_empty() => crate::minimax::describe_code(code, label),
        Some(label) => crate::minimax::describe_code(code, &format!("{label}：{msg}")),
        None => crate::minimax::describe_code(code, msg),
    }
}

/// Decode the `data.audio` hex string.
///
/// Hand-rolled, like the multipart body in `stepfun_media`: it is a dozen lines,
/// and the `hex` crate is in the tree only as somebody else's dependency. Errors
/// say what was wrong with the shape and never echo the payload.
fn decode_hex(s: &str) -> Result<Vec<u8>, String> {
    let digits = s.trim().as_bytes();
    if digits.len() % 2 != 0 {
        return Err(format!(
            "返回的音频数据不完整（十六进制长度 {} 是奇数），可能被截断了，请重试。",
            digits.len()
        ));
    }
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    for (i, pair) in digits.chunks_exact(2).enumerate() {
        match (nibble(pair[0]), nibble(pair[1])) {
            (Some(hi), Some(lo)) => out.push(hi << 4 | lo),
            _ => {
                return Err(format!(
                    "返回的音频数据里有非十六进制的字符（第 {} 个字节），无法解码。",
                    i + 1
                ))
            }
        }
    }
    Ok(out)
}

/// Turn a parsed T2A response into the artifact, or the reason there is none.
///
/// Checked in the order the failures actually arrive: the business verdict
/// first (a 200 that says no), then a missing `data` (documented as possible),
/// then an empty or undecodable payload.
fn artifact_from_response(
    value: &serde_json::Value,
    model: &str,
    format: &str,
) -> Result<MediaArtifact, String> {
    if let Some((code, msg)) = crate::minimax::base_resp_error(value) {
        return Err(speech_error(code, &msg));
    }
    let trace = value
        .get("trace_id")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(|t| format!("（trace_id {t}）"))
        .unwrap_or_default();
    let hex = value
        .get("data")
        .and_then(|d| d.get("audio"))
        .and_then(|a| a.as_str())
        .ok_or_else(|| format!("接口没有返回音频数据{trace}。"))?;
    let bytes = decode_hex(hex)?;
    if bytes.is_empty() {
        return Err(format!("接口返回了空音频{trace}。"));
    }

    let mime = mime_for(format);
    // The billed length, as a caption under the clip in the studio. Present on
    // every documented success; a body without it simply has no caption.
    let caption = value
        .get("extra_info")
        .and_then(|e| e.get("usage_characters"))
        .and_then(|n| n.as_u64())
        .map(|n| format!("计费 {n} 字符"));
    Ok(MediaArtifact {
        mime: mime.to_string(),
        data_url: Some(to_data_url(mime, &bytes)),
        url: None,
        text: caption,
        filename: Some(format!("{model}.{}", ext_for(mime))),
    })
}

async fn synthesize(
    provider: &AiProvider,
    api_key: &str,
    req: &MediaRequest,
) -> Result<Vec<MediaArtifact>, String> {
    let text = validate_text(&req.prompt)?;
    // The chat models share this provider and none of them can speak; picking
    // one here is a settings mix-up worth naming rather than a 2013 to decode.
    if !req.model.starts_with("speech-") {
        return Err(format!(
            "「{}」不是 MiniMax 的语音合成模型，请到 设置 → AI 随航 → 朗读 重新选择。",
            req.model
        ));
    }
    let body = t2a_body(req, text);
    let format = format_for(req);

    let client = crate::llm::build_client()?;
    let resp = client
        .post(t2a_url(&provider.base_url))
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| crate::llm::describe_reqwest_error(&e))?;
    let status = resp.status().as_u16();
    let raw = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {}", crate::llm::describe_reqwest_error(&e)))?;
    if status >= 400 {
        return Err(crate::llm::friendly_error(status, &raw));
    }
    let value: serde_json::Value = serde_json::from_str(&raw).map_err(|e| {
        // The body can be megabytes of hex; the parser's position is enough.
        format!("语音合成返回的内容无法解析：{e}")
    })?;
    Ok(vec![artifact_from_response(&value, &req.model, format)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(model: &str, prompt: &str, options: serde_json::Value) -> MediaRequest {
        MediaRequest {
            provider_id: "mm".into(),
            kind: MediaKind::Speech,
            model: model.into(),
            prompt: prompt.into(),
            inputs: vec![],
            options: options.as_object().cloned().unwrap_or_default(),
        }
    }

    fn body(model: &str, options: serde_json::Value) -> serde_json::Value {
        t2a_body(&req(model, "Hello.", options), "Hello.")
    }

    fn provider_at(base_url: &str) -> AiProvider {
        serde_json::from_value(serde_json::json!({
            "id": "mm", "name": "MiniMax", "kind": "minimax",
            "base_url": base_url, "created_at": ""
        }))
        .unwrap()
    }

    fn hex_of(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    // ── the form ─────────────────────────────────────────────────────────────

    #[test]
    fn the_form_describes_every_knob_the_adapter_reads() {
        let caps = capabilities();
        assert_eq!(caps.len(), 1);
        assert_eq!(caps[0].kind, MediaKind::Speech);
        assert!(!caps[0].models.is_empty());

        for m in &caps[0].models {
            let keys: Vec<&str> = m.fields.iter().map(|f| f.key.as_str()).collect();
            for want in [
                "voice",
                "voice_custom",
                "speed",
                "vol",
                "pitch",
                "emotion",
                "language_boost",
                "text_normalization",
                "format",
            ] {
                assert!(keys.contains(&want), "{}: no {want} field in {keys:?}", m.id);
            }
            // Declared, and the same number `run` enforces.
            assert_eq!(m.max_prompt_chars, Some(T2A_MAX_CHARS), "{}", m.id);
            assert!(m.prompt_required);
            assert!(m.accepts.is_empty() && !m.file_required);
            assert!(
                m.prompt_placeholder.as_deref().unwrap().contains("9999"),
                "{:?}",
                m.prompt_placeholder
            );
        }
    }

    /// Prices are quoted only where the platform publishes one, and the 2.8
    /// pair is the pair the pricing page lists first.
    #[test]
    fn only_published_prices_are_quoted() {
        let caps = capabilities();
        let note = |id: &str| {
            caps[0]
                .models
                .iter()
                .find(|m| m.id == id)
                .and_then(|m| m.note.clone())
                .unwrap_or_default()
        };
        assert!(note("speech-2.8-turbo").starts_with("2 元/万字符"));
        assert!(note("speech-2.8-hd").starts_with("3.5 元/万字符"));
        assert!(note("speech-02-hd").starts_with("3.5 元/万字符"));
        // `speech-01-*` has no published price, so it is not offered.
        assert!(caps[0].models.iter().all(|m| !m.id.starts_with("speech-01")));
        // The unit that makes the price mean anything.
        assert!(caps[0].note.as_deref().unwrap().contains("1 个汉字算 2 个字符"));
    }

    #[test]
    fn the_default_voice_is_in_the_list_and_is_english() {
        assert!(VOICES.iter().any(|(id, _)| *id == DEFAULT_VOICE));
        assert!(DEFAULT_VOICE.starts_with("English_"));
        assert_eq!(VOICES[0].0, DEFAULT_VOICE, "the default heads the list");
        let mut ids: Vec<&str> = VOICES.iter().map(|(id, _)| *id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), VOICES.len(), "duplicate voice id");
        assert!((12..=20).contains(&VOICES.len()), "{}", VOICES.len());
        for (id, label) in VOICES {
            assert!(!id.trim().is_empty() && id.trim() == *id, "{id:?}");
            assert!(!label.is_empty());
        }
        // The dropdown's own note must not promise something it cannot do.
        let voice = tts_fields("speech-2.8-turbo")
            .into_iter()
            .find(|f| f.key == "voice")
            .unwrap();
        assert!(voice.note.is_none());
    }

    #[test]
    fn emotion_is_offered_only_where_the_docs_say_it_works() {
        let options = |model: &str| -> Vec<String> {
            tts_fields(model)
                .into_iter()
                .find(|f| f.key == "emotion")
                .map(|f| f.options.into_iter().map(|o| o.value).collect())
                .unwrap_or_default()
        };
        // 2.8: the seven common ones, and "auto" for omitting the field.
        let o28 = options("speech-2.8-hd");
        assert_eq!(o28[0], "auto");
        assert!(o28.contains(&"calm".to_string()));
        assert!(!o28.contains(&"whisper".to_string()), "2.8 does not support whisper");
        assert!(!o28.contains(&"fluent".to_string()));
        // 2.6: the same plus the two that only it takes.
        let o26 = options("speech-2.6-turbo");
        assert!(o26.contains(&"whisper".to_string()) && o26.contains(&"fluent".to_string()));
        // 02 is documented for emotion but without the 2.6-only pair.
        let o02 = options("speech-02-hd");
        assert!(o02.contains(&"happy".to_string()) && !o02.contains(&"fluent".to_string()));
        // A model line the docs do not list gets no emotion control at all.
        assert!(options("speech-9.9-hd").is_empty());
    }

    /// The shape the studio and the read-aloud settings render from. `media.rs`
    /// runs the structural rules over every adapter; these are the MiniMax
    /// specifics.
    #[test]
    fn no_unplayable_format_is_offered() {
        for model in ["speech-2.8-turbo", "speech-2.6-hd", "speech-02-turbo"] {
            let formats: Vec<String> = tts_fields(model)
                .into_iter()
                .find(|f| f.key == "format")
                .unwrap()
                .options
                .into_iter()
                .map(|o| o.value)
                .collect();
            assert_eq!(formats, vec!["mp3", "wav", "flac"], "{model}");
        }
        for bad in ["pcm", "pcmu_raw", "pcmu_wav", "opus"] {
            assert!(!FORMATS.contains(&bad));
        }
    }

    #[test]
    fn the_wire_shape_is_camel_case_as_the_frontend_expects() {
        let json = serde_json::to_value(capabilities()).unwrap();
        let model = &json[0]["models"][0];
        assert_eq!(json[0]["kind"], "speech");
        assert_eq!(model["maxPromptChars"], 9999);
        for key in ["id", "displayName", "promptRequired", "promptPlaceholder", "note", "fields"] {
            assert!(!model[key].is_null(), "{key}");
        }
        let toggle = model["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["key"] == "text_normalization")
            .unwrap();
        assert_eq!(toggle["kind"], "toggle");
        assert_eq!(toggle["default"], true);
    }

    // ── the request ──────────────────────────────────────────────────────────

    #[test]
    fn a_request_with_no_options_is_the_documented_defaults() {
        let b = body("speech-2.8-turbo", serde_json::json!({}));
        assert_eq!(b["model"], "speech-2.8-turbo");
        assert_eq!(b["text"], "Hello.");
        assert_eq!(b["stream"], false);
        assert_eq!(b["output_format"], "hex");
        assert_eq!(b["voice_setting"]["voice_id"], DEFAULT_VOICE);
        // What the form says its default is must be what an empty request does.
        assert_eq!(b["voice_setting"]["text_normalization"], true);
        assert_eq!(b["audio_setting"]["format"], "mp3");
        assert_eq!(b["language_boost"], "auto");
        // Knobs nobody set are left to MiniMax's own defaults.
        for k in ["speed", "vol", "pitch", "emotion"] {
            assert!(b["voice_setting"].get(k).is_none(), "{k} should be omitted");
        }
    }

    #[test]
    fn a_full_form_reaches_the_wire() {
        let b = body(
            "speech-2.6-hd",
            serde_json::json!({
                "voice": "Chinese (Mandarin)_News_Anchor", "speed": 1.25, "vol": 2, "pitch": -3,
                "emotion": "whisper", "language_boost": "Chinese", "text_normalization": false,
                "format": "flac"
            }),
        );
        let v = &b["voice_setting"];
        assert_eq!(v["voice_id"], "Chinese (Mandarin)_News_Anchor");
        assert_eq!(v["speed"], 1.25);
        assert_eq!(v["vol"], 2.0);
        assert_eq!(v["pitch"], -3);
        assert_eq!(v["emotion"], "whisper");
        assert_eq!(v["text_normalization"], false);
        assert_eq!(b["language_boost"], "Chinese");
        assert_eq!(b["audio_setting"]["format"], "flac");
    }

    /// A cloned voice id is not in the dropdown and never can be, so the free
    /// text field has to win when it is filled in.
    #[test]
    fn a_custom_voice_id_overrides_the_dropdown() {
        let b = body(
            "speech-2.8-hd",
            serde_json::json!({"voice": "English_Graceful_Lady", "voice_custom": "  my_clone_01 "}),
        );
        assert_eq!(b["voice_setting"]["voice_id"], "my_clone_01");
        // Blank override falls back to the dropdown rather than sending "".
        let b = body(
            "speech-2.8-hd",
            serde_json::json!({"voice": "female-chengshu", "voice_custom": "   "}),
        );
        assert_eq!(b["voice_setting"]["voice_id"], "female-chengshu");
    }

    /// Both adapters name their dropdown `voice`, so switching provider leaves
    /// the other's id behind in the saved options. It must not become a
    /// MiniMax request.
    #[test]
    fn a_voice_left_over_from_another_provider_falls_back_to_the_default() {
        for stale in ["wenrounansheng", "alloy", "", "English_Made_Up_Voice"] {
            let b = body("speech-2.8-turbo", serde_json::json!({ "voice": stale }));
            assert_eq!(b["voice_setting"]["voice_id"], DEFAULT_VOICE, "{stale:?}");
        }
    }

    #[test]
    fn emotion_is_sent_only_when_the_model_takes_it() {
        let emotion = |model: &str, e: &str| {
            body(model, serde_json::json!({ "emotion": e }))["voice_setting"]
                .get("emotion")
                .cloned()
        };
        assert_eq!(emotion("speech-2.8-hd", "happy"), Some(serde_json::json!("happy")));
        assert_eq!(emotion("speech-2.6-hd", "whisper"), Some(serde_json::json!("whisper")));
        assert_eq!(emotion("speech-2.6-turbo", "fluent"), Some(serde_json::json!("fluent")));
        // 2.8 and 02 do not take the 2.6-only pair: dropped, not sent to fail.
        assert_eq!(emotion("speech-2.8-hd", "whisper"), None);
        assert_eq!(emotion("speech-2.8-turbo", "fluent"), None);
        assert_eq!(emotion("speech-02-hd", "whisper"), None);
        // "auto" is the form's word for omitting the field, never the API's.
        assert_eq!(emotion("speech-2.8-hd", "auto"), None);
        assert_eq!(emotion("speech-2.8-hd", ""), None);
        assert_eq!(emotion("speech-2.8-hd", "furious"), None);
        // A model the docs do not list is sent nothing.
        assert_eq!(emotion("speech-9.9-hd", "happy"), None);
    }

    #[test]
    fn numbers_are_clamped_into_the_documented_ranges() {
        let v = |options: serde_json::Value| body("speech-2.8-turbo", options)["voice_setting"].clone();

        assert_eq!(v(serde_json::json!({"speed": 5}))["speed"], 2.0);
        assert_eq!(v(serde_json::json!({"speed": 0.1}))["speed"], 0.5);
        assert_eq!(v(serde_json::json!({"speed": 1.2000000000000002}))["speed"], 1.2);
        // "(0, 10]": zero is out, so the floor is the form's 0.1.
        assert_eq!(v(serde_json::json!({"vol": 0}))["vol"], 0.1);
        assert_eq!(v(serde_json::json!({"vol": -4}))["vol"], 0.1);
        assert_eq!(v(serde_json::json!({"vol": 99}))["vol"], 10.0);
        // Pitch is an integer on the wire.
        assert_eq!(v(serde_json::json!({"pitch": 20.4}))["pitch"], 12);
        assert_eq!(v(serde_json::json!({"pitch": -50}))["pitch"], -12);
        assert_eq!(v(serde_json::json!({"pitch": -3.6}))["pitch"], -4);
        assert!(v(serde_json::json!({"pitch": 2.0}))["pitch"].is_i64());
        // A number input hands back a string.
        assert_eq!(v(serde_json::json!({"speed": "1.5"}))["speed"], 1.5);
        // "NaN" and "inf" parse as floats; neither means anything, so neither is sent.
        for junk in ["NaN", "inf", "-inf", "fast"] {
            let s = v(serde_json::json!({"speed": junk, "vol": junk, "pitch": junk}));
            for k in ["speed", "vol", "pitch"] {
                assert!(s.get(k).is_none(), "{junk}: {k} = {}", s[k]);
            }
        }
    }

    #[test]
    fn language_boost_is_checked_and_cantonese_follows_its_voice() {
        let lb = |options: serde_json::Value| body("speech-2.8-hd", options)["language_boost"].clone();

        assert_eq!(lb(serde_json::json!({})), "auto");
        assert_eq!(lb(serde_json::json!({"language_boost": "English"})), "English");
        assert_eq!(lb(serde_json::json!({"language_boost": "Chinese,Yue"})), "Chinese,Yue");
        // Not an option offered: back to the default instead of a 2013.
        assert_eq!(lb(serde_json::json!({"language_boost": "Klingon"})), "auto");
        assert_eq!(lb(serde_json::json!({"language_boost": ""})), "auto");

        // The docs: a Cantonese voice needs "Chinese,Yue".
        let yue = serde_json::json!({"voice": "Cantonese_GentleLady"});
        assert_eq!(lb(yue), "Chinese,Yue");
        // …but an explicit choice is the user's to make.
        let explicit = serde_json::json!({"voice": "Cantonese_GentleLady", "language_boost": "English"});
        assert_eq!(lb(explicit), "English");
        // Every value offered is one the API documents (spot-check the awkward one).
        assert!(LANGUAGE_BOOSTS.iter().any(|(v, _)| *v == "Chinese,Yue"));
    }

    #[test]
    fn only_playable_formats_are_requested_and_each_has_its_media_type() {
        let fmt = |f: &str| body("speech-2.8-hd", serde_json::json!({"format": f}))["audio_setting"]["format"].clone();
        assert_eq!(fmt("mp3"), "mp3");
        assert_eq!(fmt("wav"), "wav");
        assert_eq!(fmt("flac"), "flac");
        // Headerless or telephony formats fall back rather than being sent.
        for bad in ["pcm", "pcmu_raw", "pcmu_wav", "opus", "ogg", ""] {
            assert_eq!(fmt(bad), "mp3", "{bad:?}");
        }
        assert_eq!(mime_for("mp3"), "audio/mpeg");
        assert_eq!(mime_for("wav"), "audio/wav");
        assert_eq!(mime_for("flac"), "audio/flac");
    }

    #[test]
    fn the_url_is_found_from_whatever_base_was_pasted() {
        for (base, want) in [
            ("https://api.minimax.cn/v1", "https://api.minimax.cn/v1/t2a_v2"),
            ("https://api.minimax.cn/v1/", "https://api.minimax.cn/v1/t2a_v2"),
            ("https://api.minimax.cn/v1///", "https://api.minimax.cn/v1/t2a_v2"),
            ("  https://api.minimax.io/v1 ", "https://api.minimax.io/v1/t2a_v2"),
            ("https://api.minimaxi.com/v1", "https://api.minimaxi.com/v1/t2a_v2"),
            // A bare host, and the Anthropic-compatible address.
            ("https://api.minimax.cn", "https://api.minimax.cn/v1/t2a_v2"),
            ("https://api.minimax.cn/", "https://api.minimax.cn/v1/t2a_v2"),
            ("https://api.minimax.io/anthropic", "https://api.minimax.io/v1/t2a_v2"),
            ("https://api.minimax.io/anthropic/", "https://api.minimax.io/v1/t2a_v2"),
            // A relay's own path is its business.
            ("https://relay.test/minimax/v1", "https://relay.test/minimax/v1/t2a_v2"),
            ("https://relay.test/custom", "https://relay.test/custom/t2a_v2"),
        ] {
            assert_eq!(t2a_url(base), want, "{base}");
        }
    }

    // ── the text ─────────────────────────────────────────────────────────────

    #[test]
    fn empty_text_is_refused_before_anything_is_billed() {
        assert!(validate_text("").is_err());
        assert!(validate_text("  \n\t ").unwrap_err().contains("请先输入"));
        assert_eq!(validate_text("  hi  ").unwrap(), "hi");
    }

    /// Characters, not bytes — 9 999 Chinese characters are about 30 kB.
    #[test]
    fn the_limit_counts_characters() {
        let at_limit = "字".repeat(T2A_MAX_CHARS as usize);
        assert!(at_limit.len() > 20_000);
        assert!(validate_text(&at_limit).is_ok());

        let over = "字".repeat(T2A_MAX_CHARS as usize + 1);
        let err = validate_text(&over).unwrap_err();
        assert!(err.contains("10000") && err.contains("9999"), "{err}");

        // Surrounding whitespace is not text.
        let padded = format!("  {}  ", "a".repeat(T2A_MAX_CHARS as usize));
        assert!(validate_text(&padded).is_ok());
    }

    // ── the response ─────────────────────────────────────────────────────────

    #[test]
    fn hex_decodes_in_either_case() {
        assert_eq!(decode_hex("00ff10").unwrap(), vec![0x00, 0xff, 0x10]);
        assert_eq!(decode_hex("DEADbeef").unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(decode_hex("  4944330a \n").unwrap(), b"ID3\n");
        assert_eq!(decode_hex("").unwrap(), Vec::<u8>::new());
        let all: Vec<u8> = (0..=255).collect();
        assert_eq!(decode_hex(&hex_of(&all)).unwrap(), all);
    }

    #[test]
    fn bad_hex_is_an_error_that_does_not_echo_the_payload() {
        let odd = decode_hex("abc").unwrap_err();
        assert!(odd.contains("奇数"), "{odd}");
        let bad = decode_hex("00zz").unwrap_err();
        assert!(bad.contains("非十六进制") && bad.contains("第 2 个字节"), "{bad}");
        // Not hex at all — e.g. the field held a URL.
        assert!(decode_hex("https://x.test/a.mp3").is_err());
        assert!(!decode_hex("secret-ish-payload-zz").unwrap_err().contains("secret"));
    }

    #[test]
    fn a_success_becomes_a_playable_data_url() {
        let value = serde_json::json!({
            "data": {"audio": hex_of(b"ID3\x03abc"), "status": 2},
            "extra_info": {"audio_length": 9900, "usage_characters": 26, "audio_format": "mp3"},
            "trace_id": "t-1",
            "base_resp": {"status_code": 0, "status_msg": "success"}
        });
        let a = artifact_from_response(&value, "speech-2.8-turbo", "mp3").unwrap();
        assert_eq!(a.mime, "audio/mpeg");
        assert_eq!(a.data_url.unwrap(), to_data_url("audio/mpeg", b"ID3\x03abc"));
        assert_eq!(a.filename.as_deref(), Some("speech-2.8-turbo.mp3"));
        assert_eq!(a.text.as_deref(), Some("计费 26 字符"));
        assert!(a.url.is_none());

        let wav = artifact_from_response(&value, "speech-2.8-hd", "wav").unwrap();
        assert_eq!(wav.mime, "audio/wav");
        assert_eq!(wav.filename.as_deref(), Some("speech-2.8-hd.wav"));

        // No `extra_info` is still a success, just without a caption.
        let bare = serde_json::json!({"data": {"audio": "4142"}});
        let a = artifact_from_response(&bare, "m", "mp3").unwrap();
        assert!(a.text.is_none());
    }

    #[test]
    fn a_missing_empty_or_broken_payload_is_reported_not_returned() {
        // `data` is documented as possibly null.
        for v in [
            serde_json::json!({"data": null, "trace_id": "abc123", "base_resp": {"status_code": 0}}),
            serde_json::json!({"base_resp": {"status_code": 0}}),
            serde_json::json!({"data": {"status": 2}}),
        ] {
            let err = artifact_from_response(&v, "m", "mp3").unwrap_err();
            assert!(err.contains("没有返回音频"), "{err}");
        }
        // The trace id is how MiniMax's support finds the request.
        let err = artifact_from_response(
            &serde_json::json!({"data": null, "trace_id": "abc123"}),
            "m",
            "mp3",
        )
        .unwrap_err();
        assert!(err.contains("abc123"), "{err}");

        let err = artifact_from_response(&serde_json::json!({"data": {"audio": ""}}), "m", "mp3")
            .unwrap_err();
        assert!(err.contains("空音频"), "{err}");
        assert!(artifact_from_response(&serde_json::json!({"data": {"audio": "xyz"}}), "m", "mp3").is_err());
        assert!(artifact_from_response(&serde_json::json!({"data": {"audio": "abc"}}), "m", "mp3").is_err());
    }

    // ── errors ───────────────────────────────────────────────────────────────

    fn failure(code: i64, msg: &str) -> String {
        let v = serde_json::json!({"data": null, "base_resp": {"status_code": code, "status_msg": msg}});
        artifact_from_response(&v, "m", "mp3").unwrap_err()
    }

    #[test]
    fn a_200_with_a_business_code_is_an_error_that_keeps_its_code() {
        use crate::llm::{classify_error, is_throttle, ErrorClass};

        // Bad key: tells the user where to fix it, and stops a batch.
        let e = failure(1004, "login fail: Please carry the API secret key");
        assert!(e.contains("API Key") && e.ends_with("(1004)"), "{e}");
        assert_eq!(classify_error(&e), ErrorClass::Fatal);

        // Rate limit: a throttle, transient.
        let e = failure(1002, "rate limit exceeded(RPM)");
        assert!(e.contains("频繁") && e.ends_with("(1002)"), "{e}");
        assert_eq!(classify_error(&e), ErrorClass::Transient);
        assert!(is_throttle(&e));

        // The Token Plan window being used up is not a throttle and not retryable.
        let e = failure(2056, "usage limit exceeded");
        assert!(e.contains("Token Plan 额度已用尽") && e.ends_with("(2056)"), "{e}");
        assert_eq!(classify_error(&e), ErrorClass::Fatal);
        assert!(!is_throttle(&e));

        // Peak-hour overload.
        let e = failure(2064, "服务器繁忙");
        assert!(e.ends_with("(2064)"), "{e}");
        assert_eq!(classify_error(&e), ErrorClass::Transient);

        // Bad parameters.
        let e = failure(2013, "invalid params");
        assert!(e.starts_with("请求参数错误") && e.ends_with("(2013)"), "{e}");

        // No balance.
        let e = failure(1008, "insufficient balance");
        assert!(e.contains("余额不足") && e.ends_with("(1008)"), "{e}");
    }

    /// 1039 reads as a token limit in the shared table (right for chat) and as a
    /// TPM rate limit on this route. The speech wording must say so, and the code
    /// must still be on the end.
    #[test]
    fn code_1039_and_1042_are_worded_for_speech() {
        let e = failure(1039, "TPM limit");
        assert!(e.contains("TPM") && e.contains("请稍后重试") && e.ends_with("(1039)"), "{e}");
        assert!(!e.contains("max_tokens"), "{e}");

        let e = failure(1042, "invisible character ratio exceeds 10%");
        assert!(e.contains("10%") && e.contains("PDF") && e.ends_with("(1042)"), "{e}");

        let e = failure(2042, "");
        assert!(e.contains("音色") && e.ends_with("(2042)"), "{e}");
        let e = failure(20132, "bad voice");
        assert!(e.contains("自定义音色 ID") && e.ends_with("(20132)"), "{e}");
    }

    #[test]
    fn an_unknown_or_odd_code_still_says_something() {
        assert!(failure(7777, "weird").ends_with("(7777)"));
        assert_eq!(failure(7777, "weird"), "weird (7777)");
        // Words and code both missing.
        assert!(failure(7777, "").contains("7777"));
        // A code that cannot be a u32 is reported with its value.
        let e = failure(-5, "negative");
        assert!(e.contains("-5") && e.contains("negative"), "{e}");
    }

    #[test]
    fn a_status_code_of_zero_is_a_success_not_an_error() {
        let v = serde_json::json!({"data": {"audio": "4142"}, "base_resp": {"status_code": 0, "status_msg": "success"}});
        assert!(artifact_from_response(&v, "m", "mp3").is_ok());
        let v = serde_json::json!({"data": {"audio": "4142"}, "base_resp": {"status_code": "0"}});
        assert!(artifact_from_response(&v, "m", "mp3").is_ok());
    }

    // ── end to end, against a canned server ──────────────────────────────────

    /// Serve one canned HTTP response on a loopback port. Returns the base URL
    /// the adapter should be pointed at and a handle that yields the raw request
    /// the server saw, so a test can check the path, the header and the body.
    async fn serve_once(
        status: &str,
        body: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                req.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&req);
                if let Some(head_end) = text.find("\r\n\r\n") {
                    let len = text[..head_end]
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if req.len() >= head_end + 4 + len {
                        break;
                    }
                }
            }
            let _ = sock.write_all(response.as_bytes()).await;
            let _ = sock.shutdown().await;
            String::from_utf8_lossy(&req).to_string()
        });
        (format!("http://{addr}/v1"), handle)
    }

    async fn speak(base: &str, key: &str, prompt: &str) -> Result<MediaResult, String> {
        run(
            &provider_at(base),
            key,
            &req(
                "speech-2.8-turbo",
                prompt,
                serde_json::json!({"voice": "English_Trustworthy_Man", "speed": 1.1}),
            ),
        )
        .await
    }

    #[tokio::test]
    async fn run_posts_the_documented_request_and_decodes_the_hex() {
        let reply = serde_json::json!({
            "data": {"audio": hex_of(b"ID3\x04hello-audio"), "status": 2},
            "extra_info": {"usage_characters": 6},
            "trace_id": "t",
            "base_resp": {"status_code": 0, "status_msg": "success"}
        })
        .to_string();
        let (base, seen) = serve_once("200 OK", &reply).await;

        let result = speak(&base, "sk-test-key", "Hello.").await.unwrap();
        assert_eq!(result.kind, MediaKind::Speech);
        assert_eq!(result.provider_id, "mm");
        assert_eq!(result.model, "speech-2.8-turbo");
        assert_eq!(result.artifacts.len(), 1);
        let a = &result.artifacts[0];
        assert_eq!(a.mime, "audio/mpeg");
        assert_eq!(a.data_url.as_deref(), Some(to_data_url("audio/mpeg", b"ID3\x04hello-audio").as_str()));
        assert_eq!(a.text.as_deref(), Some("计费 6 字符"));

        // What the server was actually sent.
        let request = seen.await.unwrap();
        let (head, payload) = request.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("POST /v1/t2a_v2 HTTP/1.1"), "{head}");
        let lower = head.to_lowercase();
        assert!(lower.contains("authorization: bearer sk-test-key"), "{head}");
        assert!(lower.contains("content-type: application/json"), "{head}");
        let sent: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(sent["model"], "speech-2.8-turbo");
        assert_eq!(sent["text"], "Hello.");
        assert_eq!(sent["stream"], false);
        assert_eq!(sent["output_format"], "hex");
        assert_eq!(sent["voice_setting"]["voice_id"], "English_Trustworthy_Man");
        assert_eq!(sent["voice_setting"]["speed"], 1.1);
        assert_eq!(sent["audio_setting"]["format"], "mp3");
    }

    #[tokio::test]
    async fn run_turns_a_200_business_error_into_a_coded_message_without_the_key() {
        let reply = r#"{"data":null,"base_resp":{"status_code":1004,"status_msg":"login fail"}}"#;
        let (base, _) = serve_once("200 OK", reply).await;
        let err = speak(&base, "sk-very-secret-key", "Hi.").await.unwrap_err();
        assert!(err.contains("(1004)"), "{err}");
        assert!(!err.contains("sk-very-secret-key"), "the key leaked into: {err}");
    }

    #[tokio::test]
    async fn run_maps_an_http_error_through_friendly_error() {
        let reply = r#"{"base_resp":{"status_code":2056,"status_msg":"usage limit exceeded"}}"#;
        let (base, _) = serve_once("429 Too Many Requests", reply).await;
        let err = speak(&base, "sk-very-secret-key", "Hi.").await.unwrap_err();
        // MiniMax's throttles and its used-up plan windows both ride a 429; the
        // code, put into words, is what says which.
        assert!(err.starts_with("请求被拒绝（429）"), "{err}");
        assert!(err.contains("Token Plan 额度已用尽") && err.contains("(2056)"), "{err}");
        assert!(!err.contains("sk-very-secret-key"), "{err}");

        let (base, _) = serve_once("401 Unauthorized", "").await;
        let err = speak(&base, "k", "Hi.").await.unwrap_err();
        assert!(err.contains("401"), "{err}");
    }

    #[tokio::test]
    async fn run_rejects_garbage_bodies_and_empty_audio() {
        let (base, _) = serve_once("200 OK", "<html>gateway</html>").await;
        assert!(speak(&base, "k", "Hi.").await.unwrap_err().contains("无法解析"));

        let (base, _) = serve_once("200 OK", r#"{"data":{"audio":"zz"},"base_resp":{"status_code":0}}"#).await;
        assert!(speak(&base, "k", "Hi.").await.unwrap_err().contains("十六进制"));

        let (base, _) = serve_once("200 OK", r#"{"data":null,"base_resp":{"status_code":0}}"#).await;
        assert!(speak(&base, "k", "Hi.").await.unwrap_err().contains("没有返回音频"));
    }

    /// Refused before any request is made: nothing is listening here, so a
    /// request would show up as a network error instead.
    #[tokio::test]
    async fn run_refuses_bad_input_without_calling_out() {
        let dead = "http://127.0.0.1:9/v1";
        let over = "x".repeat(T2A_MAX_CHARS as usize + 1);
        let err = speak(dead, "k", &over).await.unwrap_err();
        assert!(err.contains("9999"), "{err}");
        assert!(speak(dead, "k", "   ").await.unwrap_err().contains("请先输入"));

        // A chat model picked as the speech model.
        let r = req("MiniMax-M3", "Hi.", serde_json::json!({}));
        let err = run(&provider_at(dead), "k", &r).await.unwrap_err();
        assert!(err.contains("不是 MiniMax 的语音合成模型"), "{err}");

        // Only speech is implemented.
        let mut r = req("speech-2.8-hd", "Hi.", serde_json::json!({}));
        r.kind = MediaKind::Music;
        assert!(run(&provider_at(dead), "k", &r).await.unwrap_err().contains("只接入了语音合成"));
    }

    /// A real call, for the person holding a key. Ignored by default — it is a
    /// probe, not an assertion, and it spends a few characters of quota.
    ///
    /// ```text
    /// ARGUS_MINIMAX_KEY=sk-… cargo test --lib minimax_media::tests::live_probe -- --ignored --nocapture
    /// ```
    ///
    /// Optional: `ARGUS_MINIMAX_BASE` (default `https://api.minimax.cn/v1`),
    /// `ARGUS_MINIMAX_MODEL` (default `speech-2.8-turbo`), `ARGUS_MINIMAX_VOICE`,
    /// and `ARGUS_TTS_OUT` — a path to write the mp3 to, to listen to it.
    #[tokio::test]
    #[ignore]
    async fn live_probe() {
        let key = std::env::var("ARGUS_MINIMAX_KEY").expect("set ARGUS_MINIMAX_KEY");
        let base = std::env::var("ARGUS_MINIMAX_BASE")
            .unwrap_or_else(|_| "https://api.minimax.cn/v1".to_string());
        let model = std::env::var("ARGUS_MINIMAX_MODEL").unwrap_or_else(|_| "speech-2.8-turbo".into());
        let mut options = serde_json::json!({});
        if let Ok(v) = std::env::var("ARGUS_MINIMAX_VOICE") {
            options["voice_custom"] = serde_json::json!(v);
        }
        let r = req(
            &model,
            "Attention is all you need. 这是一次朗读功能的连通性测试。",
            options,
        );
        let result = run(&provider_at(&base), &key, &r)
            .await
            .unwrap_or_else(|e| panic!("live call failed: {e}"));
        let a = &result.artifacts[0];
        let (mime, bytes) = crate::media::split_data_url(a.data_url.as_deref().unwrap()).unwrap();
        eprintln!("{mime}, {} bytes, {:?}", bytes.len(), a.text);
        assert!(bytes.len() > 1000, "suspiciously small audio");
        if let Ok(path) = std::env::var("ARGUS_TTS_OUT") {
            std::fs::write(&path, &bytes).unwrap();
            eprintln!("wrote {path}");
        }
    }
}
