use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub ts: String,
    pub source: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

// Current library root — set whenever a library is opened.
static CURRENT_ROOT: std::sync::OnceLock<Mutex<String>> = std::sync::OnceLock::new();

fn root_mutex() -> &'static Mutex<String> {
    CURRENT_ROOT.get_or_init(|| Mutex::new(String::new()))
}

pub fn set_root(root: &str) {
    *root_mutex().lock().unwrap() = root.to_string();
}

fn current_root() -> Option<String> {
    let s = root_mutex().lock().unwrap().clone();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn usage_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join("token_usage.jsonl")
}

/// Append one usage record to the JSONL file for the current library.
pub fn record(source: &str, provider: &str, model: &str, input_tokens: u64, output_tokens: u64) {
    if input_tokens == 0 && output_tokens == 0 {
        return;
    }
    let Some(root) = current_root() else { return };
    let ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let rec = UsageRecord {
        ts,
        source: source.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        input_tokens,
        output_tokens,
    };
    let Ok(line) = serde_json::to_string(&rec) else {
        return;
    };
    let path = usage_path(&root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(format!("{line}\n").as_bytes());
    }
}

pub fn get_all(root: &str) -> Vec<UsageRecord> {
    let path = usage_path(root);
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

pub fn clear(root: &str) -> Result<(), String> {
    let path = usage_path(root);
    if path.exists() {
        std::fs::write(&path, "").map_err(|e| e.to_string())?;
    }
    Ok(())
}
