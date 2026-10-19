//! Durable observations, cheap scheduling, and bounded background consolidation.
//! Scheduling never calls a classifier/model and never drops unprocessed turns.
use crate::user_memory::Observation;
use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use tauri::{Emitter, Manager};

const IDLE_SECONDS: i64 = 180;
const MIN_INTERVAL: i64 = 180;
const MAX_RUNS_PER_HOUR: usize = 6;
const READY_TURNS: usize = 6;
const BATCH_TURNS: usize = 16;
const BATCH_CHARS: usize = 24_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingTurn {
    pub observation: Observation,
    pub root: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    queued_at: i64,
    #[serde(default)]
    retry_at: i64,
    #[serde(default)]
    failures: u32,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Queue {
    pending: Vec<PendingTurn>,
    attempts: Vec<i64>,
    last_attempt: Option<i64>,
    background_runs: u64,
}

struct Runtime {
    path: PathBuf,
    queue: Queue,
    active: bool,
}
static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
fn notify() -> &'static tokio::sync::Notify {
    static SIGNAL: OnceLock<tokio::sync::Notify> = OnceLock::new();
    SIGNAL.get_or_init(tokio::sync::Notify::new)
}
fn control() -> &'static tokio::sync::Notify {
    static SIGNAL: OnceLock<tokio::sync::Notify> = OnceLock::new();
    SIGNAL.get_or_init(tokio::sync::Notify::new)
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn announce(app: &tauri::AppHandle) {
    let _ = app.emit("user-memory-changed", ());
}
fn save(runtime: &Runtime) -> Result<(), String> {
    if let Some(parent) = runtime.path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::fsutil::atomic_write_str(
        &runtime.path,
        &serde_json::to_string(&runtime.queue).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

impl Queue {
    fn enqueue(&mut self, turn: PendingTurn) {
        if !self.pending.iter().any(|p| {
            p.observation.fingerprint == turn.observation.fingerprint
                && p.observation.generation == turn.observation.generation
        }) {
            self.pending.push(turn);
        }
    }
    fn batch_at(&self, at: i64) -> Vec<PendingTurn> {
        let Some(first) = self
            .pending
            .iter()
            .find(|p| p.retry_at <= at)
            .or_else(|| self.pending.iter().min_by_key(|p| p.retry_at))
        else {
            return vec![];
        };
        let mut chars = 0;
        self.pending
            .iter()
            .filter(|p| {
                p.root == first.root
                    && p.provider_id == first.provider_id
                    && p.model_id == first.model_id
                    && p.retry_at <= at.max(first.retry_at)
            })
            // Malformed/truncated responses can be retried as smaller batches
            // without losing any observations or repeatedly buying a huge call.
            .take((BATCH_TURNS >> first.failures.min(4)).max(1))
            .take_while(|p| {
                let next =
                    p.observation.text.chars().count() + p.observation.context.chars().count();
                let accept = chars == 0 || chars + next <= BATCH_CHARS;
                if accept {
                    chars += next;
                }
                accept
            })
            .cloned()
            .collect()
    }
    fn due_at(&self, batch: &[PendingTurn], at: i64) -> Option<i64> {
        if batch.is_empty() {
            return None;
        }
        let newest = batch.iter().map(|p| p.queued_at).max().unwrap_or(at);
        let mut due = if batch.len() >= READY_TURNS {
            at
        } else {
            newest + IDLE_SECONDS
        };
        if let Some(last) = self.last_attempt {
            due = due.max(last + MIN_INTERVAL);
        }
        due = due.max(batch.iter().map(|turn| turn.retry_at).max().unwrap_or(0));
        let recent: Vec<_> = self
            .attempts
            .iter()
            .copied()
            .filter(|time| *time > at - 3600)
            .collect();
        if recent.len() >= MAX_RUNS_PER_HOUR {
            due = due.max(recent[recent.len() - MAX_RUNS_PER_HOUR] + 3600);
        }
        Some(due)
    }
    fn claim(&mut self, at: i64) {
        self.attempts.retain(|time| *time > at - 3600);
        self.attempts.push(at);
        self.last_attempt = Some(at);
        self.background_runs = self.background_runs.saturating_add(1);
    }
    fn finish(&mut self, batch: &[PendingTurn], success: bool, at: i64) {
        if success {
            self.pending.retain(|turn| {
                !batch.iter().any(|done| {
                    done.observation.fingerprint == turn.observation.fingerprint
                        && done.observation.generation == turn.observation.generation
                })
            });
        } else {
            let mut retry = Vec::new();
            self.pending.retain(|turn| {
                if batch.iter().any(|done| {
                    done.observation.fingerprint == turn.observation.fingerprint
                        && done.observation.generation == turn.observation.generation
                }) {
                    let mut turn = turn.clone();
                    turn.failures = turn.failures.saturating_add(1);
                    turn.retry_at =
                        at + (60i64.saturating_mul(1i64 << turn.failures.min(5))).min(1800);
                    retry.push(turn);
                    false
                } else {
                    true
                }
            });
            // A broken/deleted provider cannot starve observations from others.
            self.pending.extend(retry);
        }
    }
}

#[derive(Default)]
pub(crate) struct Status {
    pub pending: usize,
    pub runs: u64,
    pub updating: bool,
}
pub(crate) fn status() -> Status {
    RUNTIME
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map(|runtime| Status {
            pending: runtime.queue.pending.len(),
            runs: runtime.queue.background_runs,
            updating: runtime.active,
        })
        .unwrap_or_default()
}
pub(crate) fn invalidate(app: &tauri::AppHandle, generation: u64, preserve: bool) {
    let mut recover = false;
    if let Some(runtime) = RUNTIME.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        if preserve {
            for turn in &mut runtime.queue.pending {
                turn.observation.generation = generation;
            }
        } else {
            runtime
                .queue
                .pending
                .retain(|turn| turn.observation.generation == generation);
        }
        let _ = save(runtime);
    } else if !preserve {
        // Explicit clear/opt-out may recover a malformed pending file. Ordinary
        // startup/editing must preserve it instead of silently replacing data.
        if let Ok(base) = app.path().app_data_dir() {
            let runtime = Runtime {
                path: base.join("user_memory_pending.json"),
                queue: Queue::default(),
                active: false,
            };
            recover = save(&runtime).is_ok();
        }
    }
    if recover {
        spawn(app);
    }
    notify().notify_one();
    control().notify_one();
}
pub(crate) fn enqueue(
    app: &tauri::AppHandle,
    root: &str,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    observation: Observation,
) {
    let saved = {
        let mut slot = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
        let Some(runtime) = slot.as_mut() else {
            drop(slot);
            crate::user_memory::set_error(app, "storage_failed");
            return;
        };
        // Check under the queue lock: clear/opt-out either invalidated this
        // ticket already or will remove it after the profile mutation commits.
        if !crate::user_memory::eligible(app, &observation) {
            return;
        }
        runtime.queue.enqueue(PendingTurn {
            observation,
            root: root.into(),
            provider_id: provider_id.map(str::to_string),
            model_id: model_id.map(str::to_string),
            queued_at: now(),
            retry_at: 0,
            failures: 0,
        });
        save(runtime)
    };
    if saved.is_err() {
        crate::user_memory::set_error(app, "storage_failed");
    }
    announce(app);
    notify().notify_one();
}

pub(crate) fn spawn(app: &tauri::AppHandle) {
    let Ok(base) = app.path().app_data_dir() else {
        return;
    };
    let path = base.join("user_memory_pending.json");
    let queue = if path.exists() {
        match std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Queue>(&text).ok())
        {
            Some(queue) => queue,
            None => {
                crate::user_memory::set_error(app, "storage_failed");
                return;
            }
        }
    } else {
        Queue::default()
    };
    {
        let mut slot = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_some() {
            return;
        }
        *slot = Some(Runtime {
            path,
            queue,
            active: false,
        });
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            // Register before inspecting the queue so an enqueue cannot lose a wakeup.
            let wake = notify().notified();
            let (batch, wait) = {
                let mut slot = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
                let Some(runtime) = slot.as_mut() else { return };
                let batch = runtime.queue.batch_at(now());
                match runtime.queue.due_at(&batch, now()) {
                    Some(due) if due <= now() => {
                        runtime.queue.claim(now());
                        runtime.active = true;
                        if save(runtime).is_ok() {
                            (batch, 0)
                        } else {
                            runtime.active = false;
                            (vec![], 60)
                        }
                    }
                    Some(due) => (vec![], (due - now()).max(1) as u64),
                    None => (vec![], 60),
                }
            };
            if !batch.is_empty() {
                announce(&app);
                let work = crate::user_memory::process_batch(&app, &batch);
                tokio::pin!(work);
                let completed = loop {
                    tokio::select! {
                        result = &mut work => break result,
                        _ = control().notified() => {
                            if !crate::user_memory::eligible(&app, &batch[0].observation) {
                                break Ok(batch.iter().map(|turn| turn.observation.fingerprint.clone()).collect::<Vec<_>>());
                            }
                        }
                    }
                };
                let success = completed.is_ok();
                let finished: Vec<_> = match &completed {
                    Ok(ids) => batch
                        .iter()
                        .filter(|turn| ids.contains(&turn.observation.fingerprint))
                        .cloned()
                        .collect(),
                    Err(_) => batch.clone(),
                };
                let save_failed = {
                    let mut slot = RUNTIME.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(runtime) = slot.as_mut() {
                        runtime.queue.finish(&finished, success, now());
                        runtime.active = false;
                        save(runtime).is_err()
                    } else {
                        false
                    }
                };
                if save_failed {
                    crate::user_memory::set_error(&app, "storage_failed");
                }
                announce(&app);
                continue;
            }
            tokio::select! { _ = wake => {}, _ = tokio::time::sleep(std::time::Duration::from_secs(wait.min(60))) => {} }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn turn(index: usize, at: i64) -> PendingTurn {
        PendingTurn {
            observation: crate::user_memory::test_observation(&format!("知识背景 {index}")),
            root: "library".into(),
            provider_id: Some("provider".into()),
            model_id: Some("model".into()),
            queued_at: at,
            retry_at: 0,
            failures: 0,
        }
    }
    #[test]
    fn six_turns_create_one_batch_instead_of_six_model_requests() {
        let mut queue = Queue::default();
        for index in 0..6 {
            queue.enqueue(turn(index, 1000 + index as i64));
            let batch = queue.batch_at(1005);
            if index < 5 {
                assert!(queue.due_at(&batch, 1000 + index as i64).unwrap() > 1000 + index as i64);
            }
        }
        let batch = queue.batch_at(1005);
        assert_eq!(batch.len(), 6);
        assert_eq!(queue.due_at(&batch, 1005), Some(1005));
        queue.claim(1005);
        queue.finish(&batch, true, 1005);
        assert_eq!(queue.background_runs, 1);
        assert!(queue.pending.is_empty());
    }
    #[test]
    fn sparse_turns_flush_after_idle_but_respect_minimum_interval() {
        let mut queue = Queue::default();
        queue.enqueue(turn(1, 1000));
        assert_eq!(queue.due_at(&queue.batch_at(1005), 1000), Some(1180));
        queue.claim(1170);
        assert_eq!(queue.due_at(&queue.batch_at(1005), 1180), Some(1350));
    }
    #[test]
    fn hourly_budget_defers_without_discarding_any_pending_feedback() {
        let mut queue = Queue::default();
        for index in 0..100 {
            queue.enqueue(turn(index, 1000));
        }
        for index in 0..6 {
            queue.claim(1000 + index * 180);
        }
        assert_eq!(queue.due_at(&queue.batch_at(1005), 2000), Some(4600));
        assert_eq!(queue.pending.len(), 100);
        let mut seen = Vec::new();
        while !queue.pending.is_empty() {
            let batch = queue.batch_at(1005);
            seen.extend(batch.iter().map(|t| t.observation.text.clone()));
            queue.finish(&batch, true, 5000);
        }
        assert_eq!(seen.len(), 100);
        seen.sort();
        seen.dedup();
        assert_eq!(seen.len(), 100);
    }
    #[test]
    fn restart_and_retry_preserve_full_turns_and_provider_identity() {
        let mut queue = Queue::default();
        queue.enqueue(turn(1, 1000));
        queue.enqueue(turn(1, 1000));
        assert_eq!(queue.pending.len(), 1);
        queue.claim(1180);
        let batch = queue.batch_at(1005);
        queue.finish(&batch, false, 1180);
        let restored: Queue =
            serde_json::from_str(&serde_json::to_string(&queue).unwrap()).unwrap();
        assert_eq!(restored.pending[0].observation.text, "知识背景 1");
        assert_eq!(restored.pending[0].model_id.as_deref(), Some("model"));
        assert_eq!(restored.background_runs, 1);
        assert!(restored.due_at(&restored.batch_at(1200), 1200).unwrap() > 1200);
    }

    #[test]
    fn failed_provider_cannot_starve_another_and_smaller_success_removes_only_completed_turns() {
        let mut queue = Queue::default();
        queue.enqueue(turn(1, 1000));
        let mut other = turn(2, 1000);
        other.provider_id = Some("other".into());
        queue.enqueue(other);
        let failed = queue.batch_at(1180);
        queue.claim(1180);
        queue.finish(&failed, false, 1180);
        let ready = queue.batch_at(1360);
        assert_eq!(ready[0].provider_id.as_deref(), Some("other"));
        queue.finish(&ready, true, 1360);
        assert_eq!(queue.pending.len(), 1);
        for index in 3..20 {
            queue.enqueue(turn(index, 1400));
        }
        let next = queue.batch_at(1540);
        let done = next[..2].to_vec();
        queue.finish(&done, true, 1540);
        assert_eq!(queue.pending.len(), 16);
        assert!(!queue.pending.iter().any(|p| done
            .iter()
            .any(|done| done.observation.fingerprint == p.observation.fingerprint)));
    }
}
