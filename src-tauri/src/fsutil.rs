use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Paths this process wrote recently, so the library file watcher can tell its
/// own writes apart from ones synced in from another machine. Without this the
/// watcher would fire continuously while reading a paper — scrolling persists
/// reading state on every move — and re-scan the library for nothing.
fn recent_self_writes() -> &'static Mutex<HashMap<PathBuf, Instant>> {
    static WRITES: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    WRITES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How long a path stays attributed to us. Generous enough to cover the delay
/// between the write and the OS delivering the event, short enough that a real
/// remote change to the same file right after ours is not swallowed for long.
const SELF_WRITE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// Key paths by their real location. The OS reports watch events against
/// resolved paths (`/var/…` arrives as `/private/var/…` on macOS, and any
/// symlink in the library path does the same), so recording the unresolved path
/// here would never match and every self-write would look external.
///
/// Canonicalizes the parent rather than the file: writers call this before the
/// file exists at its final name.
fn canonical_key(path: &Path) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) => std::fs::canonicalize(dir)
            .map(|d| d.join(name))
            .unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    }
}

pub fn note_self_write(path: &Path) {
    if let Ok(mut map) = recent_self_writes().lock() {
        let now = Instant::now();
        map.retain(|_, at| now.duration_since(*at) < SELF_WRITE_TTL);
        map.insert(canonical_key(path), now);
    }
}

/// True when this process wrote `path` within the TTL.
pub fn was_self_write(path: &Path) -> bool {
    let Ok(map) = recent_self_writes().lock() else { return false };
    map.get(&canonical_key(path))
        .is_some_and(|at| Instant::now().duration_since(*at) < SELF_WRITE_TTL)
}

/// Write `content` to `path` atomically: write to a uniquely-named temp file in
/// the same directory, then rename it into place. A same-directory rename is
/// atomic on POSIX and Windows, so a crash or power loss mid-write can never
/// leave a truncated, unparseable file at `path`.
///
/// The temp file carries a random suffix so two concurrent writers to the same
/// path don't clobber each other's temp file before the rename.
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<(), String> {
    let suffix: String = uuid::Uuid::new_v4()
        .to_string()
        .replace('-', "")
        .chars()
        .take(8)
        .collect();
    let tmp = path.with_extension(format!("tmp.{suffix}"));

    if let Err(e) = std::fs::write(&tmp, content) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Write temp file {}: {e}", tmp.display()));
    }
    // Record before the rename: the watcher can observe the new file the instant
    // the rename lands, so the attribution must already be in place.
    note_self_write(path);
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Rename temp file to {}: {e}", path.display()));
    }
    Ok(())
}

/// Convenience wrapper for writing string content atomically.
pub fn atomic_write_str(path: &Path, content: &str) -> Result<(), String> {
    atomic_write(path, content.as_bytes())
}
