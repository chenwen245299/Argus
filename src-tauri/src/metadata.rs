use std::path::PathBuf;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::{
    extraction,
    models::{CitationRef, JournalCustomRank, JournalRank, PaperMeta},
    paper,
};

// ── Internal intermediate struct for API results ──────────────────────────────

#[derive(Default)]
pub struct MetaUpdate {
    pub title: Option<String>,
    pub authors: Option<Vec<String>>,
    pub year: Option<u32>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub venue: Option<String>,
    pub paper_abstract: Option<String>,
    pub cite_count: Option<u32>,
}

// ── PDF text extraction ───────────────────────────────────────────────────────

/// Sanitize a string so it's a valid filename on both macOS and Windows.
pub fn sanitize_filename(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' | '\n' | '\r' | '\t' => '_',
            c if (c as u32) < 32 => '_',
            _ => c,
        })
        .collect();

    let s = s.trim_matches(|c: char| c == ' ' || c == '.').to_string();

    // Windows reserved names (checked against stem, case-insensitive)
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let stem_upper = s
        .rfind('.')
        .map(|i| s[..i].to_uppercase())
        .unwrap_or_else(|| s.to_uppercase());
    let s = if reserved.contains(&stem_upper.as_str()) {
        format!("_{}", s)
    } else {
        s
    };

    // Limit length (leave room for .pdf extension, keep ≤ 200 chars)
    let s: String = s.chars().take(200).collect();

    if s.is_empty() {
        "paper".to_string()
    } else {
        s
    }
}

/// Find the PDF file inside a paper directory. Scans for any .pdf, falls back to paper.pdf.
pub fn find_pdf_in_dir(root: &str, slug: &str) -> PathBuf {
    let dir = paper::paper_dir(root, slug);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("pdf") {
                return p;
            }
        }
    }
    dir.join("paper.pdf")
}

pub fn extract_text_sync(root: &str, slug: &str) -> Result<String, String> {
    let path = find_pdf_in_dir(root, slug);
    let bytes = std::fs::read(&path).map_err(|e| format!("Cannot read PDF: {e}"))?;
    extract_text_from_bytes(&bytes)
}

fn extract_text_from_bytes(bytes: &[u8]) -> Result<String, String> {
    let doc = lopdf::Document::load_mem(bytes).map_err(|e| format!("lopdf: {e}"))?;
    let pages = doc.get_pages();
    // Take first 2 pages only (fast and sufficient for metadata)
    let page_nums: Vec<u32> = pages.keys().take(2).copied().collect();
    if page_nums.is_empty() {
        return Ok(String::new());
    }
    let text = doc.extract_text(&page_nums).unwrap_or_default();
    Ok(text.chars().take(4000).collect())
}

/// Return the first `limit` whitespace-delimited words, with a hard character
/// cap for malformed text that lacks spaces.
pub fn leading_words(text: &str, limit: usize) -> String {
    const MAX_CHARS: usize = 12_000;
    let mut out = String::new();
    let mut count = 0usize;

    for word in text.split_whitespace() {
        if count >= limit {
            break;
        }
        let separator = usize::from(!out.is_empty());
        if out.len() + separator + word.len() > MAX_CHARS {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        count += 1;
    }

    if out.is_empty() {
        text.chars().take(4000).collect()
    } else {
        out
    }
}

fn normalize_space(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_xml_tags(text: &str) -> String {
    let no_tags = Regex::new(r"<[^>]+>")
        .map(|re| re.replace_all(text, " ").to_string())
        .unwrap_or_else(|_| text.to_string());
    xml_unescape(&no_tags)
}

// ── Identifier extraction ─────────────────────────────────────────────────────

pub fn find_arxiv_id(text: &str) -> Option<String> {
    // New-style arXiv IDs: YYMM.NNNNN[vN]
    let re = Regex::new(r"(?i)(?:arxiv[:\s./]*)?(\d{4}\.\d{4,5}(?:v\d+)?)").ok()?;
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let id = m.as_str().to_string();
            // Sanity: first 2 chars are month 01-12
            let month: u32 = id[2..4].parse().unwrap_or(0);
            if (1..=12).contains(&month) {
                // Strip version suffix for API call
                let clean = id.split('v').next().unwrap_or(&id).to_string();
                return Some(clean);
            }
        }
    }
    None
}

pub fn find_doi(text: &str) -> Option<String> {
    let re = Regex::new(r"\b(10\.\d{4,}/[^\s<>',;()\[\]{}]+)").ok()?;
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let doi = m
                .as_str()
                .trim_end_matches(['.', ',', ';', ')'].as_ref())
                .to_string();
            if doi.len() > 8 {
                return Some(doi);
            }
        }
    }
    None
}

// ── HTTP client ───────────────────────────────────────────────────────────────

fn build_client() -> reqwest::Client {
    // Cache a single client: reqwest clients pool connections, so rebuilding one
    // per request is wasteful. Never panic on builder failure — fall back to a
    // default client so metadata lookups degrade gracefully instead of aborting.
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return client.clone();
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("Argus/0.1 (local-first literature manager)")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    CLIENT.get_or_init(|| client).clone()
}

/// Upper bound on a metadata API response. These endpoints return small
/// JSON/XML documents; the cap guards against a hostile or misbehaving server
/// streaming an unbounded body into memory.
const MAX_META_BYTES: u64 = 20 * 1024 * 1024;

// ── arXiv API ─────────────────────────────────────────────────────────────────

pub async fn fetch_arxiv(arxiv_id: &str) -> Option<MetaUpdate> {
    let url = format!("https://export.arxiv.org/api/query?id_list={}", arxiv_id);
    let client = build_client();
    let resp = client.get(&url).send().await.ok()?;
    let xml = crate::net::fetch_text_capped(resp, MAX_META_BYTES).await.ok()?;

    // Check there's an actual entry (not just a feed with zero results)
    if !xml.contains("<entry>") {
        return None;
    }

    parse_arxiv_xml(&xml)
}

fn parse_arxiv_xml(xml: &str) -> Option<MetaUpdate> {
    let entry_start = xml.find("<entry>")?;
    let entry_end = xml.find("</entry>").unwrap_or(xml.len());
    let entry = &xml[entry_start..entry_end.min(xml.len())];

    // Title (skip the feed-level title which appears before <entry>)
    let title = extract_between(entry, "<title>", "</title>").map(xml_unescape)?;
    let title = title.trim().to_string();
    if title.is_empty() || title.starts_with("Error") {
        return None;
    }

    // Authors
    let name_re = Regex::new(r"<name>(.*?)</name>").ok()?;
    let authors: Vec<String> = name_re
        .captures_iter(entry)
        .filter_map(|c| c.get(1).map(|m| xml_unescape(m.as_str().trim())))
        .filter(|s| !s.is_empty())
        .collect();

    // Year from <published>YYYY-…
    let year = extract_between(entry, "<published>", "</published>")
        .and_then(|s| s.chars().take(4).collect::<String>().parse::<u32>().ok());

    // Optional DOI
    let doi = extract_between(entry, "<arxiv:doi>", "</arxiv:doi>")
        .map(|s| xml_unescape(s.trim()))
        .filter(|s| !s.is_empty());

    // Optional venue (journal_ref)
    let venue = extract_between(entry, "<arxiv:journal_ref>", "</arxiv:journal_ref>")
        .map(|s| xml_unescape(s.trim()))
        .filter(|s| !s.is_empty());
    let paper_abstract = extract_between(entry, "<summary>", "</summary>")
        .map(|s| xml_unescape(s.trim()))
        .map(|s| normalize_space(&s))
        .filter(|s| !s.is_empty());

    Some(MetaUpdate {
        title: Some(title),
        authors: if authors.is_empty() {
            None
        } else {
            Some(authors)
        },
        year,
        doi,
        arxiv_id: None,
        venue,
        paper_abstract,
        cite_count: None,
    })
}

// ── Crossref API ──────────────────────────────────────────────────────────────

pub async fn fetch_crossref(doi: &str) -> Option<MetaUpdate> {
    let url = format!("https://api.crossref.org/works/{}", doi);
    let client = build_client();
    let resp = client.get(&url).send().await.ok()?;
    let json: Value = crate::net::fetch_json_capped(resp, MAX_META_BYTES).await.ok()?;
    parse_crossref_json(&json)
}

fn parse_crossref_json(json: &Value) -> Option<MetaUpdate> {
    let msg = json.get("message")?;

    let title = msg["title"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;

    let authors = msg["author"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|a| {
                let family = a["family"].as_str()?.trim().to_string();
                let given = a["given"].as_str().unwrap_or("").trim().to_string();
                if given.is_empty() {
                    Some(family)
                } else {
                    Some(format!("{} {}", given, family))
                }
            })
            .collect::<Vec<_>>()
    });

    // Year: try several date fields
    let year = ["published", "published-print", "published-online", "issued"]
        .iter()
        .find_map(|field| {
            msg[field]["date-parts"]
                .as_array()?
                .first()?
                .as_array()?
                .first()?
                .as_u64()
                .map(|y| y as u32)
        });

    let venue = msg["container-title"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let doi = msg["DOI"].as_str().map(|s| s.to_string());
    let paper_abstract = msg["abstract"]
        .as_str()
        .map(strip_xml_tags)
        .map(|s| normalize_space(&s))
        .filter(|s| !s.is_empty());

    Some(MetaUpdate {
        title: Some(title),
        authors,
        year,
        doi,
        arxiv_id: None,
        venue,
        paper_abstract,
        cite_count: None,
    })
}

// ── Semantic Scholar API ──────────────────────────────────────────────────────

pub async fn fetch_semantic_scholar(
    arxiv_id: Option<&str>,
    doi: Option<&str>,
) -> Option<MetaUpdate> {
    let paper_id = if let Some(id) = arxiv_id {
        format!("arXiv:{}", id)
    } else if let Some(d) = doi {
        format!("DOI:{}", d)
    } else {
        return None;
    };
    let url = format!(
        "https://api.semanticscholar.org/graph/v1/paper/{}?fields=title,authors,year,venue,abstract,externalIds,citationCount",
        paper_id
    );
    let client = build_client();
    let resp = client.get(&url).send().await.ok()?;
    let json: Value = crate::net::fetch_json_capped(resp, MAX_META_BYTES).await.ok()?;
    parse_s2_json(&json)
}

fn parse_s2_json(json: &Value) -> Option<MetaUpdate> {
    // S2 returns {"error": "..."} if not found
    if json.get("error").is_some() {
        return None;
    }
    let title = json["title"].as_str()?.to_string();

    let authors = json["authors"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|a| a["name"].as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
    });

    let year = json["year"].as_u64().map(|y| y as u32);

    let venue = json["venue"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let doi = json["externalIds"]["DOI"].as_str().map(|s| s.to_string());
    let arxiv_id = json["externalIds"]["ArXiv"].as_str().map(|s| s.to_string());
    let paper_abstract = json["abstract"]
        .as_str()
        .map(|s| normalize_space(s))
        .filter(|s| !s.is_empty());
    let cite_count = json["citationCount"].as_u64().map(|n| n as u32);

    Some(MetaUpdate {
        title: Some(title),
        authors,
        year,
        doi,
        arxiv_id,
        venue,
        paper_abstract,
        cite_count,
    })
}

// ── Semantic Scholar title search ─────────────────────────────────────────────

pub async fn fetch_semantic_scholar_by_title(title: &str) -> Option<MetaUpdate> {
    let client = build_client();
    let resp = client
        .get("https://api.semanticscholar.org/graph/v1/paper/search")
        .query(&[
            ("query", title),
            ("fields", "title,authors,year,venue,abstract,externalIds,citationCount"),
            ("limit", "3"),
        ])
        .send()
        .await
        .ok()?;
    let json: Value = crate::net::fetch_json_capped(resp, MAX_META_BYTES).await.ok()?;
    let papers = json["data"].as_array()?;
    // Pick the first result whose title roughly matches (case-insensitive substring)
    let title_lower = title.to_lowercase();
    // Char-safe prefix: byte-slicing `[..40]` panics when the cut lands mid-character,
    // which is essentially guaranteed for CJK titles (3 bytes/char).
    let title_prefix: String = title_lower.chars().take(40).collect();
    for paper in papers {
        let candidate = paper["title"].as_str().unwrap_or("").to_lowercase();
        let candidate_prefix: String = candidate.chars().take(40).collect();
        // Accept if either title contains the other (handles truncated/subtitle variants)
        if candidate.contains(title_prefix.as_str())
            || title_lower.contains(candidate_prefix.as_str())
        {
            return parse_s2_json(paper);
        }
    }
    // Fall back to first result if query is specific enough
    if !papers.is_empty() && title.split_whitespace().count() >= 5 {
        return parse_s2_json(&papers[0]);
    }
    None
}

// ── Crossref title search ──────────────────────────────────────────────────────

pub async fn fetch_crossref_by_title(title: &str) -> Option<MetaUpdate> {
    let client = build_client();
    let resp = client
        .get("https://api.crossref.org/works")
        .query(&[
            ("query.title", title),
            ("rows", "1"),
            (
                "select",
                "DOI,title,author,published-print,published-online,container-title",
            ),
        ])
        .send()
        .await
        .ok()?;
    let json: Value = crate::net::fetch_json_capped(resp, MAX_META_BYTES).await.ok()?;
    let item = json["message"]["items"].as_array()?.first()?;

    let title = item["title"].as_array()?.first()?.as_str()?.to_string();
    let authors = item["author"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|a| {
                let given = a["given"].as_str().unwrap_or("").trim().to_string();
                let family = a["family"].as_str()?.trim().to_string();
                if given.is_empty() {
                    Some(family)
                } else {
                    Some(format!("{} {}", given, family))
                }
            })
            .collect::<Vec<_>>()
    });
    let year = item["published-print"]["date-parts"]
        .as_array()
        .or_else(|| item["published-online"]["date-parts"].as_array())
        .and_then(|dp| dp.first())
        .and_then(|inner| inner.as_array())
        .and_then(|parts| parts.first())
        .and_then(|y| y.as_u64())
        .map(|y| y as u32);
    let doi = item["DOI"].as_str().map(|s| s.to_string());
    let venue = item["container-title"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Some(MetaUpdate {
        title: Some(title),
        authors,
        year,
        doi,
        arxiv_id: None,
        venue,
        paper_abstract: None,
        cite_count: None,
    })
}

// ── AI fallback ───────────────────────────────────────────────────────────────

fn parse_ai_content(raw: &str) -> Option<MetaUpdate> {
    // Extract JSON from a markdown code block first (```json ... ``` or ``` ... ```),
    // then fall back to scanning for the first { ... } pair.
    let json_str: &str = if let Some(s) = raw.find("```json") {
        let after = &raw[s + 7..];
        let end = after.find("```").unwrap_or(after.len());
        after[..end].trim()
    } else if let Some(s) = raw.find("```") {
        let after = &raw[s + 3..];
        let end = after.find("```").unwrap_or(after.len());
        after[..end].trim()
    } else {
        let start = raw.find('{')?;
        let end = raw.rfind('}')?;
        if end < start {
            return None;
        }
        &raw[start..=end]
    };

    let json: Value = serde_json::from_str(json_str).ok()?;

    let title = json["title"].as_str().map(|s| s.to_string());
    let authors = json["authors"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect::<Vec<_>>()
    });
    let year = json["year"].as_u64().map(|y| y as u32);
    let venue = json["venue"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let doi = json["doi"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let arxiv_id = json["arxiv_id"]
        .as_str()
        .or_else(|| json["arxiv"].as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    Some(MetaUpdate {
        title,
        authors,
        year,
        doi,
        arxiv_id,
        venue,
        paper_abstract: None,
        cite_count: None,
    })
}

fn render_metadata_prompt(template: &str, text: &str) -> String {
    let mut prompt = if template.trim().is_empty() {
        crate::models::default_metadata_ai_prompt()
    } else {
        template.to_string()
    };
    let had_text_placeholder = prompt.contains("{text}");
    prompt = prompt.replace("{text}", text);

    if !had_text_placeholder {
        prompt.push_str("\n\nText:\n");
        prompt.push_str(text);
    }

    prompt
}

// ── Tiered fetch orchestration ────────────────────────────────────────────────

/// Run the full fetch cascade: arXiv → Crossref → S2 → AI fallback.
/// Updates meta.json and .status.json on success; leaves them unchanged on failure.
pub async fn fetch_and_apply(
    root: &str,
    slug: &str,
    _app: &AppHandle,
) -> Result<PaperMeta, String> {
    // Step 1: extract first-page text (blocking I/O + CPU, use spawn_blocking)
    let root_c = root.to_string();
    let slug_c = slug.to_string();
    let text = tauri::async_runtime::spawn_blocking(move || {
        extract_text_sync(&root_c, &slug_c).unwrap_or_default()
    })
    .await
    .unwrap_or_default();

    let arxiv_id = find_arxiv_id(&text);
    let doi = find_doi(&text);

    // Step 2: tier 1 — arXiv
    let update = if let Some(ref id) = arxiv_id {
        fetch_arxiv(id).await
    } else {
        None
    };

    // Step 3: tier 2 — Crossref
    let update = if update.is_none() {
        if let Some(ref d) = doi {
            fetch_crossref(d).await
        } else {
            None
        }
    } else {
        update
    };

    // Step 4: tier 3 — Semantic Scholar by ID
    let update = if update.is_none() {
        fetch_semantic_scholar(arxiv_id.as_deref(), doi.as_deref()).await
    } else {
        update
    };

    // Step 5: tier 4 — title-based search (when no IDs found)
    // Read current title from meta to search with
    let current_title = crate::paper::read_meta(root, slug)
        .map(|m| m.title)
        .unwrap_or_default();
    let update = if update.is_none() && current_title.split_whitespace().count() >= 3 {
        // Try Semantic Scholar by title first, then Crossref
        let s2_result = fetch_semantic_scholar_by_title(&current_title).await;
        if s2_result.is_some() {
            s2_result
        } else {
            fetch_crossref_by_title(&current_title).await
        }
    } else {
        update
    };

    // (AI fallback removed — user triggers AI extraction manually via context menu)

    // Merge update into current meta
    let mut meta = crate::paper::read_meta(root, slug)?;

    if let Some(u) = update {
        if let Some(t) = u.title {
            meta.title = t;
        }
        if let Some(a) = u.authors {
            if !a.is_empty() {
                meta.authors = a;
            }
        }
        if let Some(y) = u.year {
            meta.year = Some(y);
        }
        if u.doi.is_some() {
            meta.doi = u.doi;
        } else if doi.is_some() {
            meta.doi = doi;
        }
        if u.arxiv_id.is_some() {
            meta.arxiv_id = u.arxiv_id;
        } else if arxiv_id.is_some() {
            meta.arxiv_id = arxiv_id;
        }
        if u.venue.is_some() {
            meta.venue = u.venue;
        }
        if u.paper_abstract.is_some() {
            meta.paper_abstract = u.paper_abstract;
        }
        // Only auto-fill citation count when the user has not provided one manually.
        if meta.cite_count.is_none() && u.cite_count.is_some() {
            meta.cite_count = u.cite_count;
        }

        crate::paper::write_meta(root, slug, &meta)?;

        let mut status = crate::paper::read_status_for(root, slug);
        status.metadata_fetched = true;
        status.last_updated = chrono::Utc::now().to_rfc3339();
        crate::paper::write_status(root, slug, &status)?;
    } else {
        // Still store identifiers we found even if API calls failed
        let mut changed = false;
        if meta.arxiv_id.is_none() && arxiv_id.is_some() {
            meta.arxiv_id = arxiv_id;
            changed = true;
        }
        if meta.doi.is_none() && doi.is_some() {
            meta.doi = doi;
            changed = true;
        }
        if changed {
            crate::paper::write_meta(root, slug, &meta)?;
        }
    }

    Ok(meta)
}

/// Look up a paper on Semantic Scholar and return its full metadata update
/// (citation count, DOI, venue, …). Tries arXiv ID / DOI first, then falls back
/// to a title search. Returns `None` when Semantic Scholar has no match.
pub async fn fetch_semantic_scholar_meta(
    root: &str,
    slug: &str,
) -> Result<Option<MetaUpdate>, String> {
    let meta = crate::paper::read_meta(root, slug)?;

    // Prefer stable identifiers from meta.
    if let Some(update) = fetch_semantic_scholar(meta.arxiv_id.as_deref(), meta.doi.as_deref()).await
    {
        return Ok(Some(update));
    }

    // Fallback to title search if title is long enough to be specific.
    if meta.title.split_whitespace().count() >= 3 {
        if let Some(update) = fetch_semantic_scholar_by_title(&meta.title).await {
            return Ok(Some(update));
        }
    }

    Ok(None)
}

// ── Venue rank cache (library-wide, keyed by venue) ─────────────────────────────
// Ranks are cached per venue string (not per paper), so once "ICLR 2026 → CCF A"
// is known every paper with that venue reuses it without another API call. The
// cache persists to `.argus/venue_ranks.json`.

fn venue_ranks_path(root: &str) -> PathBuf {
    std::path::Path::new(root)
        .join(".argus")
        .join("venue_ranks.json")
}

/// True for a 4-digit year token (e.g. "2020", "2020,", "(2019)").
fn is_year_token(tok: &str) -> bool {
    let digits: String = tok.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.len() == 4 && (digits.starts_with("19") || digits.starts_with("20"))
}

/// Normalize a venue into a cache key: drop year tokens (so different years of
/// the same venue share one entry), collapse whitespace, lowercase. Must match
/// the frontend's `normVenue`.
fn normalize_venue_key(venue: &str) -> String {
    venue
        .split_whitespace()
        .filter(|tok| !is_year_token(tok))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn read_venue_ranks(root: &str) -> std::collections::BTreeMap<String, JournalRank> {
    std::fs::read_to_string(venue_ranks_path(root))
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

fn write_venue_ranks(
    root: &str,
    map: &std::collections::BTreeMap<String, JournalRank>,
) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(map).map_err(|e| format!("Serialize venue ranks: {e}"))?;
    crate::fsutil::atomic_write_str(&venue_ranks_path(root), &content)
        .map_err(|e| format!("Write venue_ranks.json: {e}"))
}

/// Look up a venue's ranking on easyScholar, trying several normalized candidates
/// (year/ordinal stripped, abbreviation↔full-name), throttled to the API's
/// 2 req/s limit. `Ok(None)` = successfully queried but no rank.
async fn resolve_venue_rank(
    secret_key: &str,
    venue: &str,
) -> Result<Option<JournalRank>, String> {
    let client = build_client();
    let candidates = venue_query_candidates(venue);
    let mut last_err: Option<String> = None;
    for (i, cand) in candidates.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        match query_easyscholar(&client, secret_key, cand).await {
            Ok(Some((official, custom))) => {
                return Ok(Some(JournalRank {
                    official,
                    custom,
                    venue: venue.to_string(),
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                }));
            }
            Ok(None) => continue,
            // An API-level error (bad key, quota, …) recurs for every candidate,
            // so stop now; a transient network error is retried on the next one.
            Err(e) if e.starts_with("easyScholar:") => return Err(e),
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) => Err(e),
        None => Ok(None),
    }
}

/// Fetch (or reuse the cached) ranking for a venue. Returns the ranking — an
/// empty `JournalRank` when the venue was queried but has no rank. Only genuine
/// query results are cached; a network/key error propagates without caching so
/// it can be retried. `force` bypasses the cache to re-query.
pub async fn fetch_venue_rank(
    root: &str,
    venue: &str,
    secret_key: &str,
    force: bool,
) -> Result<JournalRank, String> {
    let secret_key = secret_key.trim();
    if secret_key.is_empty() {
        return Err("未配置 easyScholar 密钥".to_string());
    }
    let venue = venue.trim();
    if venue.is_empty() {
        return Err("这篇论文没有期刊/会议信息".to_string());
    }
    // A withdrawn submission wasn't accepted — never attribute a venue rank to it.
    if venue.to_lowercase().contains("withdrawn") {
        return Ok(JournalRank {
            official: Default::default(),
            custom: Vec::new(),
            venue: venue.to_string(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
        });
    }
    let key = normalize_venue_key(venue);

    let mut cache = read_venue_ranks(root);
    if !force {
        if let Some(cached) = cache.get(&key) {
            return Ok(cached.clone());
        }
    }

    let resolved = resolve_venue_rank(secret_key, venue).await?;
    let rank = resolved.unwrap_or_else(|| JournalRank {
        official: Default::default(),
        custom: Vec::new(),
        venue: venue.to_string(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
    });
    cache.insert(key, rank.clone());
    write_venue_ranks(root, &cache)?;
    Ok(rank)
}

/// Query easyScholar once for a single publication name. Returns the parsed
/// (official, custom) ranks, `Ok(None)` when unranked, or `Err` on failure.
async fn query_easyscholar(
    client: &reqwest::Client,
    secret_key: &str,
    publication_name: &str,
) -> Result<Option<(std::collections::BTreeMap<String, String>, Vec<JournalCustomRank>)>, String> {
    // reqwest encodes the query params, so no manual URL encoding is needed even
    // for names containing '&', spaces, or CJK characters.
    let resp = client
        .get("https://www.easyscholar.cc/open/getPublicationRank")
        .query(&[("secretKey", secret_key), ("publicationName", publication_name)])
        .send()
        .await
        .map_err(|e| format!("easyScholar 请求失败: {e}"))?;
    let json: Value = crate::net::fetch_json_capped(resp, MAX_META_BYTES).await?;

    let code = json.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != 200 {
        let msg = json.get("msg").and_then(Value::as_str).unwrap_or("未知错误");
        // A key/auth error recurs for every candidate, so surface it and stop.
        // Any other non-200 (e.g. publication not found) just means THIS name
        // didn't match — return no result so the caller tries the next
        // candidate (e.g. "NeurIPS 2020" → "NeurIPS" → full name).
        let lower = msg.to_lowercase();
        if code == 40002 || lower.contains("key") || msg.contains("密钥") {
            return Err(format!("easyScholar: {msg}"));
        }
        return Ok(None);
    }
    let data = match json.get("data") {
        Some(d) if !d.is_null() => d,
        _ => return Ok(None),
    };

    // officialRank.all: dataset code -> level text (e.g. "sci" -> "Q2").
    let mut official = std::collections::BTreeMap::new();
    if let Some(all) = data.pointer("/officialRank/all").and_then(Value::as_object) {
        for (key, val) in all {
            let text = match val {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => continue,
            };
            if !text.is_empty() {
                official.insert(key.clone(), text);
            }
        }
    }

    // customRank: resolve each "uuid&&level" entry against rankInfo's datasets.
    let mut custom = Vec::new();
    if let Some(cr) = data.get("customRank") {
        let rank_info = cr.get("rankInfo").and_then(Value::as_array);
        let rank = cr.get("rank").and_then(Value::as_array);
        if let (Some(rank_info), Some(rank)) = (rank_info, rank) {
            for entry in rank.iter().filter_map(Value::as_str) {
                // Format is "<uuid>&&<n>" (docs also mention "&&&"); splitting on
                // '&' tolerates either separator width.
                let uuid = entry.split('&').next().unwrap_or("").trim();
                let level: usize = entry
                    .rsplit('&')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .parse()
                    .unwrap_or(0);
                if uuid.is_empty() || level == 0 {
                    continue;
                }
                let Some(info) = rank_info
                    .iter()
                    .find(|ri| ri.get("uuid").and_then(Value::as_str) == Some(uuid))
                else {
                    continue;
                };
                let name = info
                    .get("abbName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let field = match level {
                    1 => "oneRankText",
                    2 => "twoRankText",
                    3 => "threeRankText",
                    4 => "fourRankText",
                    5 => "fiveRankText",
                    _ => continue,
                };
                let rank_text = info
                    .get(field)
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if name.is_empty() || rank_text.is_empty() {
                    continue;
                }
                custom.push(JournalCustomRank {
                    name,
                    rank: rank_text,
                });
            }
        }
    }

    if official.is_empty() && custom.is_empty() {
        return Ok(None);
    }
    Ok(Some((official, custom)))
}

/// Common CS venue abbreviation → full name(s). Used to retry with the alternate
/// form when the given one has no ranking (and vice-versa).
const VENUE_ALIASES: &[(&str, &[&str])] = &[
    ("neurips", &["Neural Information Processing Systems", "Advances in Neural Information Processing Systems"]),
    ("nips", &["Neural Information Processing Systems", "Advances in Neural Information Processing Systems"]),
    ("icml", &["International Conference on Machine Learning"]),
    ("iclr", &["International Conference on Learning Representations"]),
    ("cvpr", &["IEEE Conference on Computer Vision and Pattern Recognition", "Computer Vision and Pattern Recognition"]),
    ("iccv", &["International Conference on Computer Vision"]),
    ("eccv", &["European Conference on Computer Vision"]),
    ("aaai", &["AAAI Conference on Artificial Intelligence"]),
    ("ijcai", &["International Joint Conference on Artificial Intelligence"]),
    ("acl", &["Annual Meeting of the Association for Computational Linguistics"]),
    ("emnlp", &["Conference on Empirical Methods in Natural Language Processing"]),
    ("naacl", &["North American Chapter of the Association for Computational Linguistics"]),
    ("kdd", &["ACM SIGKDD Conference on Knowledge Discovery and Data Mining", "Knowledge Discovery and Data Mining"]),
    ("sigir", &["International ACM SIGIR Conference on Research and Development in Information Retrieval"]),
    ("www", &["The Web Conference", "International World Wide Web Conference"]),
    ("aistats", &["International Conference on Artificial Intelligence and Statistics"]),
    ("coling", &["International Conference on Computational Linguistics"]),
    ("uai", &["Conference on Uncertainty in Artificial Intelligence"]),
    ("wacv", &["Winter Conference on Applications of Computer Vision"]),
    ("miccai", &["Medical Image Computing and Computer Assisted Intervention"]),
    ("icassp", &["International Conference on Acoustics, Speech and Signal Processing"]),
    ("interspeech", &["Conference of the International Speech Communication Association"]),
    ("mm", &["ACM Multimedia"]),
    ("acmmm", &["ACM Multimedia"]),
    ("iros", &["International Conference on Intelligent Robots and Systems"]),
    ("icra", &["International Conference on Robotics and Automation"]),
    ("sigmod", &["International Conference on Management of Data"]),
    ("vldb", &["International Conference on Very Large Data Bases", "Very Large Data Bases"]),
    ("icde", &["International Conference on Data Engineering"]),
    ("nsdi", &["Symposium on Networked Systems Design and Implementation"]),
    ("osdi", &["Symposium on Operating Systems Design and Implementation"]),
    ("sosp", &["Symposium on Operating Systems Principles"]),
];

/// Build a prioritized, de-duplicated list of query strings for a venue: the raw
/// name first, then a cleaned form, then abbreviation↔full-name expansions.
fn venue_query_candidates(venue: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    push_unique(&mut out, venue.trim().to_string());
    let cleaned = clean_venue(venue);
    push_unique(&mut out, cleaned.clone());
    expand_aliases(venue, &mut out);
    if !cleaned.eq_ignore_ascii_case(venue.trim()) {
        expand_aliases(&cleaned, &mut out);
    }
    out
}

fn push_unique(out: &mut Vec<String>, s: String) {
    let s = s.trim().to_string();
    if !s.is_empty() && !out.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        out.push(s);
    }
}

/// Strip decorations that stop easyScholar from matching a venue: a leading
/// "In "/"Proceedings of the", ordinals ("35th"), 4-digit years, a leading
/// "The", and collapsed whitespace/edge punctuation.
fn clean_venue(venue: &str) -> String {
    use std::sync::OnceLock;
    static PROC: OnceLock<Regex> = OnceLock::new();
    static ORDINAL: OnceLock<Regex> = OnceLock::new();
    static YEAR: OnceLock<Regex> = OnceLock::new();
    static SPACES: OnceLock<Regex> = OnceLock::new();
    let proc = PROC
        .get_or_init(|| Regex::new(r"(?i)^\s*(in\s+)?proc(eedings|\.)?\s+of\s+(the\s+)?").unwrap());
    let ordinal = ORDINAL.get_or_init(|| Regex::new(r"(?i)\b\d{1,3}(st|nd|rd|th)\b").unwrap());
    let year = YEAR.get_or_init(|| Regex::new(r"\b(19|20)\d{2}\b").unwrap());
    let spaces = SPACES.get_or_init(|| Regex::new(r"\s{2,}").unwrap());

    let mut s = venue.trim().to_string();
    s = proc.replace(&s, "").into_owned();
    s = ordinal.replace_all(&s, "").into_owned();
    s = year.replace_all(&s, "").into_owned();
    let trimmed = s.trim_start();
    if let Some(rest) = trimmed
        .strip_prefix("The ")
        .or_else(|| trimmed.strip_prefix("the "))
    {
        s = rest.to_string();
    }
    s = spaces.replace_all(&s, " ").into_owned();
    s.trim()
        .trim_matches(|c: char| c == ',' || c == '.' || c == ':' || c == '-' || c.is_whitespace())
        .to_string()
}

/// Add abbreviation↔full-name alternatives for `text` to `out`.
fn expand_aliases(text: &str, out: &mut Vec<String>) {
    let lower = text.trim().to_lowercase();
    // Whole-string abbreviation → full name(s).
    for (abbr, fulls) in VENUE_ALIASES {
        if lower == *abbr {
            for f in *fulls {
                push_unique(out, (*f).to_string());
            }
        }
    }
    // Stray abbreviation token (e.g. "…, ICML") → full name(s).
    for tok in text.split(|c: char| !c.is_ascii_alphanumeric()) {
        if tok.len() < 2 {
            continue;
        }
        let t = tok.to_lowercase();
        for (abbr, fulls) in VENUE_ALIASES {
            if t == *abbr {
                for f in *fulls {
                    push_unique(out, (*f).to_string());
                }
            }
        }
    }
    // Full name present → also try the abbreviation.
    for (abbr, fulls) in VENUE_ALIASES {
        for f in *fulls {
            if lower.contains(&f.to_lowercase()) {
                push_unique(out, abbr.to_uppercase());
            }
        }
    }
}

// ── Semantic Scholar references (citation graph) ────────────────────────────────

const S2_REF_FIELDS: &str = "title,year,venue,citationCount,externalIds,authors";
/// References for a heavily-cited paper can be a few hundred entries; cap higher
/// than the metadata cap but still bounded.
const MAX_REFS_BYTES: u64 = 25 * 1024 * 1024;

fn references_path(root: &str, slug: &str) -> PathBuf {
    paper::paper_dir(root, slug).join("references.json")
}

/// Read cached references (if any) for a paper. Never hits the network; returns
/// an empty vec when nothing is cached.
pub fn read_cached_references(root: &str, slug: &str) -> Vec<CitationRef> {
    std::fs::read_to_string(references_path(root, slug))
        .ok()
        .and_then(|c| serde_json::from_str::<Vec<CitationRef>>(&c).ok())
        .unwrap_or_default()
}

fn cache_references(root: &str, slug: &str, refs: &[CitationRef]) -> Result<(), String> {
    let content =
        serde_json::to_string_pretty(refs).map_err(|e| format!("Serialize references: {e}"))?;
    crate::fsutil::atomic_write_str(&references_path(root, slug), &content)
        .map_err(|e| format!("Write references.json: {e}"))
}

/// Resolve a Semantic Scholar paper identifier for a library paper: prefer the
/// stable arXiv id / DOI, else resolve via a title search.
async fn s2_resolve_paper_id(
    arxiv_id: Option<&str>,
    doi: Option<&str>,
    title: &str,
) -> Option<String> {
    if let Some(a) = arxiv_id.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(format!("arXiv:{a}"));
    }
    if let Some(d) = doi.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(format!("DOI:{d}"));
    }
    if title.split_whitespace().count() < 3 {
        return None;
    }
    let client = build_client();
    let resp = client
        .get("https://api.semanticscholar.org/graph/v1/paper/search")
        .query(&[("query", title), ("limit", "1"), ("fields", "title")])
        .send()
        .await
        .ok()?;
    let json: Value = crate::net::fetch_json_capped(resp, MAX_META_BYTES).await.ok()?;
    let pid = json
        .get("data")?
        .as_array()?
        .first()?
        .get("paperId")?
        .as_str()?;
    Some(pid.to_string())
}

/// Fetch the reference list (papers cited by this paper) from Semantic Scholar
/// and cache it. Library matching is applied separately by the caller so newly
/// added papers get linked without a re-fetch.
pub async fn fetch_references(root: &str, slug: &str) -> Result<Vec<CitationRef>, String> {
    let meta = paper::read_meta(root, slug)?;
    let pid = s2_resolve_paper_id(meta.arxiv_id.as_deref(), meta.doi.as_deref(), &meta.title)
        .await
        .ok_or_else(|| "无法在 Semantic Scholar 中定位这篇论文".to_string())?;

    let url = format!(
        "https://api.semanticscholar.org/graph/v1/paper/{}/references",
        pid
    );
    let client = build_client();
    let resp = client
        .get(&url)
        .query(&[("fields", S2_REF_FIELDS), ("limit", "1000")])
        .send()
        .await
        .map_err(|e| format!("Semantic Scholar 请求失败: {e}"))?;
    if resp.status().as_u16() == 429 {
        return Err("Semantic Scholar 请求过于频繁，请稍后再试".to_string());
    }
    if !resp.status().is_success() {
        return Err(format!("Semantic Scholar 返回错误：{}", resp.status()));
    }
    let json: Value = crate::net::fetch_json_capped(resp, MAX_REFS_BYTES).await?;
    let refs = parse_s2_references(&json);
    cache_references(root, slug, &refs)?;
    Ok(refs)
}

fn parse_s2_references(json: &Value) -> Vec<CitationRef> {
    let mut out = Vec::new();
    let Some(data) = json.get("data").and_then(Value::as_array) else {
        return out;
    };
    for item in data {
        // The references endpoint nests the cited paper under `citedPaper`.
        let Some(cp) = item.get("citedPaper").filter(|v| !v.is_null()) else {
            continue;
        };
        let title = cp
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if title.is_empty() {
            continue;
        }
        let authors = cp
            .get("authors")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("name").and_then(Value::as_str))
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let year = cp.get("year").and_then(Value::as_u64).map(|y| y as u32);
        let venue = cp
            .get("venue")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let cite_count = cp
            .get("citationCount")
            .and_then(Value::as_u64)
            .map(|c| c as u32);
        let ext = cp.get("externalIds");
        let doi = ext
            .and_then(|e| e.get("DOI"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let arxiv_id = ext
            .and_then(|e| e.get("ArXiv"))
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let paper_id = cp
            .get("paperId")
            .and_then(Value::as_str)
            .map(|s| s.to_string());
        out.push(CitationRef {
            paper_id,
            title,
            authors,
            year,
            venue,
            doi,
            arxiv_id,
            cite_count,
            library_slug: None,
            library_id: None,
        });
    }
    out
}

fn normalize_arxiv(id: &str) -> String {
    let mut s = id.trim().to_lowercase();
    if let Some(rest) = s.strip_prefix("arxiv:") {
        s = rest.to_string();
    }
    // Drop a trailing version suffix like "v2".
    if let Some(pos) = s.rfind('v') {
        if s[pos + 1..].chars().all(|c| c.is_ascii_digit()) && pos + 1 < s.len() {
            s.truncate(pos);
        }
    }
    s
}

fn normalize_title(title: &str) -> String {
    title
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Fill in `library_slug` / `library_id` for each reference that matches a paper
/// already in the library. Matches by arXiv id, then DOI, then normalized title.
pub fn match_references_to_library(root: &str, refs: &mut [CitationRef]) {
    use std::collections::HashMap;
    let mut by_arxiv: HashMap<String, (String, String)> = HashMap::new();
    let mut by_doi: HashMap<String, (String, String)> = HashMap::new();
    let mut by_title: HashMap<String, (String, String)> = HashMap::new();

    for (slug, _path) in paper::list_paper_dirs(root).unwrap_or_default() {
        let Ok(m) = paper::read_meta(root, &slug) else {
            continue;
        };
        let entry = (m.id.clone(), slug.clone());
        if let Some(a) = m.arxiv_id.as_deref().map(normalize_arxiv).filter(|s| !s.is_empty()) {
            by_arxiv.entry(a).or_insert_with(|| entry.clone());
        }
        if let Some(d) = m
            .doi
            .as_deref()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
        {
            by_doi.entry(d).or_insert_with(|| entry.clone());
        }
        let tn = normalize_title(&m.title);
        if !tn.is_empty() {
            by_title.entry(tn).or_insert(entry);
        }
    }

    for r in refs.iter_mut() {
        let hit = r
            .arxiv_id
            .as_deref()
            .map(normalize_arxiv)
            .filter(|s| !s.is_empty())
            .and_then(|k| by_arxiv.get(&k))
            .or_else(|| {
                r.doi
                    .as_deref()
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .and_then(|k| by_doi.get(&k))
            })
            .or_else(|| {
                let tn = normalize_title(&r.title);
                if tn.is_empty() {
                    None
                } else {
                    by_title.get(&tn)
                }
            });
        if let Some((id, slug)) = hit {
            r.library_id = Some(id.clone());
            r.library_slug = Some(slug.clone());
        } else {
            r.library_id = None;
            r.library_slug = None;
        }
    }
}

/// Extract metadata using AI only (manual trigger from context menu).
/// Requires fulltext to already be cached; sends the first 512 words to the AI model.
/// Emits:
///   "ai-meta-start" { slug, group_id, answer_id, prompt, provider_id, provider_name, model_id }
///   "paper-ai-chat-{answer_id}" { delta, done }  (streaming tokens)
///   "ai-meta-done"  { slug, answer_id }  or  "ai-meta-error" { slug, answer_id, error }
pub async fn fetch_metadata_with_ai(
    root: &str,
    slug: &str,
    app: &AppHandle,
) -> Result<PaperMeta, String> {
    // Require fulltext to already be extracted — do not run extraction on the fly.
    let root_c = root.to_string();
    let slug_c = slug.to_string();
    let text =
        tauri::async_runtime::spawn_blocking(move || extraction::read_fulltext(&root_c, &slug_c))
            .await
            .unwrap_or_default();

    if text.trim().is_empty() {
        return Err("全文尚未提取，请先右键论文提取全文后再使用 AI 元数据提取。".to_string());
    }

    // Load AI config — need both the AiProvider struct (for llm streaming) and the prompt.
    let settings = crate::settings::read_settings(root);
    let ai_settings = crate::ai_manager::read_ai_settings(root);

    let (provider_id, model_id) = match (
        settings.metadata_ai_provider_id.as_deref(),
        settings.metadata_ai_model_id.as_deref(),
    ) {
        (Some(pid), Some(mid)) if !pid.is_empty() && !mid.is_empty() => {
            (pid.to_string(), mid.to_string())
        }
        _ => {
            let pid = ai_settings
                .default_provider_id
                .clone()
                .ok_or("No AI provider configured.")?;
            let mid = ai_settings
                .default_model_id
                .clone()
                .ok_or("No default model configured.")?;
            (pid, mid)
        }
    };

    let provider = ai_settings
        .providers
        .iter()
        .find(|p| p.id == provider_id && p.enabled)
        .ok_or_else(|| format!("Provider '{provider_id}' not found or disabled."))?
        .clone();

    let api_key = crate::ai_manager::get_api_key(root, &provider_id)
        .ok_or_else(|| format!("No API key set for '{}'.", provider.name))?;

    // Build the prompt
    let snippet = leading_words(&text, 512);
    let prompt = render_metadata_prompt(&settings.metadata_ai_prompt, &snippet);

    // Generate IDs for the streamed conversation entry
    let group_id = uuid::Uuid::new_v4().to_string();
    let answer_id = uuid::Uuid::new_v4().to_string();

    // Tell the frontend to create a conversation entry and prepare to stream
    let _ = app.emit(
        "ai-meta-start",
        serde_json::json!({
            "slug": slug,
            "group_id": group_id,
            "answer_id": answer_id,
            "prompt": prompt,
            "provider_id": provider_id,
            "provider_name": provider.name,
            "model_id": model_id,
        }),
    );

    // Stream the AI response (emits paper-ai-chat-{answer_id} events)
    let event_name = format!("paper-ai-chat-{}", answer_id);
    let messages = vec![crate::models::ChatMessage {
        role: "user".to_string(),
        content: prompt.clone().into(),
    }];

    let raw = match crate::llm::chat_completion_stream(
        &provider,
        &api_key,
        &model_id,
        &messages,
        &event_name,
        app,
        false,
        None,
        "metadata",
        None,
    )
    .await
    {
        Ok(text) => {
            let _ = app.emit(
                "ai-meta-done",
                serde_json::json!({ "slug": slug, "answer_id": answer_id }),
            );
            text
        }
        Err(e) => {
            let _ = app.emit(
                "ai-meta-error",
                serde_json::json!({ "slug": slug, "answer_id": answer_id, "error": e }),
            );
            return Err(e);
        }
    };

    // Parse and apply metadata
    let update = parse_ai_content(&raw)
        .ok_or_else(|| format!("Could not parse metadata JSON from: {raw}"))?;

    let mut meta = crate::paper::read_meta(root, slug)?;
    if let Some(t) = update.title {
        if !t.is_empty() {
            meta.title = t;
        }
    }
    if let Some(a) = update.authors {
        if !a.is_empty() {
            meta.authors = a;
        }
    }
    if let Some(y) = update.year {
        meta.year = Some(y);
    }
    if update.venue.is_some() {
        meta.venue = update.venue;
    }
    if update.doi.is_some() {
        meta.doi = update.doi;
    }
    if update.arxiv_id.is_some() {
        meta.arxiv_id = update.arxiv_id;
    }

    crate::paper::write_meta(root, slug, &meta)?;

    let mut status = crate::paper::read_status_for(root, slug);
    status.metadata_fetched = true;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    crate::paper::write_status(root, slug, &status)?;

    Ok(meta)
}

// ── Slug generation ───────────────────────────────────────────────────────────

pub fn title_to_slug(title: &str, year: Option<u32>) -> String {
    // Remove characters that are invalid in folder names on most OSes
    let invalid = Regex::new(r#"[/\\:*?"<>|]"#).unwrap();
    let cleaned = invalid.replace_all(title, "");

    // Collapse whitespace/multiple-underscores to single underscore
    let ws = Regex::new(r"[\s ]+").unwrap();
    let underscored = ws.replace_all(&cleaned, "_");

    // Trim leading/trailing underscores and dots
    let trimmed = underscored
        .trim_matches(|c: char| c == '_' || c == '.')
        .to_string();

    // Limit title part length
    let max_len: usize = if year.is_some() { 70 } else { 78 };
    let title_part: String = trimmed.chars().take(max_len).collect();
    let title_part = title_part
        .trim_matches(|c: char| c == '_' || c == '.')
        .to_string();

    if title_part.is_empty() {
        return format!(
            "{}",
            uuid::Uuid::new_v4()
                .to_string()
                .split('-')
                .next()
                .unwrap_or("paper")
        );
    }

    if let Some(y) = year {
        format!("{}_{}", y, title_part)
    } else {
        title_part
    }
}

// ── Folder rename ─────────────────────────────────────────────────────────────

pub fn rename_folder(root: &str, old_slug: &str) -> Result<String, String> {
    let meta = crate::paper::read_meta(root, old_slug)?;
    let base_slug = title_to_slug(&meta.title, meta.year);

    // If the desired slug is the same as current, nothing to do
    if base_slug == old_slug {
        return Ok(old_slug.to_string());
    }

    let new_slug = if crate::paper::find_paper_dir(root, &base_slug).is_none() {
        base_slug
    } else {
        let suffix: String = uuid::Uuid::new_v4()
            .to_string()
            .replace('-', "")
            .chars()
            .take(6)
            .collect();
        format!("{}_{}", base_slug, suffix)
    };

    let old_path = crate::paper::paper_dir(root, old_slug);
    let parent = old_path
        .parent()
        .ok_or_else(|| "Current paper folder has no parent".to_string())?;
    let new_path = parent.join(&new_slug);

    if new_path.exists() {
        return Err(format!(
            "Target folder already exists: {}",
            new_path.display()
        ));
    }

    std::fs::rename(&old_path, &new_path).map_err(|e| format!("Failed to rename folder: {e}"))?;

    // Rename the PDF file inside to match the paper title
    let desired_pdf_name = format!(
        "{}.pdf",
        sanitize_filename(&meta.title)
            .chars()
            .take(150)
            .collect::<String>()
    );
    if let Ok(entries) = std::fs::read_dir(&new_path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("pdf") {
                let current_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if current_name != desired_pdf_name {
                    let _ = std::fs::rename(&p, new_path.join(&desired_pdf_name));
                }
                break;
            }
        }
    }

    Ok(new_slug)
}

// ── XML helpers ───────────────────────────────────────────────────────────────

fn extract_between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let si = s.find(start)? + start.len();
    let ei = s[si..].find(end)? + si;
    Some(&s[si..ei])
}

fn xml_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#xA;", "\n")
}

#[cfg(test)]
mod journal_rank_tests {
    use super::{clean_venue, venue_query_candidates};

    fn has(cands: &[String], needle: &str) -> bool {
        cands.iter().any(|c| c.eq_ignore_ascii_case(needle))
    }

    #[test]
    fn clean_venue_strips_proceedings_ordinal_and_year() {
        assert_eq!(
            clean_venue("Proceedings of the 35th International Conference on Machine Learning"),
            "International Conference on Machine Learning"
        );
        assert_eq!(clean_venue("NeurIPS 2020"), "NeurIPS");
        assert_eq!(
            clean_venue("In Proc. of the 42nd IEEE Symposium on Security and Privacy, 2021"),
            "IEEE Symposium on Security and Privacy"
        );
    }

    #[test]
    fn candidates_expand_abbreviation_to_full_name() {
        let c = venue_query_candidates("NeurIPS 2020");
        assert!(has(&c, "NeurIPS 2020")); // raw first
        assert!(has(&c, "NeurIPS")); // year stripped
        assert!(has(&c, "Neural Information Processing Systems")); // full name
    }

    #[test]
    fn candidates_expand_full_name_to_abbreviation() {
        let c = venue_query_candidates(
            "Proceedings of the 35th International Conference on Machine Learning",
        );
        assert!(has(&c, "International Conference on Machine Learning"));
        assert!(has(&c, "ICML"));
    }
}
