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
/// and takes the lines up to the next same-or-shallower heading. Returns "" when
/// nothing usable.
///
/// Heading location is deliberately careful: a naive "first line that contains
/// the title" match anchors onto a leading table-of-contents / outline block
/// (whose entries repeat every section name in order), collapsing each section's
/// body to a single TOC line. We instead prefer a *body-backed standalone
/// heading* — a short line whose normalized text equals the title and that is
/// followed by prose — and only fall back to a loose substring match when no
/// such line exists, so papers that resolve today keep resolving.
pub fn extract_selected_sections_text(root: &str, slug: &str, selected: &[String]) -> String {
    if selected.is_empty() {
        return String::new();
    }
    let Some(index) = read_sections(root, slug) else {
        return String::new();
    };
    if index.sections.is_empty() {
        return String::new();
    }
    let fulltext = crate::extraction::read_fulltext(root, slug);
    select_sections_text(&index.sections, &fulltext, selected)
}

/// Pure core of [`extract_selected_sections_text`], split out for unit testing:
/// given the section index and the full text, return the selected sections'
/// bodies. See the wrapper's doc comment for the matching strategy.
fn select_sections_text(secs: &[PaperSection], fulltext: &str, selected: &[String]) -> String {
    if selected.is_empty() || secs.is_empty() || fulltext.trim().is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = fulltext.lines().collect();
    let norm_lines: Vec<String> = lines.iter().map(|l| normalize(l)).collect();
    let line_words: Vec<usize> = lines.iter().map(|l| l.split_whitespace().count()).collect();

    // Running headers/footers (venue banner, repeated "References", page numbers)
    // recur across pages; fulltext.txt — unlike the frontend detector — does not
    // strip them. Exclude any normalized line seen >= 3 times from heading
    // candidacy so a section never anchors onto a repeated header.
    let mut freq: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    for nl in &norm_lines {
        if !nl.is_empty() {
            *freq.entry(nl.as_str()).or_insert(0) += 1;
        }
    }
    let repeated = |li: usize| -> bool {
        let nl = norm_lines[li].as_str();
        !nl.is_empty() && freq.get(nl).copied().unwrap_or(0) >= 3
    };

    // Per-section normalized match keys, precomputed once.
    let full_keys: Vec<String> = secs.iter().map(|s| normalize(&s.title)).collect();
    let content_keys: Vec<String> = secs.iter().map(|s| content_key(&s.title)).collect();
    let short_keys: Vec<String> = content_keys
        .iter()
        .map(|k| k.split(' ').take(5).collect::<Vec<_>>().join(" "))
        .collect();
    let title_words: Vec<usize> = secs.iter().map(|s| s.title.split_whitespace().count()).collect();

    // A *standalone heading* for section i: the normalized line equals the title
    // (with or without its leading section number) and the raw line is short.
    // This rejects a TOC entry ("1 live repo context workspacecontext" — a
    // superset) and a heading glued onto its first body sentence.
    let is_standalone = |li: usize, i: usize| -> bool {
        let nl = &norm_lines[li];
        if nl.is_empty() {
            return false;
        }
        let eq = (!full_keys[i].is_empty() && *nl == full_keys[i])
            || (!content_keys[i].is_empty() && *nl == content_keys[i]);
        eq && line_words[li] <= title_words[i] + 2
    };
    let contains_match = |li: usize, i: usize| -> bool {
        let nl = &norm_lines[li];
        (!full_keys[i].is_empty() && nl.contains(&full_keys[i]))
            || (!content_keys[i].is_empty() && nl.contains(&content_keys[i]))
            || (short_keys[i].len() > 3 && nl.contains(&short_keys[i]))
    };
    // A real heading is followed within a few non-empty lines by body prose (a
    // longish line); a TOC entry is followed only by more short entries. This is
    // what separates a real heading from its TOC twin even when the TOC prints the
    // title verbatim.
    const PROSE_WORDS: usize = 12;
    const LOOKAHEAD: usize = 8;
    let body_backed = |li: usize| -> bool {
        let mut seen = 0usize;
        let mut j = li + 1;
        while j < lines.len() && seen < LOOKAHEAD {
            if norm_lines[j].is_empty() {
                j += 1;
                continue;
            }
            if line_words[j] >= PROSE_WORDS {
                return true;
            }
            seen += 1;
            j += 1;
        }
        false
    };

    // Locate each heading, scanning forward from a monotonic lower bound (sections
    // are in reading order). Prefer, in order: a body-backed standalone heading,
    // any standalone heading, a body-backed substring line, then any substring
    // line (the original behaviour, kept as a guaranteed non-regressing fallback).
    let mut start_line: Vec<Option<usize>> = vec![None; secs.len()];
    let mut min_line = 0usize;
    for i in 0..secs.len() {
        if full_keys[i].is_empty() && content_keys[i].is_empty() {
            continue;
        }
        let found = (min_line..lines.len())
            .find(|&li| !repeated(li) && is_standalone(li, i) && body_backed(li))
            .or_else(|| (min_line..lines.len()).find(|&li| !repeated(li) && is_standalone(li, i)))
            .or_else(|| {
                (min_line..lines.len()).find(|&li| !repeated(li) && contains_match(li, i) && body_backed(li))
            })
            .or_else(|| (min_line..lines.len()).find(|&li| contains_match(li, i)));
        if let Some(li) = found {
            start_line[i] = Some(li);
            min_line = li + 1;
        }
    }

    let selected_set: std::collections::HashSet<&str> =
        selected.iter().map(|s| s.as_str()).collect();
    // De-dup emitted lines so a selected parent and its selected children (whose
    // ranges overlap) don't repeat the same text.
    let mut emitted: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut out = String::new();
    for (i, sec) in secs.iter().enumerate() {
        if !selected_set.contains(sec.title.as_str()) {
            continue;
        }
        let Some(start) = start_line[i] else {
            continue;
        };
        // End at the next located heading at the SAME OR SHALLOWER level, so
        // selecting a parent section pulls in all of its subsections' bodies
        // rather than stopping at the first subsection heading.
        let end = start_line
            .iter()
            .enumerate()
            .skip(i + 1)
            .filter_map(|(j, s)| s.map(|s| (j, s)))
            .filter(|&(j, s)| s > start && secs[j].level <= sec.level)
            .map(|(_, s)| s)
            .min()
            .unwrap_or(lines.len());
        // Start after the heading line itself so it isn't duplicated in the body.
        let mut body = String::new();
        for li in (start + 1)..end {
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

    // Provider/model and the system prompt are user-configurable in Settings →
    // Extraction. Explicit call args (rare) take precedence; otherwise use the
    // section-splitting settings, which themselves fall back to the global default.
    let s = crate::settings::read_settings(root);
    let pid = provider_id.or(s.sections_ai_provider_id.as_deref());
    let mid = model_id.or(s.sections_ai_model_id.as_deref());
    let (provider, api_key, model) = crate::ai_manager::resolve_provider_model(root, pid, mid)?;

    let system = if s.sections_ai_prompt.trim().is_empty() {
        crate::models::default_sections_ai_prompt()
    } else {
        s.sections_ai_prompt.clone()
    };

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
    // Drop consecutive duplicate / near-duplicate headings (e.g. a table-of-contents
    // entry followed by the real heading for the same section) — compared on the
    // content key so "1. Introduction" and "Introduction" collapse to one.
    let mut out: Vec<PaperSection> = Vec::new();
    for r in parsed {
        let title = r.title.trim().to_string();
        if title.is_empty() {
            continue;
        }
        let key = content_key(&title);
        if let Some(prev) = out.last() {
            let prev_key = content_key(&prev.title);
            if !key.is_empty() && key == prev_key {
                continue;
            }
        }
        out.push(PaperSection {
            title,
            level: r.level.clamp(1, 3),
            page: 0,
        });
    }
    Ok(out)
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

    // A "table of contents" page lists many section titles together. Sections
    // whose wording also appears in the body would otherwise all resolve to this
    // page and jump there instead of to their real location — so we de-prioritize
    // any page that contains several different section titles at once.
    let all_keys: Vec<String> = sections.iter().map(|s| content_key(&s.title)).collect();
    let toc_pages: std::collections::HashSet<u32> = page_texts
        .iter()
        .filter(|(_, t)| {
            all_keys
                .iter()
                .filter(|k| !k.is_empty() && t.contains(k.as_str()))
                .count()
                >= 4
        })
        .map(|(p, _)| *p)
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
        let matches = |t: &str, skip_toc: bool, p: u32| -> bool {
            if skip_toc && toc_pages.contains(&p) {
                return false;
            }
            (!full.is_empty() && t.contains(&full))
                || (!key.is_empty() && t.contains(&key))
                || (short.len() > 3 && t.contains(&short))
        };
        // Prefer a non-TOC page; fall back to allowing a TOC page only if the
        // heading appears nowhere else.
        sec.page = page_texts
            .iter()
            .find(|(p, t)| matches(t, true, *p))
            .or_else(|| page_texts.iter().find(|(p, t)| matches(t, false, *p)))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(title: &str, level: u8) -> PaperSection {
        PaperSection {
            title: title.to_string(),
            level,
            page: 0,
        }
    }

    // Regression for the reported bug: a document that opens with a
    // table-of-contents block listing every section, followed by the real
    // bodies. The old forward-substring matcher anchored every section onto its
    // TOC line, so a selected section's "body" collapsed to that single TOC line.
    #[test]
    fn skips_leading_toc_and_returns_real_body() {
        let fulltext = "\
###########################
# 1) Live Repo Context -> WorkspaceContext
# 2) Prompt Shape And Cache Reuse -> build_prefix, memory_text, prompt
# 3) Delegation And Bounded Subagents -> tool_delegate

1. Live Repo Context

This is maybe the most obvious component, but it is also one of the most important ones.
When a user says fix the tests the model should know whether it is inside a Git repo.

2. Prompt Shape And Cache Reuse

The prompt prefix should be stable so the provider can reuse its cache across turns.

3. Delegation And Bounded Subagents

A subagent runs with a bounded tool set and returns only its final answer.";
        let secs = vec![
            sec("1. Live Repo Context", 1),
            sec("2. Prompt Shape And Cache Reuse", 1),
            sec("3. Delegation And Bounded Subagents", 1),
        ];
        let out = select_sections_text(&secs, fulltext, &["1. Live Repo Context".to_string()]);
        assert!(
            out.contains("This is maybe the most obvious component"),
            "should include the real section body, got:\n{out}"
        );
        assert!(
            !out.contains("-> WorkspaceContext"),
            "should NOT anchor onto the table-of-contents line, got:\n{out}"
        );
        // The next section's body must not leak in.
        assert!(!out.contains("stable so the provider"), "leaked next section:\n{out}");
    }

    // Selecting a top-level parent must include its subsections' bodies, not stop
    // at the first subsection heading.
    #[test]
    fn parent_selection_includes_subsection_bodies() {
        let fulltext = "\
1 Introduction

We introduce the problem and summarize our contributions in this opening paragraph.

1.1 Motivation

Prior systems guess at repository layout which wastes tokens and produces wrong edits.

2 Method

We describe the approach in detail across the following paragraphs of this section.";
        let secs = vec![
            sec("1 Introduction", 1),
            sec("1.1 Motivation", 2),
            sec("2 Method", 1),
        ];
        // Parent only (level-aware end should still pull in the subsection body).
        let out = select_sections_text(&secs, fulltext, &["1 Introduction".to_string()]);
        assert!(out.contains("summarize our contributions"), "missing intro body:\n{out}");
        assert!(out.contains("Prior systems guess"), "missing subsection body:\n{out}");
        assert!(!out.contains("describe the approach"), "leaked next top-level section:\n{out}");
    }

    // A normal paper with no TOC and clean headings must still resolve, and the
    // heading line itself must not be duplicated into the body.
    #[test]
    fn normal_paper_no_toc_resolves_without_heading_duplication() {
        let fulltext = "\
Abstract

We present a method that improves retrieval accuracy on long documents by a wide margin.

Introduction

Long-context retrieval is hard because relevant passages are sparse and far apart here.";
        let secs = vec![sec("Introduction", 1), sec("Abstract", 1)];
        // Note secs order differs from document order; matcher works on document order.
        let out = select_sections_text(&secs, fulltext, &["Introduction".to_string()]);
        assert!(out.contains("Long-context retrieval is hard"), "missing body:\n{out}");
        // "## Introduction" header is present once; the raw heading line is dropped.
        assert_eq!(out.matches("Introduction").count(), 1, "heading duplicated:\n{out}");
    }

    #[test]
    fn parse_dedups_consecutive_near_duplicate_titles() {
        let raw = r#"[
            {"title": "1. Introduction", "level": 1},
            {"title": "Introduction", "level": 1},
            {"title": "2. Method", "level": 1}
        ]"#;
        let parsed = parse_sections_json(raw).unwrap();
        let titles: Vec<&str> = parsed.iter().map(|s| s.title.as_str()).collect();
        assert_eq!(titles, vec!["1. Introduction", "2. Method"]);
    }
}
