use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use tauri::Emitter;

use crate::models::{
    ArxivAnalysisPause, ArxivAnalysisRun, ArxivConfig, ArxivFilteredPaper, ArxivInbox,
    ArxivPaper, ArxivRunCounts, ArxivScheduleStatus, ChatMessage, ImportResult, PaperMeta,
    PaperStatus, DEFAULT_ARXIV_ANALYSIS_PROMPT,
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
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| e.to_string())
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
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| format!("Write day file: {e}"))
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
            // Only YYYY-MM-DD.json. Checking the length alone let
            // read_state.json and feed_cache.json (also 15 characters) pass
            // as "dates", and save_inbox then deleted them as empty days —
            // taking every read mark and rating with it.
            let stem = name.strip_suffix(".json")?;
            chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d")
                .ok()
                .filter(|_| stem.len() == 10)
                .map(|_| stem.to_string())
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

/// A day file's papers: empty when the file does not exist, an error when it
/// exists but cannot be read or parsed. `read_day_papers` reads both as empty,
/// which is harmless for display but not before a write: writing that back —
/// or deleting the "empty" day — destroys the file.
fn read_day_papers_checked(root: &str, date: &str) -> Result<Vec<ArxivPaper>, String> {
    let path = day_file(root, date);
    if !path.exists() {
        return Ok(vec![]);
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("inbox/{date}.json 读取失败：{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("inbox/{date}.json 无法解析：{e}"))
}

// ── Read/rating state file (independent of paper data) ──────────────────────

/// Serialises load→modify→save of read_state.json. Selecting a paper marks it
/// read and a star click rates it, as two concurrent commands; each rewrote the
/// whole file from its own copy, so one of the two changes was lost.
fn read_state_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

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
    crate::fsutil::atomic_write_str(&read_state_path(root), &content).map_err(|e| e.to_string())
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
    let _guard = inbox_lock();
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

    // `inbox` came from get_inbox, which reads an unreadable day file as empty.
    // Re-bucketing would then delete that file as a day with no papers left.
    for date in &existing {
        if let Err(e) = read_day_papers_checked(root, date) {
            return Err(format!(
                "{e}。为免删掉其中的论文，这次没有改写收件箱；请检查或移走这个文件后重试。"
            ));
        }
    }

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
    let mut inbox = {
        let _guard = inbox_lock();
        let mut inbox = get_inbox(root);
        let (kept, dropped): (Vec<_>, Vec<_>) =
            inbox.papers.into_iter().partition(|paper| {
                paper.kept
                    || paper
                        .relevance_score
                        .map(|score| score >= threshold)
                        .unwrap_or(true)
            });
        inbox.papers = kept;
        save_inbox(root, &inbox)?;
        record_filtered(
            root,
            dropped.into_iter().map(|p| filtered_entry(p, threshold)).collect(),
        );
        inbox
    };
    mark_in_library_statuses(root, &mut inbox.papers);
    Ok(inbox)
}

/// Delete all papers fetched on a specific date (YYYY-MM-DD).
/// Removes the day file and cleans up the read-state entries.
pub fn delete_inbox_by_date(root: &str, date: &str) -> Result<ArxivInbox, String> {
    let guard = inbox_lock();
    let papers = read_day_papers(root, date);
    if !papers.is_empty() {
        let _states_guard = read_state_lock();
        let mut states = load_read_states(root);
        for p in &papers {
            states.remove(&p.arxiv_id);
        }
        let _ = save_read_states(root, &states);
    }
    // write_day_papers with empty slice removes the file
    write_day_papers(root, date, &[])?;
    drop(guard);
    Ok(get_inbox(root))
}

/// Delete specific papers by arxiv_id from the inbox.
pub fn delete_inbox_papers(root: &str, arxiv_ids: &[String]) -> Result<ArxivInbox, String> {
    if arxiv_ids.is_empty() {
        return Ok(get_inbox(root));
    }
    let id_set: std::collections::HashSet<&String> = arxiv_ids.iter().collect();
    // Clean read states
    {
        let _guard = read_state_lock();
        let mut states = load_read_states(root);
        for id in arxiv_ids {
            states.remove(id);
        }
        let _ = save_read_states(root, &states);
    }
    // Remove from each day file that contains any of the ids
    {
        let _guard = inbox_lock();
        for date in list_day_dates(root) {
            let papers = read_day_papers(root, &date);
            if papers.iter().any(|p| id_set.contains(&p.arxiv_id)) {
                let kept: Vec<_> = papers.into_iter().filter(|p| !id_set.contains(&p.arxiv_id)).collect();
                write_day_papers(root, &date, &kept)?;
            }
        }
    }
    Ok(get_inbox(root))
}

// ── Recently filtered (inbox/filtered.json) ─────────────────────────────────
//
// A paper scored below the filter threshold leaves the inbox. It used to be
// deleted outright, which with a strict model looked exactly like a failure:
// MiniMax-M3 gave 0–3 to papers other models scored 6–7, answered in two
// seconds, and a batch emptied the inbox several papers a second with nothing
// left to show what had happened or why. The latest ones are now kept here —
// analysis included — to be looked through and put back.

const FILTERED_FILE: &str = "filtered.json";
/// Newest first, capped: a look back at recent runs, not an archive.
const FILTERED_KEEP: usize = 500;

fn filtered_path(root: &str) -> PathBuf {
    inbox_dir(root).join(FILTERED_FILE)
}

fn read_filtered(root: &str) -> Vec<ArxivFilteredPaper> {
    std::fs::read_to_string(filtered_path(root))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn write_filtered(root: &str, list: &[ArxivFilteredPaper]) -> Result<(), String> {
    let path = filtered_path(root);
    if list.is_empty() {
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| format!("Remove {FILTERED_FILE}: {e}"))?;
        }
        return Ok(());
    }
    std::fs::create_dir_all(inbox_dir(root)).map_err(|e| format!("Create inbox dir: {e}"))?;
    // Compact, unlike the day files: it is rewritten on every flush of a batch
    // that is dropping papers, and nobody reads it by hand.
    let content =
        serde_json::to_string(list).map_err(|e| format!("Serialize {FILTERED_FILE}: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write {FILTERED_FILE}: {e}"))
}

fn filtered_entry(paper: ArxivPaper, threshold: f32) -> ArxivFilteredPaper {
    ArxivFilteredPaper {
        paper,
        filtered_at: chrono::Local::now().to_rfc3339(),
        filter_threshold: threshold,
    }
}

/// Put papers at the front of the record, replacing older entries for the same
/// ids. The caller holds `inbox_lock`. A failure to write is logged rather than
/// returned: the inbox itself is already written, and the record is a courtesy.
fn record_filtered(root: &str, mut entries: Vec<ArxivFilteredPaper>) {
    if entries.is_empty() {
        return;
    }
    // A paper can sit in two day files, and so be dropped twice in one pass.
    let mut seen: HashSet<String> = HashSet::new();
    entries.extend(read_filtered(root));
    entries.retain(|e| seen.insert(e.paper.arxiv_id.clone()));
    entries.truncate(FILTERED_KEEP);
    if let Err(e) = write_filtered(root, &entries) {
        eprintln!("[arxiv] record filtered papers: {e}");
    }
}

pub fn get_filtered(root: &str) -> Vec<ArxivFilteredPaper> {
    read_filtered(root)
}

/// Put filtered papers back into the inbox, analysis and all, and drop them
/// from the record. One that is in the inbox again already — fetched again
/// since — or was imported meanwhile is only dropped from the record.
pub fn restore_filtered(root: &str, arxiv_ids: &[String]) -> Result<ArxivInbox, String> {
    let guard = inbox_lock();
    let wanted: HashSet<&str> = arxiv_ids.iter().map(String::as_str).collect();
    let (back, keep): (Vec<_>, Vec<_>) = read_filtered(root)
        .into_iter()
        .partition(|e| wanted.contains(e.paper.arxiv_id.as_str()));
    if !back.is_empty() {
        let mut present = collect_library_arxiv_ids(root);
        for date in list_day_dates(root) {
            present.extend(read_day_papers(root, &date).into_iter().map(|p| p.arxiv_id));
        }
        let mut by_date: HashMap<String, Vec<ArxivPaper>> = HashMap::new();
        for entry in back {
            let mut p = entry.paper;
            if present.contains(&p.arxiv_id) {
                continue;
            }
            p.analysis_status = "done".to_string();
            p.analysis_error = None;
            p.kept = true;
            by_date.entry(date_from_fetched_at(&p.fetched_at)).or_default().push(p);
        }
        for (date, papers) in by_date {
            // An unreadable day file is never written over; the record keeps
            // every paper that could not go back.
            let mut day = read_day_papers_checked(root, &date)
                .map_err(|e| format!("{e}。这篇论文没有恢复，请检查或移走这个文件后重试。"))?;
            day.extend(papers);
            mark_in_library_statuses(root, &mut day);
            write_day_papers(root, &date, &day)?;
        }
        write_filtered(root, &keep)?;
    }
    drop(guard);
    Ok(get_inbox(root))
}

pub fn clear_filtered(root: &str) -> Result<(), String> {
    let _guard = inbox_lock();
    write_filtered(root, &[])
}

/// Mark a single paper as read in the dedicated state file.
pub fn mark_paper_read(root: &str, arxiv_id: &str) -> Result<(), String> {
    let _guard = read_state_lock();
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
    let _guard = read_state_lock();
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

pub fn mark_in_library_statuses(root: &str, papers: &mut Vec<ArxivPaper>) {
    let lookup = build_library_lookup(root);
    for p in papers.iter_mut() {
        p.in_library = lookup_contains_paper(&lookup, &p.arxiv_id, &p.title, &p.authors);
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateHit {
    pub slug: String,
    pub title: String,
}

/// Scan the library for a paper matching the given identity, returning the
/// existing paper's slug + title if found. Match order: (1) arxiv_id
/// (version-insensitive) or DOI equality, (2) normalized title + first-author
/// lastname (matching also when either side has no first author). `exclude_slug`
/// skips one folder — used when the candidate is itself already present as a
/// freshly-imported temp folder (local PDF/ebook path).
pub fn find_duplicate(
    root: &str,
    arxiv_id: Option<&str>,
    doi: Option<&str>,
    title: &str,
    authors: &[String],
    exclude_slug: Option<&str>,
) -> Option<DuplicateHit> {
    let cand_arxiv_base = arxiv_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.split('v').next().unwrap_or(s));
    let cand_doi = doi.map(str::trim).filter(|s| !s.is_empty());
    let cand_title = normalize_title(title);
    let cand_lastname = authors
        .first()
        .map(|a| normalize_lastname(a))
        .unwrap_or_default();

    let entries = crate::paper::list_paper_dirs(root).ok()?;
    for (slug, path) in entries {
        if exclude_slug == Some(slug.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(path.join("meta.json")) else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let stored_title_raw = meta.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let hit = || {
            Some(DuplicateHit {
                slug: slug.clone(),
                title: stored_title_raw.to_string(),
            })
        };

        // 1a. arXiv id (version-insensitive)
        if let Some(base_input) = cand_arxiv_base {
            let stored = meta.get("arxiv_id").and_then(|v| v.as_str()).unwrap_or("");
            if !stored.is_empty() && stored.split('v').next().unwrap_or(stored) == base_input {
                return hit();
            }
        }
        // 1b. DOI
        if let Some(cd) = cand_doi {
            let stored = meta.get("doi").and_then(|v| v.as_str()).unwrap_or("");
            if !stored.is_empty() && stored.eq_ignore_ascii_case(cd) {
                return hit();
            }
        }
        // 2. Normalized title + first-author lastname
        if !cand_title.is_empty() && normalize_title(stored_title_raw) == cand_title {
            let stored_lastname = meta
                .get("authors")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|v| v.as_str())
                .map(normalize_lastname)
                .unwrap_or_default();
            if cand_lastname.is_empty() || stored_lastname.is_empty() || stored_lastname == cand_lastname
            {
                return hit();
            }
        }
    }
    None
}


/// Merge new papers into per-day files (dedup against all existing day files).
pub fn merge_into_inbox(root: &str, new_papers: Vec<ArxivPaper>) -> Result<ArxivInbox, String> {
    let guard = inbox_lock();
    // Collect all existing papers grouped by their day file.
    let all_dates = list_day_dates(root);
    // A day file that cannot be read stays out of the buckets, and is never
    // written below: writing its bucket would replace every paper in it with
    // just the newly fetched ones.
    let mut unreadable: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut day_buckets: std::collections::HashMap<String, Vec<ArxivPaper>> =
        std::collections::HashMap::new();
    for d in &all_dates {
        match read_day_papers_checked(root, d) {
            Ok(papers) => {
                day_buckets.insert(d.clone(), papers);
            }
            Err(e) => {
                eprintln!("[arxiv] {e}");
                unreadable.insert(d.clone(), e);
            }
        }
    }

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
                    new_p.analysis_error = old_p.analysis_error;
                    new_p.kept = old_p.kept;
                    // The state file is the authority for these, but a copy
                    // baked into the day file is all there is when that file
                    // is gone — and a fresh fetch knows neither.
                    new_p.read = new_p.read || old_p.read;
                    if new_p.rating == 0 {
                        new_p.rating = old_p.rating;
                    }
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

    if let Some(e) = changed_dates.iter().find_map(|d| unreadable.get(d)) {
        return Err(format!(
            "{e}。为免覆盖其中的论文，这次抓取的结果没有写入；请检查或移走这个文件后重试。"
        ));
    }

    // Write only modified day files
    for date in &changed_dates {
        if let Some(papers) = day_buckets.get_mut(date) {
            mark_in_library_statuses(root, papers);
            write_day_papers(root, date, papers)?;
        }
    }
    drop(guard);

    Ok(get_inbox(root))
}

// ── AI Analysis ───────────────────────────────────────────────────────────────

#[derive(Clone)]
struct AnalysisResult {
    relevance_score: f32,
    relevance_reason: String,
    key_contributions: Vec<String>,
    summary: Option<String>,
    matched_topics: Vec<String>,
}

/// The analysis object in a model reply, every field still the JSON value the
/// model sent. Typing it at this stage made one mismatched field — a list where
/// a string was asked for — fail the whole paper; the fields are converted
/// leniently in [`parse_analysis_result`] instead.
type RawAnalysis = serde_json::Map<String, serde_json::Value>;

const ANALYSIS_SYSTEM_PROMPT: &str =
    "你是一名严谨的研究助理。请只输出用户要求的有效 JSON，不要添加 Markdown 或解释。";

/// Builds the (system, user) pair for one paper.
///
/// The user's free-text requirements (`focus`) go into the system message rather
/// than being appended to the template, because the template ends with "仅回复
/// 符合此模式的有效 JSON" and anything tacked on after that weakens it. A template
/// containing `{focus}` opts into inline placement instead — that is an explicit
/// choice, so we honour it and leave the system message alone.
fn build_analysis_messages(
    template: &str,
    topics: &str,
    focus: &str,
    paper: &ArxivPaper,
) -> (String, String) {
    let base = if template.trim().is_empty() {
        DEFAULT_ARXIV_ANALYSIS_PROMPT
    } else {
        template
    };
    let focus = focus.trim();
    let inline = base.contains("{focus}");

    let user = base
        .replace("{focus}", focus)
        .replace("{topics}", topics)
        .replace("{title}", &paper.title)
        .replace("{authors}", &paper.authors.join(", "))
        .replace("{abstract}", &paper.summary);

    let system = if focus.is_empty() || inline {
        ANALYSIS_SYSTEM_PROMPT.to_string()
    } else {
        format!(
            "{}\n\n用户的额外要求（请在评分、匹配主题与总结时一并考虑）：\n{}",
            ANALYSIS_SYSTEM_PROMPT, focus
        )
    };

    (system, user)
}

/// The score as a number. A string is read up to its first non-numeric
/// character, so `"7"`, `"7分"` and `"7/10"` all count as 7.
fn parse_score(value: &serde_json::Value) -> Result<f32, String> {
    const MESSAGE: &str = "relevance_score must be a number from 0 to 10";
    if let Some(n) = value.as_f64() {
        return Ok(n as f32);
    }
    let s = value.as_str().ok_or_else(|| MESSAGE.to_string())?.trim();
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    s[..end].parse::<f32>().map_err(|_| MESSAGE.to_string())
}

/// A text field: a string, or a list where one string was asked for, joined.
/// `None` when there is nothing in it.
fn json_text(value: &serde_json::Value) -> Option<String> {
    let text = match value {
        serde_json::Value::String(s) => s.trim().to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Array(items) => {
            let mut out = String::new();
            for piece in items.iter().filter_map(json_text) {
                match out.chars().last() {
                    None => {}
                    Some('。' | '！' | '？' | '；') => {}
                    Some('.' | '!' | '?' | ';') => out.push(' '),
                    Some(_) => out.push('；'),
                }
                out.push_str(&piece);
            }
            out
        }
        _ => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

/// A list field: a list, or one string where a list was asked for — one item
/// per line, and for `tags` also per comma, with list markers dropped.
fn json_list(value: &serde_json::Value, tags: bool) -> Vec<String> {
    match value {
        serde_json::Value::Array(items) => items.iter().filter_map(json_text).collect(),
        serde_json::Value::String(s) => s
            .split(|c: char| c == '\n' || (tags && matches!(c, ',' | '，' | '、' | ';' | '；')))
            .map(|item| strip_list_marker(item).to_string())
            .filter(|item| !item.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// `- a`, `• a`, `1. a`, `1、a`, `1) a` → `a`. A number that is the text
/// itself — `3D 重建`, `1.5 倍加速` — is left alone.
fn strip_list_marker(item: &str) -> &str {
    let item = item.trim();
    if let Some(rest) = item.strip_prefix(['-', '*', '•', '·']) {
        return rest.trim_start();
    }
    let digits = item.find(|c: char| !c.is_ascii_digit()).unwrap_or(item.len());
    if digits > 0 {
        if let Some(rest) = item[digits..].strip_prefix(['.', '、', ')', '）']) {
            if !rest.starts_with(|c: char| c.is_ascii_digit()) {
                return rest.trim_start();
            }
        }
    }
    item
}

/// The first JSON value at the start of `text`, if it is an object carrying a
/// `relevance_score`. Whatever follows the value is ignored.
fn object_with_score(text: &str) -> Option<RawAnalysis> {
    let mut values = serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>();
    match values.next() {
        Some(Ok(serde_json::Value::Object(obj)))
            if obj.get("relevance_score").is_some_and(|v| !v.is_null()) =>
        {
            Some(obj)
        }
        _ => None,
    }
}

/// Escape what makes a reply's strings invalid JSON: raw line breaks and other
/// control characters, and double quotes left unescaped inside a value —
/// `"提出了一种"先高亮后摘要"的方法"`, which is how MiniMax writes a Chinese
/// quotation, and which failed that paper on every retry.
///
/// A `"` inside a string closes it only when what follows could follow a
/// string in JSON (see [`closes_string`]); any other is taken as part of the
/// text. `text` must start outside a string. Only ever applied to a reply that
/// has already failed to parse as it is, so valid JSON is never rewritten.
fn repair_json_strings(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let (mut in_string, mut escaped) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if !in_string {
            in_string = c == '"';
            out.push(c);
        } else if escaped {
            escaped = false;
            out.push(c);
        } else {
            match c {
                '\\' => {
                    escaped = true;
                    out.push(c);
                }
                '"' if closes_string(&chars[i + 1..]) => {
                    in_string = false;
                    out.push(c);
                }
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
                c => out.push(c),
            }
        }
    }
    out
}

/// Whether a `"` followed by `rest` can be the end of a JSON string: next comes
/// `:` (it was a key), the end of an object or list, the end of the text, or a
/// comma leading into another key or element.
fn closes_string(rest: &[char]) -> bool {
    let mut next = rest.iter().copied().filter(|c| !c.is_whitespace());
    match next.next() {
        None | Some(':' | '}' | ']') => true,
        Some(',') => matches!(
            next.next(),
            None | Some('"' | '{' | '[' | '}' | ']' | '-' | '0'..='9')
        ),
        Some(_) => false,
    }
}

/// The analysis object in a model reply.
///
/// The whole reply is tried first. Failing that, every `{` is tried from the
/// last one backwards, reading one JSON value and ignoring whatever follows it,
/// and the first object carrying a `relevance_score` wins. That survives
/// Markdown fences, prose after the JSON, and a leaked `<think>` draft that
/// echoes the schema — a draft that the old first-`{`-to-last-`}` slice glued
/// onto the real answer and then failed to parse. Scanning from the end picks
/// the model's final answer over any draft before it.
///
/// A candidate that does not parse is tried once more through
/// [`repair_json_strings`] before the scan moves on, so an answer with a stray
/// quote in it still wins over a valid draft earlier in the reply.
fn extract_analysis_json(content: &str) -> Result<RawAnalysis, String> {
    let trimmed = content.trim();
    let whole = serde_json::from_str::<serde_json::Value>(trimmed);
    if let Ok(serde_json::Value::Object(obj)) = &whole {
        if obj.get("relevance_score").is_some_and(|v| !v.is_null()) {
            return Ok(obj.clone());
        }
    }
    for (i, _) in trimmed.rmatch_indices('{') {
        let tail = &trimmed[i..];
        if let Some(obj) = object_with_score(tail) {
            return Ok(obj);
        }
        let repaired = repair_json_strings(tail);
        if repaired != tail {
            if let Some(obj) = object_with_score(&repaired) {
                return Ok(obj);
            }
        }
    }
    match whole {
        // Valid JSON without a score: `parse_score` says what is missing.
        Ok(serde_json::Value::Object(obj)) => Ok(obj),
        Ok(_) => Err("the reply is not a JSON object".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

fn parse_analysis_result(content: &str) -> Result<AnalysisResult, String> {
    let preview: String = content.chars().take(200).collect();
    let raw = extract_analysis_json(content)
        .map_err(|e| format!("Parse AI JSON: {e}\nContent was: {preview}"))?;
    let field = |key: &str| raw.get(key).unwrap_or(&serde_json::Value::Null);
    let relevance_score = parse_score(field("relevance_score"))?;
    let relevance_reason = json_text(field("relevance_reason"))
        .ok_or_else(|| "AI response missing relevance_reason".to_string())?;

    Ok(AnalysisResult {
        relevance_score,
        relevance_reason,
        key_contributions: json_list(field("key_contributions"), false),
        summary: json_text(field("summary")),
        matched_topics: json_list(field("matched_topics"), true),
    })
}

/// Why one paper's analysis produced no result.
enum CallError {
    /// The request failed — sorted by [`llm::classify_error`].
    Llm(String),
    /// The provider answered, but not with the JSON asked for. Always this
    /// paper's problem, whatever the text happens to contain.
    Parse(String),
}

impl CallError {
    fn message(&self) -> &str {
        match self {
            CallError::Llm(m) | CallError::Parse(m) => m,
        }
    }

    fn class(&self) -> llm::ErrorClass {
        match self {
            CallError::Llm(m) => llm::classify_error(m),
            CallError::Parse(_) => llm::ErrorClass::Request,
        }
    }
}

async fn call_ai_single(
    provider: &crate::models::AiProvider,
    api_key: &str,
    model: &str,
    topics: &str,
    focus: &str,
    prompt_template: &str,
    paper: &ArxivPaper,
) -> Result<AnalysisResult, CallError> {
    let (system, user) = build_analysis_messages(prompt_template, topics, focus, paper);
    let messages = vec![
        ChatMessage { role: "system".to_string(), content: system.into() },
        ChatMessage { role: "user".to_string(), content: user.into() },
    ];

    let content = llm::chat_completion(provider, api_key, model, &messages, "arxiv")
        .await
        .map_err(CallError::Llm)?;
    parse_analysis_result(&content).map_err(CallError::Parse)
}

// ── Inbox writes during analysis ──────────────────────────────────────────────

/// Papers a single-paper analysis ("AI 分析" on one paper) has in flight.
///
/// A bulk run started meanwhile must leave them alone: it would read their
/// "analyzing" as left over from a run that died, send them a second time, and
/// whichever of the two finished last would overwrite the other's result.
/// `analyze_single` registers *before* it checks for a bulk run and the claim
/// reads this *after* the running flag is set, so one of them always sees the
/// other.
fn single_in_flight() -> &'static Mutex<HashSet<String>> {
    static SET: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SET.get_or_init(Default::default)
}

struct SingleInFlight(String);

impl SingleInFlight {
    fn register(id: &str) -> Self {
        locked(single_in_flight()).insert(id.to_string());
        SingleInFlight(id.to_string())
    }
}

impl Drop for SingleInFlight {
    fn drop(&mut self) {
        locked(single_in_flight()).remove(&self.0);
    }
}

/// The pause a running batch is in, and how the last one ended — kept for
/// `get_schedule_status`, since a window opened mid-run missed the events.
fn analysis_pause() -> &'static Mutex<Option<ArxivAnalysisPause>> {
    static PAUSE: OnceLock<Mutex<Option<ArxivAnalysisPause>>> = OnceLock::new();
    PAUSE.get_or_init(Default::default)
}

fn last_analysis_run() -> &'static Mutex<Option<ArxivAnalysisRun>> {
    static RUN: OnceLock<Mutex<Option<ArxivAnalysisRun>>> = OnceLock::new();
    RUN.get_or_init(Default::default)
}

/// Outcomes of the running batch so far — for `get_schedule_status` too.
fn run_counts() -> &'static Mutex<ArxivRunCounts> {
    static COUNTS: OnceLock<Mutex<ArxivRunCounts>> = OnceLock::new();
    COUNTS.get_or_init(Default::default)
}

fn epoch_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Serialises read-modify-write passes over the inbox day files.
///
/// A fetch (`merge_into_inbox`), an import, a delete and the analysis batch all
/// rewrite whole day files from a snapshot they read first. Unserialised,
/// whichever wrote last silently undid the other — analysis results vanished,
/// or papers stayed "analyzing" forever. The scheduled fetch fires at a fixed
/// time of day, which is exactly when a user tends to start the analysis.
///
/// A plain `std` mutex, never held across an `.await`: every holder is a
/// synchronous block of file I/O.
fn inbox_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// A day file's papers, or `None` when the file is missing or cannot be
/// parsed. `read_day_papers` reads both as empty, and writing that back would
/// wipe the file — so the analysis passes use this and skip such files.
fn read_day_papers_strict(root: &str, date: &str) -> Option<Vec<ArxivPaper>> {
    if !day_file(root, date).exists() {
        return None;
    }
    match read_day_papers_checked(root, date) {
        Ok(papers) => Some(papers),
        Err(e) => {
            eprintln!("[arxiv] skipping {e}");
            None
        }
    }
}

/// The papers one click of "AI 分析全部" works on, after marking them
/// "analyzing" on disk.
struct Claimed {
    /// Never-analyzed papers first (newest day first), then the ones that
    /// failed before — so a retry of old failures never delays fresh papers.
    papers: Vec<ArxivPaper>,
    /// The status each paper had, restored for any paper the run does not
    /// finish (stopped, cancelled). A stale "analyzing" from a run that died
    /// counts as "pending".
    original: HashMap<String, String>,
    /// Every day file each paper was found in, so results are written where the
    /// paper actually is rather than where its `fetched_at` says it should be.
    dates: HashMap<String, Vec<String>>,
    retrying_failed: usize,
}

fn claim_papers_for_analysis(root: &str) -> Claimed {
    let _guard = inbox_lock();
    let busy: HashSet<String> = locked(single_in_flight()).clone();
    let mut pending = Vec::new();
    let mut failed = Vec::new();
    let mut original: HashMap<String, String> = HashMap::new();
    let mut dates: HashMap<String, Vec<String>> = HashMap::new();

    for date in list_day_dates(root) {
        let Some(mut papers) = read_day_papers_strict(root, &date) else { continue };
        let mut changed = false;
        for p in papers.iter_mut() {
            let was = match p.analysis_status.as_str() {
                // No run is active (the caller holds the running flag), so an
                // "analyzing" on disk is left over from one that never finished.
                "pending" | "analyzing" => "pending",
                "failed" => "failed",
                _ => continue,
            };
            if busy.contains(&p.arxiv_id) {
                continue;
            }
            dates.entry(p.arxiv_id.clone()).or_default().push(date.clone());
            if !original.contains_key(&p.arxiv_id) {
                original.insert(p.arxiv_id.clone(), was.to_string());
                if was == "failed" {
                    failed.push(p.clone());
                } else {
                    pending.push(p.clone());
                }
            }
            p.analysis_status = "analyzing".to_string();
            changed = true;
        }
        if changed {
            if let Err(e) = write_day_papers(root, &date, &papers) {
                eprintln!("[arxiv] mark analyzing in {date}: {e}");
            }
        }
    }

    let retrying_failed = failed.len();
    pending.extend(failed);
    Claimed { papers: pending, original, dates, retrying_failed }
}

/// What the batch decided for one paper, waiting to be written.
enum PaperUpdate {
    Done(AnalysisResult),
    /// Scored below the filter threshold (the second field): out of the inbox
    /// and into the filtered record.
    Remove(AnalysisResult, f32),
    Failed(String),
    /// Not finished in this run: back to the status it had before.
    Revert(String),
}

/// Write a set of results in one pass per day file, under the inbox lock.
///
/// Each paper is looked for in the files it was claimed from; one that has
/// moved since (a fetch during the run re-buckets papers it sees again) is
/// looked for everywhere else. A paper found nowhere was imported or deleted
/// meanwhile, and its update is dropped.
fn apply_updates(
    root: &str,
    mut updates: HashMap<String, PaperUpdate>,
    dates: &HashMap<String, Vec<String>>,
) {
    if updates.is_empty() {
        return;
    }
    let _guard = inbox_lock();
    let expected: BTreeSet<String> = updates
        .keys()
        .filter_map(|id| dates.get(id))
        .flatten()
        .cloned()
        .collect();
    let mut found: HashSet<String> = HashSet::new();
    let mut dropped: Vec<ArxivFilteredPaper> = Vec::new();
    for date in &expected {
        apply_updates_to_day(root, date, &updates, &mut found, &mut dropped);
    }
    updates.retain(|id, _| !found.contains(id));
    if !updates.is_empty() {
        for date in list_day_dates(root) {
            if !expected.contains(&date) {
                apply_updates_to_day(root, &date, &updates, &mut found, &mut dropped);
            }
        }
    }
    record_filtered(root, dropped);
}

fn apply_result(p: &mut ArxivPaper, r: &AnalysisResult) {
    p.relevance_score = Some(r.relevance_score.clamp(0.0, 10.0));
    p.relevance_reason = Some(r.relevance_reason.clone());
    p.key_contributions = r.key_contributions.clone();
    p.analysis_summary = r.summary.clone();
    p.matched_topics = r.matched_topics.clone();
    p.analysis_status = "done".to_string();
    p.analysis_error = None;
}

fn apply_updates_to_day(
    root: &str,
    date: &str,
    updates: &HashMap<String, PaperUpdate>,
    found: &mut HashSet<String>,
    dropped: &mut Vec<ArxivFilteredPaper>,
) {
    let Some(mut papers) = read_day_papers_strict(root, date) else { return };
    let mut changed = false;
    papers.retain_mut(|p| {
        let Some(update) = updates.get(&p.arxiv_id) else { return true };
        found.insert(p.arxiv_id.clone());
        // A result lands only on the mark this run set, or on the "pending" a
        // fetch during the run reset it to — never over a result someone else
        // wrote since.
        let ours = matches!(p.analysis_status.as_str(), "analyzing" | "pending");
        if !ours && !matches!(update, PaperUpdate::Revert(_)) {
            return true;
        }
        match update {
            PaperUpdate::Remove(r, threshold) => {
                apply_result(p, r);
                dropped.push(filtered_entry(p.clone(), *threshold));
                changed = true;
                return false;
            }
            PaperUpdate::Done(r) => {
                apply_result(p, r);
                changed = true;
            }
            PaperUpdate::Failed(message) => {
                p.analysis_status = "failed".to_string();
                p.analysis_error = Some(message.clone());
                changed = true;
            }
            // Only undo our own mark: a fetch during the run may already have
            // reset the paper, and that is not ours to overwrite.
            PaperUpdate::Revert(status) => {
                if p.analysis_status == "analyzing" {
                    p.analysis_status = status.clone();
                    changed = true;
                }
            }
        }
        true
    });
    if changed {
        if let Err(e) = write_day_papers(root, date, &papers) {
            eprintln!("[arxiv] write results to {date}: {e}");
        }
    }
}

// ── Batch runner ──────────────────────────────────────────────────────────────
//
// The batch used to send every paper as fast as the concurrency setting allowed
// and mark any error "failed" for good. Against a subscription plan — MiniMax's
// Token Plan answers `529 当前为整点高峰时段…请稍后重试 (2064)` around the top of
// the hour — that turned a throttle of a minute or two into thousands of failed
// papers within seconds, none of which the next click would retry. Errors are
// now sorted by `llm::classify_error`:
//
//   * Transient (throttling, overload, timeouts, 5xx): the whole batch pauses —
//     10 s, doubling up to 5 min — concurrency halves, and the paper goes back
//     into the queue a little way down, so one paper that keeps failing can
//     neither hold up the rest nor be pushed to the very end, where it would
//     retry alone and could not be told apart from an outage. Concurrency
//     creeps back up one step after each run of successes, so the batch
//     settles at what the provider tolerates.
//   * Fatal (bad key, empty balance, a used-up plan window): the batch stops at
//     once — every further request would fail the same way.
//   * Request (this paper's input or the reply to it): marked failed, with the
//     reason, and retried on the next click.
//
// The batch also stops when the provider has answered nothing for 12 minutes,
// or after a run of failures with no success in between. Whatever it did not
// finish goes back to the status it had, so the next click picks it up.

#[derive(Clone, Copy)]
struct BatchTuning {
    first_backoff: Duration,
    max_backoff: Duration,
    /// Stop after this long with no answer at all from the provider.
    stall_limit: Duration,
    /// Transient failures one paper may take — counting only those while other
    /// requests got answers — before it is marked failed. A provider-wide
    /// outage never counts, and is left to `stall_limit`.
    max_attempts: u32,
    /// Stop after this many request-level failures in a row.
    failure_streak_limit: u32,
    /// Successes in a row before concurrency is raised by one.
    raise_after: u32,
    /// How far down the queue a paper goes back in after a transient failure.
    requeue_gap: usize,
    /// Least time between two request starts; zero for none. Set from the
    /// provider's published limits (`minimax::batch_pacing`), so a batch — and
    /// every resume after a pause — ramps up instead of firing all its workers
    /// in the same instant.
    min_interval: Duration,
    poll: Duration,
}

const BATCH_TUNING: BatchTuning = BatchTuning {
    first_backoff: Duration::from_secs(10),
    max_backoff: Duration::from_secs(300),
    stall_limit: Duration::from_secs(12 * 60),
    max_attempts: 6,
    failure_streak_limit: 12,
    raise_after: 8,
    requeue_gap: 20,
    min_interval: Duration::ZERO,
    poll: Duration::from_millis(250),
};

/// Shared pacing state of one batch. Only ever locked briefly, never across
/// an `.await`.
struct Throttle {
    max: usize,
    /// Current concurrency: between 1 and `max`.
    limit: usize,
    in_flight: usize,
    /// Consecutive pauses without a success in between; sets the backoff.
    level: u32,
    paused_until: Option<Instant>,
    /// Last time the provider answered anything (a result or a per-paper error).
    last_answer: Instant,
    /// When the next request may go out (see `BatchTuning::min_interval`).
    next_start: Instant,
    successes: u32,
    failure_streak: u32,
    /// Set once the batch must stop; the reason is shown to the user.
    stop: Option<String>,
}

impl Throttle {
    fn new(max: usize, now: Instant) -> Self {
        let max = max.max(1);
        Throttle {
            max,
            limit: max,
            in_flight: 0,
            level: 0,
            paused_until: None,
            last_answer: now,
            next_start: now,
            successes: 0,
            failure_streak: 0,
            stop: None,
        }
    }

    fn paused(&self, now: Instant) -> bool {
        self.paused_until.is_some_and(|until| now < until)
    }

    /// A transient failure. Returns the pause to announce, or `None` when there
    /// is nothing new to announce: the batch is already paused (this request
    /// was in flight when the pause began — one overload, one pause), or it has
    /// just been stopped because the provider stayed silent too long.
    fn on_transient(&mut self, now: Instant, message: &str, t: &BatchTuning) -> Option<Duration> {
        if self.stop.is_some() {
            return None;
        }
        if now.saturating_duration_since(self.last_answer) >= t.stall_limit {
            self.stop = Some(format!(
                "服务商持续繁忙，{} 分钟内没有一次成功返回：{message}",
                (t.stall_limit.as_secs() / 60).max(1)
            ));
            return None;
        }
        if self.paused(now) {
            return None;
        }
        self.level = (self.level + 1).min(16);
        let factor = 1u32 << (self.level - 1).min(10);
        let wait = t.first_backoff.saturating_mul(factor).min(t.max_backoff);
        self.paused_until = Some(now + wait);
        self.limit = (self.limit / 2).max(1);
        self.successes = 0;
        Some(wait)
    }

    fn on_success(&mut self, now: Instant, t: &BatchTuning) {
        self.last_answer = now;
        self.level = 0;
        self.failure_streak = 0;
        self.successes += 1;
        if self.limit < self.max && self.successes >= t.raise_after {
            self.limit += 1;
            self.successes = 0;
        }
    }

    fn on_request_failure(&mut self, now: Instant, message: &str, t: &BatchTuning) {
        // The provider answered: it is up, just not with a usable result.
        self.last_answer = now;
        self.failure_streak += 1;
        if self.failure_streak >= t.failure_streak_limit && self.stop.is_none() {
            self.stop = Some(format!(
                "连续 {} 篇分析失败，已暂停以免继续消耗额度。最近一次：{message}",
                self.failure_streak
            ));
        }
    }

    fn on_fatal(&mut self, message: &str) {
        if self.stop.is_none() {
            self.stop = Some(message.to_string());
        }
    }
}

/// How one paper left the batch.
enum Outcome {
    Done(AnalysisResult),
    Failed(String),
    /// Not analyzed in this run — stopped, cancelled, or lost with a worker
    /// that panicked. It goes back to the status it had before.
    Untouched,
}

enum BatchMsg {
    /// A request for this paper is going out (sent again on every retry).
    Sending(String),
    Outcome(String, Outcome),
    /// The batch paused on a transient error.
    Waiting { message: String, retry_in: Duration, concurrency: usize },
}

type AnalyzeFn = Arc<
    dyn Fn(ArxivPaper) -> Pin<Box<dyn Future<Output = Result<AnalysisResult, CallError>> + Send>>
        + Send
        + Sync,
>;

struct Batch {
    queue: Mutex<VecDeque<(ArxivPaper, u32)>>,
    throttle: Mutex<Throttle>,
    cancel: Arc<AtomicBool>,
    tuning: BatchTuning,
}

fn locked<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// One of the batch's concurrency slots. Given back on drop, so a worker that
/// panics mid-request cannot shrink the batch for good — with the limit down
/// to 1, a leaked slot would stall every other worker forever.
struct Slot<'a>(&'a Mutex<Throttle>);

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        let mut t = locked(self.0);
        t.in_flight = t.in_flight.saturating_sub(1);
    }
}

async fn wait_for_flag(flag: &AtomicBool, poll: Duration) {
    while !flag.load(Ordering::SeqCst) {
        tokio::time::sleep(poll).await;
    }
}

/// Start `max_concurrency` workers over `papers`. Progress arrives on the
/// returned channel, which closes once every worker has exited; read
/// `Batch::throttle.stop` afterwards for why it stopped early, if it did.
fn spawn_batch(
    papers: Vec<ArxivPaper>,
    max_concurrency: usize,
    cancel: Arc<AtomicBool>,
    tuning: BatchTuning,
    analyze: AnalyzeFn,
) -> (Arc<Batch>, tokio::sync::mpsc::UnboundedReceiver<BatchMsg>) {
    let max = max_concurrency.max(1);
    let batch = Arc::new(Batch {
        queue: Mutex::new(papers.into_iter().map(|p| (p, 0)).collect()),
        throttle: Mutex::new(Throttle::new(max, Instant::now())),
        cancel,
        tuning,
    });
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    for _ in 0..max {
        tokio::spawn(batch_worker(batch.clone(), analyze.clone(), tx.clone()));
    }
    (batch, rx)
}

async fn batch_worker(
    batch: Arc<Batch>,
    analyze: AnalyzeFn,
    tx: tokio::sync::mpsc::UnboundedSender<BatchMsg>,
) {
    let tuning = batch.tuning;
    loop {
        // Wait for a free slot while the batch is not paused. A worker that
        // finds the queue empty is done: a paper still in flight elsewhere is
        // retried by the worker holding it.
        let (paper, attempts, slot) = loop {
            if batch.cancel.load(Ordering::SeqCst) {
                return;
            }
            let mut wait = tuning.poll;
            {
                let mut t = locked(&batch.throttle);
                if t.stop.is_some() {
                    return;
                }
                let mut queue = locked(&batch.queue);
                if queue.is_empty() {
                    return;
                }
                let now = Instant::now();
                if !t.paused(now) && t.in_flight < t.limit {
                    if now < t.next_start {
                        // Paced: come back when the next start is due.
                        wait = wait.min(t.next_start - now);
                    } else if let Some((paper, attempts)) = queue.pop_front() {
                        t.in_flight += 1;
                        t.next_start = now + tuning.min_interval;
                        drop(queue);
                        drop(t);
                        break (paper, attempts, Slot(&batch.throttle));
                    }
                }
            }
            tokio::time::sleep(wait).await;
        };

        let id = paper.arxiv_id.clone();
        let _ = tx.send(BatchMsg::Sending(id.clone()));
        let sent_at = Instant::now();
        // Cancelling drops the in-flight request instead of waiting it out.
        let result = tokio::select! {
            r = analyze(paper.clone()) => Some(r),
            _ = wait_for_flag(&batch.cancel, tuning.poll) => None,
        };
        drop(slot);

        let now = Instant::now();
        let outcome = match result {
            None => Outcome::Untouched,
            Some(Ok(r)) => {
                locked(&batch.throttle).on_success(now, &tuning);
                Outcome::Done(r)
            }
            Some(Err(e)) => {
                let message = e.message().to_string();
                match e.class() {
                    llm::ErrorClass::Transient => {
                        let (pause, stopped, limit, others_answered) = {
                            let mut t = locked(&batch.throttle);
                            // Did anything else get an answer while this was
                            // out? Only then is the failure this paper's; in an
                            // outage every paper fails alike and the stall
                            // limit, which reverts rather than fails, decides.
                            let others_answered = t.last_answer > sent_at;
                            let pause = t.on_transient(now, &message, &tuning);
                            (pause, t.stop.is_some(), t.limit, others_answered)
                        };
                        // A throttle lands on whichever paper happened to be
                        // next — never that paper's fault — so only the other
                        // transient errors (a timeout, a 5xx) count towards
                        // giving up on it. Throttling that never lets up is
                        // the stall limit's business, which reverts.
                        let counts = others_answered && !llm::is_throttle(&message);
                        let attempts = attempts + u32::from(counts);
                        if let Some(retry_in) = pause {
                            let _ = tx.send(BatchMsg::Waiting {
                                message: message.clone(),
                                retry_in,
                                concurrency: limit,
                            });
                        }
                        if stopped {
                            Outcome::Untouched
                        } else if attempts >= tuning.max_attempts {
                            Outcome::Failed(message)
                        } else {
                            let mut queue = locked(&batch.queue);
                            let at = queue.len().min(tuning.requeue_gap);
                            queue.insert(at, (paper, attempts));
                            continue;
                        }
                    }
                    llm::ErrorClass::Fatal => {
                        locked(&batch.throttle).on_fatal(&message);
                        Outcome::Untouched
                    }
                    llm::ErrorClass::Request => {
                        locked(&batch.throttle).on_request_failure(now, &message, &tuning);
                        Outcome::Failed(message)
                    }
                }
            }
        };
        let _ = tx.send(BatchMsg::Outcome(id, outcome));
    }
}

/// Clears the running flag on every exit path, a panic included — left set,
/// every later click of "AI 分析全部" would return silently until a restart.
struct RunningFlag;

impl Drop for RunningFlag {
    fn drop(&mut self) {
        analysis_running().store(false, Ordering::SeqCst);
    }
}

/// How often buffered results are written while a batch runs. Rewriting a
/// multi-megabyte day file once per result, several times a second, is what a
/// synced library folder turned into conflict copies ("2026-09-21 2.json").
const FLUSH_EVERY: Duration = Duration::from_secs(2);
const FLUSH_AT: usize = 25;

async fn flush_updates(
    root: &str,
    updates: HashMap<String, PaperUpdate>,
    dates: &Arc<HashMap<String, Vec<String>>>,
) {
    if updates.is_empty() {
        return;
    }
    let root = root.to_string();
    let dates = dates.clone();
    let _ = tokio::task::spawn_blocking(move || apply_updates(&root, updates, &dates)).await;
}

/// Analyze a single paper by arxiv_id regardless of its current status.
/// Skips the ai_analysis_enabled gate so users can manually trigger analysis.
///
/// Errors returned from here happened before anything was sent or written;
/// once the request is out, a failure is recorded on the paper and reported as
/// a `failed` event instead.
pub async fn analyze_single(
    root: &str,
    arxiv_id: &str,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    // Registered before the check, so a bulk run starting at this moment either
    // makes this refuse or sees the paper as taken (see `single_in_flight`).
    let _in_flight = SingleInFlight::register(arxiv_id);
    // A bulk run owns every pending/failed paper and would write over this one.
    if analysis_running().load(Ordering::SeqCst) {
        return Err("批量分析正在进行中，请等它结束后再单独分析。".to_string());
    }

    let config = get_arxiv_config(root);

    let provider_id = config
        .ai_provider_id
        .as_deref()
        .ok_or("未配置 AI 提供商，请在「设置 → AI 随航 → arXiv 爬取」中配置")?;
    let model_id = config
        .ai_model_id
        .as_deref()
        .ok_or("未配置 AI 模型，请在「设置 → AI 随航 → arXiv 爬取」中配置")?;

    let (provider, api_key, model, _fallback) =
        ai_manager::resolve_provider_model_or_default(root, Some(provider_id), Some(model_id))?;

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

    // A click deserves a couple of short retries on a throttle before it is
    // reported; the long waits are the batch's business.
    const RETRY_WAITS: [u64; 2] = [5, 15];
    let mut retries = 0;
    let outcome = loop {
        let r = call_ai_single(
            &provider,
            &api_key,
            &model,
            &keywords,
            &config.ai_analysis_focus,
            &prompt_template,
            &paper,
        )
        .await;
        match r {
            Err(e) if e.class() == llm::ErrorClass::Transient && retries < RETRY_WAITS.len() => {
                tokio::time::sleep(Duration::from_secs(RETRY_WAITS[retries])).await;
                retries += 1;
            }
            other => break other,
        }
    };

    match outcome {
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
                p.analysis_error = None;
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
            let message = e.message().to_string();
            eprintln!("Analysis error for {}: {}", arxiv_id, message);
            update_paper_in_day_files(root, arxiv_id, |p| {
                p.analysis_status = "failed".to_string();
                p.analysis_error = Some(message.clone());
            });
            let _ = app.emit(
                "arxiv-analysis",
                serde_json::json!({
                    "done": 1, "total": 1, "arxiv_id": arxiv_id, "status": "failed",
                    "message": &message
                }),
            );
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
        return Err("AI analysis is not enabled in Settings → AI Copilot → arXiv crawler.".to_string());
    }

    let provider_id = config
        .ai_provider_id
        .as_deref()
        .ok_or("No AI provider configured for arXiv analysis. Go to Settings → AI Copilot → arXiv crawler.")?;
    let model_id = config
        .ai_model_id
        .as_deref()
        .ok_or("No AI model configured for arXiv analysis. Go to Settings → AI Copilot → arXiv crawler.")?;

    let (provider, api_key, model, _fallback) =
        ai_manager::resolve_provider_model_or_default(root, Some(provider_id), Some(model_id))?;

    let keywords = if config.keywords.is_empty() {
        "machine learning, AI research".to_string()
    } else {
        config.keywords.join(", ")
    };

    let mut concurrency = config.ai_analysis_concurrency.clamp(1, 10) as usize;
    // MiniMax publishes its limits, so the batch keeps under them rather than
    // finding them by being throttled: fewer in flight on a Token Plan key,
    // and request starts spaced to the model's RPM on every key.
    let mut tuning = BATCH_TUNING;
    let mut concurrency_note: Option<String> = None;
    if crate::minimax::is_minimax(&provider) {
        let pacing = crate::minimax::batch_pacing(&api_key, &model);
        tuning.min_interval = pacing.min_interval;
        if let Some(cap) = pacing.max_in_flight.filter(|cap| concurrency > *cap) {
            concurrency = cap;
            concurrency_note = Some(format!(
                "MiniMax Token Plan 最多同时 {cap} 个请求（官方：高峰时 Plus 约 3–4 个）"
            ));
        }
    }

    // Atomically claim the "running" flag: only the caller that flips it from
    // false→true proceeds; concurrent callers see it already set and bail out.
    if analysis_running()
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(());
    }
    let _running = RunningFlag;
    *locked(analysis_pause()) = None;
    *locked(run_counts()) = ArxivRunCounts::default();
    analysis_cancel().store(false, Ordering::SeqCst);
    analysis_progress_done().store(0, Ordering::SeqCst);
    analysis_progress_total().store(0, Ordering::SeqCst);

    // Pending and previously failed papers alike: a failure is never final.
    let claimed = {
        let root = root.to_string();
        tokio::task::spawn_blocking(move || claim_papers_for_analysis(&root))
            .await
            .map_err(|e| format!("Claim papers for analysis: {e}"))?
    };
    let Claimed { papers, original, dates, retrying_failed } = claimed;
    let dates = Arc::new(dates);

    let total = papers.len() as u32;
    if total == 0 {
        let finished_at_ms = epoch_ms();
        *locked(last_analysis_run()) = Some(ArxivAnalysisRun {
            finished_at_ms, total: 0, succeeded: 0, failed: 0, filtered: 0, reverted: 0,
            stopped_reason: None, cancelled: false,
        });
        drop(_running);
        let _ = app.emit("arxiv-analysis", serde_json::json!({
            "done": 0, "total": 0, "arxiv_id": "", "status": "finished", "bulk": true,
            "succeeded": 0, "failed": 0, "filtered": 0, "reverted": 0,
            "stopped_reason": null, "cancelled": false, "finished_at_ms": finished_at_ms
        }));
        return Ok(());
    }

    analysis_progress_total().store(total, Ordering::SeqCst);

    let _ = app.emit("arxiv-analysis", serde_json::json!({
        "done": 0, "total": total, "arxiv_id": "", "status": "started", "bulk": true,
        "retrying_failed": retrying_failed,
        "concurrency": concurrency, "concurrency_note": concurrency_note
    }));

    let prompt_template = if config.ai_analysis_prompt.trim().is_empty() {
        DEFAULT_ARXIV_ANALYSIS_PROMPT.to_string()
    } else {
        config.ai_analysis_prompt.clone()
    };
    let filter_threshold = config.ai_filter_threshold.clamp(0.0, 10.0);
    let filter_enabled = config.ai_filter_enabled;

    let analyze: AnalyzeFn = {
        let provider = Arc::new(provider);
        let api_key = Arc::new(api_key);
        let model = Arc::new(model);
        let keywords = Arc::new(keywords);
        let focus = Arc::new(config.ai_analysis_focus.clone());
        let prompt_template = Arc::new(prompt_template);
        Arc::new(move |paper: ArxivPaper| {
            let (provider, api_key, model) = (provider.clone(), api_key.clone(), model.clone());
            let (keywords, focus, tmpl) = (keywords.clone(), focus.clone(), prompt_template.clone());
            Box::pin(async move {
                call_ai_single(&provider, &api_key, &model, &keywords, &focus, &tmpl, &paper).await
            })
        })
    };

    let (batch, mut rx) =
        spawn_batch(papers, concurrency, analysis_cancel().clone(), tuning, analyze);

    // Papers without a final result yet, with the status to restore if the
    // run ends before they get one.
    let mut outstanding = original;
    let mut buffer: HashMap<String, PaperUpdate> = HashMap::new();
    let mut last_flush = Instant::now();
    // Sent with every result, so the window can say how many were kept and
    // how many left the inbox as they go — without it, a fast model filtering
    // most papers out looked like one failing them all.
    let mut counts = ArxivRunCounts::default();
    let done_arc = analysis_progress_done().clone();

    loop {
        let msg = match tokio::time::timeout(FLUSH_EVERY, rx.recv()).await {
            Ok(Some(msg)) => Some(msg),
            Ok(None) => break,
            Err(_) => None,
        };
        match msg {
            None => {}
            Some(BatchMsg::Sending(id)) => {
                *locked(analysis_pause()) = None;
                let _ = app.emit("arxiv-analysis", serde_json::json!({
                    "done": done_arc.load(Ordering::SeqCst), "total": total,
                    "arxiv_id": &id, "status": "analyzing", "bulk": true
                }));
            }
            Some(BatchMsg::Waiting { message, retry_in, concurrency }) => {
                eprintln!("[arxiv] provider busy, pausing {}s: {message}", retry_in.as_secs());
                *locked(analysis_pause()) = Some(ArxivAnalysisPause {
                    message: message.clone(),
                    until_ms: epoch_ms() + retry_in.as_millis() as u64,
                    concurrency,
                });
                let _ = app.emit("arxiv-analysis", serde_json::json!({
                    "done": done_arc.load(Ordering::SeqCst), "total": total,
                    "arxiv_id": "", "status": "waiting", "bulk": true,
                    "message": message,
                    "retry_in": retry_in.as_secs_f64().ceil() as u64,
                    "concurrency": concurrency
                }));
            }
            Some(BatchMsg::Outcome(_, Outcome::Untouched)) => {}
            Some(BatchMsg::Outcome(id, Outcome::Done(result))) => {
                outstanding.remove(&id);
                let done_val = done_arc.fetch_add(1, Ordering::SeqCst) + 1;
                let score = result.relevance_score.clamp(0.0, 10.0);
                let removed = filter_enabled && score < filter_threshold;
                if removed {
                    counts.filtered += 1;
                } else {
                    counts.succeeded += 1;
                }
                *locked(run_counts()) = counts;
                let _ = app.emit("arxiv-analysis", serde_json::json!({
                    "done": done_val, "total": total,
                    "arxiv_id": &id,
                    "status": if removed { "filtered" } else { "done" },
                    "bulk": true, "removed": removed,
                    "score": score,
                    "reason": &result.relevance_reason,
                    "key_contributions": &result.key_contributions,
                    "analysis_summary": &result.summary,
                    "matched_topics": &result.matched_topics,
                    "succeeded": counts.succeeded, "filtered": counts.filtered,
                    "failed": counts.failed
                }));
                buffer.insert(
                    id,
                    if removed {
                        PaperUpdate::Remove(result, filter_threshold)
                    } else {
                        PaperUpdate::Done(result)
                    },
                );
            }
            Some(BatchMsg::Outcome(id, Outcome::Failed(message))) => {
                outstanding.remove(&id);
                counts.failed += 1;
                *locked(run_counts()) = counts;
                let done_val = done_arc.fetch_add(1, Ordering::SeqCst) + 1;
                eprintln!("Analysis error for {}: {}", id, message);
                let _ = app.emit("arxiv-analysis", serde_json::json!({
                    "done": done_val, "total": total,
                    "arxiv_id": &id, "status": "failed",
                    "bulk": true, "message": &message,
                    "succeeded": counts.succeeded, "filtered": counts.filtered,
                    "failed": counts.failed
                }));
                buffer.insert(id, PaperUpdate::Failed(message));
            }
        }
        if !buffer.is_empty() && (buffer.len() >= FLUSH_AT || last_flush.elapsed() >= FLUSH_EVERY) {
            flush_updates(root, std::mem::take(&mut buffer), &dates).await;
            last_flush = Instant::now();
        }
    }

    // Every worker has exited. Anything still outstanding was not finished in
    // this run: put it back as it was, so the next click picks it up again.
    let reverted = outstanding.len() as u32;
    for (id, status) in outstanding {
        buffer.entry(id).or_insert(PaperUpdate::Revert(status));
    }
    flush_updates(root, buffer, &dates).await;

    let ArxivRunCounts { succeeded, filtered, failed } = counts;
    let stopped_reason = locked(&batch.throttle).stop.clone();
    let cancelled = analysis_cancel().load(Ordering::SeqCst);
    if let Some(reason) = &stopped_reason {
        eprintln!("[arxiv] analysis stopped early ({reverted} left): {reason}");
    }
    let final_done = done_arc.load(Ordering::SeqCst);
    let finished_at_ms = epoch_ms();
    *locked(analysis_pause()) = None;
    *locked(last_analysis_run()) = Some(ArxivAnalysisRun {
        finished_at_ms,
        total,
        succeeded,
        failed,
        filtered,
        reverted,
        stopped_reason: stopped_reason.clone(),
        cancelled,
    });
    // Clear the flag before announcing the end, so a click prompted by the
    // "finished" event is not turned away by a run that is already over.
    drop(_running);
    let _ = app.emit("arxiv-analysis", serde_json::json!({
        "done": final_done, "total": total,
        "arxiv_id": "", "status": "finished", "bulk": true,
        "succeeded": succeeded, "failed": failed, "filtered": filtered,
        "reverted": reverted,
        "stopped_reason": stopped_reason,
        "cancelled": cancelled,
        "finished_at_ms": finished_at_ms
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
    force: bool,
) -> Result<ImportResult, String> {
    if let Some(cid) = collection_id.filter(|s| !s.is_empty()) {
        crate::collections::ensure_collection_can_receive_papers(root, cid)?;
    }

    // Find paper in inbox
    let inbox = get_inbox(root);
    let paper = inbox
        .papers
        .iter()
        .find(|p| p.arxiv_id == arxiv_id)
        .cloned()
        .ok_or_else(|| format!("Paper {} not found in inbox.", arxiv_id))?;

    // Duplicate check by arxiv_id/DOI or title+first-author (unless forced). For
    // bioRxiv recommendations the `arxiv_id` field actually carries the DOI.
    if !force {
        let is_biorxiv = paper.source.as_deref() == Some("biorxiv");
        let (cand_arxiv, cand_doi) = if is_biorxiv {
            (None, Some(arxiv_id))
        } else {
            (Some(arxiv_id), None)
        };
        if let Some(hit) =
            find_duplicate(root, cand_arxiv, cand_doi, &paper.title, &paper.authors, None)
        {
            return Ok(ImportResult::duplicate(hit.slug, hit.title));
        }
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

    // Verify the response is a real PDF (checks HTTP status + `%PDF` magic) so a
    // 404 / rate-limit / HTML error page never gets written as a broken paper.
    let pdf_bytes = download_pdf(&client, &pdf_url, &[]).await?;

    let _ = app.emit(
        "arxiv-import",
        serde_json::json!({
            "arxiv_id": arxiv_id, "status": "importing"
        }),
    );

    // Create paper directory
    let slug = make_slug(&paper.authors, &paper.published, &paper.title);
    let papers_root = Path::new(root).join("papers");

    // If slug collision, find the next free suffix instead of blindly using
    // `-2` (which would overwrite an existing `-2` paper and lose its data).
    let final_dir = unique_paper_dir(&papers_root, &slug);
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
        file_type: None,
        related_ids: Vec::new(),
        journal_rank: None,
    };
    paper::write_meta(root, &final_slug, &meta)?;
    paper::ensure_paper_files(root, &final_slug);

    // Mark metadata_fetched in status
    let mut status = PaperStatus::default();
    status.metadata_fetched = true;
    paper::write_status(root, &final_slug, &status)?;
    let _ = search::index_paper(root, &final_slug);

    // Once imported, remove the recommendation from the arXiv inbox.
    let inbox = {
        let _guard = inbox_lock();
        let mut inbox = get_inbox(root);
        inbox.papers.retain(|p| p.arxiv_id != arxiv_id);
        let _ = save_inbox(root, &inbox);
        inbox
    };
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
        crate::collections::move_paper_to_collection(root, &meta.id, cid)?;
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

    Ok(ImportResult::imported(final_slug))
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

    // Windows has no overlay titlebar; drop the native decorations so the window
    // uses our custom in-app titlebar (WindowControls) instead of showing an
    // extra native title row above it — matching the main window.
    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false);

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
    let analyzing = analysis_running().load(Ordering::SeqCst);

    // Outside a run: everything the next "AI 分析全部" would pick up is still to
    // do — a failed paper is retried, and a stale "analyzing" is one a run never
    // finished. During a run the live counters answer, so the whole inbox is
    // not re-read on every poll.
    let (analyzed, total_pending) = if analyzing {
        (0, 0)
    } else {
        let inbox = get_inbox(root);
        let to_do = inbox
            .papers
            .iter()
            .filter(|p| matches!(p.analysis_status.as_str(), "pending" | "failed" | "analyzing"))
            .count() as u32;
        let done = inbox
            .papers
            .iter()
            .filter(|p| p.analysis_status == "done")
            .count() as u32;
        (done, to_do)
    };

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
        waiting: if analyzing {
            locked(analysis_pause()).clone().filter(|p| p.until_ms > epoch_ms())
        } else {
            None
        },
        last_run: if analyzing { None } else { locked(last_analysis_run()).clone() },
        run_counts: if analyzing { Some(*locked(run_counts())) } else { None },
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

/// Resolve a collision-free paper directory under `papers_root` for `slug`.
/// Returns `papers_root/slug` if free, otherwise `slug-2`, `slug-3`, … picking
/// the first path that does not yet exist so an existing paper is never
/// overwritten.
pub(crate) fn unique_paper_dir(papers_root: &Path, slug: &str) -> PathBuf {
    let base = papers_root.join(slug);
    if !base.exists() {
        return base;
    }
    // Bounded search for a free suffix; fall back to a UUID so a pathological
    // run of collisions can never spin forever.
    for n in 2u32..=10_000 {
        let candidate = papers_root.join(format!("{slug}-{n}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    papers_root.join(format!("{slug}-{}", uuid::Uuid::new_v4()))
}

/// Import an arXiv paper by URL or bare ID.
/// Fetches metadata by scraping the abs page HTML (no API calls — they time out).
/// Downloads the PDF from the link found in the page.
pub async fn import_by_url(
    root: &str,
    url: &str,
    collection_id: &str,
    app: &tauri::AppHandle,
    force: bool,
) -> Result<ImportResult, String> {
    if !collection_id.is_empty() {
        collections::ensure_collection_can_receive_papers(root, collection_id)?;
    }

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

    // Pin the scrape to arxiv.org (and subdomains). A user-supplied URL only has
    // to *contain* "arxiv.org/abs/" to reach this branch, so without this a
    // link like https://evil.com/arxiv.org/abs/… would be fetched verbatim.
    crate::net::validate_host_suffix(&abs_url, &["arxiv.org"])?;

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

    // ── Already in library? Report the duplicate so the frontend can confirm ──
    if !force {
        if let Some(hit) = find_duplicate(root, Some(&arxiv_id), None, &title, &authors, None) {
            return Ok(ImportResult::duplicate(hit.slug, hit.title));
        }
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
        file_type: None,
        related_ids: Vec::new(),
        journal_rank: None,
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
    {
        let _guard = inbox_lock();
        let mut inbox = get_inbox(root);
        if let Some(p) = inbox.papers.iter_mut().find(|p| p.arxiv_id == arxiv_id) {
            p.in_library = true;
            let _ = save_inbox(root, &inbox);
        }
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

    Ok(ImportResult::imported(final_slug))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper() -> ArxivPaper {
        ArxivPaper {
            arxiv_id: "2401.00001".to_string(),
            title: "A Study".to_string(),
            authors: vec!["Ann".to_string(), "Bo".to_string()],
            summary: "We show things.".to_string(),
            categories: vec![],
            published: String::new(),
            updated: String::new(),
            pdf_url: String::new(),
            abs_url: String::new(),
            relevance_score: None,
            relevance_reason: None,
            key_contributions: vec![],
            analysis_summary: None,
            matched_topics: vec![],
            analysis_status: "pending".to_string(),
            analysis_error: None,
            in_library: false,
            fetched_at: String::new(),
            read: false,
            rating: 0,
            source: None,
            kept: false,
        }
    }

    #[test]
    fn placeholders_are_filled() {
        let (_, user) = build_analysis_messages(
            "主题：{topics}\n标题：{title}\n作者：{authors}\n摘要：{abstract}",
            "LLM, RL",
            "",
            &paper(),
        );
        assert_eq!(user, "主题：LLM, RL\n标题：A Study\n作者：Ann, Bo\n摘要：We show things.");
    }

    #[test]
    fn empty_focus_leaves_system_prompt_alone() {
        let (system, _) = build_analysis_messages(DEFAULT_ARXIV_ANALYSIS_PROMPT, "LLM", "   ", &paper());
        assert_eq!(system, ANALYSIS_SYSTEM_PROMPT);
    }

    #[test]
    fn focus_goes_to_system_and_keeps_json_instruction_last() {
        let (system, user) =
            build_analysis_messages(DEFAULT_ARXIV_ANALYSIS_PROMPT, "LLM", "偏好开源代码", &paper());
        assert!(system.contains("偏好开源代码"));
        // The template's closing "reply with JSON only" must stay at the end of the
        // user message — that is the whole reason focus lives in the system message.
        assert!(!user.contains("偏好开源代码"));
        assert!(user.trim_end().ends_with(
            r#"{"relevance_score": 0, "relevance_reason": "", "key_contributions": [], "summary": "", "matched_topics": []}"#
        ));
    }

    #[test]
    fn focus_placeholder_opts_into_inline_placement() {
        let (system, user) = build_analysis_messages(
            "要求：{focus}\n标题：{title}",
            "LLM",
            "偏好开源代码",
            &paper(),
        );
        assert_eq!(user, "要求：偏好开源代码\n标题：A Study");
        assert_eq!(system, ANALYSIS_SYSTEM_PROMPT, "inline placement must not also duplicate into system");
    }

    #[test]
    fn focus_placeholder_is_removed_when_focus_is_empty() {
        let (_, user) = build_analysis_messages("要求：{focus}|结束", "LLM", "", &paper());
        assert_eq!(user, "要求：|结束");
    }

    // ── Reply parsing ────────────────────────────────────────────────────────

    const ANSWER: &str = r#"{"relevance_score": 7, "relevance_reason": "相关", "key_contributions": ["a"], "summary": "s", "matched_topics": ["AI for Biology"]}"#;

    #[test]
    fn a_plain_or_fenced_reply_parses() {
        assert_eq!(parse_analysis_result(ANSWER).unwrap().relevance_score, 7.0);
        let fenced = format!("```json\n{ANSWER}\n```");
        assert_eq!(parse_analysis_result(&fenced).unwrap().relevance_score, 7.0);
    }

    #[test]
    fn a_leaked_draft_that_echoes_the_schema_does_not_break_parsing() {
        // The old first-`{`-to-last-`}` slice spanned the draft and the answer.
        let reply = format!(
            "<think>要输出 {{\"relevance_score\": 0, \"relevance_reason\": \"\"}} 这种格式</think>\n{ANSWER}"
        );
        let r = parse_analysis_result(&reply).unwrap();
        assert_eq!(r.relevance_score, 7.0);
        assert_eq!(r.relevance_reason, "相关");
    }

    #[test]
    fn prose_with_braces_after_the_answer_is_ignored() {
        let reply = format!("{ANSWER}\n注：分数按 {{0-10}} 计。");
        assert_eq!(parse_analysis_result(&reply).unwrap().relevance_score, 7.0);
    }

    #[test]
    fn a_reply_without_the_object_is_a_parse_error() {
        let err = parse_analysis_result("抱歉，我无法完成。").err().unwrap();
        assert!(err.starts_with("Parse AI JSON"), "{err}");
        let err = parse_analysis_result(r#"{"relevance_reason": "x"}"#).err().unwrap();
        assert!(err.contains("relevance_score"), "{err}");
    }

    #[test]
    fn unescaped_quotes_inside_a_chinese_value_are_read_as_text() {
        // MiniMax-M3's reply for 2609.31382, verbatim in shape: it failed with
        // "expected `,` or `}`" on every retry.
        let reply = r#"{"relevance_score": 2, "relevance_reason": "与所列主题均无关。", "key_contributions": ["提出H2S范式"], "summary": "这篇论文提出了一种"先高亮后摘要"的方法，让模型先找出"关键证据"。", "matched_topics": []}"#;
        assert!(serde_json::from_str::<serde_json::Value>(reply).is_err());
        let r = parse_analysis_result(reply).unwrap();
        assert_eq!(r.relevance_score, 2.0);
        assert_eq!(
            r.summary.as_deref(),
            Some(r#"这篇论文提出了一种"先高亮后摘要"的方法，让模型先找出"关键证据"。"#)
        );
        assert_eq!(r.key_contributions, vec!["提出H2S范式"]);

        // Before a closing brace, a comma into the next key, or a quote that
        // ends a list element, the quote is the real end of the string.
        let reply = "{\"relevance_score\": 6, \"relevance_reason\": \"称为\"X\"\", \"matched_topics\": [\"A\", \"B\"]}";
        let r = parse_analysis_result(reply).unwrap();
        assert_eq!(r.relevance_reason, "称为\"X\"");
        assert_eq!(r.matched_topics, vec!["A", "B"]);
    }

    #[test]
    fn raw_line_breaks_inside_a_value_are_escaped() {
        let reply = "{\"relevance_score\": 7, \"relevance_reason\": \"第一行\n第二行\", \"summary\": \"s\"}";
        assert_eq!(parse_analysis_result(reply).unwrap().relevance_reason, "第一行\n第二行");
    }

    #[test]
    fn a_repaired_answer_still_wins_over_a_valid_draft_before_it() {
        let reply = concat!(
            r#"草稿：{"relevance_score": 0, "relevance_reason": "占位"}"#,
            "\n",
            r#"{"relevance_score": 8, "relevance_reason": "提出"组合泛化"基准", "summary": "s"}"#
        );
        let r = parse_analysis_result(reply).unwrap();
        assert_eq!(r.relevance_score, 8.0);
        assert_eq!(r.relevance_reason, r#"提出"组合泛化"基准"#);
    }

    #[test]
    fn valid_json_is_never_rewritten() {
        // An escaped quote and a backslash survive exactly as sent.
        let reply = r#"{"relevance_score": 5, "relevance_reason": "称为\"X\"，路径 C:\\data", "summary": "s"}"#;
        assert_eq!(parse_analysis_result(reply).unwrap().relevance_reason, r#"称为"X"，路径 C:\data"#);
    }

    #[test]
    fn a_list_where_a_string_was_asked_for_and_the_other_way_round() {
        // "invalid type: sequence, expected a string" used to fail the paper.
        let reply = r#"{"relevance_score": "7分", "relevance_reason": ["与组合泛化相关。", "方法新颖"], "key_contributions": "1. 提出A\n2. 提出B\n- 3D 重建", "summary": ["第一句。", "第二句。"], "matched_topics": "AI for Biology，Compositional Generalization"}"#;
        let r = parse_analysis_result(reply).unwrap();
        assert_eq!(r.relevance_score, 7.0);
        assert_eq!(r.relevance_reason, "与组合泛化相关。方法新颖");
        assert_eq!(r.key_contributions, vec!["提出A", "提出B", "3D 重建"]);
        assert_eq!(r.summary.as_deref(), Some("第一句。第二句。"));
        assert_eq!(r.matched_topics, vec!["AI for Biology", "Compositional Generalization"]);

        assert_eq!(parse_score(&serde_json::json!("7/10")).unwrap(), 7.0);
        assert_eq!(parse_score(&serde_json::json!(" 6.5 ")).unwrap(), 6.5);
        assert!(parse_score(&serde_json::json!("高")).is_err());
        assert_eq!(strip_list_marker("1.5 倍加速"), "1.5 倍加速");
    }

    // ── Throttle ─────────────────────────────────────────────────────────────

    const FAST: BatchTuning = BatchTuning {
        first_backoff: Duration::from_millis(20),
        max_backoff: Duration::from_millis(80),
        stall_limit: Duration::from_millis(400),
        max_attempts: 50,
        failure_streak_limit: 4,
        raise_after: 2,
        requeue_gap: 4,
        min_interval: Duration::ZERO,
        poll: Duration::from_millis(2),
    };

    #[test]
    fn one_overload_is_one_pause_and_concurrency_halves() {
        let t0 = Instant::now();
        let mut t = Throttle::new(8, t0);
        assert_eq!(t.on_transient(t0, "busy", &FAST), Some(Duration::from_millis(20)));
        assert_eq!(t.limit, 4);
        // Requests that were already in flight fail too; that is still one pause.
        assert_eq!(t.on_transient(t0 + Duration::from_millis(5), "busy", &FAST), None);
        assert_eq!(t.limit, 4);
        // Still busy after the pause: wait twice as long, halve again.
        let t1 = t0 + Duration::from_millis(25);
        assert_eq!(t.on_transient(t1, "busy", &FAST), Some(Duration::from_millis(40)));
        assert_eq!(t.limit, 2);
        let t2 = t1 + Duration::from_millis(45);
        assert_eq!(t.on_transient(t2, "busy", &FAST), Some(Duration::from_millis(80)));
        let t3 = t2 + Duration::from_millis(85);
        assert_eq!(t.on_transient(t3, "busy", &FAST), Some(Duration::from_millis(80)), "capped");
        assert_eq!(t.limit, 1, "never below one");
        assert!(t.stop.is_none());
    }

    #[test]
    fn successes_reset_the_backoff_and_raise_concurrency_step_by_step() {
        let t0 = Instant::now();
        let mut t = Throttle::new(4, t0);
        t.on_transient(t0, "busy", &FAST);
        assert_eq!(t.limit, 2);
        let t1 = t0 + Duration::from_millis(30);
        t.on_success(t1, &FAST);
        assert_eq!(t.limit, 2);
        t.on_success(t1, &FAST);
        assert_eq!(t.limit, 3);
        t.on_success(t1, &FAST);
        t.on_success(t1, &FAST);
        assert_eq!(t.limit, 4);
        for _ in 0..10 {
            t.on_success(t1, &FAST);
        }
        assert_eq!(t.limit, 4, "never above the setting");
        assert_eq!(t.on_transient(t1, "busy", &FAST), Some(Duration::from_millis(20)), "backoff reset");
    }

    #[test]
    fn silence_past_the_stall_limit_stops_the_batch() {
        let t0 = Instant::now();
        let mut t = Throttle::new(2, t0);
        assert!(t.on_transient(t0 + Duration::from_millis(401), "busy (2064)", &FAST).is_none());
        let reason = t.stop.clone().unwrap();
        assert!(reason.contains("busy (2064)"), "{reason}");
    }

    #[test]
    fn a_run_of_request_failures_stops_the_batch() {
        let t0 = Instant::now();
        let mut t = Throttle::new(2, t0);
        for _ in 0..3 {
            t.on_request_failure(t0, "bad", &FAST);
        }
        assert!(t.stop.is_none());
        t.on_success(t0, &FAST);
        for _ in 0..3 {
            t.on_request_failure(t0, "bad", &FAST);
        }
        assert!(t.stop.is_none(), "a success in between resets the streak");
        t.on_request_failure(t0, "bad", &FAST);
        assert!(t.stop.is_some());
    }

    // ── Batch runner ─────────────────────────────────────────────────────────

    fn papers(n: usize) -> Vec<ArxivPaper> {
        (0..n)
            .map(|i| ArxivPaper { arxiv_id: format!("p{i}"), ..paper() })
            .collect()
    }

    fn ok_result() -> AnalysisResult {
        AnalysisResult {
            relevance_score: 7.0,
            relevance_reason: "ok".into(),
            key_contributions: vec![],
            summary: None,
            matched_topics: vec![],
        }
    }

    /// A fake provider: `reply(call_number, paper_id)` decides each answer.
    fn provider(
        reply: impl Fn(usize, &str) -> Result<(), CallError> + Send + Sync + 'static,
    ) -> (AnalyzeFn, Arc<std::sync::atomic::AtomicUsize>) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reply = Arc::new(reply);
        let counter = calls.clone();
        let f: AnalyzeFn = Arc::new(move |p: ArxivPaper| {
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let r = reply(n, &p.arxiv_id);
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                r.map(|_| ok_result())
            })
        });
        (f, calls)
    }

    #[derive(Default, Debug)]
    struct Seen {
        done: HashSet<String>,
        failed: HashMap<String, String>,
        untouched: HashSet<String>,
        waits: Vec<(Duration, usize)>,
    }

    async fn run(papers: Vec<ArxivPaper>, max: usize, analyze: AnalyzeFn) -> (Seen, Option<String>) {
        run_with(papers, max, analyze, Arc::new(AtomicBool::new(false)), FAST).await
    }

    async fn run_with(
        papers: Vec<ArxivPaper>,
        max: usize,
        analyze: AnalyzeFn,
        cancel: Arc<AtomicBool>,
        tuning: BatchTuning,
    ) -> (Seen, Option<String>) {
        let (batch, mut rx) = spawn_batch(papers, max, cancel, tuning, analyze);
        let mut seen = Seen::default();
        let drained = tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(msg) = rx.recv().await {
                match msg {
                    BatchMsg::Sending(_) => {}
                    BatchMsg::Waiting { retry_in, concurrency, .. } => {
                        seen.waits.push((retry_in, concurrency))
                    }
                    BatchMsg::Outcome(id, Outcome::Done(_)) => {
                        assert!(seen.done.insert(id), "a paper finished twice");
                    }
                    BatchMsg::Outcome(id, Outcome::Failed(m)) => {
                        assert!(seen.failed.insert(id, m).is_none(), "a paper failed twice");
                    }
                    BatchMsg::Outcome(id, Outcome::Untouched) => {
                        seen.untouched.insert(id);
                    }
                }
            }
        })
        .await;
        assert!(drained.is_ok(), "the batch never finished");
        let stop = locked(&batch.throttle).stop.clone();
        (seen, stop)
    }

    const OVERLOAD: &str = "API error 529: 当前为整点高峰时段，服务器短暂繁忙，通常 1-5 分钟内恢复。请稍后重试 (2064)";

    #[tokio::test]
    async fn a_peak_hour_overload_is_waited_out_and_nothing_fails() {
        let (analyze, _) = provider(|n, _| {
            if n < 5 { Err(CallError::Llm(OVERLOAD.into())) } else { Ok(()) }
        });
        let (seen, stop) = run(papers(12), 4, analyze).await;
        assert_eq!(stop, None);
        assert_eq!(seen.done.len(), 12, "{seen:?}");
        assert!(seen.failed.is_empty(), "{seen:?}");
        assert!(!seen.waits.is_empty());
        assert!(seen.waits[0].1 < 4, "concurrency was lowered");
    }

    #[tokio::test]
    async fn a_used_up_plan_window_stops_the_batch_without_failing_anything() {
        let (analyze, _) = provider(|n, _| {
            if n < 3 {
                Ok(())
            } else {
                Err(CallError::Llm(
                    "Rate limited (429): usage limit exceeded, 5-hour usage limit reached for Token Plan Plus, resets at 2026-09-28T15:00:00Z (2056)".into(),
                ))
            }
        });
        let (seen, stop) = run(papers(20), 2, analyze).await;
        assert!(stop.unwrap().contains("2056"));
        assert!(seen.failed.is_empty(), "quota is not the papers' fault: {seen:?}");
        assert!(seen.done.len() >= 3 && seen.done.len() < 20, "{seen:?}");
    }

    #[tokio::test]
    async fn a_bad_reply_fails_only_that_paper() {
        let (analyze, _) = provider(|_, id| {
            if id == "p2" { Err(CallError::Parse("Parse AI JSON: EOF".into())) } else { Ok(()) }
        });
        let (seen, stop) = run(papers(6), 3, analyze).await;
        assert_eq!(stop, None);
        assert_eq!(seen.failed.keys().collect::<Vec<_>>(), vec!["p2"]);
        assert_eq!(seen.done.len(), 5);
    }

    #[tokio::test]
    async fn a_run_of_errors_stops_before_burning_through_the_inbox() {
        let (analyze, calls) = provider(|_, _| Err(CallError::Llm("API error 400: bad".into())));
        let (seen, stop) = run(papers(50), 2, analyze).await;
        assert!(stop.is_some());
        assert!(seen.failed.len() >= 4 && seen.failed.len() <= 5, "{seen:?}");
        assert!(calls.load(Ordering::SeqCst) <= 5);
        assert!(seen.done.is_empty());
    }

    #[tokio::test]
    async fn a_provider_that_never_recovers_stops_the_batch_without_failing_anything() {
        let (analyze, _) = provider(|_, _| Err(CallError::Llm(OVERLOAD.into())));
        let (seen, stop) = run(papers(10), 3, analyze).await;
        assert!(stop.unwrap().contains("持续繁忙"));
        assert!(seen.failed.is_empty(), "{seen:?}");
        assert!(seen.done.is_empty());
    }

    #[tokio::test]
    async fn a_paper_that_keeps_timing_out_while_others_succeed_is_marked_failed() {
        // p0's request hangs until it times out; everyone else answers at once.
        let analyze: AnalyzeFn = Arc::new(|p: ArxivPaper| {
            Box::pin(async move {
                if p.arxiv_id == "p0" {
                    tokio::time::sleep(Duration::from_millis(15)).await;
                    Err(CallError::Llm("请求超时（120 秒内未完成）".into()))
                } else {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    Ok(ok_result())
                }
            })
        });
        // Re-queued a little way down, it keeps being retried alongside papers
        // that get answers, so its failures count and it is failed rather than
        // left to stall the batch; it never holds up the others either.
        let tuning = BatchTuning { max_attempts: 3, ..FAST };
        let cancel = Arc::new(AtomicBool::new(false));
        let (seen, stop) = run_with(papers(60), 3, analyze, cancel, tuning).await;
        assert_eq!(stop, None, "{seen:?}");
        assert!(seen.failed["p0"].contains("请求超时"), "{seen:?}");
        assert_eq!(seen.done.len(), 59);
    }

    #[tokio::test]
    async fn a_throttled_paper_is_never_failed_while_others_get_through() {
        // p0 is throttled on each of its first eight tries — more than
        // max_attempts — while everyone else is answered. Which paper a
        // throttle lands on is chance: p0 waits its turn and is analysed.
        let p0_tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tries = p0_tries.clone();
        let analyze: AnalyzeFn = Arc::new(move |p: ArxivPaper| {
            let throttled = p.arxiv_id == "p0" && tries.fetch_add(1, Ordering::SeqCst) < 8;
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                if throttled {
                    Err(CallError::Llm(
                        "Token Plan 请求过于频繁，已被限流，请稍后重试：已达到 Token Plan 速率限制 (2062)".into(),
                    ))
                } else {
                    Ok(ok_result())
                }
            })
        });
        let tuning = BatchTuning { max_attempts: 3, ..FAST };
        let cancel = Arc::new(AtomicBool::new(false));
        let (seen, stop) = run_with(papers(60), 3, analyze, cancel, tuning).await;
        assert_eq!(stop, None, "{seen:?}");
        assert!(seen.failed.is_empty(), "{seen:?}");
        assert_eq!(seen.done.len(), 60);
        assert!(p0_tries.load(Ordering::SeqCst) > 8);
    }

    #[tokio::test]
    async fn request_starts_are_spaced_by_the_pacing_interval() {
        let starts = Arc::new(Mutex::new(Vec::<Instant>::new()));
        let log = starts.clone();
        let analyze: AnalyzeFn = Arc::new(move |_p: ArxivPaper| {
            locked(&log).push(Instant::now());
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                Ok(ok_result())
            })
        });
        let gap = Duration::from_millis(25);
        let tuning = BatchTuning { min_interval: gap, ..FAST };
        // Eight workers for six papers: only the pacing holds them back.
        let cancel = Arc::new(AtomicBool::new(false));
        let (seen, stop) = run_with(papers(6), 8, analyze, cancel, tuning).await;
        assert_eq!((seen.done.len(), stop), (6, None));
        let starts = locked(&starts).clone();
        assert_eq!(starts.len(), 6);
        for pair in starts.windows(2) {
            // The mark is set a moment before the call is logged.
            let apart = pair[1] - pair[0];
            assert!(apart >= gap - Duration::from_millis(3), "{apart:?}");
        }
    }

    #[tokio::test]
    async fn cancelling_drops_the_requests_in_flight() {
        let analyze: AnalyzeFn = Arc::new(|_p: ArxivPaper| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(ok_result())
            })
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::SeqCst);
        });
        let (seen, _) = run_with(papers(5), 2, analyze, cancel, FAST).await;
        assert!(seen.done.is_empty() && seen.failed.is_empty());
        assert_eq!(seen.untouched.len(), 2, "the two in flight: {seen:?}");
    }

    #[tokio::test]
    async fn a_panicking_request_does_not_stall_the_other_workers() {
        let analyze: AnalyzeFn = Arc::new(|p: ArxivPaper| {
            Box::pin(async move {
                if p.arxiv_id == "p0" {
                    panic!("boom");
                }
                Ok(ok_result())
            })
        });
        let (seen, stop) = run(papers(6), 2, analyze).await;
        assert_eq!(stop, None);
        assert_eq!(seen.done.len(), 5, "{seen:?}");
        assert!(!seen.done.contains("p0"));
    }

    // ── Claim and write-back ─────────────────────────────────────────────────

    fn temp_root() -> String {
        let dir = std::env::temp_dir().join(format!("argus-arxiv-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("inbox")).unwrap();
        dir.to_string_lossy().to_string()
    }

    fn with_status(id: &str, status: &str) -> ArxivPaper {
        ArxivPaper {
            arxiv_id: id.into(),
            analysis_status: status.into(),
            fetched_at: "2026-09-20T03:00:00Z".into(),
            ..paper()
        }
    }

    fn status_on_disk(root: &str, id: &str) -> Option<(String, Option<String>)> {
        list_day_dates(root).into_iter().find_map(|d| {
            read_day_papers(root, &d)
                .into_iter()
                .find(|p| p.arxiv_id == id)
                .map(|p| (p.analysis_status, p.analysis_error))
        })
    }

    #[test]
    fn failed_papers_are_claimed_again_after_the_fresh_ones() {
        let root = temp_root();
        write_day_papers(&root, "2026-09-19", &[with_status("old-failed", "failed")]).unwrap();
        write_day_papers(
            &root,
            "2026-09-20",
            &[
                with_status("failed", "failed"),
                with_status("done", "done"),
                with_status("new", "pending"),
                with_status("stale", "analyzing"),
            ],
        )
        .unwrap();

        let claimed = claim_papers_for_analysis(&root);
        let ids: Vec<&str> = claimed.papers.iter().map(|p| p.arxiv_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "stale", "failed", "old-failed"]);
        assert_eq!(claimed.retrying_failed, 2);
        assert_eq!(claimed.original["stale"], "pending");
        assert_eq!(claimed.original["failed"], "failed");
        assert_eq!(status_on_disk(&root, "new").unwrap().0, "analyzing");
        assert_eq!(status_on_disk(&root, "done").unwrap().0, "done");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn results_and_reverts_land_where_the_paper_is_now() {
        let root = temp_root();
        write_day_papers(
            &root,
            "2026-09-20",
            &[
                with_status("a", "pending"),
                with_status("b", "failed"),
                with_status("c", "pending"),
                with_status("d", "pending"),
                with_status("moved", "pending"),
            ],
        )
        .unwrap();
        let claimed = claim_papers_for_analysis(&root);

        // A fetch during the run moved one paper to another day file.
        let mut day = read_day_papers(&root, "2026-09-20");
        let moved: Vec<ArxivPaper> = day.iter().filter(|p| p.arxiv_id == "moved").cloned().collect();
        day.retain(|p| p.arxiv_id != "moved");
        write_day_papers(&root, "2026-09-20", &day).unwrap();
        write_day_papers(&root, "2026-09-21", &moved).unwrap();

        let mut updates = HashMap::new();
        updates.insert("a".to_string(), PaperUpdate::Done(ok_result()));
        updates.insert("b".to_string(), PaperUpdate::Revert(claimed.original["b"].clone()));
        updates.insert("c".to_string(), PaperUpdate::Failed("API error 400: bad".into()));
        let low = AnalysisResult { relevance_score: 2.0, relevance_reason: "无关".into(), ..ok_result() };
        updates.insert("d".to_string(), PaperUpdate::Remove(low, 6.0));
        updates.insert("moved".to_string(), PaperUpdate::Done(ok_result()));
        apply_updates(&root, updates, &claimed.dates);

        assert_eq!(status_on_disk(&root, "a"), Some(("done".into(), None)));
        assert_eq!(status_on_disk(&root, "b"), Some(("failed".into(), None)), "back as it was");
        assert_eq!(
            status_on_disk(&root, "c"),
            Some(("failed".into(), Some("API error 400: bad".into())))
        );
        assert_eq!(status_on_disk(&root, "d"), None, "filtered out");
        assert_eq!(status_on_disk(&root, "moved").unwrap().0, "done");
        // ...and into the record, with the analysis that put it there.
        let record = get_filtered(&root);
        assert_eq!(record.len(), 1);
        assert_eq!(record[0].paper.arxiv_id, "d");
        assert_eq!(record[0].paper.relevance_score, Some(2.0));
        assert_eq!(record[0].paper.relevance_reason.as_deref(), Some("无关"));
        assert_eq!(record[0].paper.analysis_status, "done");
        assert_eq!(record[0].filter_threshold, 6.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn scored(id: &str, score: f32) -> ArxivPaper {
        ArxivPaper {
            relevance_score: Some(score),
            relevance_reason: Some("r".into()),
            ..with_status(id, "done")
        }
    }

    #[test]
    fn the_record_is_newest_first_without_repeats_and_capped() {
        let root = temp_root();
        let _guard = inbox_lock();
        record_filtered(&root, vec![filtered_entry(scored("a", 1.0), 6.0)]);
        record_filtered(&root, vec![filtered_entry(scored("b", 2.0), 6.0)]);
        // "a" filtered again, twice in one pass (it sat in two day files).
        record_filtered(
            &root,
            vec![filtered_entry(scored("a", 3.0), 6.0), filtered_entry(scored("a", 3.0), 6.0)],
        );
        let ids: Vec<String> = read_filtered(&root).into_iter().map(|e| e.paper.arxiv_id).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(read_filtered(&root)[0].paper.relevance_score, Some(3.0));

        let many = (0..FILTERED_KEEP + 7).map(|i| filtered_entry(scored(&format!("p{i}"), 1.0), 6.0));
        record_filtered(&root, many.collect());
        let record = read_filtered(&root);
        assert_eq!(record.len(), FILTERED_KEEP);
        assert_eq!(record[0].paper.arxiv_id, "p0");
        // The record is not a day file, whatever else lives in the folder.
        assert!(list_day_dates(&root).is_empty());
        drop(_guard);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restored_paper_comes_back_analysed_and_the_threshold_leaves_it_alone() {
        let root = temp_root();
        {
            let _guard = inbox_lock();
            record_filtered(
                &root,
                vec![
                    filtered_entry(scored("low", 3.0), 6.0),
                    filtered_entry(scored("again", 2.0), 6.0),
                    filtered_entry(scored("other", 1.0), 6.0),
                ],
            );
        }
        // "again" was fetched again since, and is waiting for analysis.
        write_day_papers(&root, "2026-09-20", &[with_status("again", "pending")]).unwrap();

        let inbox = restore_filtered(&root, &["low".into(), "again".into()]).unwrap();
        let low = inbox.papers.iter().find(|p| p.arxiv_id == "low").unwrap();
        assert_eq!(low.analysis_status, "done");
        assert_eq!(low.relevance_score, Some(3.0));
        assert!(low.kept);
        let again: Vec<_> = inbox.papers.iter().filter(|p| p.arxiv_id == "again").collect();
        assert_eq!(again.len(), 1, "not added a second time");
        assert_eq!(again[0].analysis_status, "pending", "the fresh copy is left as it is");
        let left: Vec<String> = get_filtered(&root).into_iter().map(|e| e.paper.arxiv_id).collect();
        assert_eq!(left, vec!["other"]);

        // The refresh button prunes below the threshold — but not what the
        // user put back, while what it does prune goes into the record.
        write_day_papers(
            &root,
            "2026-09-21",
            &[ArxivPaper { fetched_at: "2026-09-21T03:00:00Z".into(), ..scored("stale", 1.0) }],
        )
        .unwrap();
        let pruned = prune_low_relevance(&root).unwrap();
        assert!(pruned.papers.iter().any(|p| p.arxiv_id == "low"));
        assert!(!pruned.papers.iter().any(|p| p.arxiv_id == "stale"));
        assert_eq!(get_filtered(&root)[0].paper.arxiv_id, "stale");

        // A re-fetch keeps the mark.
        let refetched = ArxivPaper { kept: false, ..with_status("low", "pending") };
        let inbox = merge_into_inbox(&root, vec![refetched]).unwrap();
        assert!(inbox.papers.iter().find(|p| p.arxiv_id == "low").unwrap().kept);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_restore_never_writes_over_an_unreadable_day_file() {
        let root = temp_root();
        {
            let _guard = inbox_lock();
            record_filtered(&root, vec![filtered_entry(scored("a", 3.0), 6.0)]);
        }
        std::fs::write(day_file(&root, "2026-09-20"), "{ not json").unwrap();
        assert!(restore_filtered(&root, &["a".into()]).is_err());
        assert_eq!(std::fs::read_to_string(day_file(&root, "2026-09-20")).unwrap(), "{ not json");
        assert_eq!(get_filtered(&root).len(), 1, "still in the record");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_success_clears_the_old_failure_reason() {
        let root = temp_root();
        let mut p = with_status("a", "failed");
        p.analysis_error = Some("API error 529".into());
        write_day_papers(&root, "2026-09-20", &[p]).unwrap();
        let claimed = claim_papers_for_analysis(&root);
        let mut updates = HashMap::new();
        updates.insert("a".to_string(), PaperUpdate::Done(ok_result()));
        apply_updates(&root, updates, &claimed.dates);
        assert_eq!(status_on_disk(&root, "a"), Some(("done".into(), None)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_day_file_is_left_alone() {
        let root = temp_root();
        let path = day_file(&root, "2026-09-20");
        std::fs::write(&path, "{ not json").unwrap();
        let claimed = claim_papers_for_analysis(&root);
        assert!(claimed.papers.is_empty());
        let mut dates = HashMap::new();
        dates.insert("x".to_string(), vec!["2026-09-20".to_string()]);
        let mut updates = HashMap::new();
        updates.insert("x".to_string(), PaperUpdate::Revert("pending".into()));
        apply_updates(&root, updates, &dates);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_paper_in_a_single_analysis_is_not_claimed_by_a_bulk_run() {
        let root = temp_root();
        write_day_papers(&root, "2026-09-20", &[with_status("busy", "analyzing"), with_status("free", "pending")])
            .unwrap();
        let guard = SingleInFlight::register("busy");
        let claimed = claim_papers_for_analysis(&root);
        drop(guard);
        let ids: Vec<&str> = claimed.papers.iter().map(|p| p.arxiv_id.as_str()).collect();
        assert_eq!(ids, vec!["free"]);
        assert!(!claimed.original.contains_key("busy"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_result_never_overwrites_one_written_by_someone_else() {
        let root = temp_root();
        write_day_papers(&root, "2026-09-20", &[with_status("a", "pending")]).unwrap();
        let claimed = claim_papers_for_analysis(&root);
        // A single analysis (or another device) finished it in the meantime.
        update_paper_in_day_files(&root, "a", |p| p.analysis_status = "done".into());
        let mut updates = HashMap::new();
        updates.insert("a".to_string(), PaperUpdate::Failed("API error 400: bad".into()));
        apply_updates(&root, updates, &claimed.dates);
        assert_eq!(status_on_disk(&root, "a"), Some(("done".into(), None)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_real_dates_are_day_files_and_the_read_state_survives_a_save() {
        let root = temp_root();
        write_day_papers(&root, "2026-09-20", &[with_status("a", "done")]).unwrap();
        let inbox = inbox_dir(&root);
        std::fs::write(inbox.join("read_state.json"), r#"{"a":{"read":true,"rating":4}}"#).unwrap();
        std::fs::write(inbox.join("2026-09-21 2.json"), "[]").unwrap();
        assert_eq!(list_day_dates(&root), vec!["2026-09-20".to_string()]);

        let snapshot = get_inbox(&root);
        assert_eq!(snapshot.last_updated, "2026-09-20");
        save_inbox(&root, &snapshot).unwrap();
        assert!(inbox.join("read_state.json").exists(), "read/rating state was deleted");
        let a = get_inbox(&root).papers.into_iter().find(|p| p.arxiv_id == "a").unwrap();
        assert!(a.read);
        assert_eq!(a.rating, 4);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unreadable_day_file_is_neither_deleted_nor_overwritten() {
        let root = temp_root();
        write_day_papers(&root, "2026-09-19", &[with_status("ok", "done")]).unwrap();
        let bad = day_file(&root, "2026-09-20");
        std::fs::write(&bad, r#"[{"arxiv_id":"x","rating":"five"}]"#).unwrap();

        // Refresh / import used to delete it as an empty day.
        let err = save_inbox(&root, &get_inbox(&root)).unwrap_err();
        assert!(err.contains("2026-09-20"), "{err}");
        assert!(bad.exists());

        // A fetch into that day used to replace it with only the new papers.
        let mut fresh = with_status("new", "pending");
        fresh.fetched_at = "2026-09-20T08:00:00Z".into();
        assert!(merge_into_inbox(&root, vec![fresh]).is_err());
        assert_eq!(std::fs::read_to_string(&bad).unwrap(), r#"[{"arxiv_id":"x","rating":"five"}]"#);

        // A fetch into another day still works.
        let mut other = with_status("other", "pending");
        other.fetched_at = "2026-09-21T08:00:00Z".into();
        assert!(merge_into_inbox(&root, vec![other]).is_ok());
        assert_eq!(status_on_disk(&root, "other").unwrap().0, "pending");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refetch_keeps_the_read_mark_and_rating_baked_into_the_day_file() {
        let root = temp_root();
        let mut old = with_status("a", "done");
        old.read = true;
        old.rating = 3;
        write_day_papers(&root, "2026-09-20", &[old]).unwrap();
        // No read_state.json (lost to the old bug); the fetch knows neither.
        let mut again = with_status("a", "pending");
        again.fetched_at = "2026-09-22T08:00:00Z".into();
        merge_into_inbox(&root, vec![again]).unwrap();
        let a = get_inbox(&root).papers.into_iter().find(|p| p.arxiv_id == "a").unwrap();
        assert!(a.read);
        assert_eq!(a.rating, 3);
        assert_eq!(a.analysis_status, "done");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn the_last_paper_is_not_failed_by_an_outage_it_is_left_to_the_stall_limit() {
        let (analyze, _) = provider(|_, _| Err(CallError::Llm(OVERLOAD.into())));
        // A cap far below the number of tries an outage produces.
        let tuning = BatchTuning { max_attempts: 2, ..FAST };
        let cancel = Arc::new(AtomicBool::new(false));
        let (seen, stop) = run_with(papers(1), 2, analyze, cancel, tuning).await;
        assert!(stop.unwrap().contains("持续繁忙"));
        assert!(seen.failed.is_empty(), "{seen:?}");
    }

    #[test]
    fn old_inbox_entries_without_the_new_field_still_load() {
        let old = r#"[{"arxiv_id":"1","title":"t","authors":[],"summary":"","categories":[],"published":"","updated":"","pdf_url":"","abs_url":"","relevance_score":null,"relevance_reason":null,"analysis_status":"failed","fetched_at":"2026-09-20T00:00:00Z"}]"#;
        let papers: Vec<ArxivPaper> = serde_json::from_str(old).unwrap();
        assert_eq!(papers[0].analysis_error, None);
        // And a None error is not written back, so older builds see the same shape.
        assert!(!serde_json::to_string(&papers[0]).unwrap().contains("analysis_error"));
    }
}
