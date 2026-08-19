//! Asking the user before the agent writes anything.
//!
//! The agent's writing tools (`mcp::app_tools`) never run on the model's say-so.
//! This module is the handshake in between: the loop parks the pending write
//! here, the window shows it, and the write happens only if the user approves
//! *that* request.
//!
//! # Shape
//!
//! ```text
//!   agent loop            this module                 the window
//!   ──────────            ───────────                 ──────────
//!   request(preview) ──▶  park a oneshot sender  ──▶  emit "<event>-confirm"
//!                                                     user clicks
//!   Decision       ◀──    resolve(id, approved) ◀──   invoke resolve_agent_write
//! ```
//!
//! # Defaults that matter
//!
//! Every way this can go wrong resolves to *not writing*: a window that never
//! answers (timeout), a window that closed with the dialog open (timeout), a
//! user who stopped the generation (cancel), a duplicate answer (the first one
//! wins and the id is gone). Approval has to be an explicit click that arrives
//! in time; everything else is a refusal.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tauri::Emitter;
use tokio::sync::oneshot;

/// How long a pending write waits for an answer before giving up.
///
/// Long enough to read a page of markdown and think about it, short enough that
/// an agent left running against a closed window does not hang forever.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(180);

/// How often the wait re-checks the generation's cancel flag, so pressing stop
/// takes the dialog down instead of leaving it stranded.
const CANCEL_POLL: Duration = Duration::from_millis(250);

/// Why a write did not happen, or that it may proceed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Approved,
    Rejected,
    TimedOut,
    Cancelled,
}

impl Decision {
    pub fn approved(self) -> bool {
        matches!(self, Decision::Approved)
    }

    /// What the model is told, as the result of its tool call. Phrased so it
    /// does not retry the same write in a loop: a refusal is an instruction to
    /// stop, not a transient error.
    pub fn message(self) -> &'static str {
        match self {
            Decision::Approved => "The user approved this write.",
            Decision::Rejected => {
                "The user declined this write, so nothing was saved. Do not retry it. \
                 Ask what they would like changed."
            }
            Decision::TimedOut => {
                "The confirmation was not answered in time, so nothing was saved. \
                 Do not retry automatically — mention it and let the user ask again."
            }
            Decision::Cancelled => "The user stopped this answer, so nothing was saved.",
        }
    }
}

fn registry() -> &'static Mutex<HashMap<String, oneshot::Sender<bool>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, oneshot::Sender<bool>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Show `preview` to the user and wait for their answer.
///
/// `event_name` is the streaming event prefix of the answer being generated, so
/// the request lands in the window and the conversation that asked for it — a
/// second chat window running its own agent never sees this dialog.
///
/// `preview` is already-serialized JSON so the one handshake serves both writes
/// that have a preview shape of their own — a note ([`crate::mcp::app_tools`])
/// and a canvas edit ([`crate::canvas_edit`]). The window tells them apart by the
/// `tool` field inside it.
pub async fn request(
    app: &tauri::AppHandle,
    event_name: &str,
    preview: serde_json::Value,
    cancel: Option<Arc<AtomicBool>>,
) -> Decision {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = oneshot::channel();
    if let Ok(mut map) = registry().lock() {
        map.insert(id.clone(), tx);
    }
    // Undoes the registration on every exit path below.
    let guard = PendingGuard(id.clone());

    let payload = serde_json::json!({
        "requestId": id,
        "preview": preview,
    });
    if app
        .emit(format!("{event_name}-confirm").as_str(), payload)
        .is_err()
    {
        return Decision::TimedOut;
    }

    let deadline = tokio::time::Instant::now() + CONFIRM_TIMEOUT;
    let decision = loop {
        // Poll rather than a bare `timeout`, so a stopped generation takes the
        // dialog down promptly instead of holding the loop for three minutes.
        tokio::select! {
            answer = &mut rx => break match answer {
                Ok(true) => Decision::Approved,
                // A dropped sender means the entry was cleared without an
                // answer; treat it exactly like a refusal.
                Ok(false) | Err(_) => Decision::Rejected,
            },
            _ = tokio::time::sleep_until(deadline) => break Decision::TimedOut,
            _ = tokio::time::sleep(CANCEL_POLL) => {
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
                    break Decision::Cancelled;
                }
            }
        }
    };

    drop(guard);
    // Take the dialog off screen for every outcome, including the ones the
    // window does not know about (timeout, stop).
    let _ = app.emit(
        format!("{event_name}-confirm-close").as_str(),
        serde_json::json!({ "requestId": id }),
    );
    decision
}

/// Removes a pending request from the registry however the wait ended.
struct PendingGuard(String);

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut map) = registry().lock() {
            map.remove(&self.0);
        }
    }
}

/// Deliver the user's answer. Unknown ids are ignored: the request may have
/// timed out, been cancelled, or already been answered.
pub fn resolve(request_id: &str, approved: bool) {
    let sender = registry().lock().ok().and_then(|mut m| m.remove(request_id));
    if let Some(tx) = sender {
        let _ = tx.send(approved);
    }
}

/// Tauri command: the window's answer to a pending write confirmation.
#[tauri::command]
pub fn resolve_agent_write(request_id: String, approved: bool) {
    resolve(&request_id, approved);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_id_is_ignored() {
        // The window answering twice, or answering after a timeout, must not
        // panic or resurrect a write.
        resolve("no-such-request", true);
    }

    #[test]
    fn every_outcome_but_approval_says_nothing_was_saved() {
        for d in [Decision::Rejected, Decision::TimedOut, Decision::Cancelled] {
            assert!(!d.approved());
            assert!(
                d.message().contains("nothing was saved"),
                "{d:?} does not tell the model the write did not happen"
            );
        }
        assert!(Decision::Approved.approved());
    }

    /// The registry hands each id to exactly one waiter: a second answer for
    /// the same request finds nothing to deliver to.
    #[tokio::test]
    async fn only_the_first_answer_counts() {
        let (tx, rx) = oneshot::channel();
        registry().lock().unwrap().insert("req-1".into(), tx);

        resolve("req-1", true);
        resolve("req-1", false);

        assert_eq!(rx.await.unwrap(), true);
        assert!(!registry().lock().unwrap().contains_key("req-1"));
    }
}
