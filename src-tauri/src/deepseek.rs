//! DeepSeek multimodal support: the Files API and the image plumbing its vision
//! models need.
//!
//! DeepSeek speaks the OpenAI-compatible dialect, so ordinary text chat needs
//! nothing special here. Images do, because DeepSeek layers its own rules on top
//! of the shared shape:
//!
//!   * images may only ride `user` messages — a `system`/`assistant` message
//!     carrying one is a hard `400`;
//!   * an inline (base64 / URL) image is capped at 32 MiB, while one uploaded
//!     through the Files API may be 64 MiB and is referenced by `file_id`;
//!   * only JPEG, PNG, GIF and WebP are accepted, sniffed from the *content*
//!     rather than the filename or the data URI's declared media type;
//!   * a single edge may not exceed 8192 px, dropping to 4096 px once a request
//!     carries 15 or more images.
//!
//! [`prepare_chat_messages`] enforces those rules before the request leaves the
//! machine, so an oversized or unsupported attachment produces a sentence the
//! user can act on instead of a bare `400` from the API. When an image is too
//! large to inline it is uploaded to the Files API and swapped for its handle,
//! which is the only way such a request can succeed at all.
//!
//! Reference: <https://api-docs.deepseek.com/zh-cn/guides/vision>

use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::models::AiProvider;

// ── Documented limits ────────────────────────────────────────────────────────

/// Whole request body, base64 expansion included.
pub const MAX_REQUEST_BODY_BYTES: u64 = 48 * 1024 * 1024;
/// One image carried inline, as base64 or fetched from a URL.
pub const MAX_INLINE_IMAGE_BYTES: u64 = 32 * 1024 * 1024;
/// One image uploaded through the Files API.
pub const MAX_FILE_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Images in a single request.
pub const MAX_IMAGES_PER_REQUEST: usize = 600;
/// Combined image bytes when none of them are `file_id` references.
pub const MAX_TOTAL_INLINE_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Combined image bytes once the Files API is in play.
pub const MAX_TOTAL_IMAGE_BYTES_WITH_FILES: u64 = 200 * 1024 * 1024;
/// External image URLs longer than this are rejected.
pub const MAX_IMAGE_URL_LEN: usize = 8192;
/// Longest edge, normally.
pub const MAX_IMAGE_EDGE: u32 = 8192;
/// Longest edge once the request is image-heavy.
pub const MAX_IMAGE_EDGE_MANY: u32 = 4096;
/// The image count at which the tighter edge limit kicks in.
pub const MANY_IMAGES_THRESHOLD: usize = 15;
/// Ceiling on what one image can cost, whatever its resolution.
pub const MAX_TOKENS_PER_IMAGE: u64 = 384;

/// Formats DeepSeek decodes, in the order they are sniffed.
pub const SUPPORTED_IMAGE_MIMES: [&str; 4] =
    ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// Values accepted by `image_url.detail`.
pub const DETAIL_VALUES: [&str; 4] = ["low", "high", "original", "auto"];

/// How long an auto-uploaded image stays on DeepSeek's side. These uploads are a
/// transport detail the user never asked for, so they expire on their own rather
/// than accumulating in the account forever; a week is long enough that
/// re-asking about the same image in a follow-up turn still hits the handle.
const AUTO_UPLOAD_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;

/// Files API expiry bounds, from the upload endpoint.
pub const MIN_EXPIRES_AFTER_SECONDS: u64 = 3_600;
pub const MAX_EXPIRES_AFTER_SECONDS: u64 = 2_592_000;

// ── Provider / model predicates ──────────────────────────────────────────────

/// Whether `model` on `provider` can be sent images.
///
/// The catalogue is consulted first: `/models` returns bare ids, so Argus's own
/// capability list is the better signal. An id that is not in the list at all
/// (hand-added by the user) falls back to the naming convention rather than
/// being refused — being wrong here would block a working call.
pub fn model_supports_vision(provider: &AiProvider, model: &str) -> bool {
    if let Some(entry) = provider.models.iter().find(|m| m.id == model) {
        if entry.capabilities.iter().any(|c| {
            let c = c.to_lowercase();
            c.contains("vision") || c.contains("image") || c.contains("multimodal")
        }) {
            return true;
        }
        // A catalogued model with no capabilities recorded tells us nothing;
        // fall through to the id. One with other capabilities listed and no
        // vision among them is a genuine "text only".
        if !entry.capabilities.is_empty() {
            return looks_like_vision_id(model);
        }
    }
    looks_like_vision_id(model)
}

fn looks_like_vision_id(model: &str) -> bool {
    let id = model.to_lowercase();
    id.contains("vision") || id.contains("-vl") || id.contains("vl-") || id.contains("omni")
}

// ── Image sniffing ───────────────────────────────────────────────────────────

/// Media type read from the leading magic bytes. DeepSeek decides an image's
/// format the same way, so a PNG named `.jpg` — or a data URI that mislabels its
/// payload — is judged on the bytes rather than the label.
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// `(width, height)` in pixels, or `None` when the header is truncated or the
/// format is one we do not parse. Only the file header is read, so passing a
/// prefix of a large image is enough — and a `None` simply skips the dimension
/// check rather than failing the request.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match sniff_image_mime(bytes)? {
        "image/png" => png_dimensions(bytes),
        "image/gif" => gif_dimensions(bytes),
        "image/jpeg" => jpeg_dimensions(bytes),
        "image/webp" => webp_dimensions(bytes),
        _ => None,
    }
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn be_u16(b: &[u8]) -> u32 {
    u32::from(u16::from_be_bytes([b[0], b[1]]))
}

fn le_u16(b: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([b[0], b[1]]))
}

fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // 8-byte signature, 4-byte chunk length, "IHDR", width, height.
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    Some((be_u32(&bytes[16..20]), be_u32(&bytes[20..24])))
}

fn gif_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 10 {
        return None;
    }
    Some((le_u16(&bytes[6..8]), le_u16(&bytes[8..10])))
}

fn jpeg_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // Walk the marker chain to the first Start-Of-Frame, whose payload carries
    // the frame size. Segments before it (EXIF, quantisation tables, an embedded
    // thumbnail) are skipped by their declared length.
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // Padding between segments, and the standalone markers that carry no
        // length field.
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if marker == 0xD8 || (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        let len = be_u16(&bytes[i + 2..i + 4]) as usize;
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && marker != 0xC4  // Huffman tables
            && marker != 0xC8  // JPEG extensions
            && marker != 0xCC; // arithmetic coding conditioning
        if is_sof {
            if i + 9 > bytes.len() {
                return None;
            }
            let height = be_u16(&bytes[i + 5..i + 7]);
            let width = be_u16(&bytes[i + 7..i + 9]);
            return Some((width, height));
        }
        if len < 2 {
            return None;
        }
        i += 2 + len;
    }
    None
}

fn webp_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    // RIFF header is 12 bytes; the first chunk's fourcc follows.
    if bytes.len() < 30 {
        return None;
    }
    match &bytes[12..16] {
        b"VP8 " => {
            // Lossy: 3-byte frame tag, 3-byte start code, then 14-bit dimensions.
            if bytes[23..26] != [0x9D, 0x01, 0x2A] {
                return None;
            }
            Some((le_u16(&bytes[26..28]) & 0x3FFF, le_u16(&bytes[28..30]) & 0x3FFF))
        }
        b"VP8L" => {
            // Lossless: signature byte, then 14 bits width-1 and 14 bits height-1
            // packed little-endian across four bytes.
            if bytes[20] != 0x2F {
                return None;
            }
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
        }
        b"VP8X" => {
            // Extended: 24-bit canvas width-1 / height-1, little-endian.
            let w = u32::from(bytes[24]) | u32::from(bytes[25]) << 8 | u32::from(bytes[26]) << 16;
            let h = u32::from(bytes[27]) | u32::from(bytes[28]) << 8 | u32::from(bytes[29]) << 16;
            Some((w + 1, h + 1))
        }
        _ => None,
    }
}

/// Decoded byte length of a base64 payload, without decoding it. Used to check
/// an attachment against the size limits before paying to materialise it.
pub fn base64_decoded_len(payload: &str) -> u64 {
    let len = payload.chars().filter(|c| !c.is_whitespace()).count() as u64;
    let padding = payload.trim_end().chars().rev().take_while(|c| *c == '=').count() as u64;
    len.saturating_mul(3) / 4 - padding.min(2)
}

/// What one image costs. DeepSeek rescales every image to roughly 800x800 before
/// it reaches the model and caps the result, so a 2000x2000 and a 5000x5000
/// image bill identically.
pub fn estimate_image_tokens(detail: Option<&str>) -> u64 {
    match detail {
        // `low` rescales to 512x512, which is under the cap.
        Some("low") => (512 * 512 * MAX_TOKENS_PER_IMAGE) / (800 * 800),
        _ => MAX_TOKENS_PER_IMAGE,
    }
}

/// Every documented limit in one payload, so the UI can render the rules and
/// pre-flight an attachment against the same numbers the request path enforces
/// instead of keeping a second copy of them in TypeScript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VisionLimits {
    pub max_request_body_bytes: u64,
    pub max_inline_image_bytes: u64,
    pub max_file_image_bytes: u64,
    pub max_images_per_request: usize,
    pub max_total_inline_image_bytes: u64,
    pub max_total_image_bytes_with_files: u64,
    pub max_image_url_len: usize,
    pub max_image_edge: u32,
    pub max_image_edge_many: u32,
    pub many_images_threshold: usize,
    pub supported_mimes: Vec<String>,
    pub detail_values: Vec<String>,
    /// Tokens one image costs at `detail: "low"` and at full resolution.
    pub tokens_per_image_low: u64,
    pub tokens_per_image_original: u64,
    pub min_expires_after_seconds: u64,
    pub max_expires_after_seconds: u64,
}

pub fn vision_limits() -> VisionLimits {
    VisionLimits {
        max_request_body_bytes: MAX_REQUEST_BODY_BYTES,
        max_inline_image_bytes: MAX_INLINE_IMAGE_BYTES,
        max_file_image_bytes: MAX_FILE_IMAGE_BYTES,
        max_images_per_request: MAX_IMAGES_PER_REQUEST,
        max_total_inline_image_bytes: MAX_TOTAL_INLINE_IMAGE_BYTES,
        max_total_image_bytes_with_files: MAX_TOTAL_IMAGE_BYTES_WITH_FILES,
        max_image_url_len: MAX_IMAGE_URL_LEN,
        max_image_edge: MAX_IMAGE_EDGE,
        max_image_edge_many: MAX_IMAGE_EDGE_MANY,
        many_images_threshold: MANY_IMAGES_THRESHOLD,
        supported_mimes: SUPPORTED_IMAGE_MIMES.iter().map(|s| s.to_string()).collect(),
        detail_values: DETAIL_VALUES.iter().map(|s| s.to_string()).collect(),
        tokens_per_image_low: estimate_image_tokens(Some("low")),
        tokens_per_image_original: estimate_image_tokens(None),
        min_expires_after_seconds: MIN_EXPIRES_AFTER_SECONDS,
        max_expires_after_seconds: MAX_EXPIRES_AFTER_SECONDS,
    }
}

// ── Files API ────────────────────────────────────────────────────────────────

/// One file object, as returned by upload / retrieve / list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSeekFile {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub purpose: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// A page of [`DeepSeekFile`]s plus the cursors needed to walk the rest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSeekFileList {
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub data: Vec<DeepSeekFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_id: Option<String>,
    #[serde(default)]
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepSeekFileDeleted {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub deleted: bool,
}

/// The only `purpose` the Files API accepts today.
pub const FILE_PURPOSE: &str = "user_data";

fn files_url(provider: &AiProvider) -> String {
    format!("{}/files", provider.base_url.trim_end_matches('/'))
}

/// `POST /files` — upload one image and get back a `file-api-…` handle.
///
/// The body is assembled by hand rather than through `reqwest`'s `multipart`
/// feature: the payload is three fixed fields, and hand-rolling it keeps the
/// dependency set (and the build) exactly as it was.
pub async fn upload_file(
    provider: &AiProvider,
    api_key: &str,
    filename: &str,
    bytes: &[u8],
    expires_after_seconds: Option<u64>,
) -> Result<DeepSeekFile, String> {
    let mime = sniff_image_mime(bytes).ok_or_else(|| {
        format!(
            "「{filename}」不是 DeepSeek 支持的图片格式，仅支持 {}。",
            SUPPORTED_IMAGE_MIMES.join("、")
        )
    })?;
    if bytes.len() as u64 > MAX_FILE_IMAGE_BYTES {
        return Err(format!(
            "「{filename}」有 {}，超过 Files API 单文件 {} 的上限。",
            human_size(bytes.len() as u64),
            human_size(MAX_FILE_IMAGE_BYTES)
        ));
    }
    if let Some(seconds) = expires_after_seconds {
        if !(MIN_EXPIRES_AFTER_SECONDS..=MAX_EXPIRES_AFTER_SECONDS).contains(&seconds) {
            return Err(format!(
                "有效期需在 {MIN_EXPIRES_AFTER_SECONDS} 秒到 {MAX_EXPIRES_AFTER_SECONDS} 秒之间。"
            ));
        }
    }

    let boundary = format!("----ArgusFormBoundary{:016x}", rand_u64());
    let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 512);
    let field = |name: &str, value: &str, body: &mut Vec<u8>| {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    };
    field("purpose", FILE_PURPOSE, &mut body);
    if let Some(seconds) = expires_after_seconds {
        field("expires_after[anchor]", "created_at", &mut body);
        field("expires_after[seconds]", &seconds.to_string(), &mut body);
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\n",
            escape_quoted(filename)
        )
        .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {mime}\r\n\r\n").as_bytes());
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let resp = crate::llm::build_client()?
        .post(files_url(provider))
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Content-Type", format!("multipart/form-data; boundary={boundary}"))
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    read_json(resp).await
}

/// `GET /files` — one page of uploaded files, newest-first by default.
pub async fn list_files(
    provider: &AiProvider,
    api_key: &str,
    after: Option<&str>,
    limit: Option<u32>,
    order: Option<&str>,
) -> Result<DeepSeekFileList, String> {
    let mut query: Vec<(&str, String)> = vec![
        ("purpose", FILE_PURPOSE.to_string()),
        ("limit", limit.unwrap_or(100).clamp(1, 1000).to_string()),
        ("order", order.unwrap_or("desc").to_string()),
    ];
    if let Some(after) = after.filter(|s| !s.is_empty()) {
        query.push(("after", after.to_string()));
    }
    let resp = crate::llm::build_client()?
        .get(files_url(provider))
        .header("Authorization", format!("Bearer {api_key}"))
        .query(&query)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    read_json(resp).await
}

/// `GET /files/{file_id}` — metadata for one handle.
pub async fn retrieve_file(
    provider: &AiProvider,
    api_key: &str,
    file_id: &str,
) -> Result<DeepSeekFile, String> {
    let resp = crate::llm::build_client()?
        .get(format!("{}/{}", files_url(provider), file_id))
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    read_json(resp).await
}

/// `DELETE /files/{file_id}`.
pub async fn delete_file(
    provider: &AiProvider,
    api_key: &str,
    file_id: &str,
) -> Result<DeepSeekFileDeleted, String> {
    let resp = crate::llm::build_client()?
        .delete(format!("{}/{}", files_url(provider), file_id))
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    read_json(resp).await
}

async fn read_json<T: serde::de::DeserializeOwned>(resp: reqwest::Response) -> Result<T, String> {
    let status = resp.status().as_u16();
    let text = crate::net::fetch_text_capped(resp, 4 * 1024 * 1024).await?;
    if status >= 400 {
        return Err(crate::llm::friendly_error(status, &text));
    }
    serde_json::from_str(&text).map_err(|e| format!("Unexpected response from DeepSeek Files API: {e}"))
}

fn escape_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"").replace(['\r', '\n'], "_")
}

fn rand_u64() -> u64 {
    use rand::RngCore;
    rand::rngs::OsRng.next_u64()
}

pub fn human_size(bytes: u64) -> String {
    const MIB: f64 = 1024.0 * 1024.0;
    const KIB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= MIB {
        format!("{:.1} MiB", b / MIB)
    } else if b >= KIB {
        format!("{:.0} KiB", b / KIB)
    } else {
        format!("{bytes} B")
    }
}

// ── Auto-upload cache ────────────────────────────────────────────────────────

/// Handles minted by [`prepare_chat_messages`], keyed by a hash of the image
/// payload. An agent run replays its whole transcript every round, so without
/// this the same oversized page render would be uploaded again on each turn.
/// Entries are dropped well before the upload's own expiry so a stale handle is
/// never handed to the API.
static UPLOAD_CACHE: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<u64, (String, std::time::Instant)>>,
> = std::sync::OnceLock::new();

const UPLOAD_CACHE_TTL: std::time::Duration =
    std::time::Duration::from_secs(AUTO_UPLOAD_TTL_SECONDS - 24 * 60 * 60);

fn payload_hash(provider_id: &str, payload: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    provider_id.hash(&mut hasher);
    payload.len().hash(&mut hasher);
    payload.hash(&mut hasher);
    hasher.finish()
}

fn cached_upload(key: u64) -> Option<String> {
    let map = UPLOAD_CACHE.get_or_init(Default::default).lock().ok()?;
    map.get(&key)
        .filter(|(_, at)| at.elapsed() < UPLOAD_CACHE_TTL)
        .map(|(id, _)| id.clone())
}

fn remember_upload(key: u64, file_id: &str) {
    if let Ok(mut map) = UPLOAD_CACHE.get_or_init(Default::default).lock() {
        map.retain(|_, (_, at)| at.elapsed() < UPLOAD_CACHE_TTL);
        map.insert(key, (file_id.to_string(), std::time::Instant::now()));
    }
}

// ── Request preparation ──────────────────────────────────────────────────────

/// One inline image found in the outgoing request, remembered so the size pass
/// can decide whether it has to move to the Files API.
struct InlineImage {
    msg: usize,
    part: usize,
    /// Decoded size of the image itself, not of its base64 form.
    size: u64,
    filename: String,
}

/// Validate and, where necessary, rewrite the `messages` array of a DeepSeek
/// `/chat/completions` request so its images satisfy the documented limits.
///
/// A text-only request is returned untouched — including its `content` strings,
/// which matters because the system prompt is the prompt-cache prefix and any
/// reshaping of it would miss the cache.
///
/// What this does to a request that carries images:
///   * refuses early, with the model named, when the target model is text-only;
///   * drops image and attachment parts from `system`/`assistant` messages,
///     which DeepSeek rejects outright;
///   * rejects a PDF attachment, which DeepSeek's chat endpoint cannot read;
///   * checks each image's real format, byte size and pixel dimensions;
///   * uploads images to the Files API and swaps them for `file_id` handles when
///     the request would otherwise be over the inline or body limits.
pub async fn prepare_chat_messages(
    provider: &AiProvider,
    api_key: &str,
    model: &str,
    msgs: Vec<serde_json::Value>,
) -> Result<Vec<serde_json::Value>, String> {
    if !carries_attachments(&msgs) {
        return Ok(msgs);
    }
    if !model_supports_vision(provider, model) {
        return Err(format!(
            "模型「{model}」不支持图片输入。请在设置 → AI 服务中选择 DeepSeek 的视觉模型\
             （如 deepseek-v4-flash-vision-exp），或移除附件后重试。"
        ));
    }

    let mut msgs = msgs;
    let mut inline: Vec<InlineImage> = Vec::new();
    let mut file_id_images = 0usize;

    // Pass 1: strip what the endpoint refuses, normalise what it accepts, and
    // note every inline image so the budget pass below can act on it.
    for (msg_idx, msg) in msgs.iter_mut().enumerate() {
        let role = msg
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();
        let Some(parts) = msg.get("content").and_then(|c| c.as_array()).cloned() else {
            continue;
        };
        let is_user = role == "user";
        let mut kept: Vec<serde_json::Value> = Vec::with_capacity(parts.len());

        for part in parts {
            let kind = part.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match kind {
                "image_url" => {
                    if !is_user {
                        // A `system`/`assistant` message with an image is a 400.
                        // The text of the turn still matters, so only the image
                        // is dropped.
                        continue;
                    }
                    let idx = kept.len();
                    let normalised = normalise_image_part(&part)?;
                    if let Some(size) = normalised.inline_size {
                        inline.push(InlineImage {
                            msg: msg_idx,
                            part: idx,
                            size,
                            filename: normalised.filename,
                        });
                    }
                    kept.push(normalised.value);
                }
                "file" => {
                    if let Some(file_id) = part.get("file_id").and_then(|v| v.as_str()) {
                        if !is_user {
                            continue;
                        }
                        file_id_images += 1;
                        kept.push(serde_json::json!({ "type": "file", "file_id": file_id }));
                        continue;
                    }
                    // An inline `file` block is OpenRouter's PDF shape. DeepSeek's
                    // chat endpoint has no equivalent, and its Files API takes
                    // images only, so say so rather than let it 400.
                    let name = part
                        .get("file")
                        .and_then(|f| f.get("filename"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("附件");
                    if is_user {
                        return Err(format!(
                            "DeepSeek 不支持直接上传「{name}」这类文件，视觉模型只接受图片\
                             （{}）。请改用 OpenRouter 等支持 PDF 的服务商，或先把页面转成图片。",
                            SUPPORTED_IMAGE_MIMES.join("、")
                        ));
                    }
                }
                _ => kept.push(part),
            }
        }
        // A turn stripped down to nothing would go out as `content: []`, which
        // is not a shape the endpoint accepts; send an empty string instead.
        msg["content"] = if kept.is_empty() {
            serde_json::Value::String(String::new())
        } else {
            serde_json::Value::Array(kept)
        };
    }

    let total_images = inline.len() + file_id_images;
    if total_images > MAX_IMAGES_PER_REQUEST {
        return Err(format!(
            "本次请求带了 {total_images} 张图片，超过 DeepSeek 单次 {MAX_IMAGES_PER_REQUEST} 张的上限。"
        ));
    }

    // The edge limit tightens once a request is image-heavy, so it can only be
    // checked after the count is known.
    let max_edge = if total_images >= MANY_IMAGES_THRESHOLD {
        MAX_IMAGE_EDGE_MANY
    } else {
        MAX_IMAGE_EDGE
    };
    for image in &inline {
        let Some(url) = image_url_of(&msgs, image) else { continue };
        let Some((_, payload)) = split_data_uri(url) else { continue };
        let header = decode_b64_prefix(payload, 96 * 1024);
        if let Some((w, h)) = image_dimensions(&header) {
            if w.max(h) > max_edge {
                return Err(format!(
                    "「{}」为 {w}×{h}，超过单边 {max_edge} 像素的上限{}。请先缩小图片。",
                    image.filename,
                    if max_edge == MAX_IMAGE_EDGE_MANY {
                        format!("（单次请求含 {MANY_IMAGES_THRESHOLD} 张以上图片时收紧到该值）")
                    } else {
                        String::new()
                    }
                ));
            }
        }
    }

    // Pass 2: move images to the Files API, largest first, until what remains
    // inline fits both the per-image cap and the request-body budget.
    let mut order: Vec<usize> = (0..inline.len()).collect();
    order.sort_by(|a, b| inline[*b].size.cmp(&inline[*a].size));
    let mut inline_bytes: u64 = inline.iter().map(|i| i.size).sum();
    let mut uploaded_bytes: u64 = 0;

    for i in order {
        let must_upload = inline[i].size > MAX_INLINE_IMAGE_BYTES;
        let over_total = inline_bytes > MAX_TOTAL_INLINE_IMAGE_BYTES;
        let over_body = encoded_len(inline_bytes) > body_budget();
        if !(must_upload || over_total || over_body) {
            break;
        }
        let file_id = upload_inline_image(provider, api_key, &msgs, &inline[i]).await?;
        let slot = &inline[i];
        msgs[slot.msg]["content"][slot.part] =
            serde_json::json!({ "type": "file", "file_id": file_id });
        inline_bytes = inline_bytes.saturating_sub(slot.size);
        uploaded_bytes += slot.size;
    }

    // The loop above always brings the inline share back inside both caps, so
    // what is left to check is the one limit uploading cannot fix.
    let grand_total = inline_bytes + uploaded_bytes;
    if uploaded_bytes > 0 && grand_total > MAX_TOTAL_IMAGE_BYTES_WITH_FILES {
        return Err(format!(
            "本次请求的图片合计 {}，超过 DeepSeek 单次 {} 的上限（含 Files API 引用）。",
            human_size(grand_total),
            human_size(MAX_TOTAL_IMAGE_BYTES_WITH_FILES)
        ));
    }

    Ok(msgs)
}

/// Leave a megabyte of the body budget for the text, tool definitions and JSON
/// scaffolding that travel alongside the images.
fn body_budget() -> u64 {
    MAX_REQUEST_BODY_BYTES.saturating_sub(1024 * 1024)
}

/// Size of `raw` once base64-encoded.
fn encoded_len(raw: u64) -> u64 {
    raw.div_ceil(3).saturating_mul(4)
}

fn carries_attachments(msgs: &[serde_json::Value]) -> bool {
    msgs.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_array())
            .is_some_and(|parts| {
                parts.iter().any(|p| {
                    matches!(
                        p.get("type").and_then(|t| t.as_str()),
                        Some("image_url") | Some("file")
                    )
                })
            })
    })
}

struct NormalisedImage {
    value: serde_json::Value,
    /// `Some(size)` for a base64 image carried in the body; `None` for a remote
    /// URL, whose bytes never pass through this process.
    inline_size: Option<u64>,
    filename: String,
}

/// Check one `image_url` part and return it in the canonical object form.
fn normalise_image_part(part: &serde_json::Value) -> Result<NormalisedImage, String> {
    // Accept both `{"image_url": {"url": …}}` and the bare-string spelling the
    // Responses API uses, so a part built for either protocol is understood.
    let (url, detail) = match part.get("image_url") {
        Some(serde_json::Value::String(s)) => (s.clone(), None),
        Some(serde_json::Value::Object(obj)) => (
            obj.get("url")
                .and_then(|v| v.as_str())
                .ok_or("图片内容块缺少 url 字段。")?
                .to_string(),
            obj.get("detail").and_then(|v| v.as_str()).map(str::to_string),
        ),
        _ => return Err("图片内容块的 image_url 字段格式不正确。".to_string()),
    };
    let detail = detail.or_else(|| part.get("detail").and_then(|v| v.as_str()).map(str::to_string));
    if let Some(d) = &detail {
        if !DETAIL_VALUES.contains(&d.as_str()) {
            return Err(format!(
                "图片精度 detail=\"{d}\" 无效，可选值为 {}。",
                DETAIL_VALUES.join("、")
            ));
        }
    }

    let mut image_url = serde_json::Map::new();
    let filename;

    if let Some((declared, payload)) = split_data_uri(&url) {
        let size = base64_decoded_len(payload);
        let header = decode_b64_prefix(payload, 4 * 1024);
        let sniffed = sniff_image_mime(&header).ok_or_else(|| {
            format!(
                "附件不是 DeepSeek 支持的图片格式（声明为 {declared}），仅支持 {}。",
                SUPPORTED_IMAGE_MIMES.join("、")
            )
        })?;
        filename = format!("image{}", extension_for(sniffed));
        if size > MAX_FILE_IMAGE_BYTES {
            return Err(format!(
                "单张图片 {} 超过 DeepSeek {} 的上限。",
                human_size(size),
                human_size(MAX_FILE_IMAGE_BYTES)
            ));
        }
        image_url.insert("url".into(), serde_json::Value::String(url));
        if let Some(d) = detail {
            image_url.insert("detail".into(), serde_json::Value::String(d));
        }
        return Ok(NormalisedImage {
            value: serde_json::json!({ "type": "image_url", "image_url": image_url }),
            inline_size: Some(size),
            filename,
        });
    }

    // Remote URL: DeepSeek fetches it itself, so only the shape is ours to check.
    if url.len() > MAX_IMAGE_URL_LEN {
        return Err(format!(
            "图片链接长度 {} 超过 DeepSeek {MAX_IMAGE_URL_LEN} 字符的上限。",
            url.len()
        ));
    }
    crate::net::validate_public_http_url(&url).map_err(|e| {
        format!("图片链接无法被 DeepSeek 访问：{e} 请改为直接上传图片。")
    })?;
    filename = url
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("image")
        .to_string();
    image_url.insert("url".into(), serde_json::Value::String(url));
    if let Some(d) = detail {
        image_url.insert("detail".into(), serde_json::Value::String(d));
    }
    Ok(NormalisedImage {
        value: serde_json::json!({ "type": "image_url", "image_url": image_url }),
        inline_size: None,
        filename,
    })
}

fn image_url_of<'a>(msgs: &'a [serde_json::Value], slot: &InlineImage) -> Option<&'a str> {
    msgs.get(slot.msg)?
        .get("content")?
        .get(slot.part)?
        .get("image_url")?
        .get("url")?
        .as_str()
}

async fn upload_inline_image(
    provider: &AiProvider,
    api_key: &str,
    msgs: &[serde_json::Value],
    slot: &InlineImage,
) -> Result<String, String> {
    let url = image_url_of(msgs, slot).ok_or("内部错误：找不到待上传的图片。")?;
    let (_, payload) = split_data_uri(url).ok_or("内部错误：图片不是 data URI。")?;
    let key = payload_hash(&provider.id, payload);
    if let Some(file_id) = cached_upload(key) {
        return Ok(file_id);
    }
    if api_key.is_empty() {
        return Err(format!(
            "「{}」有 {}，需要先上传到 DeepSeek Files API，但该服务商未配置 API Key。",
            slot.filename,
            human_size(slot.size)
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload.replace(['\n', '\r'], ""))
        .map_err(|e| format!("图片 base64 解码失败：{e}"))?;
    let file = upload_file(
        provider,
        api_key,
        &slot.filename,
        &bytes,
        Some(AUTO_UPLOAD_TTL_SECONDS),
    )
    .await?;
    remember_upload(key, &file.id);
    Ok(file.id)
}

/// Split `data:<mime>;base64,<payload>` into its media type and payload.
fn split_data_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    Some((meta.split(';').next().unwrap_or("application/octet-stream"), payload))
}

/// Decode at most `max_chars` of base64 — enough for a format signature and a
/// dimension header without materialising a 30 MiB image.
fn decode_b64_prefix(payload: &str, max_chars: usize) -> Vec<u8> {
    let cleaned: String = payload
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(max_chars)
        .collect();
    let keep = cleaned.len() - (cleaned.len() % 4);
    base64::engine::general_purpose::STANDARD
        .decode(&cleaned[..keep])
        .unwrap_or_default()
}

fn extension_for(mime: &str) -> &'static str {
    match mime {
        "image/png" => ".png",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        _ => ".jpg",
    }
}

// ── Responses API ────────────────────────────────────────────────────────────

/// Rewrite a prepared `/chat/completions` message array into the `instructions`
/// + `input` pair the Responses API takes.
///
/// That endpoint speaks its own vocabulary: `input_text` / `input_image` instead
/// of `text` / `image_url`, the image URL as a bare string rather than an
/// object, and a Files API handle as `file_id` on the image block itself. System
/// turns are not messages there at all — they become the top-level
/// `instructions` string.
///
/// Runs on the output of [`prepare_chat_messages`], so the images reaching it
/// have already been checked and, where needed, moved to the Files API.
pub fn to_responses_input(msgs: &[serde_json::Value]) -> (String, Vec<serde_json::Value>) {
    let mut instructions = String::new();
    let mut input: Vec<serde_json::Value> = Vec::new();

    for m in msgs {
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = m.get("content");

        if role == "system" {
            let text = flatten_text(content);
            if text.trim().is_empty() {
                continue;
            }
            if !instructions.is_empty() {
                instructions.push_str("\n\n");
            }
            instructions.push_str(&text);
            continue;
        }

        // Only a user turn may carry images; everything else is flattened to
        // text, which is all the endpoint would accept from it anyway.
        let blocks: Option<Vec<serde_json::Value>> = if role == "user" {
            content
                .and_then(|c| c.as_array())
                .map(|parts| parts.iter().filter_map(to_responses_block).collect())
        } else {
            None
        };

        match blocks {
            Some(blocks) if blocks.iter().any(|b| b["type"] != "input_text") => {
                input.push(serde_json::json!({ "role": role, "content": blocks }));
            }
            _ => {
                let text = flatten_text(content);
                input.push(serde_json::json!({ "role": role, "content": text }));
            }
        }
    }

    (instructions, input)
}

fn to_responses_block(part: &serde_json::Value) -> Option<serde_json::Value> {
    match part.get("type").and_then(|t| t.as_str())? {
        "text" => Some(serde_json::json!({
            "type": "input_text",
            "text": part.get("text").and_then(|t| t.as_str()).unwrap_or(""),
        })),
        "image_url" => {
            let image_url = part.get("image_url")?;
            let url = match image_url {
                serde_json::Value::String(s) => s.as_str(),
                other => other.get("url")?.as_str()?,
            };
            let mut block = serde_json::json!({ "type": "input_image", "image_url": url });
            if let Some(detail) = image_url.get("detail").and_then(|d| d.as_str()) {
                block["detail"] = serde_json::json!(detail);
            }
            Some(block)
        }
        // A Files API handle rides the image block rather than a block of its own.
        "file" => part
            .get("file_id")
            .and_then(|v| v.as_str())
            .map(|id| serde_json::json!({ "type": "input_image", "file_id": id })),
        _ => None,
    }
}

/// Every text fragment of a message, joined — the shape used for the turns that
/// cannot carry blocks.
fn flatten_text(content: Option<&serde_json::Value>) -> String {
    match content {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(parts)) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn provider(models: Vec<(&str, Vec<&str>)>) -> AiProvider {
        AiProvider {
            id: "p1".into(),
            name: "DeepSeek".into(),
            kind: "openai_compatible".into(),
            base_url: "https://api.deepseek.com/v1".into(),
            enabled: true,
            models: models
                .into_iter()
                .map(|(id, caps)| {
                    serde_json::from_value(serde_json::json!({
                        "id": id,
                        "display_name": id,
                        "capabilities": caps,
                        "context_length": 131072,
                    }))
                    .unwrap()
                })
                .collect(),
            server_tools: Default::default(),
        created_at: "2026-01-01T00:00:00Z".into(),
        }
    }

    /// A 1x1 PNG, and the same bytes as a data URI.
    fn png_bytes() -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend_from_slice(&[0, 0, 0, 13]);
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&1u32.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v
    }

    fn png_data_uri() -> String {
        format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png_bytes())
        )
    }

    fn user_with_image(uri: &str) -> serde_json::Value {
        serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "这张图里有什么？"},
                {"type": "image_url", "image_url": {"url": uri}},
            ]
        })
    }

    #[test]
    fn sniffs_formats_from_content_not_labels() {
        assert_eq!(sniff_image_mime(&png_bytes()), Some("image/png"));
        assert_eq!(sniff_image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff_image_mime(b"GIF89a\x10\x00\x08\x00"), Some("image/gif"));
        let mut webp = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        webp.extend_from_slice(b"VP8 ");
        assert_eq!(sniff_image_mime(&webp), Some("image/webp"));
        assert_eq!(sniff_image_mime(b"%PDF-1.7"), None);
    }

    #[test]
    fn reads_dimensions_from_each_header() {
        let mut png = png_bytes();
        png[16..20].copy_from_slice(&4096u32.to_be_bytes());
        png[20..24].copy_from_slice(&2048u32.to_be_bytes());
        assert_eq!(image_dimensions(&png), Some((4096, 2048)));

        // GIF stores its logical screen size little-endian at offset 6.
        let gif = b"GIF89a\x10\x00\x08\x00rest".to_vec();
        assert_eq!(image_dimensions(&gif), Some((16, 8)));

        // JPEG: an APP0 segment, skipped by its length, then SOF0.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
        jpeg.extend_from_slice(&600u16.to_be_bytes()); // height
        jpeg.extend_from_slice(&800u16.to_be_bytes()); // width
        jpeg.extend_from_slice(&[0; 8]);
        assert_eq!(image_dimensions(&jpeg), Some((800, 600)));
    }

    #[test]
    fn webp_lossy_dimensions() {
        let mut w = b"RIFF\x00\x00\x00\x00WEBPVP8 ".to_vec();
        w.extend_from_slice(&[0; 3]); // chunk size tail
        w.extend_from_slice(&[0, 0, 0, 0]); // frame tag
        w.extend_from_slice(&[0x9D, 0x01, 0x2A]);
        w.extend_from_slice(&320u16.to_le_bytes());
        w.extend_from_slice(&240u16.to_le_bytes());
        assert_eq!(image_dimensions(&w), Some((320, 240)));
    }

    #[test]
    fn base64_length_matches_a_real_decode() {
        for len in [1usize, 2, 3, 4, 100, 1000] {
            let raw = vec![7u8; len];
            let encoded = base64::engine::general_purpose::STANDARD.encode(&raw);
            assert_eq!(base64_decoded_len(&encoded), len as u64, "len={len}");
        }
    }

    #[test]
    fn vision_is_read_from_the_catalogue_then_the_id() {
        let p = provider(vec![
            ("deepseek-v4-flash-vision-exp", vec!["vision"]),
            ("deepseek-v4-pro", vec!["reasoning", "tool_calling"]),
            ("unknown-model", vec![]),
        ]);
        assert!(model_supports_vision(&p, "deepseek-v4-flash-vision-exp"));
        assert!(!model_supports_vision(&p, "deepseek-v4-pro"));
        // Not catalogued, but named like a vision model: allowed rather than
        // blocked, since a hand-added id carries no capability list.
        assert!(model_supports_vision(&p, "deepseek-v9-vision"));
        assert!(!model_supports_vision(&p, "unknown-model"));
    }

    /// A text-only request must come back byte-identical: the system prompt is
    /// the prompt-cache prefix, and reshaping it would miss the cache.
    #[tokio::test]
    async fn text_only_requests_are_untouched() {
        let p = provider(vec![("deepseek-v4-pro", vec!["reasoning"])]);
        let msgs = vec![
            serde_json::json!({"role": "system", "content": "long cached paper context"}),
            serde_json::json!({"role": "user", "content": "总结一下"}),
        ];
        let out = prepare_chat_messages(&p, "sk-test", "deepseek-v4-pro", msgs.clone())
            .await
            .unwrap();
        assert_eq!(out, msgs);
    }

    #[tokio::test]
    async fn text_only_model_refuses_images_before_the_request_is_sent() {
        let p = provider(vec![("deepseek-v4-pro", vec!["reasoning"])]);
        let err = prepare_chat_messages(
            &p,
            "sk-test",
            "deepseek-v4-pro",
            vec![user_with_image(&png_data_uri())],
        )
        .await
        .unwrap_err();
        assert!(err.contains("deepseek-v4-pro"), "{err}");
    }

    #[tokio::test]
    async fn images_are_stripped_from_system_and_assistant_turns() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let msgs = vec![
            serde_json::json!({
                "role": "system",
                "content": [
                    {"type": "text", "text": "instructions"},
                    {"type": "image_url", "image_url": {"url": png_data_uri()}},
                ]
            }),
            user_with_image(&png_data_uri()),
        ];
        let out = prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", msgs)
            .await
            .unwrap();
        assert_eq!(out[0]["content"].as_array().unwrap().len(), 1);
        assert_eq!(out[0]["content"][0]["type"], "text");
        // The user turn keeps its image.
        assert_eq!(out[1]["content"][1]["type"], "image_url");
    }

    #[tokio::test]
    async fn pdf_attachments_are_named_in_the_error() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let msgs = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "读一下"},
                {"type": "file", "file": {"filename": "paper.pdf", "file_data": "data:application/pdf;base64,JVBERi0="}},
            ]
        })];
        let err = prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", msgs)
            .await
            .unwrap_err();
        assert!(err.contains("paper.pdf"), "{err}");
    }

    #[tokio::test]
    async fn a_non_image_payload_is_rejected_whatever_it_claims_to_be() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let uri = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(b"%PDF-1.7 not an image")
        );
        let err = prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", vec![user_with_image(&uri)])
            .await
            .unwrap_err();
        assert!(err.contains("image/jpeg"), "{err}");
    }

    #[tokio::test]
    async fn oversized_dimensions_are_reported_with_the_actual_size() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let mut png = png_bytes();
        png[16..20].copy_from_slice(&9000u32.to_be_bytes());
        png[20..24].copy_from_slice(&100u32.to_be_bytes());
        let uri = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(&png)
        );
        let err = prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", vec![user_with_image(&uri)])
            .await
            .unwrap_err();
        assert!(err.contains("9000×100"), "{err}");
    }

    #[tokio::test]
    async fn detail_is_validated_and_preserved() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let msgs = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": png_data_uri(), "detail": "low"}},
            ]
        })];
        let out = prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", msgs)
            .await
            .unwrap();
        assert_eq!(out[0]["content"][0]["image_url"]["detail"], "low");

        let bad = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "image_url", "image_url": {"url": png_data_uri(), "detail": "medium"}},
            ]
        })];
        assert!(
            prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", bad)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn file_id_references_pass_through_on_user_turns() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let msgs = vec![serde_json::json!({
            "role": "user",
            "content": [
                {"type": "text", "text": "看图"},
                {"type": "file", "file_id": "file-api-0a1b2c3d"},
            ]
        })];
        let out = prepare_chat_messages(&p, "sk-test", "deepseek-v4-flash-vision-exp", msgs)
            .await
            .unwrap();
        assert_eq!(out[0]["content"][1]["file_id"], "file-api-0a1b2c3d");
    }

    #[tokio::test]
    async fn unreachable_image_links_are_refused_locally() {
        let p = provider(vec![("deepseek-v4-flash-vision-exp", vec!["vision"])]);
        let err = prepare_chat_messages(
            &p,
            "sk-test",
            "deepseek-v4-flash-vision-exp",
            vec![user_with_image("http://localhost:8080/fig.png")],
        )
        .await
        .unwrap_err();
        assert!(err.contains("无法被 DeepSeek 访问"), "{err}");
    }

    #[test]
    fn responses_input_splits_instructions_from_blocks() {
        let msgs = vec![
            serde_json::json!({"role": "system", "content": "be brief"}),
            serde_json::json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "这是什么"},
                    {"type": "image_url", "image_url": {"url": "https://x.test/a.png", "detail": "low"}},
                    {"type": "file", "file_id": "file-api-99"},
                ]
            }),
            serde_json::json!({"role": "assistant", "content": "一张图"}),
        ];
        let (instructions, input) = to_responses_input(&msgs);
        assert_eq!(instructions, "be brief");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[0]["content"][1]["type"], "input_image");
        assert_eq!(input[0]["content"][1]["image_url"], "https://x.test/a.png");
        assert_eq!(input[0]["content"][1]["detail"], "low");
        assert_eq!(input[0]["content"][2]["file_id"], "file-api-99");
        // A turn that cannot carry blocks stays a plain string.
        assert_eq!(input[1]["content"], "一张图");
    }

    #[test]
    fn many_images_tighten_the_edge_limit() {
        assert!(MANY_IMAGES_THRESHOLD < MAX_IMAGES_PER_REQUEST);
        assert!(MAX_IMAGE_EDGE_MANY < MAX_IMAGE_EDGE);
    }
}
