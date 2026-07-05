//! Ebook (EPUB / MOBI / AZW3 / FB2 / TXT) support.
//!
//! Every format is normalized into one internal representation
//! ([`ParsedEbook`]): an ordered list of chapters holding sanitize-ready HTML
//! plus a table of contents and document metadata. The frontend viewer, the
//! fulltext pipeline (search / RAG / AI summary) and the sections system all
//! consume that representation — none of them know about concrete formats.
//!
//! PDF papers never enter this module; callers must branch on
//! `PaperMeta.file_type` before calling in.

mod epub;
mod fb2;
mod mobi_fmt;
mod txt;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::sections::{PaperSection, PaperSections};

/// Supported ebook extensions, in priority order when a paper directory
/// contains more than one candidate file.
pub const EBOOK_EXTENSIONS: [&str; 5] = ["epub", "mobi", "azw3", "fb2", "txt"];

/// Companion files living in the paper dir that must never be mistaken for
/// the book itself.
const EXCLUDED_FILES: [&str; 2] = ["fulltext.txt", "notes.md"];

// ── Wire / storage types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbookMeta {
    pub title: String,
    #[serde(default)]
    pub authors: Vec<String>,
    pub year: Option<u32>,
    pub language: Option<String>,
    pub publisher: Option<String>,
    pub identifier: Option<String>,
    pub description: Option<String>,
}

impl Default for EbookMeta {
    fn default() -> Self {
        EbookMeta {
            title: String::new(),
            authors: vec![],
            year: None,
            language: None,
            publisher: None,
            identifier: None,
            description: None,
        }
    }
}

/// One table-of-contents entry, pointing at a chapter (1-based spine index).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbookTocEntry {
    pub title: String,
    /// 1 = top-level, 2 = nested, 3 = deeper (clamped).
    pub level: u8,
    /// 1-based chapter (spine) index.
    pub chapter: u32,
    /// Optional intra-chapter element id.
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbookChapterMeta {
    /// 1-based chapter index.
    pub index: u32,
    pub title: Option<String>,
    /// Plain-text character count — the viewer uses it to estimate shell
    /// heights before a chapter is lazily rendered.
    pub char_count: u32,
}

/// What the viewer needs up front: metadata + TOC + chapter list (no HTML).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbookManifest {
    pub format: String,
    pub meta: EbookMeta,
    pub toc: Vec<EbookTocEntry>,
    pub chapters: Vec<EbookChapterMeta>,
}

/// A binary resource (image) returned to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct EbookResource {
    pub mime: String,
    pub base64: String,
}

// ── Internal representation ───────────────────────────────────────────────────

/// Where a resource's bytes live. EPUB images stay in the zip (re-read on
/// demand so image-heavy books don't sit in memory); FB2/MOBI images are
/// small embedded blobs kept inline.
#[derive(Debug, Clone)]
pub enum ResourceData {
    ZipEntry { inner_path: String },
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct ResourceEntry {
    pub mime: String,
    pub data: ResourceData,
}

/// The normalized in-memory form of a parsed book.
#[derive(Debug)]
pub struct ParsedEbook {
    pub format: String,
    /// Absolute path of the book file this was parsed from.
    pub book_path: PathBuf,
    pub meta: EbookMeta,
    pub toc: Vec<EbookTocEntry>,
    /// Sanitize-ready HTML per chapter (body content only; resource refs
    /// rewritten to `data-argus-res` placeholders, scripts/styles stripped).
    pub chapter_html: Vec<String>,
    /// Chapter title (from TOC or first heading), if known.
    pub chapter_titles: Vec<Option<String>>,
    /// Plain text per chapter (derived from the HTML).
    pub chapter_text: Vec<String>,
    /// Normalized href → resource entry.
    pub resources: HashMap<String, ResourceEntry>,
}

impl ParsedEbook {
    pub fn manifest(&self) -> EbookManifest {
        EbookManifest {
            format: self.format.clone(),
            meta: self.meta.clone(),
            toc: self.toc.clone(),
            chapters: (0..self.chapter_html.len())
                .map(|i| EbookChapterMeta {
                    index: (i + 1) as u32,
                    title: self.chapter_titles[i].clone(),
                    char_count: self.chapter_text[i].chars().count() as u32,
                })
                .collect(),
        }
    }
}

// ── Format detection / file discovery ─────────────────────────────────────────

/// Detect the ebook format of `path` from its file name, or `None` when it is
/// not a supported ebook. `.fb2.zip` is recognized as FB2.
pub fn detect_format(path: &Path) -> Option<&'static str> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if EXCLUDED_FILES.contains(&name.as_str()) {
        return None;
    }
    if name.ends_with(".fb2.zip") {
        return Some("fb2");
    }
    match name.rsplit('.').next()? {
        "epub" => Some("epub"),
        "mobi" => Some("mobi"),
        "azw3" | "azw" => Some("azw3"),
        "fb2" => Some("fb2"),
        "txt" => Some("txt"),
        _ => None,
    }
}

/// Find the ebook file inside a paper directory (parallel to
/// `metadata::find_pdf_in_dir`, which only ever looks at `.pdf`).
pub fn find_ebook_in_dir(root: &str, slug: &str) -> Option<(PathBuf, &'static str)> {
    let dir = crate::paper::paper_dir(root, slug);
    let mut best: Option<(usize, PathBuf, &'static str)> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(fmt) = detect_format(&p) {
            let rank = EBOOK_EXTENSIONS.iter().position(|e| *e == fmt).unwrap_or(9);
            if best.as_ref().map_or(true, |(r, _, _)| rank < *r) {
                best = Some((rank, p, fmt));
            }
        }
    }
    best.map(|(_, p, f)| (p, f))
}

/// True when a `PaperMeta.file_type` value denotes an ebook (anything other
/// than absent/"pdf").
pub fn is_ebook_file_type(file_type: Option<&str>) -> bool {
    matches!(file_type, Some(ft) if !ft.is_empty() && ft != "pdf")
}

// ── Parsing + cache ───────────────────────────────────────────────────────────

/// Parse a book file into the normalized representation.
pub fn parse(path: &Path, format: &str) -> Result<ParsedEbook, String> {
    match format {
        "epub" => epub::parse(path),
        "fb2" => fb2::parse(path),
        "mobi" | "azw3" => mobi_fmt::parse(path, format),
        "txt" => txt::parse(path),
        other => Err(format!("Unsupported ebook format: {other}")),
    }
}

struct CacheEntry {
    slug: String,
    mtime: u64,
    parsed: Arc<ParsedEbook>,
}

/// Small LRU of parsed books — repeated chapter/resource fetches from the
/// viewer must not re-unzip the whole book. Front = most recent.
static PARSE_CACHE: Mutex<Vec<CacheEntry>> = Mutex::new(Vec::new());
const PARSE_CACHE_CAP: usize = 4;

fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Get the parsed book for a paper, from cache when fresh. Parsing is
/// CPU/IO-heavy — call from `spawn_blocking` in command handlers.
pub fn get_parsed(root: &str, slug: &str) -> Result<Arc<ParsedEbook>, String> {
    let (path, format) =
        find_ebook_in_dir(root, slug).ok_or_else(|| format!("No ebook file found for {slug}"))?;
    let mtime = file_mtime(&path);

    if let Ok(mut cache) = PARSE_CACHE.lock() {
        if let Some(pos) = cache
            .iter()
            .position(|e| e.slug == slug && e.mtime == mtime && e.parsed.book_path == path)
        {
            let entry = cache.remove(pos);
            let parsed = entry.parsed.clone();
            cache.insert(0, entry);
            return Ok(parsed);
        }
    }

    let parsed = Arc::new(parse(&path, format)?);
    if let Ok(mut cache) = PARSE_CACHE.lock() {
        cache.retain(|e| e.slug != slug);
        cache.insert(
            0,
            CacheEntry { slug: slug.to_string(), mtime, parsed: parsed.clone() },
        );
        cache.truncate(PARSE_CACHE_CAP);
    }
    Ok(parsed)
}

/// Drop a paper's cache entry (after folder rename the slug changes anyway,
/// but deletes should not pin a stale Arc).
pub fn evict_from_cache(slug: &str) {
    if let Ok(mut cache) = PARSE_CACHE.lock() {
        cache.retain(|e| e.slug != slug);
    }
}

/// Read one resource's bytes. Zip-backed entries are re-read from the book
/// file so images never live in the parse cache.
pub fn read_resource(parsed: &ParsedEbook, href: &str) -> Result<EbookResource, String> {
    // hrefs were normalized at parse time; normalize the request the same way
    // so lookups are exact and traversal tricks ("../") cannot escape the map.
    let key = normalize_zip_path(href);
    let entry = parsed
        .resources
        .get(&key)
        .ok_or_else(|| format!("Unknown ebook resource: {href}"))?;
    let bytes = match &entry.data {
        ResourceData::Bytes(b) => b.clone(),
        ResourceData::ZipEntry { inner_path } => {
            let file = std::fs::File::open(&parsed.book_path)
                .map_err(|e| format!("Open book file: {e}"))?;
            let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file))
                .map_err(|e| format!("Open book archive: {e}"))?;
            let mut zf = archive
                .by_name(inner_path)
                .map_err(|e| format!("Resource {inner_path}: {e}"))?;
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut zf, &mut buf)
                .map_err(|e| format!("Read resource: {e}"))?;
            buf
        }
    };
    use base64::Engine;
    Ok(EbookResource {
        mime: entry.mime.clone(),
        base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

// ── Fulltext / sections derivation ────────────────────────────────────────────

/// Build fulltext.txt content: chapters in order, each preceded by its title
/// on a standalone line (unless the chapter text already starts with it) so
/// `sections::select_sections_text` can anchor section headings.
pub fn fulltext_from(parsed: &ParsedEbook) -> String {
    let mut out = String::new();
    for (i, text) in parsed.chapter_text.iter().enumerate() {
        let text = text.trim();
        if let Some(title) = parsed.chapter_titles[i].as_deref() {
            let first_line = text.lines().next().unwrap_or("").trim();
            if !first_line.eq_ignore_ascii_case(title.trim()) {
                out.push_str(title.trim());
                out.push_str("\n\n");
            }
        }
        out.push_str(text);
        out.push_str("\n\n");
    }
    out
}

/// Build sections.json content from the book's TOC (falling back to chapter
/// titles). Returns `None` when the book yields no usable structure.
pub fn sections_from(parsed: &ParsedEbook) -> Option<PaperSections> {
    let sections: Vec<PaperSection> = if !parsed.toc.is_empty() {
        parsed
            .toc
            .iter()
            .map(|t| PaperSection {
                title: t.title.clone(),
                level: t.level.clamp(1, 3),
                page: t.chapter,
            })
            .collect()
    } else {
        parsed
            .chapter_titles
            .iter()
            .enumerate()
            .filter_map(|(i, t)| {
                t.as_ref().map(|title| PaperSection {
                    title: title.clone(),
                    level: 1,
                    page: (i + 1) as u32,
                })
            })
            .collect()
    };
    if sections.is_empty() {
        None
    } else {
        Some(PaperSections {
            source: "outline".to_string(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            sections,
        })
    }
}

/// Extract fulltext for an ebook paper and write fulltext.txt + status flag.
/// Mirrors what `extraction::extract_and_write` guarantees for PDFs,
/// including the "existing fulltext is good enough" early exit.
pub fn extract_fulltext_and_write(root: &str, slug: &str) -> Result<String, String> {
    let fulltext_path = crate::paper::paper_dir(root, slug).join("fulltext.txt");
    if let Ok(existing) = std::fs::read_to_string(&fulltext_path) {
        if existing.chars().filter(|c| !c.is_whitespace()).count() > 200 {
            let mut status = crate::paper::read_status_for(root, slug);
            if !status.text_extracted {
                status.text_extracted = true;
                status.last_updated = chrono::Utc::now().to_rfc3339();
                let _ = crate::paper::write_status(root, slug, &status);
            }
            return Ok(existing);
        }
    }

    let parsed = get_parsed(root, slug)?;
    let fulltext = fulltext_from(&parsed);
    if fulltext.trim().is_empty() {
        return Err("Ebook produced no text".to_string());
    }
    let path = crate::paper::paper_dir(root, slug).join("fulltext.txt");
    crate::fsutil::atomic_write_str(&path, &fulltext)
        .map_err(|e| format!("write fulltext.txt: {e}"))?;
    let mut status = crate::paper::read_status_for(root, slug);
    status.text_extracted = true;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    let _ = crate::paper::write_status(root, slug, &status);
    Ok(fulltext)
}

// ── Shared HTML helpers (used by the format parsers) ──────────────────────────

/// Normalize a path inside an archive/book: decode %XX, strip a leading "./"
/// or "/", resolve "." and ".." segments (".." never escapes the root).
pub fn normalize_zip_path(path: &str) -> String {
    let decoded = percent_decode(path.split('#').next().unwrap_or(path));
    let mut parts: Vec<&str> = Vec::new();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// Resolve `href` relative to the directory of `base_file` (both zip paths).
pub fn resolve_href(base_file: &str, href: &str) -> String {
    let base_dir = match base_file.rfind('/') {
        Some(i) => &base_file[..i],
        None => "",
    };
    if href.starts_with('/') || base_dir.is_empty() {
        normalize_zip_path(href)
    } else {
        normalize_zip_path(&format!("{base_dir}/{href}"))
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode the handful of named entities common in book HTML plus numeric ones.
pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        // Search the raw bytes — a str slice like rest[..12] panics when byte
        // 12 lands inside a multi-byte char (e.g. mathematical italic '𝑖').
        // ';' is ASCII, so the found index is always a valid char boundary.
        let end = rest.as_bytes()[..rest.len().min(12)]
            .iter()
            .position(|&b| b == b';');
        match end {
            Some(e) => {
                let ent = &rest[1..e];
                let decoded: Option<String> = match ent {
                    "amp" => Some("&".into()),
                    "lt" => Some("<".into()),
                    "gt" => Some(">".into()),
                    "quot" => Some("\"".into()),
                    "apos" => Some("'".into()),
                    "nbsp" => Some("\u{a0}".into()),
                    "mdash" => Some("—".into()),
                    "ndash" => Some("–".into()),
                    "hellip" => Some("…".into()),
                    "ldquo" => Some("\u{201c}".into()),
                    "rdquo" => Some("\u{201d}".into()),
                    "lsquo" => Some("\u{2018}".into()),
                    "rsquo" => Some("\u{2019}".into()),
                    _ if ent.starts_with("#x") || ent.starts_with("#X") => u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                        .map(String::from),
                    _ if ent.starts_with('#') => ent[1..]
                        .parse::<u32>()
                        .ok()
                        .and_then(char::from_u32)
                        .map(String::from),
                    _ => None,
                };
                match decoded {
                    Some(d) => {
                        out.push_str(&d);
                        rest = &rest[e + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = &rest[1..];
                    }
                }
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Convert chapter HTML to plain text: block-level tags become newlines, all
/// other tags are dropped, entities decoded, blank runs collapsed.
pub fn html_to_text(html: &str) -> String {
    use regex::Regex;
    // Newlines at block boundaries so paragraphs survive tag stripping.
    static BLOCK_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static TAG_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let block_re = BLOCK_RE.get_or_init(|| {
        Regex::new(r"(?i)</?(p|div|h[1-6]|li|tr|section|article|blockquote|figure|figcaption|table|ul|ol|dl|dt|dd|pre|hr)[^>]*>|<br[^>]*>")
            .unwrap()
    });
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());

    let with_breaks = block_re.replace_all(html, "\n");
    let stripped = tag_re.replace_all(&with_breaks, "");
    let decoded = decode_entities(&stripped);

    let mut out = String::with_capacity(decoded.len());
    let mut blank_run = 0;
    for line in decoded.lines() {
        let line = line.trim();
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
            if blank_run > 0 {
                out.push('\n');
            }
        }
        blank_run = 0;
        out.push_str(line);
    }
    out
}

/// Strip scripts/styles/links/comments from raw chapter markup and return the
/// `<body>` inner HTML (or the whole input when no body tag exists).
pub fn extract_body_html(raw: &str) -> String {
    use regex::Regex;
    static SCRIPT_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static STYLE_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static LINKMETA_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static COMMENT_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static BODY_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let script_re =
        SCRIPT_RE.get_or_init(|| Regex::new(r"(?is)<script\b[^>]*>.*?</script>").unwrap());
    let style_re = STYLE_RE.get_or_init(|| Regex::new(r"(?is)<style\b[^>]*>.*?</style>").unwrap());
    let linkmeta_re =
        LINKMETA_RE.get_or_init(|| Regex::new(r"(?i)<(link|meta|base)\b[^>]*/?>").unwrap());
    let comment_re = COMMENT_RE.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    let body_re = BODY_RE.get_or_init(|| Regex::new(r"(?is)<body[^>]*>(.*)</body>").unwrap());

    let body = match body_re.captures(raw) {
        Some(c) => c.get(1).map(|m| m.as_str()).unwrap_or(raw).to_string(),
        None => raw.to_string(),
    };
    let no_comments = comment_re.replace_all(&body, "");
    let no_scripts = script_re.replace_all(&no_comments, "");
    let no_styles = style_re.replace_all(&no_scripts, "");
    linkmeta_re.replace_all(&no_styles, "").into_owned()
}

/// Neutralize `<a href>` so clicks never navigate the WebView.
pub fn neutralize_links(html: &str) -> String {
    use regex::Regex;
    static A_HREF_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let a_href_re = A_HREF_RE
        .get_or_init(|| Regex::new(r#"(?i)(<a\b[^>]*?)\shref\s*=\s*("[^"]*"|'[^']*')"#).unwrap());
    a_href_re.replace_all(html, "$1").into_owned()
}

/// Neutralize `<a href>` so clicks never navigate the WebView, and rewrite
/// image references to `data-argus-res` placeholders the viewer resolves to
/// blob URLs. `resolve` maps a raw href to the normalized resource key (or
/// `None` to drop the image).
pub fn rewrite_chapter_html<F>(html: &str, resolve: F) -> String
where
    F: Fn(&str) -> Option<String>,
{
    use regex::Regex;
    static IMG_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static SVG_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static SRC_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static XLINK_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let img_re = IMG_RE.get_or_init(|| Regex::new(r"(?is)<img\b[^>]*>").unwrap());
    let svg_re = SVG_RE.get_or_init(|| Regex::new(r"(?is)<svg\b.*?</svg>").unwrap());
    let src_re =
        SRC_RE.get_or_init(|| Regex::new(r#"(?i)\ssrc\s*=\s*("([^"]*)"|'([^']*)')"#).unwrap());
    let xlink_re = XLINK_RE.get_or_init(|| {
        Regex::new(r#"(?i)\s(?:xlink:)?href\s*=\s*("([^"]*)"|'([^']*)')"#).unwrap()
    });

    // <img src> first — the SVG pass below *generates* src-less <img> tags
    // that must not be re-visited (and deleted) by this pass.
    let img_replaced = img_re.replace_all(html, |caps: &regex::Captures| {
        let tag = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        if let Some(c) = src_re.captures(tag) {
            let href = c.get(2).or(c.get(3)).map(|m| m.as_str()).unwrap_or("");
            if let Some(key) = resolve(href) {
                // Replace the src attribute, keep the rest of the tag (alt, …).
                return src_re
                    .replace(tag, format!(r#" data-argus-res="{key}""#).as_str())
                    .into_owned();
            }
        }
        String::new()
    });

    // SVG-wrapped cover images (<svg><image xlink:href=…/></svg>) → plain <img>.
    let svg_replaced = svg_re.replace_all(&img_replaced, |caps: &regex::Captures| {
        let block = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        if let Some(c) = xlink_re.captures(block) {
            let href = c.get(2).or(c.get(3)).map(|m| m.as_str()).unwrap_or("");
            if let Some(key) = resolve(href) {
                return format!(r#"<img data-argus-res="{key}" alt="">"#);
            }
        }
        String::new()
    });

    neutralize_links(&svg_replaced)
}

/// Pull the first h1/h2/h3 text out of chapter HTML (title fallback).
pub fn first_heading_text(html: &str) -> Option<String> {
    use regex::Regex;
    static H_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let h_re = H_RE.get_or_init(|| Regex::new(r"(?is)<h[1-3][^>]*>(.*?)</h[1-3]>").unwrap());
    let inner = h_re.captures(html)?.get(1)?.as_str();
    let text = html_to_text(inner).trim().to_string();
    if text.is_empty() || text.chars().count() > 120 {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_format_basics() {
        assert_eq!(detect_format(Path::new("a/b/Book.EPUB")), Some("epub"));
        assert_eq!(detect_format(Path::new("book.fb2.zip")), Some("fb2"));
        assert_eq!(detect_format(Path::new("book.azw3")), Some("azw3"));
        assert_eq!(detect_format(Path::new("fulltext.txt")), None);
        assert_eq!(detect_format(Path::new("paper.pdf")), None);
    }

    #[test]
    fn zip_path_normalization() {
        assert_eq!(normalize_zip_path("OEBPS/../images/a%20b.png"), "images/a b.png");
        assert_eq!(normalize_zip_path("/OEBPS/./ch1.xhtml#frag"), "OEBPS/ch1.xhtml");
        assert_eq!(resolve_href("OEBPS/text/ch1.xhtml", "../images/i.png"), "OEBPS/images/i.png");
        assert_eq!(resolve_href("ch1.xhtml", "i.png"), "i.png");
        // ".." can never escape the archive root
        assert_eq!(normalize_zip_path("../../etc/passwd"), "etc/passwd");
    }

    #[test]
    fn decode_entities_multibyte_after_ampersand() {
        // Regression: math-italic chars (4-byte UTF-8) right after an entity
        // used to panic the 12-byte lookahead slice at a non-char boundary.
        let s = "&lt;𝑡(𝑖) represents &amp; 𝐴𝑖 &unknown𝑥; &#120894;";
        let out = decode_entities(s);
        assert!(out.starts_with("<𝑡(𝑖) represents & 𝐴𝑖"));
        // Unknown entity with multibyte chars passes through unchanged.
        assert!(out.contains("&unknown𝑥;"));
    }

    #[test]
    fn html_to_text_blocks_and_entities() {
        let html = "<h1>T&amp;itle</h1><p>Line one.</p>\n<p>Line&nbsp;two &#65;</p><script>x</script>";
        let text = html_to_text(&extract_body_html(html));
        assert!(text.contains("T&itle"));
        assert!(text.contains("Line one."));
        assert!(text.contains("Line\u{a0}two A"));
        assert!(!text.contains('x'));
    }

    #[test]
    fn rewrite_images_and_links() {
        let html = r#"<p><a href="ch2.xhtml">next</a><img src="../img/pic.png" alt="p"/></p>
            <svg xmlns="s"><image xlink:href="cover.jpg"/></svg>"#;
        let out = rewrite_chapter_html(html, |h| Some(normalize_zip_path(h)));
        assert!(out.contains(r#"data-argus-res="img/pic.png""#) || out.contains(r#"data-argus-res="../img/pic.png""#));
        assert!(!out.contains("href=\"ch2.xhtml\""));
        assert!(out.contains(r#"<img data-argus-res="cover.jpg" alt="">"#));
        assert!(out.contains(r#"alt="p""#));
    }

    #[test]
    fn fulltext_prepends_missing_titles() {
        let parsed = ParsedEbook {
            format: "epub".into(),
            book_path: PathBuf::new(),
            meta: EbookMeta::default(),
            toc: vec![],
            chapter_html: vec![String::new(), String::new()],
            chapter_titles: vec![Some("第一章".into()), Some("Two".into())],
            chapter_text: vec!["第一章\n正文A".into(), "Body two".into()],
            resources: HashMap::new(),
        };
        let ft = fulltext_from(&parsed);
        assert_eq!(ft.matches("第一章").count(), 1);
        assert!(ft.contains("Two\n\nBody two"));
    }
}
