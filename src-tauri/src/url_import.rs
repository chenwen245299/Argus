/// Unified URL import dispatcher.
///
/// Routes to the appropriate importer based on URL pattern.
/// Add new sources here; one file handles all of them.
///
/// Currently supported:
///   - ACL Anthology   (aclanthology.org)
///   - OpenReview      (openreview.net)
///   - arXiv           (arxiv.org, or bare IDs — default fallback)
pub async fn import_by_url(
    root: &str,
    url: &str,
    collection_id: &str,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let u = url.trim().to_ascii_lowercase();

    if u.ends_with(".pdf") || u.contains(".pdf?") {
        // Direct PDF link — download and add with filename-derived title
        return pdf_url::import(root, url, collection_id, app).await;
    }

    if u.contains("aclanthology.org") {
        acl::import(root, url, collection_id, app).await
    } else if u.contains("openreview.net") {
        openreview::import(root, url, collection_id, app).await
    } else {
        // Default: arXiv (handles arxiv.org URLs and bare IDs)
        crate::arxiv::import_by_url(root, url, collection_id, app).await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Direct PDF URL import
// ─────────────────────────────────────────────────────────────────────────────

mod pdf_url {
    use super::*;

    /// Derive a human-readable title from the last path segment of a URL.
    fn title_from_url(url: &str) -> String {
        let path = url.split('?').next().unwrap_or(url);
        let last = path.split('/').last().unwrap_or("paper");
        let stem = last.trim_end_matches(".pdf").trim_end_matches(".PDF");
        // Replace common separators with spaces and title-case
        let readable = stem
            .replace(['-', '_', '+'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if readable.is_empty() {
            "Imported Paper".to_string()
        } else {
            readable
        }
    }

    pub async fn import(
        root: &str,
        url: &str,
        collection_id: &str,
        app: &tauri::AppHandle,
    ) -> Result<String, String> {
        use tauri::Emitter;

        let emit = |s: &str| {
            let _ = app.emit(
                "paper-url-import",
                serde_json::json!({ "source": "pdf", "status": s }),
            );
        };
        emit("downloading");

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Argus/1.0)")
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;

        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| format!("Download PDF: {e}"))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {} from {url}", resp.status()));
        }

        let pdf_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Read PDF bytes: {e}"))?;

        if !pdf_bytes.starts_with(b"%PDF") {
            return Err(format!("URL did not return a valid PDF: {url}"));
        }

        emit("importing");

        let title = title_from_url(url);
        let slug_base = super::build_slug(&[], None, &title);
        let papers_dir = Path::new(root).join("papers");
        let final_dir = {
            let c = papers_dir.join(&slug_base);
            if c.exists() { papers_dir.join(format!("{slug_base}-2")) } else { c }
        };
        let final_slug = final_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&slug_base)
            .to_string();

        std::fs::create_dir_all(&final_dir).map_err(|e| format!("mkdir: {e}"))?;
        std::fs::write(final_dir.join("paper.pdf"), &pdf_bytes)
            .map_err(|e| format!("write PDF: {e}"))?;

        // Derive an original filename from the URL
        let original_filename = url
            .split('?').next().unwrap_or(url)
            .split('/').last()
            .map(|s| s.to_string());

        let paper_meta = PaperMeta {
            id: uuid::Uuid::new_v4().to_string(),
            title,
            authors: vec![],
            year: None,
            doi: None,
            arxiv_id: None,
            venue: None,
            tags: vec![],
            added_at: chrono::Utc::now().to_rfc3339(),
            original_filename,
            reading_status: "unread".to_string(),
            paper_abstract: None,
            bibtex: None,
            canvas_notes: vec![],
            import_source: Some("url".to_string()),
            cite_count: None,
        };

        super::finalize_paper(root, &final_dir, &final_slug, paper_meta, collection_id, app, "pdf").await?;
        Ok(final_slug)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers
// ─────────────────────────────────────────────────────────────────────────────

use crate::models::{PaperMeta, PaperStatus};
use crate::{collections, extraction, paper, search, settings};
use std::path::Path;
use tauri::Emitter;

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

fn build_slug(authors: &[String], year: Option<u32>, title: &str) -> String {
    let year_str = year.map(|y| y.to_string()).unwrap_or_default();
    let last_name = authors
        .first()
        .and_then(|a| a.split_whitespace().last())
        .unwrap_or("unknown");
    let title_words: String = title
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
    sanitize(&format!("{last_name}-{year_str}-{title_words}-{short_id}"))
}

/// Write all paper files, index, and optionally assign to a collection.
async fn finalize_paper(
    root: &str,
    final_dir: &Path,
    final_slug: &str,
    meta: PaperMeta,
    collection_id: &str,
    app: &tauri::AppHandle,
    source_tag: &str,
) -> Result<(), String> {
    let paper_id = meta.id.clone();
    paper::write_meta(root, final_slug, &meta)?;
    paper::ensure_paper_files(root, final_slug);

    let mut status = PaperStatus::default();
    status.metadata_fetched = true;
    paper::write_status(root, final_slug, &status)?;
    let _ = search::index_paper(root, final_slug);

    if !collection_id.is_empty() {
        collections::move_paper_to_collection(root, &paper_id, collection_id)?;
    }

    // Best-effort fulltext extraction
    let s = settings::read_settings(root);
    let root_c = root.to_string();
    let slug_c = final_slug.to_string();
    if let Ok(result) = tauri::async_runtime::spawn_blocking(move || {
        extraction::extract_and_write(&root_c, &slug_c, &s)
    })
    .await
    {
        if matches!(result, extraction::ExtractionResult::Text) {
            let root_c = root.to_string();
            let slug_c = final_slug.to_string();
            let _ =
                tauri::async_runtime::spawn_blocking(move || search::index_paper(&root_c, &slug_c))
                    .await;
        }
    }

    let _ = app.emit("library-updated", serde_json::json!({ "slug": final_slug }));
    let _ = app.emit(
        "paper-url-import",
        serde_json::json!({ "source": source_tag, "status": "done" }),
    );

    let _ = final_dir; // suppress unused warning
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// ACL Anthology
// ─────────────────────────────────────────────────────────────────────────────

mod acl {
    use super::*;

    pub fn parse_id(input: &str) -> Option<String> {
        let s = input.trim().trim_end_matches('/');

        if let Some(pos) = s.find("aclanthology.org/") {
            let after = &s[pos + "aclanthology.org/".len()..];
            let id = after.split(['?', '#']).next()?.trim_end_matches('/').trim();
            if looks_like_id(id) {
                return Some(id.to_string());
            }
            return None;
        }

        if looks_like_id(s) {
            return Some(s.to_string());
        }
        None
    }

    fn looks_like_id(s: &str) -> bool {
        // Modern: YYYY.venue-type.num  e.g. 2020.emnlp-main.731
        let parts: Vec<&str> = s.splitn(3, '.').collect();
        if parts.len() == 3 {
            let y = parts[0];
            if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) {
                let year: u32 = y.parse().unwrap_or(0);
                if (1990..=2040).contains(&year) {
                    return !parts[1].is_empty() && !parts[2].is_empty();
                }
            }
        }
        // Legacy: [A-Z]\d\d-\d+  e.g. P19-1001
        let chars: Vec<char> = s.chars().collect();
        chars.len() >= 6
            && chars[0].is_ascii_uppercase()
            && chars[1].is_ascii_digit()
            && chars[2].is_ascii_digit()
            && chars[3] == '-'
            && chars[4..].iter().all(|c| c.is_ascii_digit())
    }

    struct Meta {
        title: String,
        authors: Vec<String>,
        year: Option<u32>,
        venue: Option<String>,
        abstract_text: Option<String>,
        doi: Option<String>,
    }

    async fn fetch_meta(client: &reqwest::Client, id: &str) -> Result<Meta, String> {
        let url = format!("https://aclanthology.org/{id}.bib");
        let bib = client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Fetch BibTeX: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Read BibTeX: {e}"))?;

        if bib.trim().is_empty() {
            return Err(format!("ACL Anthology returned empty response for '{id}'"));
        }
        parse_bib(&bib)
    }

    fn extract_field(bib: &str, field: &str) -> Option<String> {
        let bib_lower = bib.to_ascii_lowercase();
        let needle = format!("\n    {} = ", field.to_ascii_lowercase());

        let pos = bib_lower.find(&needle).map(|p| p + needle.len())?;
        let rest = &bib[pos..];

        match rest.chars().next()? {
            '{' => {
                let mut depth = 0i32;
                let mut result = String::new();
                for c in rest.chars() {
                    match c {
                        '{' => {
                            depth += 1;
                            if depth > 1 {
                                result.push(c);
                            }
                        }
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            result.push(c);
                        }
                        _ => {
                            if depth > 0 {
                                result.push(c);
                            }
                        }
                    }
                }
                Some(result.trim().to_string())
            }
            '"' => {
                let inner = &rest[1..];
                let mut result = String::new();
                let mut prev = ' ';
                for c in inner.chars() {
                    if c == '"' && prev != '\\' {
                        break;
                    }
                    result.push(c);
                    prev = c;
                }
                Some(result.trim().to_string())
            }
            _ => {
                let end = rest.find([',', '\n', '}'])?;
                Some(rest[..end].trim().to_string())
            }
        }
    }

    fn parse_authors(raw: &str) -> Vec<String> {
        let normalized: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        normalized
            .split(" and ")
            .map(|a| {
                let a = a.trim();
                if let Some(comma) = a.find(',') {
                    let last = a[..comma].trim();
                    let first = a[comma + 1..].trim();
                    if first.is_empty() {
                        last.to_string()
                    } else {
                        format!("{first} {last}")
                    }
                } else {
                    a.to_string()
                }
            })
            .filter(|a| !a.is_empty())
            .collect()
    }

    fn parse_bib(bib: &str) -> Result<Meta, String> {
        let title = extract_field(bib, "title")
            .unwrap_or_else(|| "Unknown Title".to_string())
            .replace(['{', '}'], "");
        let authors = parse_authors(&extract_field(bib, "author").unwrap_or_default());
        let year = extract_field(bib, "year").and_then(|y| y.trim_matches('"').parse::<u32>().ok());
        let venue = extract_field(bib, "booktitle").or_else(|| extract_field(bib, "journal"));
        let doi = extract_field(bib, "doi");
        let abstract_text = extract_field(bib, "abstract").filter(|s| !s.is_empty());
        Ok(Meta {
            title,
            authors,
            year,
            venue,
            abstract_text,
            doi,
        })
    }

    pub async fn import(
        root: &str,
        url: &str,
        collection_id: &str,
        app: &tauri::AppHandle,
    ) -> Result<String, String> {
        let id = parse_id(url)
            .ok_or_else(|| format!("Could not find an ACL Anthology paper ID in: {url}"))?;

        let emit = |s: &str| {
            let _ = app.emit(
                "paper-url-import",
                serde_json::json!({ "id": &id, "source": "acl", "status": s }),
            );
        };
        emit("fetching");

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Argus/0.1)")
            .timeout(std::time::Duration::from_secs(40))
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;

        let meta = fetch_meta(&client, &id).await?;

        emit("downloading");
        let pdf_resp = client
            .get(format!("https://aclanthology.org/{id}.pdf"))
            .send()
            .await
            .map_err(|e| format!("Download PDF: {e}"))?;
        if !pdf_resp.status().is_success() {
            return Err(format!("PDF unavailable: HTTP {}", pdf_resp.status()));
        }
        let pdf_bytes = pdf_resp
            .bytes()
            .await
            .map_err(|e| format!("Read PDF: {e}"))?;

        emit("importing");

        let slug_base = super::build_slug(&meta.authors, meta.year, &meta.title);
        let papers_dir = Path::new(root).join("papers");
        let final_dir = {
            let c = papers_dir.join(&slug_base);
            if c.exists() {
                papers_dir.join(format!("{slug_base}-2"))
            } else {
                c
            }
        };
        let final_slug = final_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&slug_base)
            .to_string();

        std::fs::create_dir_all(&final_dir).map_err(|e| format!("mkdir: {e}"))?;
        std::fs::write(final_dir.join("paper.pdf"), &pdf_bytes)
            .map_err(|e| format!("write PDF: {e}"))?;

        let paper_meta = PaperMeta {
            id: uuid::Uuid::new_v4().to_string(),
            title: meta.title,
            authors: meta.authors,
            year: meta.year,
            doi: meta.doi,
            arxiv_id: None,
            venue: meta.venue,
            tags: vec![],
            added_at: chrono::Utc::now().to_rfc3339(),
            original_filename: Some(format!("{id}.pdf")),
            reading_status: "unread".to_string(),
            paper_abstract: meta.abstract_text,
            bibtex: None,
            canvas_notes: vec![],
            import_source: Some("url".to_string()),
            cite_count: None,
        };

        super::finalize_paper(
            root,
            &final_dir,
            &final_slug,
            paper_meta,
            collection_id,
            app,
            "acl",
        )
        .await?;
        Ok(final_slug)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OpenReview
// ─────────────────────────────────────────────────────────────────────────────

mod openreview {
    use super::*;

    /// Extract the `?id=` value from any openreview.net URL (forum or pdf).
    pub fn parse_id(input: &str) -> Option<String> {
        let s = input.trim();
        if let Some(pos) = s.find("?id=") {
            let after = &s[pos + 4..];
            let id = after.split(|c: char| c == '&' || c == '#').next()?.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
        None
    }

    struct Meta {
        title: String,
        authors: Vec<String>,
        year: Option<u32>,
        venue: Option<String>,
        abstract_text: Option<String>,
        pdf_url: String,
    }

    fn html_unescape(s: &str) -> String {
        s.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#x27;", "'")
            .replace("&#39;", "'")
            .replace("&apos;", "'")
    }

    /// Extract the `content="..."` value from the first matching `<meta name="...">` tag.
    fn meta_content(html: &str, name: &str) -> Option<String> {
        let pat = format!(r#"name="{name}" content=""#);
        let pos = html.find(pat.as_str())?;
        let after = &html[pos + pat.len()..];
        let end = after.find('"')?;
        let val = after[..end].trim();
        if val.is_empty() { None } else { Some(html_unescape(val)) }
    }

    /// Extract all `content="..."` values from `<meta name="...">` tags (e.g. multiple authors).
    fn meta_content_all(html: &str, name: &str) -> Vec<String> {
        let pat = format!(r#"name="{name}" content=""#);
        let mut out = vec![];
        let mut rest = html;
        while let Some(pos) = rest.find(pat.as_str()) {
            let after = &rest[pos + pat.len()..];
            if let Some(end) = after.find('"') {
                let val = after[..end].trim();
                if !val.is_empty() {
                    out.push(html_unescape(val));
                }
            }
            rest = &rest[pos + pat.len()..];
        }
        out
    }

    async fn fetch_meta(client: &reqwest::Client, id: &str) -> Result<Meta, String> {
        let forum_url = format!("https://openreview.net/forum?id={id}");
        let html = client
            .get(&forum_url)
            .send()
            .await
            .map_err(|e| format!("Fetch OpenReview page: {e}"))?
            .text()
            .await
            .map_err(|e| format!("Read page: {e}"))?;

        let title = meta_content(&html, "citation_title")
            .ok_or_else(|| format!("OpenReview: could not find title for '{id}'"))?;
        let authors = meta_content_all(&html, "citation_author");
        let pdf_url = meta_content(&html, "citation_pdf_url")
            .unwrap_or_else(|| format!("https://openreview.net/pdf?id={id}"));
        let venue = meta_content(&html, "citation_conference_title");
        let abstract_text = meta_content(&html, "citation_abstract")
            .or_else(|| meta_content(&html, "description"))
            .filter(|s| !s.is_empty());
        // citation_online_date is "YYYY/MM/DD"
        let year = meta_content(&html, "citation_online_date")
            .and_then(|d| d.split('/').next().and_then(|y| y.parse::<u32>().ok()));

        Ok(Meta { title, authors, year, venue, abstract_text, pdf_url })
    }

    pub async fn import(
        root: &str,
        url: &str,
        collection_id: &str,
        app: &tauri::AppHandle,
    ) -> Result<String, String> {
        let id = parse_id(url)
            .ok_or_else(|| format!("Could not find an OpenReview paper ID in: {url}"))?;

        let emit = |s: &str| {
            let _ = app.emit(
                "paper-url-import",
                serde_json::json!({ "id": &id, "source": "openreview", "status": s }),
            );
        };
        emit("fetching");

        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Argus/1.0)")
            .timeout(std::time::Duration::from_secs(40))
            .build()
            .map_err(|e| format!("HTTP client: {e}"))?;

        let meta = fetch_meta(&client, &id).await?;

        emit("downloading");
        let resp = client
            .get(&meta.pdf_url)
            .send()
            .await
            .map_err(|e| format!("Download PDF: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("PDF unavailable: HTTP {}", resp.status()));
        }
        let pdf_bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Read PDF: {e}"))?;

        emit("importing");

        let slug_base = super::build_slug(&meta.authors, meta.year, &meta.title);
        let papers_dir = Path::new(root).join("papers");
        let final_dir = {
            let c = papers_dir.join(&slug_base);
            if c.exists() { papers_dir.join(format!("{slug_base}-2")) } else { c }
        };
        let final_slug = final_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&slug_base)
            .to_string();

        std::fs::create_dir_all(&final_dir).map_err(|e| format!("mkdir: {e}"))?;
        std::fs::write(final_dir.join("paper.pdf"), &pdf_bytes)
            .map_err(|e| format!("write PDF: {e}"))?;

        let paper_meta = PaperMeta {
            id: uuid::Uuid::new_v4().to_string(),
            title: meta.title,
            authors: meta.authors,
            year: meta.year,
            doi: None,
            arxiv_id: None,
            venue: meta.venue,
            tags: vec![],
            added_at: chrono::Utc::now().to_rfc3339(),
            original_filename: Some(format!("{id}.pdf")),
            reading_status: "unread".to_string(),
            paper_abstract: meta.abstract_text,
            bibtex: None,
            canvas_notes: vec![],
            import_source: Some("url".to_string()),
            cite_count: None,
        };

        super::finalize_paper(
            root, &final_dir, &final_slug, paper_meta, collection_id, app, "openreview",
        )
        .await?;
        Ok(final_slug)
    }
}
