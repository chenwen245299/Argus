use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::Emitter;

use crate::models::{
    ArxivConfig, ArxivInbox, ArxivPaper, ArxivScheduleStatus, ChatMessage, PaperMeta, PaperStatus,
    DEFAULT_ARXIV_ANALYSIS_PROMPT,
};
use crate::{ai_manager, collections, extraction, llm, paper, search, settings};

const CONFIG_KEY: &str = "arxiv_config";
const ARXIV_WINDOW_SIZE_STORE_KEY: &str = "arxiv_window_size_v1";
const ARXIV_DEFAULT_WINDOW_W: f64 = 1100.0;
const ARXIV_DEFAULT_WINDOW_H: f64 = 750.0;
const ARXIV_MIN_WINDOW_W: f64 = 800.0;
const ARXIV_MIN_WINDOW_H: f64 = 500.0;

// ── Cancel tokens ─────────────────────────────────────────────────────────────

static ANALYSIS_CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static FETCH_RUNNING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static ANALYSIS_RUNNING: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn analysis_cancel() -> &'static Arc<AtomicBool> {
    ANALYSIS_CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn fetch_running() -> &'static Arc<AtomicBool> {
    FETCH_RUNNING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn analysis_running() -> &'static Arc<AtomicBool> {
    ANALYSIS_RUNNING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn cancel_analysis() {
    analysis_cancel().store(true, Ordering::SeqCst);
}

// ── Config ────────────────────────────────────────────────────────────────────

pub fn get_arxiv_config(root: &str) -> ArxivConfig {
    let path = Path::new(root).join(".argus").join("config.json");
    if !path.exists() {
        return ArxivConfig::default();
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).unwrap_or_default();
    map.get(CONFIG_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub fn save_arxiv_config(root: &str, config: &ArxivConfig) -> Result<(), String> {
    let path = Path::new(root).join(".argus").join("config.json");
    let mut map: serde_json::Map<String, serde_json::Value> = if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    map.insert(
        CONFIG_KEY.to_string(),
        serde_json::to_value(config).map_err(|e| e.to_string())?,
    );
    let content = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

// ── Inbox (per-day files: inbox/YYYY-MM-DD.json) ─────────────────────────────

fn inbox_dir(root: &str) -> std::path::PathBuf {
    Path::new(root).join("inbox")
}

fn day_file(root: &str, date: &str) -> std::path::PathBuf {
    inbox_dir(root).join(format!("{}.json", date))
}

/// Extract "YYYY-MM-DD" from an RFC3339 `fetched_at` string.
fn date_from_fetched_at(fetched_at: &str) -> String {
    let d: String = fetched_at.chars().take(10).collect();
    if d.len() == 10 && d.chars().nth(4) == Some('-') && d.chars().nth(7) == Some('-') {
        d
    } else {
        chrono::Utc::now().format("%Y-%m-%d").to_string()
    }
}

fn read_day_papers(root: &str, date: &str) -> Vec<ArxivPaper> {
    let path = day_file(root, date);
    if !path.exists() {
        return vec![];
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_day_papers(root: &str, date: &str, papers: &[ArxivPaper]) -> Result<(), String> {
    let dir = inbox_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Create inbox dir: {e}"))?;
    let path = day_file(root, date);
    if papers.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("Remove empty day file: {e}"))?;
        }
        return Ok(());
    }
    let content =
        serde_json::to_string_pretty(papers).map_err(|e| format!("Serialize day papers: {e}"))?;
    std::fs::write(&path, content).map_err(|e| format!("Write day file: {e}"))
}

/// List existing fetch-date strings (YYYY-MM-DD), newest first.
fn list_day_dates(root: &str) -> Vec<String> {
    let dir = inbox_dir(root);
    if !dir.exists() {
        return vec![];
    }
    let mut dates: Vec<String> = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // Accept only YYYY-MM-DD.json (15 chars)
            if name.ends_with(".json") && name.len() == 15 {
                Some(name[..10].to_string())
            } else {
                None
            }
        })
        .collect();
    dates.sort_by(|a, b| b.cmp(a));
    dates
}

/// One-time migration from the old single feed_cache.json to per-day files.
fn migrate_old_inbox(root: &str) {
    let old_path = Path::new(root).join("inbox").join("feed_cache.json");
    if !old_path.exists() {
        return;
    }
    let content = match std::fs::read_to_string(&old_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let old_inbox: ArxivInbox = match serde_json::from_str(&content) {
        Ok(i) => i,
        Err(_) => return,
    };
    let mut buckets: std::collections::HashMap<String, Vec<ArxivPaper>> =
        std::collections::HashMap::new();
    for paper in old_inbox.papers {
        let date = date_from_fetched_at(&paper.fetched_at);
        buckets.entry(date).or_default().push(paper);
    }
    for (date, papers) in &buckets {
        let _ = write_day_papers(root, date, papers);
    }
    let _ = std::fs::remove_file(&old_path);
}

// ── Read/rating state file (independent of paper data) ──────────────────────

fn read_state_path(root: &str) -> std::path::PathBuf {
    inbox_dir(root).join("read_state.json")
}

#[derive(serde::Serialize, serde::Deserialize, Default, Clone)]
struct PaperUserState {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub rating: u8,
}

fn load_read_states(root: &str) -> std::collections::HashMap<String, PaperUserState> {
    let path = read_state_path(root);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_read_states(
    root: &str,
    states: &std::collections::HashMap<String, PaperUserState>,
) -> Result<(), String> {
    let dir = inbox_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let content = serde_json::to_string_pretty(states).map_err(|e| e.to_string())?;
    std::fs::write(read_state_path(root), content).map_err(|e| e.to_string())
}

pub fn get_inbox(root: &str) -> ArxivInbox {
    migrate_old_inbox(root);

    let dates = list_day_dates(root);
    if dates.is_empty() {
        return ArxivInbox {
            papers: vec![],
            last_updated: String::new(),
        };
    }

    let mut papers: Vec<ArxivPaper> = vec![];
    let mut seen = std::collections::HashSet::new();
    for date in &dates {
        for p in read_day_papers(root, date) {
            if seen.insert(p.arxiv_id.clone()) {
                papers.push(p);
            }
        }
    }

    // Overlay read/rating from the dedicated state file (authoritative source).
    let states = load_read_states(root);
    for p in papers.iter_mut() {
        if let Some(s) = states.get(&p.arxiv_id) {
            p.read = s.read;
            p.rating = s.rating;
        }
    }

    let last_updated = dates.first().cloned().unwrap_or_default();
    ArxivInbox {
        papers,
        last_updated,
    }
}

/// Persist the inbox by re-bucketing papers into per-day files.
/// Day files whose papers have all been removed (filtered) are deleted.
fn save_inbox(root: &str, inbox: &ArxivInbox) -> Result<(), String> {
    let existing: std::collections::HashSet<String> = list_day_dates(root).into_iter().collect();

    let mut buckets: std::collections::HashMap<String, Vec<ArxivPaper>> =
        std::collections::HashMap::new();
    for paper in &inbox.papers {
        let date = date_from_fetched_at(&paper.fetched_at);
        buckets.entry(date).or_default().push(paper.clone());
    }

    for (date, day_papers) in &buckets {
        write_day_papers(root, date, day_papers)?;
    }
    // Delete day files that are now empty after filtering.
    for date in &existing {
        if !buckets.contains_key(date) {
            let _ = write_day_papers(root, date, &[]);
        }
    }
    Ok(())
}

pub fn prune_low_relevance(root: &str) -> Result<ArxivInbox, String> {
    let config = get_arxiv_config(root);
    let threshold = config.ai_filter_threshold.clamp(0.0, 10.0);
    let mut inbox = get_inbox(root);

    inbox.papers.retain(|paper| {
        paper
            .relevance_score
            .map(|score| score >= threshold)
            .unwrap_or(true)
    });

    save_inbox(root, &inbox)?;
    mark_in_library_statuses(root, &mut inbox.papers);
    Ok(inbox)
}

/// Mark a single paper as read in the dedicated state file.
pub fn mark_paper_read(root: &str, arxiv_id: &str) -> Result<(), String> {
    let mut states = load_read_states(root);
    let entry = states.entry(arxiv_id.to_string()).or_default();
    if entry.read {
        return Ok(());
    }
    entry.read = true;
    save_read_states(root, &states)
}

/// Set the user rating (0–5) for a paper in the dedicated state file.
pub fn rate_paper(root: &str, arxiv_id: &str, rating: u8) -> Result<(), String> {
    let rating = rating.min(5);
    let mut states = load_read_states(root);
    states.entry(arxiv_id.to_string()).or_default().rating = rating;
    save_read_states(root, &states)
}

fn normalize_title(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_lastname(name: &str) -> String {
    name.split_whitespace()
        .last()
        .unwrap_or(name)
        .chars()
        .filter(|c| c.is_alphabetic())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

fn check_in_library(root: &str, title: &str, authors: &[String]) -> bool {
    let target_title = normalize_title(title);
    if target_title.is_empty() {
        return false;
    }
    let target_lastname = authors
        .first()
        .map(|a| normalize_lastname(a))
        .unwrap_or_default();

    let entries = match crate::paper::list_paper_dirs(root) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for (_, path) in entries {
        let meta_path = path.join("meta.json");
        if !meta_path.exists() {
            continue;
        }
        let text = match std::fs::read_to_string(&meta_path) {
            Ok(t) => t,
            Err(_) => continue,
        };
        let meta: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let lib_title = meta.get("title").and_then(|v| v.as_str()).unwrap_or("");
        if normalize_title(lib_title) != target_title {
            continue;
        }
        // Title matches — check first author's last name if available on both sides
        if !target_lastname.is_empty() {
            let lib_lastname = meta
                .get("authors")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .map(normalize_lastname)
                .unwrap_or_default();
            if !lib_lastname.is_empty() && lib_lastname != target_lastname {
                continue;
            }
        }
        return true;
    }
    false
}

fn mark_in_library_statuses(root: &str, papers: &mut Vec<ArxivPaper>) {
    for p in papers.iter_mut() {
        p.in_library = check_in_library(root, &p.title, &p.authors);
    }
}

// ── arXiv Atom XML parsing ────────────────────────────────────────────────────

fn parse_atom_xml(xml: &str, fetched_at: &str) -> Vec<ArxivPaper> {
    let mut papers = Vec::new();
    let parts: Vec<&str> = xml.split("<entry>").collect();
    for part in parts.iter().skip(1) {
        let block = match part.split("</entry>").next() {
            Some(b) => b,
            None => continue,
        };
        let arxiv_id = extract_arxiv_id(block);
        if arxiv_id.is_empty() {
            continue;
        }
        let title = extract_tag(block, "title")
            .trim()
            .replace('\n', " ")
            .replace("  ", " ");
        let summary = extract_tag(block, "summary").trim().to_string();
        let published = extract_tag(block, "published");
        let updated = extract_tag(block, "updated");
        let authors = extract_authors(block);
        let categories = extract_categories(block);
        let (pdf_url, abs_url) = extract_links(block, &arxiv_id);
        papers.push(ArxivPaper {
            arxiv_id,
            title,
            authors,
            summary,
            categories,
            published,
            updated,
            pdf_url,
            abs_url,
            relevance_score: None,
            relevance_reason: None,
            key_contributions: vec![],
            analysis_summary: None,
            matched_topics: vec![],
            analysis_status: "pending".to_string(),
            in_library: false,
            fetched_at: fetched_at.to_string(),
            read: false,
            rating: 0,
        });
    }
    papers
}

fn extract_arxiv_id(block: &str) -> String {
    extract_tag(block, "id")
        .split("/abs/")
        .nth(1)
        .unwrap_or("")
        .trim()
        .split('v')
        .next()
        .unwrap_or("")
        .to_string()
}

fn extract_tag(block: &str, tag: &str) -> String {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    if let Some(start) = block.find(&open) {
        let after = &block[start + open.len()..];
        if let Some(end) = after.find(&close) {
            return after[..end].to_string();
        }
    }
    String::new()
}

fn extract_authors(block: &str) -> Vec<String> {
    let mut authors = Vec::new();
    let mut remaining = block;
    while let Some(start) = remaining.find("<author>") {
        let after = &remaining[start + 8..];
        if let Some(end) = after.find("</author>") {
            let name = extract_tag(&after[..end], "name");
            if !name.is_empty() {
                authors.push(name.trim().to_string());
            }
            remaining = &after[end + 9..];
        } else {
            break;
        }
    }
    authors
}

fn extract_categories(block: &str) -> Vec<String> {
    let mut cats = Vec::new();
    let mut remaining = block;
    while let Some(start) = remaining.find("<category ") {
        let after = &remaining[start..];
        let end = after
            .find("/>")
            .or_else(|| after.find(">"))
            .unwrap_or(after.len());
        let tag_str = &after[..end];
        if let Some(term_start) = tag_str.find("term=\"") {
            let term_after = &tag_str[term_start + 6..];
            if let Some(term_end) = term_after.find('"') {
                let term = &term_after[..term_end];
                if !term.is_empty() {
                    cats.push(term.to_string());
                }
            }
        }
        remaining = &after[end.min(after.len())..];
        if remaining.starts_with("/>") {
            remaining = &remaining[2..];
        } else if !remaining.is_empty() {
            remaining = &remaining[1..];
        }
    }
    cats.dedup();
    cats
}

fn extract_links(block: &str, arxiv_id: &str) -> (String, String) {
    let mut found_pdf = format!("https://arxiv.org/pdf/{}", arxiv_id);
    let mut found_abs = format!("https://arxiv.org/abs/{}", arxiv_id);
    let mut remaining = block;
    while let Some(start) = remaining.find("<link ") {
        let after = &remaining[start..];
        let end = after
            .find("/>")
            .or_else(|| after.find(">"))
            .unwrap_or(after.len());
        let tag_str = &after[..end];
        let href = extract_attr(tag_str, "href");
        let title = extract_attr(tag_str, "title");
        let link_type = extract_attr(tag_str, "type");
        if title == "pdf" || href.contains("/pdf/") {
            found_pdf = href;
        } else if link_type.contains("text/html") || href.contains("/abs/") {
            found_abs = href;
        }
        remaining = &after[end.min(after.len())..];
        if remaining.starts_with("/>") {
            remaining = &remaining[2..];
        } else if !remaining.is_empty() {
            remaining = &remaining[1..];
        }
    }
    (found_pdf, found_abs)
}

fn extract_attr(tag: &str, attr: &str) -> String {
    let pattern = format!("{}=\"", attr);
    if let Some(start) = tag.find(&pattern) {
        let after = &tag[start + pattern.len()..];
        if let Some(end) = after.find('"') {
            return after[..end].to_string();
        }
    }
    String::new()
}

/// Merge new papers into today's day file (dedup against all existing day files).
pub fn merge_into_inbox(root: &str, new_papers: Vec<ArxivPaper>) -> Result<ArxivInbox, String> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Collect all existing papers grouped by their day file.
    let all_dates = list_day_dates(root);
    let mut day_buckets: std::collections::HashMap<String, Vec<ArxivPaper>> = all_dates
        .iter()
        .map(|d| (d.clone(), read_day_papers(root, d)))
        .collect();

    // Build a lookup: arxiv_id → which date file it lives in.
    let mut id_to_date: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    for (date, papers) in &day_buckets {
        for p in papers {
            id_to_date.insert(p.arxiv_id.clone(), date.clone());
        }
    }

    let mut changed_dates: std::collections::HashSet<String> = std::collections::HashSet::new();

    for new_p in new_papers {
        if let Some(existing_date) = id_to_date.get(&new_p.arxiv_id) {
            // Duplicate: overwrite with latest fetch data, but keep analysis results
            // if they exist (read/rating are handled by the state file).
            let bucket = day_buckets.entry(existing_date.clone()).or_default();
            if let Some(old_p) = bucket.iter_mut().find(|p| p.arxiv_id == new_p.arxiv_id) {
                // Preserve analysis results from prior run
                let keep_score = old_p.relevance_score;
                let keep_reason = old_p.relevance_reason.clone();
                let keep_contributions = old_p.key_contributions.clone();
                let keep_summary = old_p.analysis_summary.clone();
                let keep_topics = old_p.matched_topics.clone();
                let keep_status = old_p.analysis_status.clone();
                *old_p = new_p;
                old_p.relevance_score = keep_score;
                old_p.relevance_reason = keep_reason;
                old_p.key_contributions = keep_contributions;
                old_p.analysis_summary = keep_summary;
                old_p.matched_topics = keep_topics;
                // Only keep done/failed status; reset analyzing/pending to pending
                old_p.analysis_status = if keep_status == "done" || keep_status == "failed" {
                    keep_status
                } else {
                    "pending".to_string()
                };
            }
            changed_dates.insert(existing_date.clone());
        } else {
            // New paper — add to today's bucket
            day_buckets.entry(today.clone()).or_default().push(new_p);
            changed_dates.insert(today.clone());
        }
    }

    // Write only modified day files
    for date in &changed_dates {
        if let Some(papers) = day_buckets.get_mut(date) {
            mark_in_library_statuses(root, papers);
            write_day_papers(root, date, papers)?;
        }
    }

    Ok(get_inbox(root))
}

// ── AI Analysis ───────────────────────────────────────────────────────────────

struct AnalysisResult {
    relevance_score: f32,
    relevance_reason: String,
    key_contributions: Vec<String>,
    summary: Option<String>,
    matched_topics: Vec<String>,
}

#[derive(serde::Deserialize)]
struct RawAnalysisResult {
    #[serde(default)]
    relevance_score: serde_json::Value,
    #[serde(default)]
    relevance_reason: Option<String>,
    #[serde(default)]
    key_contributions: Option<Vec<String>>,
    #[serde(default, rename = "summary")]
    summary: Option<String>,
    #[serde(default)]
    matched_topics: Option<Vec<String>>,
}

fn build_analysis_prompt(template: &str, topics: &str, paper: &ArxivPaper) -> String {
    let base = if template.trim().is_empty() {
        DEFAULT_ARXIV_ANALYSIS_PROMPT
    } else {
        template
    };
    let authors = paper.authors.join(", ");
    base.replace("{topics}", topics)
        .replace("{title}", &paper.title)
        .replace("{authors}", &authors)
        .replace("{abstract}", &paper.summary)
}

fn parse_score(value: &serde_json::Value) -> Result<f32, String> {
    if let Some(n) = value.as_f64() {
        return Ok(n as f32);
    }
    if let Some(s) = value.as_str() {
        return s
            .trim()
            .parse::<f32>()
            .map_err(|_| "relevance_score must be a number from 0 to 10".to_string());
    }
    Err("relevance_score must be a number from 0 to 10".to_string())
}

fn parse_analysis_result(content: &str) -> Result<AnalysisResult, String> {
    // Some providers wrap JSON in Markdown fences; keep only the outer JSON object.
    let json_str = if let Some(start) = content.find('{') {
        if let Some(end) = content.rfind('}') {
            &content[start..=end]
        } else {
            content
        }
    } else {
        content
    };

    let preview: String = content.chars().take(200).collect();
    let raw: RawAnalysisResult = serde_json::from_str(json_str)
        .map_err(|e| format!("Parse AI JSON: {e}\nContent was: {preview}"))?;
    let relevance_score = parse_score(&raw.relevance_score)?;
    let relevance_reason = raw
        .relevance_reason
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "AI response missing relevance_reason".to_string())?;

    Ok(AnalysisResult {
        relevance_score,
        relevance_reason,
        key_contributions: raw.key_contributions.unwrap_or_default(),
        summary: raw.summary.filter(|s| !s.trim().is_empty()),
        matched_topics: raw.matched_topics.unwrap_or_default(),
    })
}

async fn call_ai_single(
    provider: &crate::models::AiProvider,
    api_key: &str,
    model: &str,
    topics: &str,
    prompt_template: &str,
    paper: &ArxivPaper,
) -> Result<AnalysisResult, String> {
    let messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content:
                "你是一名严谨的研究助理。请只输出用户要求的有效 JSON，不要添加 Markdown 或解释。"
                    .to_string(),
        },
        ChatMessage {
            role: "user".to_string(),
            content: build_analysis_prompt(prompt_template, topics, paper),
        },
    ];

    let content = llm::chat_completion(provider, api_key, model, &messages, "arxiv").await?;
    parse_analysis_result(&content)
}

/// Analyze a single paper by arxiv_id regardless of its current status.
/// Skips the ai_analysis_enabled gate so users can manually trigger analysis.
pub async fn analyze_single(
    root: &str,
    arxiv_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let config = get_arxiv_config(root);

    let provider_id = config
        .ai_provider_id
        .as_deref()
        .ok_or("未配置 AI 提供商，请前往设置 → arXiv 配置")?;
    let model_id = config
        .ai_model_id
        .as_deref()
        .ok_or("未配置 AI 模型，请前往设置 → arXiv 配置")?;

    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, Some(provider_id), Some(model_id))?;

    let keywords = if config.keywords.is_empty() {
        "machine learning, AI research".to_string()
    } else {
        config.keywords.join(", ")
    };

    let prompt_template = if config.ai_analysis_prompt.trim().is_empty() {
        DEFAULT_ARXIV_ANALYSIS_PROMPT.to_string()
    } else {
        config.ai_analysis_prompt.clone()
    };

    let mut inbox = get_inbox(root);
    let paper = inbox
        .papers
        .iter()
        .find(|p| p.arxiv_id == arxiv_id)
        .cloned()
        .ok_or_else(|| format!("Paper {} not found in inbox", arxiv_id))?;

    // Mark as analyzing
    if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == arxiv_id) {
        p.analysis_status = "analyzing".to_string();
    }
    let _ = save_inbox(root, &inbox);
    let _ = app.emit(
        "arxiv-analysis",
        serde_json::json!({
            "done": 0, "total": 1, "arxiv_id": arxiv_id, "status": "analyzing"
        }),
    );

    match call_ai_single(
        &provider,
        &api_key,
        &model,
        &keywords,
        &prompt_template,
        &paper,
    )
    .await
    {
        Ok(result) => {
            let score = result.relevance_score.clamp(0.0, 10.0);
            if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == arxiv_id) {
                p.relevance_score = Some(score);
                p.relevance_reason = Some(result.relevance_reason.clone());
                p.key_contributions = result.key_contributions.clone();
                p.analysis_summary = result.summary.clone();
                p.matched_topics = result.matched_topics.clone();
                p.analysis_status = "done".to_string();
            }
            let _ = save_inbox(root, &inbox);
            let _ = app.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": 1, "total": 1, "arxiv_id": arxiv_id, "status": "done",
                    "score": score,
                    "reason": &result.relevance_reason,
                    "key_contributions": &result.key_contributions,
                    "analysis_summary": &result.summary,
                    "matched_topics": &result.matched_topics
                }),
            );
        }
        Err(e) => {
            if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == arxiv_id) {
                p.analysis_status = "failed".to_string();
            }
            let _ = save_inbox(root, &inbox);
            let _ = app.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": 0, "total": 1, "arxiv_id": arxiv_id, "status": "failed",
                    "message": &e
                }),
            );
            let _ = app.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": 1, "total": 1, "arxiv_id": "", "status": "finished"
                }),
            );
            return Err(e);
        }
    }

    let _ = app.emit(
        "arxiv-analysis",
        serde_json::json!({
            "done": 1, "total": 1, "arxiv_id": "", "status": "finished"
        }),
    );

    Ok(())
}

pub async fn start_analysis(root: &str, app: &tauri::AppHandle) -> Result<(), String> {
    let config = get_arxiv_config(root);
    if !config.ai_analysis_enabled {
        return Err("AI analysis is not enabled in Settings → arXiv.".to_string());
    }

    let provider_id = config
        .ai_provider_id
        .as_deref()
        .ok_or("No AI provider configured for arXiv analysis. Go to Settings → arXiv.")?;
    let model_id = config
        .ai_model_id
        .as_deref()
        .ok_or("No AI model configured for arXiv analysis. Go to Settings → arXiv.")?;

    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, Some(provider_id), Some(model_id))?;

    let keywords = if config.keywords.is_empty() {
        "machine learning, AI research".to_string()
    } else {
        config.keywords.join(", ")
    };

    // Prevent concurrent bulk analysis runs
    if analysis_running().load(Ordering::SeqCst) {
        return Ok(());
    }

    analysis_cancel().store(false, Ordering::SeqCst);
    analysis_running().store(true, Ordering::SeqCst);

    let mut inbox = get_inbox(root);

    // Reset any papers stuck in "analyzing" (e.g. from a previous interrupted run)
    // so they are picked up again as pending.
    let had_stale = inbox
        .papers
        .iter()
        .any(|p| p.analysis_status == "analyzing");
    if had_stale {
        for p in inbox.papers.iter_mut() {
            if p.analysis_status == "analyzing" {
                p.analysis_status = "pending".to_string();
            }
        }
        let _ = save_inbox(root, &inbox);
    }

    let pending_ids: Vec<String> = inbox
        .papers
        .iter()
        .filter(|p| p.analysis_status == "pending")
        .map(|p| p.arxiv_id.clone())
        .collect();

    let total = pending_ids.len() as u32;
    let mut done = 0u32;

    let _ = app.emit(
        "arxiv-analysis",
        serde_json::json!({
            "done": 0, "total": total, "arxiv_id": "", "status": "started"
        }),
    );

    let prompt_template = if config.ai_analysis_prompt.trim().is_empty() {
        DEFAULT_ARXIV_ANALYSIS_PROMPT.to_string()
    } else {
        config.ai_analysis_prompt.clone()
    };
    let filter_threshold = config.ai_filter_threshold.clamp(0.0, 10.0);

    for id in pending_ids {
        if analysis_cancel().load(Ordering::SeqCst) {
            break;
        }

        if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == id) {
            p.analysis_status = "analyzing".to_string();
            let _ = app.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": done, "total": total,
                    "arxiv_id": &id, "status": "analyzing"
                }),
            );
        }
        let _ = save_inbox(root, &inbox);

        let paper = match inbox.papers.iter().find(|p| p.arxiv_id == id).cloned() {
            Some(p) => p,
            None => continue,
        };

        match call_ai_single(
            &provider,
            &api_key,
            &model,
            &keywords,
            &prompt_template,
            &paper,
        )
        .await
        {
            Ok(result) => {
                let score = result.relevance_score.clamp(0.0, 10.0);
                done += 1;

                if config.ai_filter_enabled && score < filter_threshold {
                    inbox.papers.retain(|p| p.arxiv_id != id);
                    let _ = app.emit(
                        "arxiv-analysis",
                        serde_json::json!({
                            "done": done, "total": total,
                            "arxiv_id": &id, "status": "filtered",
                            "removed": true,
                            "score": score,
                            "reason": &result.relevance_reason,
                            "key_contributions": &result.key_contributions,
                            "analysis_summary": &result.summary,
                            "matched_topics": &result.matched_topics
                        }),
                    );
                } else if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == id) {
                    p.relevance_score = Some(score);
                    p.relevance_reason = Some(result.relevance_reason.clone());
                    p.key_contributions = result.key_contributions.clone();
                    p.analysis_summary = result.summary.clone();
                    p.matched_topics = result.matched_topics.clone();
                    p.analysis_status = "done".to_string();
                    let _ = app.emit(
                        "arxiv-analysis",
                        serde_json::json!({
                            "done": done, "total": total,
                            "arxiv_id": &id, "status": "done",
                            "score": score,
                            "reason": &result.relevance_reason,
                            "key_contributions": &result.key_contributions,
                            "analysis_summary": &result.summary,
                            "matched_topics": &result.matched_topics
                        }),
                    );
                }
            }
            Err(e) => {
                eprintln!("Analysis error for {}: {}", id, e);
                if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == id) {
                    p.analysis_status = "failed".to_string();
                    done += 1;
                    let _ = app.emit(
                        "arxiv-analysis",
                        serde_json::json!({
                            "done": done, "total": total,
                            "arxiv_id": &id, "status": "failed",
                            "message": e
                        }),
                    );
                }
            }
        }

        let _ = save_inbox(root, &inbox);

        // Polite delay between model calls.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    analysis_running().store(false, Ordering::SeqCst);
    let _ = app.emit(
        "arxiv-analysis",
        serde_json::json!({
            "done": done, "total": total, "arxiv_id": "", "status": "finished"
        }),
    );

    Ok(())
}

// ── Add to library ────────────────────────────────────────────────────────────

/// Build a canonical slug from author/year/title.
fn make_slug(authors: &[String], published: &str, title: &str) -> String {
    let year = &published[..published.len().min(4)];
    let last_name = authors
        .first()
        .map(|a| a.split_whitespace().last().unwrap_or("unknown"))
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

    format!(
        "{}-{}-{}-{}",
        sanitize_slug(last_name),
        year,
        title_words,
        short_id
    )
}

fn sanitize_slug(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_lowercase().next().unwrap_or(c)
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

pub async fn add_to_library(
    root: &str,
    arxiv_id: &str,
    collection_id: Option<&str>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    // Find paper in inbox
    let inbox = get_inbox(root);
    let paper = inbox
        .papers
        .iter()
        .find(|p| p.arxiv_id == arxiv_id)
        .cloned()
        .ok_or_else(|| format!("Paper {} not found in inbox.", arxiv_id))?;

    // Check not already in library by title + first author
    if check_in_library(root, &paper.title, &paper.authors) {
        return Err(format!(
            "「{}」already exists in your library.",
            paper.title
        ));
    }

    let _ = app.emit(
        "arxiv-import",
        serde_json::json!({
            "arxiv_id": arxiv_id, "status": "downloading"
        }),
    );

    // Download PDF
    let client = reqwest::Client::builder()
        .user_agent("Argus/0.1")
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Build client: {e}"))?;

    let pdf_url = if paper.pdf_url.is_empty() {
        format!("https://arxiv.org/pdf/{}", arxiv_id)
    } else {
        paper.pdf_url.clone()
    };

    let pdf_bytes = client
        .get(&pdf_url)
        .send()
        .await
        .map_err(|e| format!("Download PDF: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Read PDF bytes: {e}"))?;

    let _ = app.emit(
        "arxiv-import",
        serde_json::json!({
            "arxiv_id": arxiv_id, "status": "importing"
        }),
    );

    // Create paper directory
    let slug = make_slug(&paper.authors, &paper.published, &paper.title);
    let paper_dir = Path::new(root).join("papers").join(&slug);

    // If slug collision, add a unique suffix
    let final_dir = if paper_dir.exists() {
        let s = format!("{}-2", slug);
        Path::new(root).join("papers").join(&s)
    } else {
        paper_dir
    };
    let final_slug = final_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&slug)
        .to_string();

    std::fs::create_dir_all(&final_dir).map_err(|e| format!("Create paper dir: {e}"))?;

    // Write PDF
    let pdf_path = final_dir.join("paper.pdf");
    std::fs::write(&pdf_path, &pdf_bytes).map_err(|e| format!("Write PDF: {e}"))?;

    // Parse year from published (YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ)
    let year: Option<u32> = paper
        .published
        .chars()
        .take(4)
        .collect::<String>()
        .parse()
        .ok();

    // Build meta
    let paper_id = uuid::Uuid::new_v4().to_string();
    let meta = PaperMeta {
        id: paper_id,
        title: paper.title.clone(),
        authors: paper.authors.clone(),
        year,
        doi: None,
        arxiv_id: Some(arxiv_id.to_string()),
        venue: None,
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(format!("{}.pdf", arxiv_id)),
        reading_status: "unread".to_string(),
        paper_abstract: Some(paper.summary.clone()).filter(|s| !s.trim().is_empty()),
        bibtex: None,
        canvas_notes: vec![],
    };
    paper::write_meta(root, &final_slug, &meta)?;
    paper::ensure_paper_files(root, &final_slug);

    // Mark metadata_fetched in status
    let mut status = PaperStatus::default();
    status.metadata_fetched = true;
    paper::write_status(root, &final_slug, &status)?;
    let _ = search::index_paper(root, &final_slug);

    // Once imported, remove the recommendation from the arXiv inbox.
    let mut inbox = get_inbox(root);
    inbox.papers.retain(|p| p.arxiv_id != arxiv_id);
    let _ = save_inbox(root, &inbox);
    let _ = app.emit(
        "arxiv-new-recommendations",
        serde_json::json!({ "count": inbox.papers.iter().filter(|p| !p.in_library).count() }),
    );

    // Notify main window to refresh (before extraction so UI appears immediately)
    let _ = app.emit("library-updated", serde_json::json!({ "slug": final_slug }));
    let _ = app.emit(
        "arxiv-import",
        serde_json::json!({
            "arxiv_id": arxiv_id, "status": "done", "slug": &final_slug
        }),
    );

    // Assign to collection if requested
    if let Some(cid) = collection_id.filter(|s| !s.is_empty()) {
        let _ = crate::collections::add_paper_to_collection(root, &meta.id, cid);
    }

    // Fulltext extraction + FTS indexing in background
    let s = settings::read_settings(root);
    let root_owned = root.to_string();
    let slug_owned = final_slug.clone();
    let app_c = app.clone();
    tauri::async_runtime::spawn(async move {
        let root1 = root_owned.clone();
        let slug1 = slug_owned.clone();
        if let Ok(result) = tauri::async_runtime::spawn_blocking(move || {
            extraction::extract_and_write(&root1, &slug1, &s)
        })
        .await
        {
            if matches!(result, extraction::ExtractionResult::Text) {
                let root2 = root_owned.clone();
                let slug2 = slug_owned.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || {
                    search::index_paper(&root2, &slug2)
                })
                .await;
                let _ = app_c.emit(
                    "argus-paper-fulltext-updated",
                    serde_json::json!({ "slug": slug_owned }),
                );
            }
        }
    });

    Ok(final_slug)
}

// ── Window ────────────────────────────────────────────────────────────────────

fn load_arxiv_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let value = store.get(ARXIV_WINDOW_SIZE_STORE_KEY)?;
    let width = value.get("w")?.as_f64()?;
    let height = value.get("h")?.as_f64()?;
    if width >= ARXIV_MIN_WINDOW_W
        && height >= ARXIV_MIN_WINDOW_H
        && width <= 4000.0
        && height <= 3000.0
    {
        Some((width, height))
    } else {
        None
    }
}

pub fn save_arxiv_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < ARXIV_MIN_WINDOW_W || height < ARXIV_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            ARXIV_WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

pub fn open_arxiv_window(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    if let Some(win) = app.get_webview_window("arxiv") {
        let _ = win.set_focus();
        return Ok(());
    }

    let (width, height) =
        load_arxiv_window_size(app).unwrap_or((ARXIV_DEFAULT_WINDOW_W, ARXIV_DEFAULT_WINDOW_H));

    let builder = WebviewWindowBuilder::new(app, "arxiv", WebviewUrl::App(std::path::PathBuf::from("/")))
        .title("Argus — arXiv")
        .inner_size(width, height)
        .min_inner_size(ARXIV_MIN_WINDOW_W, ARXIV_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    let win = builder
        .build()
        .map_err(|e| format!("Open arXiv window: {e}"))?;

    // Save size on every resize and on close.
    // Using win.clone() avoids a lookup that can fail during macOS close animation.
    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_arxiv_window_size(&app_handle, phys.width as f64 / sf, phys.height as f64 / sf);
                }
            }
        };
        match event {
            tauri::WindowEvent::Resized(_) | tauri::WindowEvent::CloseRequested { .. } => {
                save(&win_ref);
            }
            _ => {}
        }
    });

    Ok(())
}

// ── Schedule status ───────────────────────────────────────────────────────────

pub fn get_schedule_status(root: &str) -> ArxivScheduleStatus {
    let config = get_arxiv_config(root);
    let inbox = get_inbox(root);

    let total_pending = inbox
        .papers
        .iter()
        .filter(|p| p.analysis_status == "pending")
        .count() as u32;
    let analyzed = inbox
        .papers
        .iter()
        .filter(|p| p.analysis_status == "done" || p.analysis_status == "failed")
        .count() as u32;

    // Compute next scheduled time
    let next_scheduled = if config.auto_fetch_enabled {
        compute_next_scheduled(&config)
    } else {
        None
    };

    ArxivScheduleStatus {
        auto_fetch_enabled: config.auto_fetch_enabled,
        last_fetch_date: config.last_fetch_date,
        next_scheduled,
        fetching: fetch_running().load(Ordering::SeqCst),
        analyzing: analysis_running().load(Ordering::SeqCst),
        analyzed_count: analyzed,
        total_pending,
    }
}

fn compute_next_scheduled(config: &ArxivConfig) -> Option<String> {
    use chrono::{Duration, Local, NaiveTime};

    let fetch_time = NaiveTime::parse_from_str(&config.fetch_time, "%H:%M").ok()?;
    let last_date = config
        .last_fetch_date
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

    let today = Local::now().naive_local().date();
    let next_date = match last_date {
        None => today,
        Some(last) => {
            let candidate = last + Duration::days(config.interval_days as i64);
            if candidate <= today {
                today
            } else {
                candidate
            }
        }
    };

    let next_dt = next_date.and_time(fetch_time);
    let now_naive = Local::now().naive_local();
    let next_dt = if next_dt <= now_naive {
        (today + Duration::days(config.interval_days as i64)).and_time(fetch_time)
    } else {
        next_dt
    };

    Some(next_dt.format("%Y-%m-%dT%H:%M").to_string())
}

// ── URL-based arXiv import ────────────────────────────────────────────────────

/// Extract an arXiv ID from a URL or bare ID string.
/// Handles:
///   https://arxiv.org/abs/1811.12889
///   https://arxiv.org/abs/1811.12889v2
///   https://arxiv.org/pdf/1811.12889
///   1811.12889  (bare new-format ID)
///   cs/0611018  (bare old-format ID)
pub fn parse_arxiv_id(input: &str) -> Option<String> {
    let s = input.trim();

    // Try URL path segments /abs/ or /pdf/
    for prefix in ["/abs/", "/pdf/"] {
        if let Some(pos) = s.find(prefix) {
            let after = &s[pos + prefix.len()..];
            let end = after
                .find(|c: char| c == '?' || c == '#')
                .unwrap_or(after.len());
            let candidate = after[..end].trim_end_matches(".pdf");
            let clean = strip_version(candidate);
            if is_arxiv_id(clean) {
                return Some(clean.to_string());
            }
        }
    }

    // Try as bare ID (possibly with version suffix)
    let clean = strip_version(s);
    if is_arxiv_id(clean) {
        return Some(clean.to_string());
    }

    None
}

fn strip_version(s: &str) -> &str {
    // Strip trailing vN version suffix
    if let Some(v) = s.rfind('v') {
        let ver = &s[v + 1..];
        if !ver.is_empty() && ver.chars().all(|c| c.is_ascii_digit()) {
            return &s[..v];
        }
    }
    s
}

fn is_arxiv_id(s: &str) -> bool {
    // New format: YYMM.NNNNN  (e.g. 1811.12889, 2406.00001)
    if let Some(dot) = s.find('.') {
        let left = &s[..dot];
        let right = &s[dot + 1..];
        if left.len() == 4
            && left.chars().all(|c| c.is_ascii_digit())
            && (right.len() == 4 || right.len() == 5)
            && right.chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    // Old format: subject/YYMMNNN  (e.g. cs/0611018)
    if let Some(slash) = s.find('/') {
        let subject = &s[..slash];
        let num = &s[slash + 1..];
        if !subject.is_empty()
            && subject
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '.')
            && num.len() == 7
            && num.chars().all(|c| c.is_ascii_digit())
        {
            return true;
        }
    }
    false
}

/// Import an arXiv paper directly from a URL or ID, fetching metadata from the
/// arXiv API and downloading the PDF. Assigns to `collection_id` if non-empty.
/// Returns the final paper slug.
pub async fn import_by_url(
    root: &str,
    url: &str,
    collection_id: &str,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    use tauri::Emitter;

    let arxiv_id =
        parse_arxiv_id(url).ok_or_else(|| format!("Could not find an arXiv ID in: {url}"))?;

    let emit = |status: &str| {
        let _ = app.emit(
            "arxiv-url-import",
            serde_json::json!({ "arxiv_id": arxiv_id, "status": status }),
        );
    };

    emit("fetching");

    // ── Fetch metadata from arXiv Atom API ───────────────────────────────────
    let client = reqwest::Client::builder()
        .user_agent("Argus/0.1")
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Build HTTP client: {e}"))?;

    let api_url = format!("https://export.arxiv.org/api/query?id_list={}", arxiv_id);
    let xml = client
        .get(&api_url)
        .send()
        .await
        .map_err(|e| format!("Fetch arXiv metadata: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Read API response: {e}"))?;

    let fetched_at = chrono::Utc::now().to_rfc3339();
    let papers = parse_atom_xml(&xml, &fetched_at);
    let paper_info = papers
        .into_iter()
        .next()
        .ok_or_else(|| format!("arXiv ID {arxiv_id} not found or API returned no results."))?;

    // Check if already in library by title + first author (after fetching so we have the title)
    if check_in_library(root, &paper_info.title, &paper_info.authors) {
        return Err(format!(
            "「{}」already exists in your library.",
            paper_info.title
        ));
    }

    emit("downloading");

    // ── Download PDF ─────────────────────────────────────────────────────────
    let pdf_url = if paper_info.pdf_url.is_empty() {
        format!("https://arxiv.org/pdf/{arxiv_id}")
    } else {
        paper_info.pdf_url.clone()
    };

    let pdf_bytes = client
        .get(&pdf_url)
        .send()
        .await
        .map_err(|e| format!("Download PDF: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("Read PDF bytes: {e}"))?;

    emit("importing");

    // ── Create paper directory ────────────────────────────────────────────────
    let slug_base = make_slug(
        &paper_info.authors,
        &paper_info.published,
        &paper_info.title,
    );
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
    let year: Option<u32> = paper_info
        .published
        .chars()
        .take(4)
        .collect::<String>()
        .parse()
        .ok();

    let paper_id = uuid::Uuid::new_v4().to_string();
    let meta = PaperMeta {
        id: paper_id.clone(),
        title: paper_info.title.clone(),
        authors: paper_info.authors.clone(),
        year,
        doi: None,
        arxiv_id: Some(arxiv_id.clone()),
        venue: None,
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(format!("{arxiv_id}.pdf")),
        reading_status: "unread".to_string(),
        paper_abstract: Some(paper_info.summary.clone()).filter(|s| !s.trim().is_empty()),
        bibtex: None,
        canvas_notes: vec![],
    };
    paper::write_meta(root, &final_slug, &meta)?;
    paper::ensure_paper_files(root, &final_slug);

    let mut status = crate::models::PaperStatus::default();
    status.metadata_fetched = true;
    paper::write_status(root, &final_slug, &status)?;
    let _ = search::index_paper(root, &final_slug);

    // ── Collection assignment ─────────────────────────────────────────────────
    if !collection_id.is_empty() {
        collections::move_paper_to_collection(root, &paper_id, collection_id)?;
    }

    // ── Mark in_library in inbox if paper was there ───────────────────────────
    let mut inbox = get_inbox(root);
    if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == arxiv_id) {
        p.in_library = true;
        let _ = save_inbox(root, &inbox);
    }

    // ── Best-effort fulltext extraction + FTS indexing ────────────────────────
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

    // ── Emit completion ───────────────────────────────────────────────────────
    let _ = app.emit("library-updated", serde_json::json!({ "slug": final_slug }));
    emit("done");

    Ok(final_slug)
}
