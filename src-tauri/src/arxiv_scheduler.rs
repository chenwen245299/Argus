use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use tauri::{Emitter, Manager};

use crate::LibraryRoot;

static SCHEDULER_ENABLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn scheduler_enabled() -> &'static Arc<AtomicBool> {
    SCHEDULER_ENABLED.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn set_enabled(enabled: bool) {
    scheduler_enabled().store(enabled, Ordering::SeqCst);
}

fn get_root(app: &tauri::AppHandle) -> Option<String> {
    let state: tauri::State<LibraryRoot> = app.state();
    // Tolerate a poisoned lock so a panic elsewhere can't take down the
    // long-lived scheduler loop.
    let guard = state.0.lock().unwrap_or_else(|e| e.into_inner());
    guard.clone()
}

/// Compute seconds until the next scheduled fetch (capped at 1 hour so config changes are picked up).
/// Returns 0 if a fetch is overdue.
fn secs_until_next_check(root: &str) -> u64 {
    use chrono::{Duration, Local, NaiveTime};

    let config = crate::arxiv::get_arxiv_config(root);
    if !config.auto_fetch_enabled {
        return 300;
    }

    let Ok(fetch_time) = NaiveTime::parse_from_str(&config.fetch_time, "%H:%M") else {
        return 300;
    };

    let now = Local::now();
    let today = now.naive_local().date();

    let next_date = match &config.last_fetch_date {
        None => today,
        Some(last) => {
            let last_date = chrono::NaiveDate::parse_from_str(last, "%Y-%m-%d")
                .unwrap_or(today - Duration::days(1));
            let next = last_date + Duration::days(config.interval_days as i64);
            if next <= today {
                today
            } else {
                next
            }
        }
    };

    let next_dt = next_date.and_time(fetch_time);
    let now_naive = now.naive_local();

    if next_dt <= now_naive {
        0
    } else {
        ((next_dt - now_naive).num_seconds().max(0) as u64).min(3600)
    }
}

fn should_fetch_now(root: &str) -> bool {
    use chrono::{Local, NaiveTime};

    let config = crate::arxiv::get_arxiv_config(root);
    if !config.auto_fetch_enabled {
        return false;
    }

    let Ok(fetch_time) = NaiveTime::parse_from_str(&config.fetch_time, "%H:%M") else {
        return false;
    };

    let now = Local::now();
    if now.time() < fetch_time {
        return false;
    }

    let today = now.naive_local().date();
    let last_date = config
        .last_fetch_date
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

    match last_date {
        None => true,
        Some(last) => (today - last).num_days() >= config.interval_days as i64,
    }
}

pub fn start_scheduler(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Wait 10 minutes after startup before the first check.
        tokio::time::sleep(std::time::Duration::from_secs(600)).await;

        loop {
            let sleep_secs: u64 = match get_root(&app) {
                None => 300, // No library open yet
                Some(ref r) => secs_until_next_check(r).max(1),
            };

            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;

            if !scheduler_enabled().load(Ordering::SeqCst) {
                continue;
            }

            let root = match get_root(&app) {
                Some(r) => r,
                None => continue,
            };

            if should_fetch_now(&root) {
                // Ask the frontend to do the fetch (TypeScript handles HTTP).
                let _ = app.emit("arxiv-fetch-due", serde_json::json!({}));
            }
        }
    });
}

/// Called when auto-fetch is toggled — updates the enabled flag.
pub fn on_auto_fetch_changed(enabled: bool) {
    set_enabled(enabled);
}
