//! Watches the open library folder for changes made outside this app.
//!
//! The case this exists for: the library lives in iCloud Drive (or Dropbox, or a
//! synced network share) and the user edits it on another machine. The files here
//! change underneath a running app, which would otherwise keep showing whatever
//! it scanned at startup.
//!
//! Two things keep this from becoming a nuisance:
//!
//! * **Self-writes are filtered out.** The app writes constantly — reading state
//!   is persisted on every scroll — and re-scanning in response to itself would
//!   burn CPU forever. `fsutil` records what this process writes and those paths
//!   are skipped (see `fsutil::was_self_write`).
//! * **Events are debounced.** A sync client lands a batch of files as a burst;
//!   we wait for the burst to go quiet before telling the frontend, so one
//!   incoming sync produces one refresh.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

/// Quiet period before a burst of filesystem events is reported.
const DEBOUNCE: Duration = Duration::from_millis(1200);
/// How often the collector thread checks whether the burst has gone quiet.
const POLL: Duration = Duration::from_millis(300);
/// Ceiling on how long changes are withheld. A first sync of a large library can
/// trickle files for minutes without ever going quiet for a full DEBOUNCE, which
/// would leave the user staring at stale content the whole time. Report anyway
/// once we have been sitting on changes this long.
const MAX_HOLD: Duration = Duration::from_secs(10);

struct Pending {
    /// Slugs of papers touched by the change.
    slugs: HashSet<String>,
    /// A change outside any single paper folder (paper added/removed, collections,
    /// settings) — the frontend has to re-scan rather than patch one entry.
    other: bool,
    last_event: Instant,
    /// When the current batch started accumulating, for the MAX_HOLD ceiling.
    first_event: Instant,
}

impl Pending {
    fn is_empty(&self) -> bool {
        self.slugs.is_empty() && !self.other
    }

    fn record(&mut self) {
        let now = Instant::now();
        if self.is_empty() {
            self.first_event = now;
        }
        self.last_event = now;
    }

    /// Settled after a quiet spell, or held long enough that waiting further just
    /// keeps the user looking at stale data.
    fn should_report(&self, debounce: Duration, max_hold: Duration) -> bool {
        !self.is_empty() && (self.last_event.elapsed() >= debounce || self.first_event.elapsed() >= max_hold)
    }
}

struct Active {
    /// Dropping the watcher stops delivery; kept alive only for that.
    _watcher: RecommendedWatcher,
    /// Cleared when this generation is replaced so its thread exits.
    alive: Arc<AtomicBool>,
}

fn active() -> &'static Mutex<Option<Active>> {
    static ACTIVE: OnceLock<Mutex<Option<Active>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// Paths that are noise: our own bookkeeping, sync/OS scratch files, and imports
/// still in flight (those announce themselves via `library-updated` when done).
fn is_ignored(path: &Path, root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return true };
    for part in rel.components() {
        let Some(name) = part.as_os_str().to_str() else { return true };
        if name == ".argus" || name == ".git" || name.starts_with("importing_") {
            return true;
        }
        if name == ".DS_Store" || name.ends_with(".icloud") || name.starts_with(".~") {
            return true;
        }
        // Temp files from fsutil::atomic_write — the rename to the real path is
        // the event that matters.
        if name.contains(".tmp.") {
            return true;
        }
    }
    false
}

/// `<root>/papers/<slug>/…` → `slug`. Anything else → None (a structural change).
fn slug_of(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = rel.components();
    if parts.next()?.as_os_str() != "papers" {
        return None;
    }
    let slug = parts.next()?.as_os_str().to_str()?.to_string();
    // `papers/<slug>` alone (the folder itself) is a structural change, not an
    // edit inside one paper.
    parts.next()?;
    Some(slug)
}

/// Start watching `root`, replacing any previous watcher.
pub fn watch(app: &AppHandle, root: &str) {
    stop();
    let app = app.clone();
    let started = watch_with(root, DEBOUNCE, move |slugs, other| {
        let _ = app.emit(
            "library-files-changed",
            serde_json::json!({ "slugs": slugs, "other": other }),
        );
    });
    if let Ok(mut guard) = active().lock() {
        *guard = started;
    }
}

/// The watcher machinery, independent of Tauri so it can be exercised in tests.
/// `sink` receives one call per settled burst.
fn watch_with<F>(root: &str, debounce: Duration, sink: F) -> Option<Active>
where
    F: Fn(Vec<String>, bool) + Send + 'static,
{
    // Resolve symlinks up front. The OS reports events against real paths — on
    // macOS `/var/...` arrives as `/private/var/...`, and any symlink in a
    // user-chosen library path has the same effect — so an unresolved root makes
    // every `strip_prefix` below fail and the watcher silently reports nothing.
    let root_path = std::fs::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let pending = Arc::new(Mutex::new(Pending {
        slugs: HashSet::new(),
        other: false,
        last_event: Instant::now(),
        first_event: Instant::now(),
    }));
    let alive = Arc::new(AtomicBool::new(true));

    let cb_pending = pending.clone();
    let cb_root = root_path.clone();
    let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
        let Ok(event) = res else { return };
        if !matches!(
            event.kind,
            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
        ) {
            return;
        }
        for path in &event.paths {
            if is_ignored(path, &cb_root) || crate::fsutil::was_self_write(path) {
                continue;
            }
            let Ok(mut p) = cb_pending.lock() else { continue };
            p.record();
            match slug_of(path, &cb_root) {
                Some(slug) => { p.slugs.insert(slug); }
                None => p.other = true,
            }
        }
    });

    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[watcher] could not create watcher: {e}");
            return None;
        }
    };
    if let Err(e) = watcher.watch(&root_path, RecursiveMode::Recursive) {
        eprintln!("[watcher] could not watch {root}: {e}");
        return None;
    }

    // Collector: reports once a burst has been quiet for `debounce`.
    let thread_alive = alive.clone();
    let thread_pending = pending;
    std::thread::spawn(move || {
        while thread_alive.load(Ordering::SeqCst) {
            std::thread::sleep(POLL);
            let payload = {
                let Ok(mut p) = thread_pending.lock() else { continue };
                if !p.should_report(debounce, MAX_HOLD) {
                    continue;
                }
                let slugs: Vec<String> = p.slugs.drain().collect();
                let other = p.other;
                p.other = false;
                (slugs, other)
            };
            sink(payload.0, payload.1);
        }
    });

    Some(Active { _watcher: watcher, alive })
}

/// Stop watching (library switched or closed).
pub fn stop() {
    if let Ok(mut guard) = active().lock() {
        if let Some(prev) = guard.take() {
            prev.alive.store(false, Ordering::SeqCst);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/lib")
    }

    #[test]
    fn maps_paper_files_to_their_slug() {
        assert_eq!(
            slug_of(Path::new("/lib/papers/smith2020/meta.json"), &root()).as_deref(),
            Some("smith2020")
        );
        assert_eq!(
            slug_of(Path::new("/lib/papers/smith2020/notes/a.md"), &root()).as_deref(),
            Some("smith2020")
        );
    }

    #[test]
    fn structural_changes_have_no_slug() {
        // The paper folder itself appearing/disappearing is a list change.
        assert_eq!(slug_of(Path::new("/lib/papers/smith2020"), &root()), None);
        assert_eq!(slug_of(Path::new("/lib/collections.json"), &root()), None);
        assert_eq!(slug_of(Path::new("/lib/papers"), &root()), None);
    }

    #[test]
    fn ignores_own_bookkeeping_and_scratch_files() {
        for p in [
            "/lib/.argus/index.json",
            "/lib/papers/a/.DS_Store",
            "/lib/papers/a/meta.tmp.1a2b3c4d",
            "/lib/papers/a/.meta.json.icloud",
            "/lib/papers/importing_123/paper.pdf",
        ] {
            assert!(is_ignored(Path::new(p), &root()), "should ignore {p}");
        }
    }

    #[test]
    fn keeps_real_paper_edits() {
        for p in [
            "/lib/papers/a/meta.json",
            "/lib/papers/a/highlights.json",
            "/lib/collections.json",
        ] {
            assert!(!is_ignored(Path::new(p), &root()), "should keep {p}");
        }
    }

    #[test]
    fn paths_outside_the_root_are_ignored() {
        assert!(is_ignored(Path::new("/elsewhere/meta.json"), &root()));
    }

    // ── End-to-end: a real notify watcher over a real temp directory ─────────

    use std::sync::mpsc;

    struct TempLib(PathBuf);
    impl TempLib {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "argus-watch-{tag}-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir_all(dir.join("papers").join("smith2020")).unwrap();
            // Let the initial directory creation settle before watching.
            std::thread::sleep(Duration::from_millis(150));
            TempLib(dir)
        }
    }
    impl Drop for TempLib {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn collect(lib: &TempLib, act: impl FnOnce(&Path)) -> Option<(Vec<String>, bool)> {
        let (tx, rx) = mpsc::channel();
        let handle = watch_with(
            lib.0.to_str().unwrap(),
            Duration::from_millis(200),
            move |slugs, other| { let _ = tx.send((slugs, other)); },
        )
        .expect("watcher should start");
        std::thread::sleep(Duration::from_millis(200));

        act(&lib.0);

        let got = rx.recv_timeout(Duration::from_secs(5)).ok();
        handle.alive.store(false, Ordering::SeqCst);
        got
    }

    #[test]
    fn reports_an_external_edit_with_its_slug() {
        let lib = TempLib::new("ext");
        let got = collect(&lib, |root| {
            std::fs::write(root.join("papers/smith2020/meta.json"), r#"{"title":"x"}"#).unwrap();
        });
        let (slugs, _) = got.expect("an external write must be reported");
        assert_eq!(slugs, vec!["smith2020".to_string()]);
    }

    #[test]
    fn does_not_report_writes_this_process_made() {
        let lib = TempLib::new("self");
        let got = collect(&lib, |root| {
            // Exactly how the app writes: atomic_write records the self-write.
            crate::fsutil::atomic_write_str(
                &root.join("papers/smith2020/reading_state.json"),
                r#"{"page":3}"#,
            )
            .unwrap();
        });
        assert!(
            got.is_none(),
            "the app's own writes must not trigger a refresh — otherwise scrolling \
             a paper re-scans the library forever"
        );
    }

    #[test]
    fn collapses_a_burst_into_one_report() {
        let lib = TempLib::new("burst");
        let got = collect(&lib, |root| {
            for i in 0..10 {
                std::fs::write(
                    root.join(format!("papers/smith2020/f{i}.json")),
                    format!("{i}"),
                )
                .unwrap();
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let (slugs, _) = got.expect("burst must still be reported");
        assert_eq!(slugs, vec!["smith2020".to_string()], "10 writes → 1 slug, 1 report");
    }

    #[test]
    fn reports_anyway_when_a_sync_never_goes_quiet() {
        // A long trickle keeps resetting the debounce; MAX_HOLD is what stops the
        // user from staring at stale data for the whole sync.
        let mut p = Pending {
            slugs: HashSet::from(["a".to_string()]),
            other: false,
            last_event: Instant::now(),
            first_event: Instant::now() - Duration::from_secs(30),
        };
        assert!(
            !p.should_report(Duration::from_millis(1200), Duration::from_secs(60)),
            "still within both windows"
        );
        assert!(
            p.should_report(Duration::from_millis(1200), Duration::from_secs(10)),
            "held past MAX_HOLD → report even though events keep arriving"
        );
        p.slugs.clear();
        assert!(
            !p.should_report(Duration::from_millis(0), Duration::from_secs(0)),
            "nothing pending → never report"
        );
    }

    #[test]
    fn a_new_paper_folder_is_reported_as_structural() {
        let lib = TempLib::new("struct");
        let got = collect(&lib, |root| {
            std::fs::create_dir_all(root.join("papers/jones2021")).unwrap();
        });
        let (_, other) = got.expect("a new paper folder must be reported");
        assert!(other, "adding a paper folder is a list change, not an edit inside one paper");
    }
}
