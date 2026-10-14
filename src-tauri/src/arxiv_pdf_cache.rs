//! Downloaded recommendation PDFs, retained for seven days from the successful
//! download (reads never refresh that timestamp). The app cache is independent
//! of library folders, so cleanup works across library switches and cannot
//! delete an imported paper. A lease protects a PDF during reading/copying.

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::models::ArxivPaper;

pub const RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);
const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Record {
    library_root: String,
    arxiv_id: String,
    source: Option<String>,
    pdf_url: String,
    updated: String,
    downloaded_at: u64,
    bytes: u64,
    #[serde(default)]
    imported: bool,
}

impl Record {
    fn matches(&self, root: &str, paper: &ArxivPaper) -> bool {
        self.library_root == root
            && self.arxiv_id == paper.arxiv_id
            && self.source.as_deref().unwrap_or("arxiv")
                == paper.source.as_deref().unwrap_or("arxiv")
            && self.updated == paper.updated
            && crate::arxiv::recommendation_pdf_url(paper).is_ok_and(|url| url == self.pdf_url)
    }
    fn expired(&self, now: u64) -> bool {
        now.saturating_sub(self.downloaded_at) >= RETENTION_SECS
    }
}

#[derive(Debug)]
pub struct CachedPdf {
    path: PathBuf,
}
impl CachedPdf {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
pub type PdfLease = Arc<CachedPdf>;

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}
fn disk_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn leases() -> &'static Mutex<HashMap<PathBuf, Weak<CachedPdf>>> {
    static LEASES: OnceLock<Mutex<HashMap<PathBuf, Weak<CachedPdf>>>> = OnceLock::new();
    LEASES.get_or_init(|| Mutex::new(HashMap::new()))
}
fn lease(path: PathBuf) -> PdfLease {
    let mut active = locked(leases());
    active.retain(|_, p| p.strong_count() > 0);
    if let Some(existing) = active.get(&path).and_then(Weak::upgrade) {
        return existing;
    }
    let pdf = Arc::new(CachedPdf { path: path.clone() });
    active.insert(path, Arc::downgrade(&pdf));
    pdf
}
fn is_leased(path: &Path) -> bool {
    locked(leases())
        .get(path)
        .is_some_and(|p| p.strong_count() > 0)
}
fn download_lock(key: String) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let mut locks = locked(LOCKS.get_or_init(|| Mutex::new(HashMap::new())));
    locks.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(tokio::sync::Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    lock
}

pub fn directory(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_cache_dir()
        .map(|dir| dir.join("arxiv-pdfs"))
        .map_err(|e| format!("Recommendation cache directory: {e}"))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Only generated UUID filenames belong to this cache. No ID or path stored in
/// a sidecar is ever used as a filesystem path, including during deletion.
fn owned_stem(path: &Path) -> bool {
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|s| uuid::Uuid::parse_str(s).is_ok_and(|id| id.to_string() == s))
}
fn owned_temp(path: &Path) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|name| {
            let Some((id, suffix)) = name.split_once(".tmp.") else {
                return false;
            };
            suffix.len() == 8
                && suffix.bytes().all(|c| c.is_ascii_hexdigit())
                && uuid::Uuid::parse_str(id).is_ok_and(|u| u.to_string() == id)
        })
}
fn read_record(path: &Path) -> Option<Record> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 16 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}
fn valid_pdf(path: &Path, bytes: u64) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() != bytes || !(5..=MAX_PDF_BYTES).contains(&bytes) {
        return false;
    }
    let mut prefix = [0; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut prefix))
        .is_ok()
        && prefix == *b"%PDF"
}
fn remove_pair(sidecar: &Path) -> Result<(), String> {
    let pdf = sidecar.with_extension("pdf");
    if is_leased(&pdf) {
        return Ok(());
    }
    match fs::remove_file(&pdf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Remove cached PDF: {e}")),
    }
    match fs::remove_file(sidecar) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Remove cached PDF record: {e}")),
    }
}

/// Caller holds disk_lock. Orphans/broken sidecars expire by file age, so a
/// crash between the two atomic writes cannot leave downloads forever.
fn cleanup_locked(dir: &Path, at: u64) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let mut first_error = None;
    for entry in fs::read_dir(dir).map_err(|e| format!("Read recommendation cache: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let temporary = owned_temp(&path);
        if (!owned_stem(&path) && !temporary)
            || !entry.file_type().map_err(|e| e.to_string())?.is_file()
        {
            continue;
        }
        if temporary {
            if file_expired(&path, at) {
                if let Err(e) = fs::remove_file(&path) {
                    first_error.get_or_insert(e.to_string());
                }
            }
            continue;
        }
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if extension == "json" {
            let expired = read_record(&path).map_or_else(
                || file_expired(&path, at) || file_expired(&path.with_extension("pdf"), at),
                |r| r.imported || r.expired(at),
            );
            if expired {
                if let Err(e) = remove_pair(&path) {
                    first_error.get_or_insert(e);
                }
            }
        } else if extension == "pdf"
            && !path.with_extension("json").exists()
            && file_expired(&path, at)
            && !is_leased(&path)
        {
            if let Err(e) = fs::remove_file(&path) {
                first_error.get_or_insert(e.to_string());
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
fn file_expired(path: &Path, at: u64) -> bool {
    fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .is_some_and(|t| at.saturating_sub(t.as_secs()) >= RETENTION_SECS)
}
pub fn cleanup(dir: &Path) -> Result<(), String> {
    let _guard = locked(disk_lock());
    cleanup_locked(dir, now())
}

fn lookup_at(
    dir: &Path,
    root: &str,
    paper: &ArxivPaper,
    at: u64,
) -> Result<Option<PdfLease>, String> {
    let _guard = locked(disk_lock());
    // A busy/locked expired PDF must not prevent reading unrelated papers.
    if let Err(e) = cleanup_locked(dir, at) {
        eprintln!("[arxiv-pdf-cache] {e}");
    }
    if !dir.exists() {
        return Ok(None);
    }
    let mut newest: Option<(u64, PathBuf)> = None;
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if !owned_stem(&path) || path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = read_record(&path) else {
            continue;
        };
        if record.imported || record.expired(at) || !record.matches(root, paper) {
            continue;
        }
        let pdf = path.with_extension("pdf");
        if valid_pdf(&pdf, record.bytes)
            && newest
                .as_ref()
                .map_or(true, |(t, _)| record.downloaded_at > *t)
        {
            newest = Some((record.downloaded_at, pdf));
        }
    }
    Ok(newest.map(|(_, path)| lease(path)))
}

fn store_at(
    dir: &Path,
    root: &str,
    paper: &ArxivPaper,
    bytes: &[u8],
    at: u64,
) -> Result<PdfLease, String> {
    if bytes.len() < 5 || bytes.len() as u64 > MAX_PDF_BYTES || !bytes.starts_with(b"%PDF") {
        return Err("Invalid or oversized recommendation PDF.".into());
    }
    let record = Record {
        library_root: root.into(),
        arxiv_id: paper.arxiv_id.clone(),
        source: paper.source.clone(),
        pdf_url: crate::arxiv::recommendation_pdf_url(paper)?,
        updated: paper.updated.clone(),
        downloaded_at: at,
        bytes: bytes.len() as u64,
        imported: false,
    };
    let _guard = locked(disk_lock());
    fs::create_dir_all(dir).map_err(|e| format!("Create recommendation cache: {e}"))?;
    let pdf = dir.join(format!("{}.pdf", uuid::Uuid::new_v4()));
    crate::fsutil::atomic_write(&pdf, bytes)?;
    if let Err(e) = crate::fsutil::atomic_write(
        &pdf.with_extension("json"),
        &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
    ) {
        let _ = fs::remove_file(&pdf);
        return Err(e);
    }
    Ok(lease(pdf))
}

#[cfg(test)]
pub(crate) fn seed_test_pdf(dir: &Path, root: &str, paper: &ArxivPaper, bytes: &[u8]) -> PdfLease {
    store_at(dir, root, paper, bytes, now()).unwrap()
}

/// Shared by the PDF tool and both normal/agent recommendation imports.
pub async fn acquire(dir: &Path, root: &str, paper: &ArxivPaper) -> Result<PdfLease, String> {
    acquire_with(dir, root, paper, || {
        crate::arxiv::download_recommendation_pdf(paper)
    })
    .await
}
async fn acquire_with<F, Fut>(
    dir: &Path,
    root: &str,
    paper: &ArxivPaper,
    download: F,
) -> Result<PdfLease, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, String>>,
{
    let key = serde_json::to_string(&(
        dir,
        root,
        &paper.arxiv_id,
        paper.source.as_deref().unwrap_or("arxiv"),
        &paper.updated,
        crate::arxiv::recommendation_pdf_url(paper)?,
    ))
    .map_err(|e| e.to_string())?;
    let lock = download_lock(key);
    let download_guard = lock.lock_owned().await;
    let d = dir.to_path_buf();
    let r = root.to_string();
    let p = paper.clone();
    if let Some(pdf) = tokio::task::spawn_blocking(move || lookup_at(&d, &r, &p, now()))
        .await
        .map_err(|e| e.to_string())??
    {
        return Ok(pdf);
    }
    let bytes = download().await?;
    let downloaded_at = now();
    let d = dir.to_path_buf();
    let r = root.to_string();
    let p = paper.clone();
    tokio::task::spawn_blocking(move || {
        // Stop can drop the awaiting future, but a started persistence task
        // retains this lock until its PDF/record pair has committed.
        let _download_guard = download_guard;
        store_at(&d, &r, &p, &bytes, downloaded_at)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Mark a cache copy disposable after the independent library PDF has committed.
/// Active readers keep their lease; cleanup removes the copy after they finish.
pub fn discard_imported(dir: &Path, root: &str, paper: &ArxivPaper) -> Result<(), String> {
    let _guard = locked(disk_lock());
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if !owned_stem(&path) || path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(mut record) = read_record(&path) else {
            continue;
        };
        if record.matches(root, paper) {
            record.imported = true;
            crate::fsutil::atomic_write(
                &path,
                &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
            )?;
            remove_pair(&path)?;
        }
    }
    Ok(())
}

pub fn spawn(app: &tauri::AppHandle) {
    let Ok(dir) = directory(app) else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(CLEANUP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // First tick is immediate: expired files from time spent with the
            // app closed are removed at the next launch, even with no library.
            interval.tick().await;
            let dir = dir.clone();
            match tokio::task::spawn_blocking(move || cleanup(&dir)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => eprintln!("[arxiv-pdf-cache] {e}"),
                Err(e) => eprintln!("[arxiv-pdf-cache] cleanup task: {e}"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const PDF: &[u8] = b"%PDF-1.4\ncached test bytes\n";
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("argus-pdf-cache-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn cache(&self) -> PathBuf {
            self.0.join("cache")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn paper() -> ArxivPaper {
        serde_json::from_value(serde_json::json!({"arxiv_id": "2610.12345", "updated": "2026-10-14", "pdf_url": "https://arxiv.org/pdf/2610.12345"})).unwrap()
    }
    fn clean_at(dir: &Path, at: u64) {
        let _guard = locked(disk_lock());
        cleanup_locked(dir, at).unwrap();
    }
    fn age_file(path: &Path, at: u64) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(at)))
            .unwrap();
    }

    #[test]
    fn seven_days_are_measured_from_download_and_reads_do_not_extend_expiry() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let start = now();
        let pdf = store_at(&dir, "library-a", &p, PDF, start).unwrap();
        let path = pdf.path().to_path_buf();
        drop(pdf);
        let reread = lookup_at(&dir, "library-a", &p, start + RETENTION_SECS - 1)
            .unwrap()
            .unwrap();
        assert_eq!(reread.path(), path);
        assert_eq!(
            read_record(&path.with_extension("json"))
                .unwrap()
                .downloaded_at,
            start
        );
        drop(reread);
        clean_at(&dir, start + RETENTION_SECS - 1);
        assert!(path.exists());
        clean_at(&dir, start + RETENTION_SECS);
        assert!(!path.exists());
        assert!(!path.with_extension("json").exists());
        assert!(lookup_at(&dir, "library-a", &p, start + RETENTION_SECS)
            .unwrap()
            .is_none());
    }

    #[test]
    fn active_reads_are_protected_but_expired_files_are_never_reused() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let start = now();
        let active = store_at(&dir, "library-a", &p, PDF, start).unwrap();
        let path = active.path().to_path_buf();
        clean_at(&dir, start + RETENTION_SECS);
        assert!(path.exists(), "do not delete a PDF a reader is using");
        assert!(
            lookup_at(&dir, "library-a", &p, start + RETENTION_SECS)
                .unwrap()
                .is_none(),
            "expired leases cannot be given to a new read"
        );
        drop(active);
        clean_at(&dir, start + RETENTION_SECS);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn later_answers_reuse_downloads_and_concurrent_reads_download_only_once() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let count = AtomicUsize::new(0);
        let download = || async {
            count.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(PDF.to_vec())
        };
        let (first, concurrent) = tokio::join!(
            acquire_with(&dir, "library-a", &p, download),
            acquire_with(&dir, "library-a", &p, download)
        );
        let first = first.unwrap();
        let concurrent = concurrent.unwrap();
        assert_eq!(first.path(), concurrent.path());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        let path = first.path().to_path_buf();
        drop(first);
        drop(concurrent);
        let reopened = acquire_with(&dir, "library-a", &p, || async {
            panic!("a later answer must not download again")
        })
        .await
        .unwrap();
        assert_eq!(reopened.path(), path);
        let mut explicit_source = p.clone();
        explicit_source.source = Some("arxiv".into());
        assert_eq!(
            acquire_with(&dir, "library-a", &explicit_source, || async {
                panic!("equivalent source must reuse PDF")
            })
            .await
            .unwrap()
            .path(),
            path
        );
        assert!(lookup_at(&dir, "library-b", &p, now()).unwrap().is_none());
        let mut version = p.clone();
        version.updated = "2026-10-15".into();
        assert!(lookup_at(&dir, "library-a", &version, now())
            .unwrap()
            .is_none());
    }

    #[test]
    fn import_keeps_an_independent_library_copy_and_removes_the_cache_after_active_reads() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let start = now();
        let active = store_at(&dir, "library-a", &p, PDF, start).unwrap();
        let cached = active.path().to_path_buf();
        let library_pdf = f.0.join("permanent-library-paper.pdf");
        fs::copy(&cached, &library_pdf).unwrap();
        discard_imported(&dir, "library-a", &p).unwrap();
        assert!(cached.exists(), "an active reader keeps its cache lease");
        assert!(
            read_record(&cached.with_extension("json"))
                .unwrap()
                .imported
        );
        assert!(lookup_at(&dir, "library-a", &p, start).unwrap().is_none());
        drop(active);
        clean_at(&dir, start);
        assert!(!cached.exists());
        assert!(library_pdf.exists());
        clean_at(&dir, start + 2 * RETENTION_SECS);
        assert_eq!(
            fs::read(&library_pdf).unwrap(),
            PDF,
            "cache cleanup never targets a library PDF"
        );
    }

    #[test]
    fn ordinary_import_disposes_the_cache_copy_immediately_after_commit() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let pdf = store_at(&dir, "library-a", &p, PDF, now()).unwrap();
        let path = pdf.path().to_path_buf();
        fs::copy(pdf.path(), f.0.join("library.pdf")).unwrap();
        drop(pdf);
        discard_imported(&dir, "library-a", &p).unwrap();
        assert!(!path.exists());
        assert!(f.0.join("library.pdf").exists());
    }

    #[test]
    fn orphaned_downloads_broken_sidecars_and_partial_writes_expire_without_deleting_unowned_files()
    {
        let f = Fixture::new();
        let dir = f.cache();
        fs::create_dir_all(&dir).unwrap();
        let old = now() - RETENTION_SECS;
        let orphan = dir.join(format!("{}.pdf", uuid::Uuid::new_v4()));
        fs::write(&orphan, PDF).unwrap();
        age_file(&orphan, old);
        let broken = dir.join(format!("{}.json", uuid::Uuid::new_v4()));
        fs::write(&broken, "broken JSON").unwrap();
        let broken_pdf = broken.with_extension("pdf");
        fs::write(&broken_pdf, PDF).unwrap();
        age_file(&broken_pdf, old);
        let partial = dir.join(format!("{}.tmp.1234abcd", uuid::Uuid::new_v4()));
        fs::write(&partial, PDF).unwrap();
        age_file(&partial, old);
        let foreign = dir.join("important-user-paper.pdf");
        fs::write(&foreign, PDF).unwrap();
        age_file(&foreign, old);
        clean_at(&dir, now());
        assert!(!orphan.exists());
        assert!(!broken.exists());
        assert!(!broken_pdf.exists());
        assert!(!partial.exists());
        assert!(foreign.exists());
    }

    #[tokio::test]
    async fn cancelling_an_unfinished_download_leaves_no_file_and_releases_the_download_lock() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let d = dir.clone();
        let paper_copy = p.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            acquire_with(&d, "library-a", &paper_copy, || async {
                let _ = started.send(());
                std::future::pending::<Result<Vec<u8>, String>>().await
            })
            .await
        });
        ready.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(!dir.exists());
        let recovered = tokio::time::timeout(
            Duration::from_secs(2),
            acquire_with(&dir, "library-a", &p, || async { Ok(PDF.to_vec()) }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fs::read(recovered.path()).unwrap(), PDF);
    }

    #[tokio::test]
    async fn an_expired_download_is_deleted_and_a_requested_paper_gets_a_new_seven_day_file() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let old = store_at(&dir, "library-a", &p, PDF, now() - RETENTION_SECS - 1).unwrap();
        let old_path = old.path().to_path_buf();
        drop(old);
        let count = AtomicUsize::new(0);
        let fresh = acquire_with(&dir, "library-a", &p, || async {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(PDF.to_vec())
        })
        .await
        .unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!old_path.exists());
        assert!(!old_path.with_extension("json").exists());
        assert_ne!(fresh.path(), old_path);
        let record = read_record(&fresh.path().with_extension("json")).unwrap();
        assert!(!record.expired(now()));
    }

    #[tokio::test]
    async fn invalid_cached_bytes_trigger_a_new_download_and_failures_do_not_persist() {
        let f = Fixture::new();
        let dir = f.cache();
        let p = paper();
        let pdf = store_at(&dir, "library-a", &p, PDF, now()).unwrap();
        let path = pdf.path().to_path_buf();
        drop(pdf);
        fs::write(&path, b"broken").unwrap();
        let recovered = acquire_with(&dir, "library-a", &p, || async { Ok(PDF.to_vec()) })
            .await
            .unwrap();
        assert_ne!(recovered.path(), path);
        assert_eq!(fs::read(recovered.path()).unwrap(), PDF);
        let failing = f.0.join("failing-cache");
        assert!(acquire_with(&failing, "library-a", &p, || async {
            Err("network failed".into())
        })
        .await
        .is_err());
        assert!(!failing.exists());
        assert!(acquire_with(&failing, "library-a", &p, || async {
            Ok(b"<html>not a PDF</html>".to_vec())
        })
        .await
        .is_err());
        assert!(!failing.exists());
    }
}
