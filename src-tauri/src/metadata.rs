use std::path::PathBuf;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::{extraction, models::PaperMeta, paper};

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
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent("Argus/0.1 (local-first literature manager)")
        .build()
        .expect("Failed to build HTTP client")
}

// ── arXiv API ─────────────────────────────────────────────────────────────────

pub async fn fetch_arxiv(arxiv_id: &str) -> Option<MetaUpdate> {
    let url = format!("https://export.arxiv.org/api/query?id_list={}", arxiv_id);
    let client = build_client();
    let xml = client.get(&url).send().await.ok()?.text().await.ok()?;

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
    })
}

// ── Crossref API ──────────────────────────────────────────────────────────────

pub async fn fetch_crossref(doi: &str) -> Option<MetaUpdate> {
    let url = format!("https://api.crossref.org/works/{}", doi);
    let client = build_client();
    let json: Value = client.get(&url).send().await.ok()?.json().await.ok()?;
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
        "https://api.semanticscholar.org/graph/v1/paper/{}?fields=title,authors,year,venue,abstract,externalIds",
        paper_id
    );
    let client = build_client();
    let json: Value = client.get(&url).send().await.ok()?.json().await.ok()?;
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

    Some(MetaUpdate {
        title: Some(title),
        authors,
        year,
        doi,
        arxiv_id,
        venue,
        paper_abstract,
    })
}

// ── Semantic Scholar title search ─────────────────────────────────────────────

pub async fn fetch_semantic_scholar_by_title(title: &str) -> Option<MetaUpdate> {
    let client = build_client();
    let json: Value = client
        .get("https://api.semanticscholar.org/graph/v1/paper/search")
        .query(&[
            ("query", title),
            ("fields", "title,authors,year,venue,abstract,externalIds"),
            ("limit", "3"),
        ])
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
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
    let json: Value = client
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
        .ok()?
        .json()
        .await
        .ok()?;
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
        content: prompt.clone(),
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
