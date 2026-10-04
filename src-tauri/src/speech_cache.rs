//! Read-aloud audio kept in the paper's own folder, `papers/<slug>/audio/`, so that
//! reading the same passage again with the same voice plays from disk instead of
//! paying the speech provider a second time — after a restart too, and on another
//! machine the library syncs to.
//!
//! One file per chunk (one speech request), named `<key>.<ext>`. The key is the
//! frontend's `speechCacheKey(voice scope, chunk text)` (`src/utils/speechEngine.ts`):
//! a hash of the provider, the model, every option and the exact text sent. A file
//! is found by its name alone, and another voice, speed or wording is simply another
//! file; nothing else is recorded. Deleting the folder, or one file, only means
//! those chunks are synthesised — and billed — again when next read.
//!
//! Paths: the slug goes through `paper::find_paper_dir` (path guard, and a folder is
//! never created for a paper that does not exist), the key must look exactly like
//! one `speechCacheKey` makes, and nothing is read or written through a symlink.

use std::path::{Path, PathBuf};

/// The folder inside a paper's folder.
pub const AUDIO_DIR: &str = "audio";

/// Largest clip kept or served back. A chunk is at most ~900 characters, about a
/// minute of speech: a few hundred kB as MP3, a few MB as WAV.
const MAX_BYTES: u64 = 32 * 1024 * 1024;

/// The containers a speech adapter returns (and WebKit plays): extension, and the
/// media type a saved file is served back as.
const FORMATS: &[(&str, &str)] = &[
    ("mp3", "audio/mpeg"),
    ("wav", "audio/wav"),
    ("flac", "audio/flac"),
    ("ogg", "audio/ogg"),
    ("m4a", "audio/mp4"),
    ("aac", "audio/aac"),
];

fn ext_for_mime(mime: &str) -> Option<&'static str> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "audio/mpeg" | "audio/mp3" | "audio/mpeg3" => Some("mp3"),
        "audio/wav" | "audio/x-wav" | "audio/wave" | "audio/vnd.wave" => Some("wav"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/ogg" | "audio/opus" => Some("ogg"),
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => Some("m4a"),
        "audio/aac" => Some("aac"),
        _ => None,
    }
}

/// A key as `speechCacheKey` makes it — base36 hash digits, a dot, the length in
/// digits — and nothing else, so a key can never be a path.
fn validate_key(key: &str) -> Result<(), String> {
    let ok = key.split_once('.').is_some_and(|(hash, len)| {
        (1..=32).contains(&hash.len())
            && hash.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
            && (1..=10).contains(&len.len())
            && len.bytes().all(|b| b.is_ascii_digit())
    });
    if ok {
        Ok(())
    } else {
        Err("Invalid read-aloud audio key".to_string())
    }
}

fn audio_dir(root: &str, slug: &str) -> Option<PathBuf> {
    crate::paper::find_paper_dir(root, slug).map(|d| d.join(AUDIO_DIR))
}

/// The size of a regular file at `path`; None for a symlink, a folder or nothing.
fn regular_file_len(path: &Path) -> Option<u64> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    (meta.is_file() && !meta.file_type().is_symlink()).then_some(meta.len())
}

/// The clip saved for `key` and its media type, or None when this paper has none.
pub fn read(root: &str, slug: &str, key: &str) -> Result<Option<(Vec<u8>, &'static str)>, String> {
    validate_key(key)?;
    let Some(dir) = audio_dir(root, slug) else { return Ok(None) };
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        _ => return Ok(None),
    }
    for (ext, mime) in FORMATS {
        let path = dir.join(format!("{key}.{ext}"));
        let Some(len) = regular_file_len(&path) else { continue };
        if len == 0 || len > MAX_BYTES {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("Failed to read saved audio: {e}"))?;
        return Ok(Some((bytes, mime)));
    }
    Ok(None)
}

/// Keep a synthesised clip under `key`. Written atomically, so a crash never leaves
/// half a file that would later be served as the whole chunk.
pub fn write(root: &str, slug: &str, key: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    validate_key(key)?;
    let ext = ext_for_mime(mime).ok_or_else(|| format!("Not an audio type kept for read-aloud: {mime}"))?;
    if bytes.is_empty() {
        return Err("Audio is empty".to_string());
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err("Audio is too large to keep".to_string());
    }
    let dir = audio_dir(root, slug).ok_or_else(|| format!("Paper not found: {slug}"))?;
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err("The paper's audio path is not a folder".to_string());
            }
        }
        Err(_) => {
            // Our own write: the library watcher must not take it for an edit made elsewhere.
            crate::fsutil::note_self_write(&dir);
            std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create the audio folder: {e}"))?;
        }
    }
    crate::fsutil::atomic_write(&dir.join(format!("{key}.{ext}")), bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library with one paper, `p1`.
    fn library() -> String {
        let root = std::env::temp_dir().join(format!("argus-speech-cache-{}", uuid::Uuid::new_v4()));
        let paper = root.join("papers").join("p1");
        std::fs::create_dir_all(&paper).unwrap();
        std::fs::write(paper.join("meta.json"), "{}").unwrap();
        root.to_string_lossy().into_owned()
    }

    const KEY: &str = "3k2j9x0q1m4n5b6v7c8.215";

    #[test]
    fn a_saved_clip_is_read_back_from_the_papers_audio_folder() {
        let root = library();
        assert_eq!(read(&root, "p1", KEY).unwrap(), None);
        write(&root, "p1", KEY, "audio/mpeg", b"ID3 fake mp3").unwrap();
        let file = Path::new(&root).join("papers/p1/audio").join(format!("{KEY}.mp3"));
        assert!(file.is_file(), "{}", file.display());
        assert_eq!(read(&root, "p1", KEY).unwrap(), Some((b"ID3 fake mp3".to_vec(), "audio/mpeg")));
        // Another type is another extension, served back with its own type.
        let other = "abc.12";
        write(&root, "p1", other, "audio/x-wav", b"RIFF").unwrap();
        assert_eq!(read(&root, "p1", other).unwrap(), Some((b"RIFF".to_vec(), "audio/wav")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keys_that_could_be_paths_are_refused() {
        let root = library();
        for key in ["", ".", "..", "../x.1", "a/b.1", "a\\b.1", ".1", "a.", "A.1", "a.b", "a.1.2", "a b.1", "a.1/..", "üb.1"] {
            assert!(validate_key(key).is_err(), "{key:?}");
            assert!(write(&root, "p1", key, "audio/mpeg", b"x").is_err(), "{key:?}");
            assert!(read(&root, "p1", key).is_err(), "{key:?}");
        }
        for key in [KEY, "0.1", "zz.9999999999"] {
            assert!(validate_key(key).is_ok(), "{key:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_is_made_for_a_paper_that_does_not_exist() {
        let root = library();
        assert_eq!(read(&root, "nope", KEY).unwrap(), None);
        assert!(write(&root, "nope", KEY, "audio/mpeg", b"x").is_err());
        assert!(write(&root, "../p1", KEY, "audio/mpeg", b"x").is_err());
        assert!(!Path::new(&root).join("papers/nope").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn only_non_empty_audio_is_kept() {
        let root = library();
        assert!(write(&root, "p1", KEY, "text/html", b"<script>").is_err());
        assert!(write(&root, "p1", KEY, "application/octet-stream", b"x").is_err());
        assert!(write(&root, "p1", KEY, "audio/mpeg", b"").is_err());
        assert!(!Path::new(&root).join("papers/p1/audio").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_audio_folder_is_neither_read_nor_written() {
        let root = library();
        let elsewhere = Path::new(&root).join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join(format!("{KEY}.mp3")), b"planted").unwrap();
        std::os::unix::fs::symlink(&elsewhere, Path::new(&root).join("papers/p1/audio")).unwrap();
        assert_eq!(read(&root, "p1", KEY).unwrap(), None);
        assert!(write(&root, "p1", "b.1", "audio/mpeg", b"x").is_err());
        assert!(!elsewhere.join("b.1.mp3").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
