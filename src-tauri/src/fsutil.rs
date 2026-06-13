use std::path::Path;

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
