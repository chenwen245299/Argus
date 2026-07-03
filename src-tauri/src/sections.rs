//! Paper section (chapter) structure: storage + AI-based segmentation.
//!
//! Structural detection (embedded PDF outline + heading heuristics) runs in the
//! frontend (pdf.js has font/layout info) and is persisted here via `save_sections`.
//! This module owns the on-disk format and the LLM fallback (`ai_split_sections`),
//! which is only ever triggered manually from the paper context menu.

use serde::{Deserialize, Serialize};

use crate::models::ChatMessage;

/// A single section heading in reading order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperSection {
    pub title: String,
    /// 1 = top-level section, 2 = subsection, 3 = sub-subsection.
    #[serde(default = "default_level")]
    pub level: u8,
    /// 1-based page number to jump to. 0 = unknown (jump disabled in the UI).
    #[serde(default)]
    pub page: u32,
}

fn default_level() -> u8 {
    1
}

/// The persisted section index for one paper (`sections.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperSections {
    /// How the sections were derived: "outline" | "heuristic" | "ai".
    pub source: String,
    #[serde(default)]
    pub generated_at: String,
    pub sections: Vec<PaperSection>,
}

fn sections_path(root: &str, slug: &str) -> std::path::PathBuf {
    crate::paper::paper_dir(root, slug).join("sections.json")
}

/// Read the stored section index, or `None` if absent / unparseable.
pub fn read_sections(root: &str, slug: &str) -> Option<PaperSections> {
    let raw = std::fs::read_to_string(sections_path(root, slug)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Persist a section index atomically.
pub fn write_sections(root: &str, slug: &str, data: &PaperSections) -> Result<(), String> {
    let raw =
        serde_json::to_string_pretty(data).map_err(|e| format!("serialize sections: {e}"))?;
    crate::fsutil::atomic_write_str(&sections_path(root, slug), &raw)
        .map_err(|e| format!("write sections.json: {e}"))
}

// ── Selected-section context extraction ──────────────────────────────────────

/// Extract the text of the selected sections, in document order, each prefixed
/// with a "## {title}" header. Locates each heading in the stored full text
/// (independent of page numbers, so it works even when page resolution failed)
/// and takes the lines up to the next heading. Returns "" when nothing usable.
pub fn extract_selected_sections_text(root: &str, slug: &str, selected: &[String]) -> String {
    if selected.is_empty() {
        return String::new();
    }
    let Some(index) = read_sections(root, slug) else {
        return String::new();
    };
    let secs = &index.sections;
    if secs.is_empty() {
        return String::new();
    }

    let fulltext = crate::extraction::read_fulltext(root, slug);
    if fulltext.trim().is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = fulltext.lines().collect();
    let norm_lines: Vec<String> = lines.iter().map(|l| normalize(l)).collect();

    // Locate each heading's line, scanning forward only so we don't match the
    // same wording in an earlier table-of-contents / abstract.
    let mut start_line: Vec<Option<usize>> = vec![None; secs.len()];
    let mut cursor = 0usize;
    for (i, sec) in secs.iter().enumerate() {
        let full = normalize(&sec.title);
        let key = content_key(&sec.title);
        let short: String = key.split(' ').take(5).collect::<Vec<_>>().join(" ");
        let found = (cursor..norm_lines.len()).find(|&li| {
            let l = &norm_lines[li];
            (!full.is_empty() && l.contains(&full))
                || (!key.is_empty() && l.contains(&key))
                || (short.len() > 3 && l.contains(&short))
        });
        if let Some(li) = found {
            start_line[i] = Some(li);
            cursor = li + 1;
        }
    }

    let selected_set: std::collections::HashSet<&str> =
        selected.iter().map(|s| s.as_str()).collect();
    // De-dup emitted lines so a selected parent and its selected children (which
    // overlap) don't repeat the same text.
    let mut emitted: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut out = String::new();
    for (i, sec) in secs.iter().enumerate() {
        if !selected_set.contains(sec.title.as_str()) {
            continue;
        }
        let Some(start) = start_line[i] else {
            continue;
        };
        // End at the next located heading that begins after this one.
        let end = start_line
            .iter()
            .skip(i + 1)
            .flatten()
            .copied()
            .filter(|&s| s > start)
            .min()
            .unwrap_or(lines.len());
        let mut body = String::new();
        for li in start..end {
            if !emitted.insert(li) {
                continue;
            }
            body.push_str(lines[li]);
            body.push('\n');
        }
        let body = body.trim();
        if body.is_empty() {
            continue;
        }
        out.push_str(&format!("## {}\n{body}\n\n", sec.title));
    }
    out.trim().to_string()
}

// ── AI fallback ──────────────────────────────────────────────────────────────

/// Ask the configured AI provider to segment the paper into sections.
/// Resolves a best-effort page number for each returned heading by matching it
/// against per-page extracted text. Persists and returns the result.
pub async fn ai_split_sections(
    root: &str,
    slug: &str,
    provider_id: Option<&str>,
    model_id: Option<&str>,
) -> Result<PaperSections, String> {
    let fulltext = crate::extraction::read_fulltext(root, slug);
    if fulltext.trim().is_empty() {
        return Err("No extracted full text available. Extract the full text first.".to_string());
    }
    let context = truncate_chars(&fulltext, 80_000);

    let (provider, api_key, model) =
        crate::ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let system = "You are an expert at analyzing the structure of academic papers. \
Given the full text of a paper, list its section headings in reading order. \
Return ONLY a compact JSON array — no markdown fences, no commentary. \
Each element is an object: {\"title\": string, \"level\": number}. \
level 1 = top-level section (e.g. Abstract, Introduction, Related Work, Method, \
Experiments, Results, Discussion, Conclusion, References, Appendix), \
level 2 = subsection, level 3 = sub-subsection. \
Use the exact heading wording as it appears in the text and do not invent headings \
that are not present.";

    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: system.into(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!("Paper full text:\n\n{context}").into(),
        },
    ];

    let raw = crate::llm::chat_completion(&provider, &api_key, &model, &messages, "summary")
        .await
        .map_err(|e| format!("AI call failed: {e}"))?;

    let mut sections = parse_sections_json(&raw)?;
    if sections.is_empty() {
        return Err("AI did not return any sections.".to_string());
    }

    resolve_pages(root, slug, &mut sections);

    let data = PaperSections {
        source: "ai".to_string(),
        generated_at: chrono::Utc::now().to_rfc3339(),
        sections,
    };
    write_sections(root, slug, &data)?;
    Ok(data)
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect()
}

#[derive(Deserialize)]
struct RawSection {
    title: String,
    #[serde(default = "default_level")]
    level: u8,
}

/// Extract the JSON array from a model response that may be wrapped in prose or
/// ```json code fences, then parse it into sections (page filled in later).
fn parse_sections_json(raw: &str) -> Result<Vec<PaperSection>, String> {
    let start = raw.find('[');
    let end = raw.rfind(']');
    let json = match (start, end) {
        (Some(s), Some(e)) if e > s => &raw[s..=e],
        _ => return Err("AI response did not contain a JSON array.".to_string()),
    };
    let parsed: Vec<RawSection> =
        serde_json::from_str(json).map_err(|e| format!("parse AI sections: {e}"))?;
    Ok(parsed
        .into_iter()
        .filter(|r| !r.title.trim().is_empty())
        .map(|r| PaperSection {
            title: r.title.trim().to_string(),
            level: r.level.clamp(1, 3),
            page: 0,
        })
        .collect())
}

/// Fill in a 1-based page number for each section by locating its (normalized)
/// title in the per-page extracted text. Leaves page = 0 when not found.
fn resolve_pages(root: &str, slug: &str, sections: &mut [PaperSection]) {
    let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
    let Ok(bytes) = std::fs::read(&pdf_path) else {
        return;
    };
    let Ok(doc) = lopdf::Document::load_mem(&bytes) else {
        return;
    };
    let mut page_nums: Vec<u32> = doc.get_pages().keys().copied().collect();
    page_nums.sort_unstable();
    let page_texts: Vec<(u32, String)> = page_nums
        .iter()
        .map(|&pnum| (pnum, normalize(&doc.extract_text(&[pnum]).unwrap_or_default())))
        .collect();

    for sec in sections.iter_mut() {
        let full = normalize(&sec.title);
        // Section numbers (e.g. "3.1.1.") become separate digit tokens after
        // normalization and rarely match the body text verbatim — drop them and
        // match on the wording, which is what actually appears in the page text.
        let key = content_key(&sec.title);
        if full.is_empty() && key.is_empty() {
            continue;
        }
        // Fall back to the first few words for long headings that wrap or differ
        // slightly (e.g. trailing author notes) from the extracted line.
        let short: String = key.split(' ').take(5).collect::<Vec<_>>().join(" ");
        sec.page = page_texts
            .iter()
            .find(|(_, t)| !full.is_empty() && t.contains(&full))
            .or_else(|| page_texts.iter().find(|(_, t)| !key.is_empty() && t.contains(&key)))
            .or_else(|| page_texts.iter().find(|(_, t)| short.len() > 3 && t.contains(&short)))
            .map(|(p, _)| *p)
            .unwrap_or(0);
    }

    // Fill unresolved pages by carrying the previous section's page forward.
    // Sections are in reading order, so an approximate page beats a dead
    // (non-clickable, no-context) entry.
    let mut last = 0u32;
    for sec in sections.iter_mut() {
        if sec.page == 0 {
            sec.page = last;
        } else {
            last = sec.page;
        }
    }
}

/// Lowercase, replace non-alphanumerics with spaces, and collapse whitespace so
/// title matching is robust to punctuation/section-number differences.
fn normalize(s: &str) -> String {
    let mapped: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    mapped.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Normalized heading with any leading pure-digit tokens (the section number)
/// dropped, so "3.1.1. Notations and Definitions" matches "notations and
/// definitions" in the body text.
fn content_key(title: &str) -> String {
    let norm = normalize(title);
    let tokens: Vec<&str> = norm.split(' ').filter(|t| !t.is_empty()).collect();
    let mut start = 0;
    while start < tokens.len() && tokens[start].chars().all(|c| c.is_ascii_digit()) {
        start += 1;
    }
    if start >= tokens.len() {
        return norm; // heading was all digits — keep the full normalized form
    }
    tokens[start..].join(" ")
}
