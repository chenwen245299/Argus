use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
static ANALYSIS_PROGRESS_DONE: OnceLock<Arc<AtomicU32>> = OnceLock::new();
static ANALYSIS_PROGRESS_TOTAL: OnceLock<Arc<AtomicU32>> = OnceLock::new();

fn analysis_cancel() -> &'static Arc<AtomicBool> {
    ANALYSIS_CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn fetch_running() -> &'static Arc<AtomicBool> {
    FETCH_RUNNING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn analysis_running() -> &'static Arc<AtomicBool> {
    ANALYSIS_RUNNING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

fn analysis_progress_done() -> &'static Arc<AtomicU32> {
    ANALYSIS_PROGRESS_DONE.get_or_init(|| Arc::new(AtomicU32::new(0)))
}

fn analysis_progress_total() -> &'static Arc<AtomicU32> {
    ANALYSIS_PROGRESS_TOTAL.get_or_init(|| Arc::new(AtomicU32::new(0)))
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
                Some(name.chars().take(10).collect::<String>())
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

/// Collect source ids of all papers already in the library.
fn collect_library_arxiv_ids(root: &str) -> std::collections::HashSet<String> {
    let papers_dir = std::path::Path::new(root).join("papers");
    let mut ids = std::collections::HashSet::new();
    if let Ok(entries) = std::fs::read_dir(&papers_dir) {
        for entry in entries.flatten() {
            let meta_path = entry.path().join("meta.json");
            if let Ok(text) = std::fs::read_to_string(&meta_path) {
                if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) {
                    for key in ["arxiv_id", "doi"] {
                        if let Some(id) = meta.get(key).and_then(|v| v.as_str()) {
                            if !id.is_empty() {
                                ids.insert(id.to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    ids
}

/// Update a single paper's fields in the appropriate day file without touching other papers.
/// Returns true if the paper was found and updated; false if the paper no longer exists in any
/// day file (e.g., it was already added to library or pruned).
fn update_paper_in_day_files(
    root: &str,
    arxiv_id: &str,
    updater: impl Fn(&mut ArxivPaper),
) -> bool {
    for date in list_day_dates(root) {
        let mut papers = read_day_papers(root, &date);
        if let Some(p) = papers.iter_mut().find(|p| p.arxiv_id == arxiv_id) {
            updater(p);
            let _ = write_day_papers(root, &date, &papers);
            return true;
        }
    }
    false // Paper not found – was removed from inbox
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

/// Delete all papers fetched on a specific date (YYYY-MM-DD).
/// Removes the day file and cleans up the read-state entries.
pub fn delete_inbox_by_date(root: &str, date: &str) -> Result<ArxivInbox, String> {
    let papers = read_day_papers(root, date);
    if !papers.is_empty() {
        let mut states = load_read_states(root);
        for p in &papers {
            states.remove(&p.arxiv_id);
        }
        let _ = save_read_states(root, &states);
    }
    // write_day_papers with empty slice removes the file
    write_day_papers(root, date, &[])?;
    Ok(get_inbox(root))
}

/// Delete specific papers by arxiv_id from the inbox.
pub fn delete_inbox_papers(root: &str, arxiv_ids: &[String]) -> Result<ArxivInbox, String> {
    if arxiv_ids.is_empty() {
        return Ok(get_inbox(root));
    }
    let id_set: std::collections::HashSet<&String> = arxiv_ids.iter().collect();
    // Clean read states
    let mut states = load_read_states(root);
    for id in arxiv_ids {
        states.remove(id);
    }
    let _ = save_read_states(root, &states);
    // Remove from each day file that contains any of the ids
    for date in list_day_dates(root) {
        let papers = read_day_papers(root, &date);
        if papers.iter().any(|p| id_set.contains(&p.arxiv_id)) {
            let kept: Vec<_> = papers.into_iter().filter(|p| !id_set.contains(&p.arxiv_id)).collect();
            write_day_papers(root, &date, &kept)?;
        }
    }
    Ok(get_inbox(root))
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

struct LibraryLookup {
    ids: std::collections::HashSet<String>,
    titles: std::collections::HashSet<String>,
    title_authors: std::collections::HashSet<(String, String)>,
    title_without_author: std::collections::HashSet<String>,
}

fn build_library_lookup(root: &str) -> LibraryLookup {
    let mut lookup = LibraryLookup {
        ids: std::collections::HashSet::new(),
        titles: std::collections::HashSet::new(),
        title_authors: std::collections::HashSet::new(),
        title_without_author: std::collections::HashSet::new(),
    };

    let Ok(entries) = crate::paper::list_paper_dirs(root) else {
        return lookup;
    };

    for (_, path) in entries {
        let meta_path = path.join("meta.json");
        if !meta_path.exists() {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&meta_path) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };

        for key in ["arxiv_id", "doi"] {
            if let Some(id) = meta.get(key).and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    lookup.ids.insert(id.to_string());
                }
            }
        }

        let title = normalize_title(meta.get("title").and_then(|v| v.as_str()).unwrap_or(""));
        if title.is_empty() {
            continue;
        }
        lookup.titles.insert(title.clone());

        let lastname = meta
            .get("authors")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_str())
            .map(normalize_lastname)
            .unwrap_or_default();
        if lastname.is_empty() {
            lookup.title_without_author.insert(title);
        } else {
            lookup.title_authors.insert((title, lastname));
        }
    }

    lookup
}

fn lookup_contains_paper(lookup: &LibraryLookup, arxiv_id: &str, title: &str, authors: &[String]) -> bool {
    if lookup.ids.contains(arxiv_id) {
        return true;
    }

    let target_title = normalize_title(title);
    if target_title.is_empty() {
        return false;
    }
    let target_lastname = authors
        .first()
        .map(|a| normalize_lastname(a))
        .unwrap_or_default();

    if target_lastname.is_empty() {
        return lookup.titles.contains(&target_title);
    }

    lookup
        .title_authors
        .contains(&(target_title.clone(), target_lastname))
        || lookup.title_without_author.contains(&target_title)
}

fn check_in_library(root: &str, title: &str, authors: &[String]) -> bool {
    let lookup = build_library_lookup(root);
    lookup_contains_paper(&lookup, "", title, authors)
}

pub fn mark_in_library_statuses(root: &str, papers: &mut Vec<ArxivPaper>) {
    let lookup = build_library_lookup(root);
    for p in papers.iter_mut() {
        p.in_library = lookup_contains_paper(&lookup, &p.arxiv_id, &p.title, &p.authors);
    }
}


/// Merge new papers into per-day files (dedup against all existing day files).
pub fn merge_into_inbox(root: &str, new_papers: Vec<ArxivPaper>) -> Result<ArxivInbox, String> {
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

    // Pre-collect library arxiv_ids once so we never re-add a paper the user already imported.
    let library_ids = collect_library_arxiv_ids(root);

    let mut changed_dates: std::collections::HashSet<String> = std::collections::HashSet::new();

    for mut new_p in new_papers {
        let target_date = date_from_fetched_at(&new_p.fetched_at);
        let paper_id = new_p.arxiv_id.clone();

        if let Some(existing_date) = id_to_date.get(&paper_id).cloned() {
            // Duplicate: overwrite with latest fetch data, but keep analysis results
            // if they exist (read/rating are handled by the state file).
            let mut found_existing = false;
            {
                let bucket = day_buckets.entry(existing_date.clone()).or_default();
                if let Some(pos) = bucket.iter().position(|p| p.arxiv_id == paper_id) {
                    let old_p = bucket.remove(pos);
                    found_existing = true;

                    // Preserve analysis results from prior run
                    new_p.relevance_score = old_p.relevance_score;
                    new_p.relevance_reason = old_p.relevance_reason;
                    new_p.key_contributions = old_p.key_contributions;
                    new_p.analysis_summary = old_p.analysis_summary;
                    new_p.matched_topics = old_p.matched_topics;
                    // Only keep done/failed status; reset analyzing/pending to pending
                    new_p.analysis_status = if old_p.analysis_status == "done" || old_p.analysis_status == "failed" {
                        old_p.analysis_status
                    } else {
                        "pending".to_string()
                    };
                }
            }

            if found_existing {
                day_buckets.entry(target_date.clone()).or_default().push(new_p);
                changed_dates.insert(existing_date);
                changed_dates.insert(target_date);
            }
        } else if !library_ids.contains(&new_p.arxiv_id) {
            // Truly new paper not already in the library — add to the paper's
            // bucket. arXiv fetches use fetch time; bioRxiv backfills use the
            // paper date so multi-day ranges stay grouped by actual day.
            day_buckets.entry(target_date.clone()).or_default().push(new_p);
            changed_dates.insert(target_date);
        }
        // If the paper is in library_ids, skip it silently (user already imported it).
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

    let inbox = get_inbox(root);
    // Read paper data from inbox (for AI prompt content only — read-only use).
    let paper = inbox
        .papers
        .iter()
        .find(|p| p.arxiv_id == arxiv_id)
        .cloned()
        .ok_or_else(|| format!("Paper {} not found in inbox", arxiv_id))?;

    // Mark as analyzing using targeted update (does not disturb other papers).
    update_paper_in_day_files(root, arxiv_id, |p| {
        p.analysis_status = "analyzing".to_string();
    });
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
            let reason = result.relevance_reason.clone();
            let contributions = result.key_contributions.clone();
            let summary = result.summary.clone();
            let topics = result.matched_topics.clone();
            update_paper_in_day_files(root, arxiv_id, |p| {
                p.relevance_score = Some(score);
                p.relevance_reason = Some(reason.clone());
                p.key_contributions = contributions.clone();
                p.analysis_summary = summary.clone();
                p.matched_topics = topics.clone();
                p.analysis_status = "done".to_string();
            });
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
            update_paper_in_day_files(root, arxiv_id, |p| {
                p.analysis_status = "failed".to_string();
            });
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

    let concurrency = config.ai_analysis_concurrency.clamp(1, 10) as usize;

    if analysis_running().load(Ordering::SeqCst) {
        return Ok(());
    }
    analysis_cancel().store(false, Ordering::SeqCst);
    analysis_running().store(true, Ordering::SeqCst);
    analysis_progress_done().store(0, Ordering::SeqCst);
    analysis_progress_total().store(0, Ordering::SeqCst);

    let mut inbox = get_inbox(root);

    // Reset papers stuck in "analyzing" from a previous interrupted run.
    if inbox.papers.iter().any(|p| p.analysis_status == "analyzing") {
        for p in inbox.papers.iter_mut() {
            if p.analysis_status == "analyzing" {
                p.analysis_status = "pending".to_string();
            }
        }
        let _ = save_inbox(root, &inbox);
    }

    // Collect pending papers (with full data — each task captures its own copy).
    let pending: Vec<ArxivPaper> = inbox
        .papers
        .iter()
        .filter(|p| p.analysis_status == "pending")
        .cloned()
        .collect();

    let total = pending.len() as u32;
    if total == 0 {
        analysis_running().store(false, Ordering::SeqCst);
        let _ = app.emit("arxiv-analysis",
            serde_json::json!({"done": 0, "total": 0, "arxiv_id": "", "status": "finished", "bulk": true}));
        return Ok(());
    }

    analysis_progress_total().store(total, Ordering::SeqCst);

    let _ = app.emit("arxiv-analysis", serde_json::json!({
        "done": 0, "total": total, "arxiv_id": "", "status": "started", "bulk": true
    }));

    let prompt_template = if config.ai_analysis_prompt.trim().is_empty() {
        DEFAULT_ARXIV_ANALYSIS_PROMPT.to_string()
    } else {
        config.ai_analysis_prompt.clone()
    };
    let filter_threshold = config.ai_filter_threshold.clamp(0.0, 10.0);
    let filter_enabled = config.ai_filter_enabled;

    // Build arxiv_id → day-file-date map once (O(n)) so result writes are O(1).
    let id_to_date: std::collections::HashMap<String, String> = pending
        .iter()
        .map(|p| (p.arxiv_id.clone(), date_from_fetched_at(&p.fetched_at)))
        .collect();

    // Bulk pre-mark: O(m) — read each day file ONCE, mark all pending papers in it.
    // Previous approach was O(n × m): scanned all files for every single paper ID.
    {
        let pending_set: std::collections::HashSet<&str> =
            pending.iter().map(|p| p.arxiv_id.as_str()).collect();
        for date in list_day_dates(root) {
            let mut day_papers = read_day_papers(root, &date);
            let mut changed = false;
            for p in day_papers.iter_mut() {
                if pending_set.contains(p.arxiv_id.as_str()) {
                    p.analysis_status = "analyzing".to_string();
                    changed = true;
                }
            }
            if changed {
                let _ = write_day_papers(root, &date, &day_papers);
            }
        }
    }

    // Shared done counter — readable by spawned tasks for real-time progress events.
    let done_arc = analysis_progress_done().clone();

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(concurrency));
    let provider   = std::sync::Arc::new(provider);
    let api_key    = std::sync::Arc::new(api_key);
    let model      = std::sync::Arc::new(model);
    let keywords   = std::sync::Arc::new(keywords);
    let prompt_template = std::sync::Arc::new(prompt_template);

    let mut join_set: tokio::task::JoinSet<(String, Result<AnalysisResult, String>)> =
        tokio::task::JoinSet::new();

    for paper in pending {
        if analysis_cancel().load(Ordering::SeqCst) {
            break;
        }

        let sem    = semaphore.clone();
        let app_c  = app.clone();
        let prov   = provider.clone();
        let key    = api_key.clone();
        let mdl    = model.clone();
        let kws    = keywords.clone();
        let tmpl   = prompt_template.clone();
        let done_c = done_arc.clone();
        let id_c   = paper.arxiv_id.clone();

        join_set.spawn(async move {
            // Block here until a concurrency slot is free.
            let _permit = sem.acquire_owned().await.ok();

            if analysis_cancel().load(Ordering::SeqCst) {
                return (id_c, Err("cancelled".to_string()));
            }

            // Emit "analyzing" with the CURRENT done count and the real total
            // so the frontend progress bar moves forward immediately.
            let current_done = done_c.load(Ordering::SeqCst);
            let _ = app_c.emit("arxiv-analysis", serde_json::json!({
                "done": current_done,
                "total": total,          // real total, not 0
                "arxiv_id": &id_c,
                "status": "analyzing",
                "bulk": true
            }));

            // The actual slow part — call the AI provider.
            let result = call_ai_single(&prov, &key, &mdl, &kws, &tmpl, &paper).await;
            (id_c, result)
        });
    }

    // ── Collect results as they complete ─────────────────────────────────────
    // File writes are serialized here (join_next is awaited one at a time).
    // Each completed task immediately frees a semaphore slot → the next queued
    // task starts its API call straight away, keeping N requests in-flight.
    while let Some(task_result) = join_set.join_next().await {
        let Ok((id, result)) = task_result else { continue };

        // Increment BEFORE emitting so the event carries the post-completion count.
        let done_val = done_arc.fetch_add(1, Ordering::SeqCst) + 1;

        // Fast O(1) lookup: use the pre-built id→date map instead of scanning all files.
        let date = id_to_date.get(&id).map(|s| s.as_str()).unwrap_or("");

        match result {
            Ok(result) => {
                let score = result.relevance_score.clamp(0.0, 10.0);

                if filter_enabled && score < filter_threshold {
                    // Remove from its specific day file — no full-inbox reload needed.
                    if !date.is_empty() {
                        let mut day_papers = read_day_papers(root, date);
                        day_papers.retain(|p| p.arxiv_id != id);
                        let _ = write_day_papers(root, date, &day_papers);
                    }
                    let _ = app.emit("arxiv-analysis", serde_json::json!({
                        "done": done_val, "total": total,
                        "arxiv_id": &id, "status": "filtered",
                        "bulk": true, "removed": true,
                        "score": score,
                        "reason": &result.relevance_reason,
                        "key_contributions": &result.key_contributions,
                        "analysis_summary": &result.summary,
                        "matched_topics": &result.matched_topics
                    }));
                } else {
                    // Write result to the specific day file (O(1) lookup).
                    if !date.is_empty() {
                        let mut day_papers = read_day_papers(root, date);
                        if let Some(p) = day_papers.iter_mut().find(|p| p.arxiv_id == id) {
                            p.relevance_score    = Some(score);
                            p.relevance_reason   = Some(result.relevance_reason.clone());
                            p.key_contributions  = result.key_contributions.clone();
                            p.analysis_summary   = result.summary.clone();
                            p.matched_topics     = result.matched_topics.clone();
                            p.analysis_status    = "done".to_string();
                        }
                        let _ = write_day_papers(root, date, &day_papers);
                    }
                    let _ = app.emit("arxiv-analysis", serde_json::json!({
                        "done": done_val, "total": total,
                        "arxiv_id": &id, "status": "done",
                        "bulk": true,
                        "score": score,
                        "reason": &result.relevance_reason,
                        "key_contributions": &result.key_contributions,
                        "analysis_summary": &result.summary,
                        "matched_topics": &result.matched_topics
                    }));
                }
            }
            Err(e) => {
                eprintln!("Analysis error for {}: {}", id, e);
                if !date.is_empty() {
                    let mut day_papers = read_day_papers(root, date);
                    if let Some(p) = day_papers.iter_mut().find(|p| p.arxiv_id == id) {
                        p.analysis_status = "failed".to_string();
                    }
                    let _ = write_day_papers(root, date, &day_papers);
                }
                let _ = app.emit("arxiv-analysis", serde_json::json!({
                    "done": done_val, "total": total,
                    "arxiv_id": &id, "status": "failed",
                    "bulk": true, "message": e
                }));
            }
        }

        if analysis_cancel().load(Ordering::SeqCst) {
            join_set.abort_all();
            break;
        }
    }

    let final_done = done_arc.load(Ordering::SeqCst);
    analysis_running().store(false, Ordering::SeqCst);
    let _ = app.emit("arxiv-analysis", serde_json::json!({
        "done": final_done, "total": total,
        "arxiv_id": "", "status": "finished", "bulk": true
    }));

    Ok(())
}


// ── Add to library ────────────────────────────────────────────────────────────

/// Build a canonical slug from author/year/title.
fn make_slug(authors: &[String], published: &str, title: &str) -> String {
    let year: String = published.chars().take(4).collect();
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
        // For bioRxiv papers the arxiv_id field holds the DOI; store it in doi instead.
        doi: if paper.source.as_deref() == Some("biorxiv") { Some(arxiv_id.to_string()) } else { None },
        arxiv_id: if paper.source.as_deref() == Some("biorxiv") { None } else { Some(arxiv_id.to_string()) },
        venue: None,
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(format!("{}.pdf", arxiv_id.replace('/', "_"))),
        reading_status: "unread".to_string(),
        paper_abstract: Some(paper.summary.clone()).filter(|s| !s.trim().is_empty()),
        bibtex: None,
        canvas_notes: vec![],
        import_source: Some(paper.source.clone().unwrap_or_else(|| "arxiv".to_string())),
        cite_count: None,
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

    // Assign to collection and physically move folder into it
    if let Some(cid) = collection_id.filter(|s| !s.is_empty()) {
        let _ = crate::collections::move_paper_to_collection(root, &meta.id, cid);
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

    let builder =
        WebviewWindowBuilder::new(app, "arxiv", WebviewUrl::App(std::path::PathBuf::from("/")))
            .title("Argus — arXiv")
            .inner_size(width, height)
            .min_inner_size(ARXIV_MIN_WINDOW_W, ARXIV_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition { x: 14.0, y: 22.0 });

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
                    save_arxiv_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
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
    let analyzing = analysis_running().load(Ordering::SeqCst);

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

    let (analyzed_count, total_pending) = if analyzing {
        let done = analysis_progress_done().load(Ordering::SeqCst);
        let total = analysis_progress_total().load(Ordering::SeqCst);
        (done, total.saturating_sub(done))
    } else {
        (analyzed, total_pending)
    };

    ArxivScheduleStatus {
        auto_fetch_enabled: config.auto_fetch_enabled,
        last_fetch_date: config.last_fetch_date,
        next_scheduled,
        fetching: fetch_running().load(Ordering::SeqCst),
        analyzing,
        analyzed_count,
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

/// Derive approximate submission year from an arXiv ID ("YYMM.NNNNN" format).
fn year_from_arxiv_id(arxiv_id: &str) -> Option<u32> {
    let left = arxiv_id.split('.').next()?;
    if left.len() == 4 && left.chars().all(|c| c.is_ascii_digit()) {
        let yy: u32 = left[..2].parse().ok()?;
        Some(if yy >= 91 { 1900 + yy } else { 2000 + yy })
    } else {
        None
    }
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

// ── HTML meta-tag helpers ─────────────────────────────────────────────────────

/// Decode common HTML entities in a meta content string.
fn decode_html_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#34;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

/// Extract the `content="..."` value of the first `<meta name="NAME" ...>` tag.
fn meta_tag_first(html: &str, name: &str) -> String {
    let needle = format!("name=\"{name}\"");
    let pos = match html.find(&needle) {
        Some(p) => p,
        None => return String::new(),
    };
    // Find the boundaries of this <meta ...> tag
    let tag_start = html[..pos].rfind('<').unwrap_or(0);
    let tag_end = html[pos..]
        .find('>')
        .map(|p| p + pos + 1)
        .unwrap_or(html.len());
    let tag = &html[tag_start..tag_end];
    // Extract content="..."
    if let Some(cp) = tag.find("content=\"") {
        let after = &tag[cp + 9..];
        if let Some(end) = after.find('"') {
            return decode_html_entities(&after[..end]);
        }
    }
    String::new()
}

/// Extract all `content="..."` values from `<meta name="NAME" ...>` tags.
fn meta_tag_all(html: &str, name: &str) -> Vec<String> {
    let needle = format!("name=\"{name}\"");
    let mut results = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = html[from..].find(&needle) {
        let pos = from + rel;
        let tag_start = html[..pos].rfind('<').unwrap_or(0);
        let tag_end = html[pos..]
            .find('>')
            .map(|p| p + pos + 1)
            .unwrap_or(html.len());
        let tag = &html[tag_start..tag_end];
        if let Some(cp) = tag.find("content=\"") {
            let after = &tag[cp + 9..];
            if let Some(end) = after.find('"') {
                let val = decode_html_entities(&after[..end]);
                if !val.is_empty() {
                    results.push(val);
                }
            }
        }
        from = tag_end;
    }
    results
}

/// Parse ArXiv page HTML and extract paper metadata from `<meta citation_*>` tags.
/// Returns (title, authors, abstract, pdf_url, year).
fn parse_arxiv_html_meta(
    html: &str,
    arxiv_id: &str,
) -> Result<(String, Vec<String>, String, String, Option<u32>), String> {
    let title = meta_tag_first(html, "citation_title");
    if title.is_empty() {
        return Err(format!(
            "Could not parse metadata from arxiv.org/abs/{arxiv_id} — page structure may have changed"
        ));
    }

    // Authors are in "Last, First" format; convert to "First Last"
    let authors: Vec<String> = meta_tag_all(html, "citation_author")
        .into_iter()
        .map(|a| {
            if let Some(comma) = a.find(',') {
                let last = a[..comma].trim();
                let first = a[comma + 1..].trim();
                if first.is_empty() {
                    last.to_string()
                } else {
                    format!("{first} {last}")
                }
            } else {
                a
            }
        })
        .collect();

    let abstract_text = meta_tag_first(html, "citation_abstract");

    // citation_pdf_url gives the direct PDF link (e.g. https://arxiv.org/pdf/2505.20278v1)
    let pdf_url = {
        let u = meta_tag_first(html, "citation_pdf_url");
        if u.is_empty() {
            format!("https://arxiv.org/pdf/{arxiv_id}")
        } else {
            u
        }
    };

    // citation_date is "YYYY/MM/DD"
    let year: Option<u32> = meta_tag_first(html, "citation_date")
        .split('/')
        .next()
        .and_then(|y| y.parse().ok())
        .or_else(|| year_from_arxiv_id(arxiv_id));

    Ok((title, authors, abstract_text, pdf_url, year))
}

/// Download PDF bytes, verifying the response is an actual PDF (`%PDF` magic).
/// Tries `primary_url` first, then each entry in `fallbacks`.
async fn download_pdf(
    client: &reqwest::Client,
    primary_url: &str,
    fallbacks: &[String],
) -> Result<Vec<u8>, String> {
    let urls: Vec<&str> = std::iter::once(primary_url)
        .chain(fallbacks.iter().map(String::as_str))
        .collect();
    let mut last_err = String::new();
    for url in urls {
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(b) if b.starts_with(b"%PDF") => return Ok(b.to_vec()),
                Ok(_) => last_err = format!("Not a PDF: {url}"),
                Err(e) => last_err = format!("Read error from {url}: {e}"),
            },
            Ok(resp) => last_err = format!("HTTP {} from {url}", resp.status()),
            Err(e) => last_err = format!("Request failed for {url}: {e}"),
        }
    }
    Err(if last_err.is_empty() {
        "No PDF URLs available".into()
    } else {
        last_err
    })
}

/// Find the slug of a paper already in the library by its arXiv ID.
fn find_paper_slug_by_arxiv_id(root: &str, arxiv_id: &str) -> Option<String> {
    let papers_dir = Path::new(root).join("papers");
    for entry in std::fs::read_dir(&papers_dir).ok()?.flatten() {
        let meta_path = entry.path().join("meta.json");
        if let Ok(text) = std::fs::read_to_string(&meta_path) {
            if let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) {
                let stored = meta
                    .get("arxiv_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Match both "2505.20278" and "2505.20278v1" style
                if !stored.is_empty() {
                    let base_stored = stored.split('v').next().unwrap_or(stored);
                    let base_input = arxiv_id.split('v').next().unwrap_or(arxiv_id);
                    if base_stored == base_input {
                        let slug = entry
                            .file_name()
                            .to_string_lossy()
                            .into_owned();
                        return Some(slug);
                    }
                }
            }
        }
    }
    None
}

/// Import an arXiv paper by URL or bare ID.
/// Fetches metadata by scraping the abs page HTML (no API calls — they time out).
/// Downloads the PDF from the link found in the page.
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
            serde_json::json!({ "arxiv_id": &arxiv_id, "status": status }),
        );
    };

    emit("fetching");

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Build HTTP client: {e}"))?;

    // ── Scrape the abs page for metadata ─────────────────────────────────────
    // Use the original URL if it already points to abs/, otherwise construct it.
    let abs_url = if url.contains("arxiv.org/abs/") {
        // Normalise to https
        if url.starts_with("http://") {
            url.replacen("http://", "https://", 1)
        } else {
            url.to_string()
        }
    } else {
        format!("https://arxiv.org/abs/{arxiv_id}")
    };

    let html = client
        .get(&abs_url)
        .send()
        .await
        .map_err(|e| format!("Fetch arXiv page: {e}"))?
        .text()
        .await
        .map_err(|e| format!("Read arXiv page: {e}"))?;

    let (title, authors, abstract_text, pdf_url, year) =
        parse_arxiv_html_meta(&html, &arxiv_id)?;

    // ── Already in library? Return existing slug so the frontend can navigate ──
    if let Some(existing_slug) = find_paper_slug_by_arxiv_id(root, &arxiv_id) {
        return Ok(existing_slug);
    }

    emit("downloading");

    // ── Download PDF ─────────────────────────────────────────────────────────
    let fallbacks = vec![
        format!("https://arxiv.org/pdf/{arxiv_id}"),
        format!("https://export.arxiv.org/pdf/{arxiv_id}"),
    ];
    let pdf_bytes = download_pdf(&client, &pdf_url, &fallbacks)
        .await
        .map_err(|e| format!("PDF download failed for {arxiv_id}: {e}"))?;

    emit("importing");

    // ── Create paper directory ────────────────────────────────────────────────
    let year_str = year.map(|y| y.to_string()).unwrap_or_default();
    let slug_base = make_slug(&authors, &format!("{year_str}-01-01"), &title);
    let papers_dir = Path::new(root).join("papers");
    // Find a non-conflicting directory name (skip orphaned dirs with no meta.json)
    let final_dir = {
        let candidate = papers_dir.join(&slug_base);
        if !candidate.exists() {
            candidate
        } else if !candidate.join("meta.json").exists() {
            // Orphaned dir from a previous failed import — reuse it
            candidate
        } else {
            let mut n = 2u32;
            loop {
                let c = papers_dir.join(format!("{slug_base}-{n}"));
                if !c.exists() || !c.join("meta.json").exists() { break c; }
                n += 1;
            }
        }
    };
    let final_slug = final_dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&slug_base)
        .to_string();

    std::fs::create_dir_all(&final_dir).map_err(|e| format!("Create paper dir: {e}"))?;
    std::fs::write(final_dir.join("paper.pdf"), &pdf_bytes)
        .map_err(|e| format!("Write PDF: {e}"))?;

    // ── Write metadata ────────────────────────────────────────────────────────
    let paper_id = uuid::Uuid::new_v4().to_string();
    let meta = PaperMeta {
        id: paper_id.clone(),
        title: title.clone(),
        authors: authors.clone(),
        year,
        doi: None,
        arxiv_id: Some(arxiv_id.clone()),
        venue: None,
        tags: vec![],
        added_at: chrono::Utc::now().to_rfc3339(),
        original_filename: Some(format!("{arxiv_id}.pdf")),
        reading_status: "unread".to_string(),
        paper_abstract: Some(abstract_text).filter(|s| !s.trim().is_empty()),
        bibtex: None,
        canvas_notes: vec![],
        import_source: Some("url".to_string()),
        cite_count: None,
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
