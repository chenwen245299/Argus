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

#[derive(Debug, PartialEq, Eq)]
pub struct Resolution {
    pub decision: Decision,
    pub passage_colors: Option<Vec<String>>,
}

impl Resolution {
    fn from_decision(decision: Decision) -> Self { Self { decision, passage_colors: None } }
    pub fn approved(&self) -> bool { self.decision.approved() }
    pub fn message(&self) -> &'static str { self.decision.message() }
}

struct PendingConfirmation {
    sender: oneshot::Sender<Resolution>,
    passage_count: Option<usize>,
}

fn registry() -> &'static Mutex<HashMap<String, PendingConfirmation>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, PendingConfirmation>>> = OnceLock::new();
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
) -> Resolution {
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = oneshot::channel();
    if let Ok(mut map) = registry().lock() {
        let passage_count = color_choice_count(&preview);
        map.insert(id.clone(), PendingConfirmation { sender: tx, passage_count });
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
        return Resolution::from_decision(Decision::TimedOut);
    }

    let deadline = tokio::time::Instant::now() + CONFIRM_TIMEOUT;
    let decision = loop {
        // Poll rather than a bare `timeout`, so a stopped generation takes the
        // dialog down promptly instead of holding the loop for three minutes.
        tokio::select! {
            answer = &mut rx => break match answer {
                Ok(resolution) => resolution,
                // A dropped sender means the entry was cleared without an
                // answer; treat it exactly like a refusal.
                Err(_) => Resolution::from_decision(Decision::Rejected),
            },
            _ = tokio::time::sleep_until(deadline) => break Resolution::from_decision(Decision::TimedOut),
            _ = tokio::time::sleep(CANCEL_POLL) => {
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst)) {
                    break Resolution::from_decision(Decision::Cancelled);
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

/// How many colors the card may send back with an approval: one per new
/// passage, or one per changed mark's new color. A retraction, a note and
/// everything else take none.
fn color_choice_count(preview: &serde_json::Value) -> Option<usize> {
    let list = |key: &str| preview.get(key).and_then(|p| p.as_array()).filter(|p| !p.is_empty());
    if let Some(passages) = list("passages") {
        return Some(passages.len());
    }
    let changes = list("changes")?;
    changes.iter().all(|c| c.get("after").is_some()).then_some(changes.len())
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
#[cfg(test)]
fn resolve(request_id: &str, approved: bool) {
    let _ = resolve_with_colors(request_id, approved, None);
}

fn resolve_with_colors(request_id: &str, approved: bool, colors: Option<Vec<String>>) -> Result<(), String> {
    let mut map = registry().lock().map_err(|_| "Confirmation registry unavailable".to_string())?;
    let Some(pending) = map.get(request_id) else { return Ok(()); };
    let passage_colors = if approved {
        match colors {
            Some(colors) => {
                if pending.passage_count != Some(colors.len()) {
                    return Err("Color choices must match the previewed PDF passages.".into());
                }
                Some(colors.iter().map(|c| crate::mcp::app_tools::normalize_mark_color(c)).collect::<Result<Vec<_>, _>>()?)
            }
            None => None,
        }
    } else { None };
    if let Some(pending) = map.remove(request_id) {
        let _ = pending.sender.send(Resolution {
            decision: if approved { Decision::Approved } else { Decision::Rejected }, passage_colors,
        });
    }
    Ok(())
}

/// Tauri command: the window's answer to a pending write confirmation.
#[tauri::command]
pub fn resolve_agent_write(request_id: String, approved: bool, passage_colors: Option<Vec<String>>) -> Result<(), String> {
    resolve_with_colors(&request_id, approved, passage_colors)
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
        registry().lock().unwrap().insert("req-1".into(), PendingConfirmation { sender: tx, passage_count: None });

        resolve("req-1", true);
        resolve("req-1", false);

        assert_eq!(rx.await.unwrap().decision, Decision::Approved);
        assert!(!registry().lock().unwrap().contains_key("req-1"));
    }

    #[tokio::test]
    async fn approved_colors_are_normalized_and_bound_to_the_preview_count() {
        let (tx, rx) = oneshot::channel();
        registry().lock().unwrap().insert("colored-batch".into(), PendingConfirmation { sender: tx, passage_count: Some(2) });
        assert!(resolve_with_colors("colored-batch", true, Some(vec!["blue".into()])).is_err());
        assert!(resolve_with_colors("colored-batch", true, Some(vec!["blue".into(), "bad-color".into()])).is_err());
        assert!(registry().lock().unwrap().contains_key("colored-batch"));
        resolve_with_colors("colored-batch", true, Some(vec!["blue".into(), "#12ab34".into()])).unwrap();
        let choice = rx.await.unwrap();
        assert!(choice.approved());
        assert_eq!(choice.passage_colors, Some(vec!["#90CAF9".into(), "#12AB34".into()]));
    }

    #[test]
    fn only_new_passages_and_recolorable_changes_accept_colors() {
        let passages = serde_json::json!({"passages": [{"color": "#FFEB3B"}, {"color": "#90CAF9"}]});
        assert_eq!(color_choice_count(&passages), Some(2));
        let update = serde_json::json!({"tool": "update_paper_highlights", "changes": [
            {"before": {"color": "#FFEB3B"}, "after": {"color": "#CE93D8"}}]});
        assert_eq!(color_choice_count(&update), Some(1));
        let removal = serde_json::json!({"tool": "remove_paper_highlights", "changes": [{"before": {"color": "#FFEB3B"}}]});
        assert_eq!(color_choice_count(&removal), None);
        assert_eq!(color_choice_count(&serde_json::json!({"content": "note", "passages": []})), None);
    }

    #[tokio::test]
    async fn notes_cannot_receive_color_edits_and_rejection_discards_edits() {
        let (tx, rx) = oneshot::channel();
        registry().lock().unwrap().insert("note-no-colors".into(), PendingConfirmation { sender: tx, passage_count: None });
        assert!(resolve_with_colors("note-no-colors", true, Some(vec!["blue".into()])).is_err());
        resolve_with_colors("note-no-colors", false, Some(vec!["bad".into()])).unwrap();
        let choice = rx.await.unwrap();
        assert!(!choice.approved());
        assert_eq!(choice.passage_colors, None);
    }
}
