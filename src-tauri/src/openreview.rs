use std::path::Path;

use tauri::Emitter;

use crate::models::{PaperMeta, PaperStatus};
use crate::{collections, extraction, paper, search, settings};

// ── URL / ID parsing ──────────────────────────────────────────────────────────

/// Extract an OpenReview submission ID from a forum/pdf/abs URL.
/// Handles:
///   https://openreview.net/forum?id=0wSlFpMsGb
///   https://openreview.net/pdf?id=0wSlFpMsGb
///   https://openreview.net/abs?id=0wSlFpMsGb
///   0wSlFpMsGb  (bare ID)
pub fn parse_openreview_id(input: &str) -> Option<String> {
    let s = input.trim();

    // Try query parameter ?id=
    if let Some(pos) = s.find("?id=") {
        let after = &s[pos + 4..];
        let id = after.split(|c: char| c == '&' || c == '#').next()?.trim();
        if looks_like_or_id(id) {
            return Some(id.to_string());
        }
    }

    // Try openreview.net path segments
    if s.contains("openreview.net") {
        return None; // handled above via ?id=; malformed URL
    }

    // Bare ID: alphanumeric, 8-16 chars, no dots (distinguishes from arXiv)
    if looks_like_or_id(s) {
        return Some(s.to_string());
    }

    None
}

fn looks_like_or_id(s: &str) -> bool {
    let len = s.len();
    len >= 6
        && len <= 32
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && s.chars().any(|c| c.is_uppercase() || c.is_lowercase())
        && !s.contains('.') // arXiv IDs contain a dot; OR IDs don't
}

// ── OpenReview API response types ─────────────────────────────────────────────

#[derive(Debug)]
pub struct OrMeta {
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u32>,
    pub venue: Option<String>,
    pub venue_tag: Option<String>,
    pub abstract_text: Option<String>,
    #[allow(dead_code)]
    pub pdf_path: Option<String>,
    #[allow(dead_code)]
    pub keywords: Vec<String>,
}

/// Fetch paper metadata from the OpenReview v2 API.
pub async fn fetch_metadata(client: &reqwest::Client, id: &str) -> Result<OrMeta, String> {
    let url = format!("https://api2.openreview.net/notes?id={id}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Fetch OpenReview API: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Read API response: {e}"))?;

    let json: serde_json::Value =
        serde_json::from_str(&resp).map_err(|e| format!("Parse JSON: {e}"))?;

    let note = json["notes"]
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or_else(|| format!("OpenReview: paper '{id}' not found."))?;

    let content = &note["content"];

    let title = content["title"]["value"]
        .as_str()
        .unwrap_or("Untitled")
        .trim()
        .to_string();

    let authors: Vec<String> = content["authors"]["value"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    let keywords: Vec<String> = content["keywords"]["value"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    let venue_raw = content["venue"]["value"]
        .as_str()
        .map(|s| s.trim().to_string());

    // Extract year from venue string (e.g. "ICLR 2024 Oral" → 2024)
    let year = venue_raw
        .as_deref()
        .and_then(extract_year_from_venue)
        .or_else(|| {
            // Fallback: cdate timestamp (milliseconds)
            note["cdate"]
                .as_i64()
                .map(|ms| {
                    let secs = ms / 1000;
                    let dt = chrono::DateTime::from_timestamp(secs, 0)?;
                    Some(dt.format("%Y").to_string().parse::<u32>().ok()?)
                })
                .flatten()
        });

    // Detect presentation type tag
    let venue_tag = venue_raw.as_deref().and_then(extract_venue_tag);

    let pdf_path = content["pdf"]["value"].as_str().map(|s| s.to_string());

    let abstract_text = content["abstract"]["value"]
        .as_str()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    Ok(OrMeta {
        title,
        authors,
        year,
        venue: venue_raw,
        venue_tag,
        abstract_text,
        pdf_path,
        keywords,
    })
}

fn extract_year_from_venue(venue: &str) -> Option<u32> {
    // Find first 4-digit sequence that looks like a year (2000–2099)
    venue.split_whitespace().find_map(|w| {
        let digits: String = w.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.len() == 4 {
            let y: u32 = digits.parse().ok()?;
            if (2000..=2099).contains(&y) {
                Some(y)
            } else {
                None
            }
        } else {
            None
        }
    })
}

fn extract_venue_tag(venue: &str) -> Option<String> {
    let v = venue.to_lowercase();
    for tag in &[
        "oral",
        "spotlight",
        "poster",
        "notable",
        "outstanding",
        "award",
        "findings",
        "workshop",
    ] {
        if v.contains(tag) {
            let mut s = tag.to_string();
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            return Some(s);
        }
    }
    None
}

// ── Full import pipeline ──────────────────────────────────────────────────────

/// Import an OpenReview paper by URL. Downloads PDF and populates metadata.
pub async fn import_by_url(
    root: &str,
    url: &str,
    collection_id: &str,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let id = parse_openreview_id(url)
        .ok_or_else(|| format!("Could not find an OpenReview paper ID in: {url}"))?;

    let emit = |status: &str| {
        let _ = app.emit(
            "paper-url-import",
            serde_json::json!({ "id": id, "source": "openreview", "status": status }),
        );
    };

    emit("fetching");

    let client = reqwest::Client::builder()
        .user_agent("Argus/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Build HTTP client: {e}"))?;

    let meta = fetch_metadata(&client, &id).await?;

    // ── Download PDF ──────────────────────────────────────────────────────────
    emit("downloading");

    // Standard pattern: https://openreview.net/pdf?id={id}
    let pdf_url = format!("https://openreview.net/pdf?id={id}");
    let pdf_bytes = client
        .get(&pdf_url)
        .send()
        .await
        .map_err(|e| format!("Download PDF: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Read PDF bytes: {e}"))?;

    emit("importing");

    // ── Build canonical slug ──────────────────────────────────────────────────
    let year_str = meta.year.map(|y| y.to_string()).unwrap_or_default();
    let last_name = meta
        .authors
        .first()
        .and_then(|a| a.split_whitespace().last())
        .unwrap_or("unknown");
    let title_words: String = meta
        .title
        .split_whitespace()
        .take(5)
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let short_id: String = uuid::Uuid::new_v4()
        .to_string()
        .replace('-', "")
        .chars()
        .take(8)
        .collect();
    let slug_base = sanitize(&format!("{last_name}-{year_str}-{title_words}-{short_id}"));

    // ── Create paper directory ────────────────────────────────────────────────
    let papers_dir = Path::new(root).join("papers");
    let final_dir = {
        let candidate = papers_dir.join(&slug_base);
        if candidate.exists() {
            papers_dir.join(format!("{slug_base}-2"))
        } else {
            candidate
        }
    };
    let final_slug = final_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&slug_base)
        .to_string();

    std::fs::create_dir_all(&final_dir).map_err(|e| format!("Create paper directory: {e}"))?;
    std::fs::write(final_dir.join("paper.pdf"), &pdf_bytes)
        .map_err(|e| format!("Write PDF: {e}"))?;

    // ── Write metadata ────────────────────────────────────────────────────────
    // Append venue tag (Oral / Spotlight / …) to venue string for prominence
    let venue_display = match (&meta.venue, &meta.venue_tag) {
        (Some(v), _) => Some(v.clone()),
        (None, Some(tag)) => Some(tag.clone()),
        _ => None,
    };

    let paper_id = uuid::Uuid::new_v4().to_string();
    let paper_meta = PaperMeta {
        id: paper_id.clone(),
        title: meta.title,
        authors: meta.authors,
        year: meta.year,
        doi: None,
        arxiv_id: None,
        venue: venue_display,
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(format!("{id}.pdf")),
        reading_status: "unread".to_string(),
        paper_abstract: meta.abstract_text,
        bibtex: None,
        canvas_notes: vec![],
    };
    paper::write_meta(root, &final_slug, &paper_meta)?;
    paper::ensure_paper_files(root, &final_slug);

    let mut status = PaperStatus::default();
    status.metadata_fetched = true;
    paper::write_status(root, &final_slug, &status)?;
    let _ = search::index_paper(root, &final_slug);

    // ── Collection assignment ─────────────────────────────────────────────────
    if !collection_id.is_empty() {
        collections::move_paper_to_collection(root, &paper_id, collection_id)?;
    }

    // ── Best-effort fulltext extraction + FTS ─────────────────────────────────
    let s = settings::read_settings(root);
    let root_c = root.to_string();
    let slug_c = final_slug.clone();
    if let Ok(result) = tauri::async_runtime::spawn_blocking(move || {
        extraction::extract_and_write(&root_c, &slug_c, &s)
    })
    .await
    {
        if matches!(result, extraction::ExtractionResult::Text) {
            let root_c = root.to_string();
            let slug_c = final_slug.clone();
            let _ =
                tauri::async_runtime::spawn_blocking(move || search::index_paper(&root_c, &slug_c))
                    .await;
        }
    }

    // ── Notify UI ─────────────────────────────────────────────────────────────
    let _ = app.emit("library-updated", serde_json::json!({ "slug": final_slug }));
    emit("done");

    Ok(final_slug)
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}
