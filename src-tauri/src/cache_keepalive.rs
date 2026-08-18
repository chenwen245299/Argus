//! Keeping an agent conversation's prompt cache alive between questions.
//!
//! # Why
//!
//! Agent mode sends a large prefix — a system prompt, seventeen tool schemas,
//! and the whole conversation so far — on every question. Providers with
//! automatic prefix caching bill a repeat of that prefix at roughly a tenth of
//! the normal rate, but only while the entry is still warm: DeepSeek drops it
//! after about ten minutes idle. Sit and think for a quarter of an hour, and the
//! next question pays full price to re-read everything.
//!
//! So after each agent answer this re-sends the same prefix every five minutes
//! with `max_tokens: 1`. The ping is itself a cache hit, so it costs about a
//! tenth of what it saves.
//!
//! # When it stops
//!
//! Three independent conditions, because a background loop that spends money
//! must not be able to outlive its reason to exist:
//!
//! - the window that started it is gone (checked before every ping, so it holds
//!   even if the front-end never gets to say goodbye),
//! - an hour has passed with no new question,
//! - two pings in a row failed — a revoked key should not be retried twelve
//!   times an hour.
//!
//! # Scope
//!
//! One conversation at a time: the most recent agent answer replaces whatever
//! was being kept warm. Keeping every open conversation warm would multiply the
//! cost by however many the user happens to have, to save on the one they
//! probably will not return to.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::{Emitter, Manager};

use crate::models::AiProvider;

/// Comfortably inside DeepSeek's ~10 minute idle expiry, without pinging so
/// often that the pings themselves become the cost.
const PING_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// After this long without a new question, assume the user has moved on.
const IDLE_LIMIT: Duration = Duration::from_secs(60 * 60);

/// Consecutive failures tolerated before giving up.
const MAX_FAILURES: u32 = 2;


/// Where the chat window hears about all of this. Background spending the user
/// cannot see is background spending they cannot judge.
const STATUS_EVENT: &str = "cache-keepalive";

/// Why a keepalive stopped, for the status the chat window shows.
#[derive(Debug, Clone, Copy)]
enum Stopped {
    /// An hour passed with no new question.
    Idle,
    /// The chat window is gone.
    Left,
    /// The provider kept rejecting the pings.
    Failing,
}

impl Stopped {
    fn as_str(self) -> &'static str {
        match self {
            Stopped::Idle => "idle",
            Stopped::Left => "left",
            Stopped::Failing => "failing",
        }
    }
}

fn emit_stopped(app: &tauri::AppHandle, reason: Stopped, conversation_id: Option<&str>) {
    let _ = app.emit(
        STATUS_EVENT,
        serde_json::json!({
            "active": false,
            "reason": reason.as_str(),
            "conversationId": conversation_id,
        }),
    );
}

/// Everything needed to reproduce the prefix that must stay warm.
pub struct Warm {
    /// Which conversation this belongs to, opaque to everything here and echoed
    /// back in the status. The cache is one conversation's prefix, so the badge
    /// belongs on that conversation and nowhere else.
    pub conversation_id: Option<String>,
    pub provider: AiProvider,
    pub api_key: String,
    pub model: String,
    /// The conversation as the *next* question will send it: system prompt,
    /// prior turns, and the answer just given. Deliberately not the agent
    /// loop's internal transcript — that carries `tool` messages the next
    /// request will not repeat, so warming it would warm the wrong prefix.
    pub messages: Vec<serde_json::Value>,
    /// The exact tool array the last turn sent, verbatim.
    pub tools: Vec<serde_json::Value>,
    /// Label of the window that asked the question. Its disappearance is what
    /// "the user left" means — and it is not always the chat window: the paper
    /// AI panel and the canvas chat run agent mode inside the main window, and
    /// the paper AI can also be popped out into a window of its own.
    pub owner_window: String,
}

/// Cancellation flag of the running loop, if any.
fn current() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static CURRENT: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    CURRENT.get_or_init(|| Mutex::new(None))
}

/// Stop keeping anything warm. Safe to call when nothing is running.
pub fn disarm() {
    if let Ok(mut slot) = current().lock() {
        if let Some(flag) = slot.take() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Stop, and tell the chat window it stopped.
///
/// Separate from `disarm` because `arm` disarms its predecessor as a matter of
/// bookkeeping — announcing "stopped" there would blink the indicator off and
/// on again between every question.
pub fn disarm_and_announce(app: &tauri::AppHandle) {
    disarm();
    // No conversation id: this stops whatever was warm, and the window's badge
    // should clear regardless of which conversation it was showing for.
    let _ = app.emit(
        STATUS_EVENT,
        serde_json::json!({ "active": false, "reason": "disarmed" }),
    );
}

/// Start (or replace) the loop keeping `warm`'s prefix cached.
pub fn arm(app: &tauri::AppHandle, warm: Warm) {
    disarm();

    let cancel = Arc::new(AtomicBool::new(false));
    if let Ok(mut slot) = current().lock() {
        *slot = Some(cancel.clone());
    }

    // Epoch millis so the window can show how long it has left without needing
    // its own clock to agree with this one.
    let stops_at_ms = chrono::Utc::now().timestamp_millis() + IDLE_LIMIT.as_millis() as i64;
    let model_label = warm.model.clone();
    let conversation_id = warm.conversation_id.clone();
    let _ = app.emit(
        STATUS_EVENT,
        serde_json::json!({
            "active": true,
            "conversationId": conversation_id,
            "model": model_label,
            "pings": 0,
            "stopsAtMs": stops_at_ms,
            "intervalSeconds": PING_INTERVAL.as_secs(),
        }),
    );

    let app = app.clone();
    tokio::spawn(async move {
        let armed_at = Instant::now();
        let mut failures = 0u32;
        let mut pings = 0u32;

        let reason = loop {
            tokio::time::sleep(PING_INTERVAL).await;

            if cancel.load(Ordering::Relaxed) {
                // A replacement is already running (or the user disarmed and
                // that path emitted its own status); say nothing.
                return;
            }
            if armed_at.elapsed() >= IDLE_LIMIT {
                break Stopped::Idle;
            }
            // The authoritative check. The front-end also disarms explicitly,
            // but a window torn down without running its cleanup must not leave
            // this spending for the next hour.
            if app.get_webview_window(&warm.owner_window).is_none() {
                break Stopped::Left;
            }

            match crate::llm::touch_prompt_cache(
                &warm.provider,
                &warm.api_key,
                &warm.model,
                &warm.messages,
                &warm.tools,
            )
            .await
            {
                Ok(hits) => {
                    failures = 0;
                    pings += 1;
                    let _ = app.emit(
                        STATUS_EVENT,
                        serde_json::json!({
                            "active": true,
                            "conversationId": conversation_id,
                            "model": model_label,
                            "pings": pings,
                            "stopsAtMs": stops_at_ms,
                            "intervalSeconds": PING_INTERVAL.as_secs(),
                            "lastHitTokens": hits,
                        }),
                    );
                }
                Err(e) => {
                    failures += 1;
                    eprintln!("[cache-keepalive] {e}");
                    if failures >= MAX_FAILURES {
                        break Stopped::Failing;
                    }
                }
            }
        };
        emit_stopped(&app, reason, conversation_id.as_deref());

        // Only clear the slot if it is still ours: a later `arm` may already
        // have replaced it, and clearing then would orphan the live loop.
        if let Ok(mut slot) = current().lock() {
            if slot.as_ref().is_some_and(|f| Arc::ptr_eq(f, &cancel)) {
                *slot = None;
            }
        }
    });
}

/// Whether pinging this provider is worth the user's money.
///
/// DeepSeek documents automatic context caching, so it qualifies from the first
/// turn — before any hit has been observed, since nothing was cached to hit.
/// Every other provider has to prove it: a turn that reported cache-hit tokens
/// is evidence the caching is real and the ping will land as a hit rather than
/// a full-price re-read.
pub fn is_worthwhile(provider: &AiProvider, observed_cache_hits: u64) -> bool {
    crate::llm::is_deepseek(provider) || observed_cache_hits > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(base_url: &str) -> AiProvider {
        AiProvider {
            id: "p".into(),
            name: "P".into(),
            kind: "openai_compatible".into(),
            base_url: base_url.into(),
            models: vec![],
            enabled: true,
            created_at: String::new(),
        }
    }

    /// The ping only pays for itself when it lands as a cache hit. Against a
    /// provider with no prefix cache it would re-read the whole conversation at
    /// full price, every five minutes, to save nothing.
    #[test]
    fn an_unproven_provider_is_not_pinged() {
        assert!(
            !is_worthwhile(&provider("https://api.example.com/v1"), 0),
            "a provider that has never reported a cache hit must not be pinged"
        );
        assert!(
            is_worthwhile(&provider("https://api.example.com/v1"), 4096),
            "an observed hit is proof the caching is real"
        );
    }

    /// DeepSeek caches from the first turn, when there is no hit to observe yet
    /// — waiting for evidence would mean never warming the first conversation.
    #[test]
    fn deepseek_qualifies_before_any_hit_is_seen() {
        assert!(is_worthwhile(&provider("https://api.deepseek.com/v1"), 0));
    }

    #[test]
    fn disarming_nothing_is_harmless() {
        disarm();
        disarm();
        assert!(current().lock().unwrap().is_none());
    }

    /// The interval has to sit inside the shortest expiry it is defending
    /// against, or the cache is already gone by the time the ping arrives.
    #[test]
    fn the_interval_beats_the_expiry_it_defends_against() {
        const DEEPSEEK_IDLE_EXPIRY: Duration = Duration::from_secs(10 * 60);
        assert!(
            PING_INTERVAL < DEEPSEEK_IDLE_EXPIRY,
            "a ping every {:?} cannot hold a cache that expires after {:?}",
            PING_INTERVAL,
            DEEPSEEK_IDLE_EXPIRY
        );
        assert!(
            IDLE_LIMIT > PING_INTERVAL,
            "the loop must get at least one ping in before it gives up"
        );
    }
}
