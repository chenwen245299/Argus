// Backend cancellation registry for in-flight streaming AI requests.
//
// The frontend generates a `request_id` (string) when it starts a streaming
// invoke and passes it to the backend. The streaming command registers an
// `Arc<AtomicBool>` flag under that id. When the user presses "stop", the
// frontend calls the `cancel_ai_request` command, which flips the flag to
// `true`. Streaming reads and response-header waits check the flag at least
// every 50 ms, dropping the reqwest response even when no chunks arrive
// (closing the connection so the provider stops generating).
//
// IMPORTANT: never hold the registry mutex across an `await`. Callers clone the
// `Arc<AtomicBool>` out of the map and then only `.load()` it inside the loop.

use std::collections::HashMap;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

fn registry() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a fresh cancel flag (initial value `false`) for `request_id` and
/// return it. If the same id is registered again, the previous flag is replaced.
pub fn register(request_id: &str) -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    if let Ok(mut map) = registry().lock() {
        map.insert(request_id.to_string(), flag.clone());
    }
    flag
}

/// Remove the flag for `request_id` from the registry (call on completion/error).
pub fn unregister(request_id: &str) {
    if let Ok(mut map) = registry().lock() {
        map.remove(request_id);
    }
}

/// Flip the cancel flag for `request_id` to `true` (if present).
pub fn cancel(request_id: &str) {
    if let Ok(map) = registry().lock() {
        if let Some(flag) = map.get(request_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

/// Drop a pending network operation promptly even when the provider stops
/// sending SSE chunks (or has not sent response headers yet). Ending chunk
/// reads with None lets each streaming adapter still account for partial usage.
pub async fn until_cancelled<F: Future>(
    flag: Option<&Arc<AtomicBool>>,
    work: F,
) -> Option<F::Output> {
    let Some(flag) = flag else {
        return Some(work.await);
    };
    if flag.load(Ordering::SeqCst) {
        return None;
    }
    tokio::select! {
        biased;
        _ = async {
            while !flag.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        } => None,
        result = work => Some(result),
    }
}

/// RAII guard that unregisters a request_id on drop, so the registry is cleaned
/// up on every exit path (normal return, `?` early-return, panic).
pub struct CancelGuard {
    request_id: Option<String>,
}

impl CancelGuard {
    /// If `request_id` is `Some`, register a flag and return `(guard, Some(flag))`.
    /// If `None`, no registration happens and the flag is `None` (cancellation
    /// is simply unavailable for that call).
    pub fn new(request_id: Option<String>) -> (Self, Option<Arc<AtomicBool>>) {
        match request_id {
            Some(id) => {
                let flag = register(&id);
                (
                    CancelGuard {
                        request_id: Some(id),
                    },
                    Some(flag),
                )
            }
            None => (CancelGuard { request_id: None }, None),
        }
    }
}

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if let Some(id) = &self.request_id {
            unregister(id);
        }
    }
}

/// Tauri command: mark the given request as cancelled.
#[tauri::command]
pub fn cancel_ai_request(request_id: String) {
    cancel(&request_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_drops_a_stalled_operation_without_waiting_for_a_chunk() {
        struct Dropped(Arc<AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let flag = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = Dropped(dropped.clone());
        let cancelling = flag.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            cancelling.store(true, Ordering::SeqCst);
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            until_cancelled(Some(&flag), async {
                let _guard = guard;
                std::future::pending::<()>().await;
            }),
        )
        .await
        .unwrap();
        assert!(result.is_none());
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_before_start_does_not_poll_the_operation() {
        let flag = Arc::new(AtomicBool::new(true));
        assert_eq!(
            until_cancelled(Some(&flag), async { panic!("must not start") }).await,
            None::<()>
        );
    }

    #[tokio::test]
    async fn completed_work_and_requests_without_cancellation_keep_their_results() {
        let flag = Arc::new(AtomicBool::new(false));
        assert_eq!(until_cancelled(Some(&flag), async { 42 }).await, Some(42));
        assert_eq!(until_cancelled(None, async { 7 }).await, Some(7));
    }
}
