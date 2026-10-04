//! Rasterise a single PDF page to a PNG image.
//!
//! Rendering runs in-process via **PDFium**, bound to a dynamic library that
//! ships inside the app (fetched at build time by `scripts/fetch-pdfium.mjs`),
//! so the user needs no poppler / `pdftoppm` install. If the PDFium library
//! cannot be located for any reason, the old poppler CLI is tried as a fallback
//! so a machine that already has it still works.
//!
//! PDFium is bound **once per process** and every call into it runs on one
//! dedicated thread that also keeps the last few opened documents (see
//! [`run_on_pdfium`] and [`DocCache`]): scrolling a PDF asks for a page at a time,
//! and re-binding the library and re-parsing the whole file for each of them was
//! most of what a page cost. The PNG is encoded afterwards on the caller's thread
//! (see [`encode_png`]), so the next page can already be rendering meanwhile.

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageEncoder};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{mpsc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

/// Default rasterisation resolution. 150 DPI renders figures, diagrams and
/// result curves legibly while keeping a page PNG to a few hundred kilobytes.
pub const DEFAULT_DPI: u32 = 150;

/// The largest bitmap the reader may ask for. A Letter page at the reader's 400 %
/// zoom on a 2× display is 4896 × 6336 ≈ 31 MP — about 120 ms to render and encode
/// in a release build — so this covers every zoom on Letter/A4 at 2×, while capping
/// what one page can cost once the WebView has decoded it (4 bytes a pixel).
pub const MAX_VIEW_PIXELS: u64 = 40_000_000;

/// Render one **1-based** page to PNG bytes at exactly `width` × `height` pixels.
///
/// For the reader, which asks for its on-screen box × devicePixelRatio so the
/// image is shown pixel for pixel. A DPI can't do that: a whole-number DPI misses
/// the box by a few pixels at almost every zoom, and the browser then resamples
/// the whole page to fit — every glyph edge smeared across two device pixels.
/// A request above [`MAX_VIEW_PIXELS`] is shrunk proportionally (the browser then
/// scales it up, softening only that extreme zoom).
pub fn render_pdf_page_png_sized(
    pdf_path: &Path,
    page: u32,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, String> {
    let (w, h) = cap_pixels(width, height, MAX_VIEW_PIXELS);
    match render_with_pdfium(pdf_path, page, Target::Size(w, h)) {
        Ok(bytes) => Ok(bytes),
        Err(pdfium_err) => render_with_pdftoppm(
            pdf_path,
            page,
            &["-scale-to-x".into(), w.to_string(), "-scale-to-y".into(), h.to_string()],
        )
        .map_err(|poppler_err| format!("{pdfium_err} · poppler 兜底也失败：{poppler_err}")),
    }
}

/// `width` × `height`, shrunk with its aspect ratio kept until it fits in `max`
/// pixels. Never zero, and never over `max`.
fn cap_pixels(width: u32, height: u32, max: u64) -> (u32, u32) {
    let (w, h) = (width.max(1), height.max(1));
    let area = w as u64 * h as u64;
    if area <= max {
        return (w, h);
    }
    let k = (max as f64 / area as f64).sqrt();
    let (mut nw, mut nh) = (
        ((w as f64 * k).floor() as u32).max(1),
        ((h as f64 * k).floor() as u32).max(1),
    );
    // Keeping a side at one pixel can undo the shrink: 1 × 4 000 000 000 scales to
    // 1 × 400 000 000, still ten times the cap. The long side then takes what the
    // cap leaves instead of the ratio's share (that ratio is degenerate anyway).
    if nw as u64 * nh as u64 > max {
        if nw == 1 {
            nh = nh.min(u32::try_from(max).unwrap_or(u32::MAX));
        } else if nh == 1 {
            nw = nw.min(u32::try_from(max).unwrap_or(u32::MAX));
        }
    }
    (nw, nh)
}

/// Render one **1-based** page of `pdf_path` to PNG bytes at `dpi`.
///
/// A page that would come out above [`MAX_VIEW_PIXELS`] at that `dpi` (a poster, or a
/// MediaBox of the PDF maximum 14 400 pt, which is 900 MP at 150 DPI) is rendered at
/// the largest size that fits instead, aspect ratio kept — see [`resolve_target`].
pub fn render_pdf_page_png(pdf_path: &Path, page: u32, dpi: u32) -> Result<Vec<u8>, String> {
    match render_with_pdfium(pdf_path, page, Target::Dpi(dpi)) {
        Ok(bytes) => Ok(bytes),
        // PDFium missing or failed: fall back to poppler so a machine that has
        // it installed keeps working. Surface both errors if that fails too.
        Err(pdfium_err) => render_with_pdftoppm(pdf_path, page, &["-r".into(), dpi.to_string()])
            .map_err(|poppler_err| format!("{pdfium_err} · poppler 兜底也失败：{poppler_err}")),
    }
}

// ── PDFium (bundled, the normal path) ─────────────────────────────────────────

/// Candidate locations for the bundled PDFium dynamic library, most specific
/// first. Resolved from the running executable so it works from both the
/// packaged app and the MCP subprocess (same binary), with a dev copy under
/// `src-tauri/lib` as the last resort.
fn pdfium_lib_candidates() -> Vec<PathBuf> {
    let name = Pdfium::pdfium_platform_library_name();
    let mut cands = Vec::new();
    // Explicit override, mostly for development.
    if let Ok(explicit) = std::env::var("ARGUS_PDFIUM_LIB") {
        cands.push(PathBuf::from(explicit));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Windows / Linux: Tauri drops bundled resources beside the exe.
            cands.push(dir.join(&name));
            cands.push(dir.join("lib").join(&name));
            // macOS .app: the exe is in Contents/MacOS, resources in Resources.
            cands.push(dir.join("../Resources").join(&name));
            cands.push(dir.join("../Resources/lib").join(&name));
        }
    }
    // Dev builds run from target/…; the fetch script drops the lib here.
    cands.push(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("lib")
            .join(&name),
    );
    cands
}

fn bind_pdfium() -> Result<Pdfium, String> {
    for cand in pdfium_lib_candidates() {
        if cand.is_file() {
            if let Ok(bindings) = Pdfium::bind_to_library(&cand) {
                return Ok(Pdfium::new(bindings));
            }
        }
    }
    // Last resort: a system-wide install, if the user happens to have one.
    Pdfium::bind_to_system_library()
        .map(Pdfium::new)
        .map_err(|e| {
            format!("找不到内置的 PDFium 渲染库（也未在系统中安装）：{e}")
        })
}

/// How big the page bitmap should be.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// Exactly this many pixels (the reader's on-screen box × devicePixelRatio).
    Size(u32, u32),
    /// The page scaled from PDF user space (72 DPI) to this resolution.
    Dpi(u32),
}

/// Pin a DPI request to a pixel size when it would be too big, once the page's own
/// size (in points) is known. A size request is already capped by the caller.
///
/// The arithmetic mirrors pdfium-render's (`round(points × scale)`), so a request
/// that is left as a DPI one renders at exactly the size tested here. Saturating
/// float-to-int casts keep absurd page sizes and DPIs from wrapping.
fn resolve_target(page_width_pt: f32, page_height_pt: f32, target: Target) -> Target {
    let Target::Dpi(dpi) = target else {
        return target;
    };
    let scale = dpi as f64 / 72.0;
    let w = (page_width_pt as f64 * scale).round() as u32;
    let h = (page_height_pt as f64 * scale).round() as u32;
    if w as u64 * h as u64 <= MAX_VIEW_PIXELS {
        return target;
    }
    let (w, h) = cap_pixels(w, h, MAX_VIEW_PIXELS);
    Target::Size(w, h)
}

impl Target {
    fn config(self) -> PdfRenderConfig {
        match self {
            Target::Size(w, h) => PdfRenderConfig::new().set_target_size(w as Pixels, h as Pixels),
            Target::Dpi(dpi) => PdfRenderConfig::new().scale_page_by_factor(dpi as f32 / 72.0),
        }
    }
}

/// Rasterise one page and encode it. The PDFium part runs on the PDFium thread;
/// the PNG encode then runs here, on the caller's own thread.
fn render_with_pdfium(pdf_path: &Path, page: u32, target: Target) -> Result<Vec<u8>, String> {
    if page == 0 {
        return Err("页码从 1 开始".to_string());
    }
    let path = pdf_path.to_path_buf();
    let image = run_on_pdfium(move |pdfium, cache| render_bitmap(pdfium, cache, &path, page, target))??;
    encode_png(&image, page)
}

/// Lossless PNG for a rendered page.
///
/// `Fast` + `Up` is the cheapest pairing that still compresses a page well: `Up`
/// needs no per-pixel branching to encode or to decode, and text on a white page
/// is almost all runs that `Fast` (fdeflate) collapses. On a 15-page Letter paper at
/// 2548 × 3298 (release build) it took 7 ms a page where the encoder's default
/// (`Fast` + `Adaptive`) took 14.5 ms, for a PNG 12 % larger (2.5 MB against 2.2 MB
/// on average) that also decodes about 40 % faster (measured with the `image`
/// decoder). Heavier settings were slower still: `Default` + `NoFilter` is 66 ms for
/// 0.8 MB. Every setting is lossless, so the decoded RGBA is bit-for-bit what the
/// default would give (see the tests).
fn encode_png(image: &DynamicImage, page: u32) -> Result<Vec<u8>, String> {
    let (w, h) = (image.width(), image.height());
    // A text page lands around a quarter of a byte per pixel; starting there saves
    // the encoder growing the buffer through a dozen doublings.
    let mut out = Vec::with_capacity((w as usize * h as usize / 4).max(64 * 1024));
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Up)
        .write_image(image.as_bytes(), w, h, image.color().into())
        .map_err(|e| format!("第 {page} 页编码 PNG 失败：{e}"))?;
    if out.is_empty() {
        return Err(format!("PDFium 渲染第 {page} 页得到空图。"));
    }
    Ok(out)
}

// ── The PDFium thread ─────────────────────────────────────────────────────────
//
// PDFium is not thread-safe, and pdfium-render makes that visible in two ways:
//
//  * with its default `thread_safe` feature (which this crate uses), creating a
//    `Pdfium` takes a process-wide lock that is held until that `Pdfium` is
//    dropped, so only one can exist at a time — it is how the old code serialised
//    its renders, one bind per page;
//  * `Pdfium` and `PdfDocument<'p>` are neither `Send` nor `Sync` (that is the
//    separate `sync` feature, which promises more than PDFium does), and a
//    document borrows the `Pdfium` it came from.
//
// So the one sound way to bind once and keep documents open is to give both to a
// single thread for good: it creates the only `Pdfium`, keeps the documents in a
// local the borrow checker can see, and everything else sends it closures. No
// `unsafe`, no leaked `'static` bindings, and "serialise every PDFium call" holds
// by construction. The cost is that renders queue behind one another — exactly as
// they did behind the old lock — while the PNG encode, which is the slow half,
// happens off this thread and overlaps the next render.
//
// Because the thread owns the lock for the life of the process, nothing else in
// the crate may create a `Pdfium` on another thread: it would wait forever. Tests
// that need one go through [`run_on_pdfium`] as well.

/// A closure for the PDFium thread, with the `Pdfium` and the document cache.
type Job = Box<dyn for<'p> FnOnce(&'p Pdfium, &mut DocCache<'p>) + Send + 'static>;

/// A running PDFium thread's mailbox. `id` tells [`forget_worker`] which one it is
/// being asked to forget, so a caller that lost a race cannot discard a worker
/// another caller has just started.
struct Worker {
    id: u64,
    tx: mpsc::Sender<Job>,
}

/// The thread, started on first use and again if it is ever found dead.
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);
static WORKER_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// PDFium parses recursively, and a hostile or just deeply nested PDF can use a lot
/// of stack. The tokio blocking threads this used to run on have 2 MiB; give the
/// one thread that now does all of it more room (it is only address space).
const WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// How long the PDFium thread may go without a job before it closes the documents it
/// kept open. Reading a paper keeps asking for pages, so this only lets go once the
/// reader has been put down: without it, four 48 MiB scanned books opened and closed
/// would sit in memory (the file bytes plus PDFium's parsed state) until the app
/// quit. The thread itself stays — only the documents go — so the next render costs
/// one re-open and no re-bind.
const CACHE_IDLE_RELEASE: Duration = Duration::from_secs(60);

/// The PDFium thread's loop: run jobs in order until every sender is gone (shutdown),
/// closing the cached documents whenever `idle` passes without a job.
///
/// With an empty cache it blocks plainly, so an idle app does not wake up to find
/// nothing to release.
fn serve_jobs<'p>(
    rx: &mpsc::Receiver<Job>,
    pdfium: &'p Pdfium,
    cache: &mut DocCache<'p>,
    idle: Duration,
) {
    loop {
        let job = if cache.is_empty() {
            match rx.recv() {
                Ok(job) => job,
                Err(_) => return,
            }
        } else {
            match rx.recv_timeout(idle) {
                Ok(job) => job,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    cache.clear();
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        };
        // A panic inside PDFium's Rust wrapper must not take the thread (and with
        // it every later render) down. The caller sees its reply channel close, and
        // the cache is dropped in case the panic left a document half-used.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job(pdfium, cache)));
        if outcome.is_err() {
            cache.clear();
        }
    }
}

fn spawn_worker() -> Result<Worker, String> {
    let (tx, rx) = mpsc::channel::<Job>();
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::Builder::new()
        .name("argus-pdfium".into())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            let pdfium = match bind_pdfium() {
                Ok(pdfium) => {
                    let _ = ready_tx.send(Ok(()));
                    pdfium
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            // Declared after `pdfium` so it is dropped first: documents must be
            // closed before the library is torn down.
            let mut cache = DocCache::new();
            serve_jobs(&rx, &pdfium, &mut cache, CACHE_IDLE_RELEASE);
        })
        .map_err(|e| format!("无法启动 PDFium 渲染线程：{e}"))?;
    ready_rx
        .recv()
        .map_err(|_| "PDFium 渲染线程启动时意外退出".to_string())??;
    Ok(Worker {
        id: WORKER_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        tx,
    })
}

/// The live worker's mailbox, starting the thread if there is none. A failed
/// start (PDFium not found) is not remembered: the next call tries again.
fn worker_sender() -> Result<(u64, mpsc::Sender<Job>), String> {
    // Poisoning is harmless here — the guarded value is only ever replaced whole.
    let mut slot = WORKER.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(spawn_worker()?);
    }
    let worker = slot.as_ref().expect("just filled");
    Ok((worker.id, worker.tx.clone()))
}

/// Drop worker `id` (if it is still the current one) so the next call starts a new
/// thread. The old thread, if it is somehow still alive, winds down once its
/// queue is empty.
fn forget_worker(id: u64) {
    let mut slot = WORKER.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_some_and(|w| w.id == id) {
        *slot = None;
    }
}

/// Run `f` on the PDFium thread and return what it returns.
///
/// An `Err` means the thread could not be started, or died while running `f`
/// (a panic); `f`'s own failures belong in `R`. Never call it from inside a job —
/// the thread would be waiting on itself.
fn run_on_pdfium<R, F>(f: F) -> Result<R, String>
where
    R: Send + 'static,
    F: for<'p> FnOnce(&'p Pdfium, &mut DocCache<'p>) -> R + Send + 'static,
{
    let (reply_tx, reply_rx) = mpsc::sync_channel::<R>(1);
    let mut job: Option<Job> = Some(Box::new(move |pdfium, cache| {
        let _ = reply_tx.send(f(pdfium, cache));
    }));
    // A send fails only when the thread is gone; start a fresh one and go again.
    for _ in 0..2 {
        let (id, tx) = worker_sender()?;
        match tx.send(job.take().expect("a failed send hands the job back")) {
            Ok(()) => {
                return reply_rx
                    .recv()
                    .map_err(|_| "PDFium 渲染线程在渲染途中异常退出".to_string());
            }
            Err(mpsc::SendError(unsent)) => {
                job = Some(unsent);
                forget_worker(id);
            }
        }
    }
    Err("PDFium 渲染线程无法恢复".to_string())
}

/// Let the PDFium thread finish its queue and exit; the next call starts a new one.
#[cfg(test)]
fn shutdown_worker() {
    *WORKER.lock().unwrap_or_else(PoisonError::into_inner) = None;
}

// ── Open-document cache ───────────────────────────────────────────────────────

/// How many documents stay open. Reading is one paper at a time, with a second or
/// third alongside it when a tab is switched; more would only hold memory.
const CACHE_DOCS: usize = 4;

/// Documents above this size are opened straight from the file for one render and
/// not kept, so the cache cannot grow past `CACHE_DOCS` × this (192 MiB at worst;
/// a paper is a few MiB).
const CACHE_MAX_DOC_BYTES: u64 = 48 * 1024 * 1024;

/// What identifies a version of a PDF on disk. The path is canonical so two spellings
/// of one file share an entry; modified time and length make a file replaced or
/// rewritten a different key, so a stale document is never served. The identity
/// catches what those two cannot (see [`file_identity`]).
#[derive(Clone, Debug, PartialEq, Eq)]
struct DocKey {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    identity: Option<FileIdentity>,
}

/// Device, inode, and change time (seconds, nanoseconds).
type FileIdentity = (u64, u64, i64, i64);

/// Which file this is, beyond what it is called and when it was last written.
///
/// A sync or restore tool that preserves modified times (`cp -p`, `rsync -t`, a
/// backup restore) can put different bytes of the same length behind the same path
/// and the same modified time, and a coarse-clock volume makes that window wider.
/// The inode tells a replaced file (a new one renamed over the old) apart, and the
/// change time, which any write bumps and which user space cannot set, tells an
/// in-place overwrite apart. Windows has no stable counterpart in `std`, so there the
/// key stays path, modified time and length.
#[cfg(unix)]
fn file_identity(meta: &std::fs::Metadata) -> Option<FileIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some((meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec()))
}

#[cfg(not(unix))]
fn file_identity(_meta: &std::fs::Metadata) -> Option<FileIdentity> {
    None
}

impl DocKey {
    fn of(path: &Path) -> Result<Self, String> {
        let meta = std::fs::metadata(path).map_err(|e| format!("无法打开 PDF：{e}"))?;
        Ok(Self {
            path: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
            modified: meta.modified().ok(),
            len: meta.len(),
            identity: file_identity(&meta),
        })
    }
}

struct CachedDoc<'p> {
    key: DocKey,
    doc: PdfDocument<'p>,
}

/// The last few opened documents, least recently used first. It lives on the
/// PDFium thread (documents borrow its `Pdfium`) and is reached only from there.
///
/// A document is parsed from a copy of the file held in memory, not from an open
/// file: a cached document that kept a file handle would stop Windows from
/// deleting or renaming the paper's folder while it sat in the cache.
struct DocCache<'p> {
    docs: Vec<CachedDoc<'p>>,
    /// Documents opened so far, cached or not (what the tests count).
    loads: usize,
}

impl<'p> DocCache<'p> {
    fn new() -> Self {
        Self { docs: Vec::new(), loads: 0 }
    }

    fn clear(&mut self) {
        self.docs.clear();
    }

    fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }

    /// Drop every cached version of the file at `path`.
    fn evict(&mut self, path: &Path) {
        self.docs.retain(|d| d.key.path != path);
    }

    /// Run `f` on the document for `key`, opening it first if it is not cached.
    /// The flag says whether the document was already cached.
    fn with_doc<R>(
        &mut self,
        pdfium: &'p Pdfium,
        key: &DocKey,
        f: impl FnOnce(&PdfDocument<'p>) -> R,
    ) -> Result<(R, bool), String> {
        if let Some(i) = self.docs.iter().position(|d| d.key == *key) {
            // Most recently used goes last.
            let entry = self.docs.remove(i);
            self.docs.push(entry);
            let entry = self.docs.last().expect("just pushed");
            return Ok((f(&entry.doc), true));
        }
        // The same file with another modified time, length or identity is a different
        // version: the old one can never match again, so free it now.
        self.evict(&key.path);
        self.loads += 1;
        let open_err = |e: &dyn std::fmt::Display| format!("无法打开 PDF：{e}");
        if key.len > CACHE_MAX_DOC_BYTES {
            let doc = pdfium.load_pdf_from_file(&key.path, None).map_err(|e| open_err(&e))?;
            return Ok((f(&doc), false));
        }
        let bytes = std::fs::read(&key.path).map_err(|e| open_err(&e))?;
        let doc = pdfium.load_pdf_from_byte_vec(bytes, None).map_err(|e| open_err(&e))?;
        if self.docs.len() >= CACHE_DOCS {
            self.docs.remove(0);
        }
        self.docs.push(CachedDoc { key: key.clone(), doc });
        let entry = self.docs.last().expect("just pushed");
        Ok((f(&entry.doc), false))
    }
}

/// Why one attempt to render a page failed.
enum Fail {
    /// The document is fine and simply has no such page: retrying cannot help.
    NoSuchPage(String),
    /// The page exists but PDFium rasterising it failed (a bitmap it would not
    /// allocate, say). That has nothing to do with the document being stale and a
    /// second try fails the same way, so it is final.
    RenderFailed(String),
    /// The document would not hand over a page it should have, and not because the
    /// page number is out of range. From a cached document that is how a copy gone
    /// bad shows (the file changed under it), so it is worth one fresh open.
    PageUnreadable(String),
}

impl Fail {
    fn into_message(self) -> String {
        match self {
            Fail::NoSuchPage(m) | Fail::RenderFailed(m) | Fail::PageUnreadable(m) => m,
        }
    }
}

fn render_page(doc: &PdfDocument<'_>, page: u32, target: Target) -> Result<DynamicImage, Fail> {
    let index = page - 1;
    let missing = |e: PdfiumError| Fail::NoSuchPage(format!("这份 PDF 没有第 {page} 页：{e}"));
    // pdfium-render indexes pages with a u16; a plain `as` cast would wrap page
    // 65 537 round to page 1.
    let Ok(index16) = u16::try_from(index) else {
        return Err(missing(PdfiumError::PageIndexOutOfBounds));
    };
    let page_obj = match doc.pages().get(index16) {
        Ok(p) => p,
        Err(PdfiumError::PageIndexOutOfBounds) => return Err(missing(PdfiumError::PageIndexOutOfBounds)),
        Err(e) => return Err(Fail::PageUnreadable(format!("这份 PDF 没有第 {page} 页：{e}"))),
    };
    // A DPI request is sized from the page itself, so it can only be checked against
    // the pixel cap here, where the page is known.
    let target = resolve_target(page_obj.width().value, page_obj.height().value, target);
    let bitmap = page_obj
        .render_with_config(&target.config())
        .map_err(|e| Fail::RenderFailed(format!("PDFium 渲染第 {page} 页失败：{e}")))?;
    Ok(bitmap.as_image())
}

/// Run `attempt` on the document for `key`. If it fails with
/// [`Fail::PageUnreadable`] on a document that came from the cache, the document is
/// thrown away and `attempt` runs once more on a fresh open before the failure is
/// believed; any other failure, and any failure of a freshly opened document, is
/// final (a retry would re-read and re-parse the whole file for nothing, and flush a
/// healthy document everyone else was using).
fn render_with_retry<'p, T>(
    pdfium: &'p Pdfium,
    cache: &mut DocCache<'p>,
    key: &DocKey,
    mut attempt: impl FnMut(&PdfDocument<'p>) -> Result<T, Fail>,
) -> Result<T, String> {
    let (outcome, was_cached) = cache.with_doc(pdfium, key, |doc| attempt(doc))?;
    match outcome {
        Ok(value) => Ok(value),
        Err(Fail::PageUnreadable(_)) if was_cached => {
            cache.evict(&key.path);
            let (outcome, _) = cache.with_doc(pdfium, key, |doc| attempt(doc))?;
            outcome.map_err(Fail::into_message)
        }
        Err(fail) => Err(fail.into_message()),
    }
}

/// Render one **1-based** page to an RGBA image. Runs on the PDFium thread.
fn render_bitmap<'p>(
    pdfium: &'p Pdfium,
    cache: &mut DocCache<'p>,
    path: &Path,
    page: u32,
    target: Target,
) -> Result<DynamicImage, String> {
    let key = DocKey::of(path)?;
    render_with_retry(pdfium, cache, &key, |doc| render_page(doc, page, target))
}

// ── Poppler `pdftoppm` (fallback for machines that already have it) ────────────

/// Locate `pdftoppm`, checking `PATH` plus the usual Homebrew / system prefixes.
///
/// A GUI app launched from Finder does not inherit a login shell's `PATH`, so
/// the bare name alone is not enough — the same absolute fallbacks the OCR code
/// relies on are checked too.
fn pdftoppm_bin() -> Option<PathBuf> {
    for cand in [
        "pdftoppm",
        "/opt/homebrew/bin/pdftoppm",
        "/usr/local/bin/pdftoppm",
        "/usr/bin/pdftoppm",
    ] {
        if !cand.contains('/') || Path::new(cand).is_file() {
            return Some(PathBuf::from(cand));
        }
    }
    None
}

/// `size_args` picks the output size: `-r <dpi>`, or `-scale-to-x <w> -scale-to-y <h>`.
fn render_with_pdftoppm(pdf_path: &Path, page: u32, size_args: &[String]) -> Result<Vec<u8>, String> {
    let bin = pdftoppm_bin().ok_or_else(|| "系统未安装 pdftoppm（poppler）".to_string())?;

    // `-singlefile` makes pdftoppm write exactly `<prefix>.png` with no page
    // number suffix, so the output path is known without globbing the temp dir.
    let prefix = std::env::temp_dir().join(format!("argus-page-{}", uuid::Uuid::new_v4()));
    let out = prefix.with_extension("png");

    let status = Command::new(&bin)
        .arg("-png")
        .arg("-singlefile")
        .args(size_args)
        .arg("-f")
        .arg(page.to_string())
        .arg("-l")
        .arg(page.to_string())
        .arg(pdf_path)
        .arg(&prefix)
        .status()
        .map_err(|e| format!("运行 pdftoppm 失败：{e}"))?;

    if !status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(format!("pdftoppm 无法渲染第 {page} 页（页码是否有效？）。"));
    }

    let bytes = std::fs::read(&out)
        .map_err(|e| format!("pdftoppm 没有为第 {page} 页产出图片：{e}"))?;
    let _ = std::fs::remove_file(&out);
    if bytes.is_empty() {
        return Err(format!("pdftoppm 为第 {page} 页产出了空图片。"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    const LETTER: (f32, f32) = (612.0, 792.0);
    const A4: (f32, f32) = (595.0, 842.0);

    fn scratch_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("argus-render-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write a PDF with one blank page per entry of `sizes` (in points).
    ///
    /// Goes through [`run_on_pdfium`] like everything else: the PDFium thread holds
    /// pdfium-render's process-wide lock, so a `Pdfium` created on the test's own
    /// thread would wait for it forever.
    fn write_pdf(path: &Path, sizes: &[(f32, f32)]) {
        let (path, sizes) = (path.to_path_buf(), sizes.to_vec());
        run_on_pdfium(move |pdfium, _| {
            let mut doc = pdfium.create_new_pdf().unwrap();
            for (w, h) in sizes {
                doc.pages_mut()
                    .create_page_at_end(PdfPagePaperSize::from_points(PdfPoints::new(w), PdfPoints::new(h)))
                    .unwrap();
            }
            doc.save_to_file(&path).unwrap();
        })
        .unwrap();
    }

    /// Width and height from a PNG's IHDR chunk: an 8-byte signature, then the chunk
    /// with big-endian width and height.
    fn png_size(png: &[u8]) -> (u32, u32) {
        (
            u32::from_be_bytes(png[16..20].try_into().unwrap()),
            u32::from_be_bytes(png[20..24].try_into().unwrap()),
        )
    }

    fn decode_rgba(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .expect("valid PNG")
            .into_rgba8()
    }

    /// The bundled PDFium library (src-tauri/lib, dropped by fetch-pdfium.mjs)
    /// must actually load at runtime — catching a wrong platform lib name or a
    /// broken download before it turns into a silent page-render failure.
    #[test]
    fn pdfium_library_binds() {
        run_on_pdfium(|_, _| ()).expect("PDFium should bind to the bundled library");
    }

    #[test]
    fn a_request_within_the_cap_is_kept_exactly() {
        assert_eq!(super::cap_pixels(2548, 3298, super::MAX_VIEW_PIXELS), (2548, 3298));
        // Letter at 400 % on a 2× display — the largest the reader asks for there.
        assert_eq!(super::cap_pixels(4896, 6336, super::MAX_VIEW_PIXELS), (4896, 6336));
        assert_eq!(super::cap_pixels(0, 0, super::MAX_VIEW_PIXELS), (1, 1));
    }

    /// A side held at one pixel must not undo the shrink: 1 × 4e9 used to come back as
    /// 1 × 4e8, ten times the cap, and 4 × 1e9 as 100 MP.
    #[test]
    fn the_cap_holds_for_degenerate_aspect_ratios() {
        let max = super::MAX_VIEW_PIXELS;
        for (w, h) in [
            (1u32, 4_000_000_000u32),
            (4_000_000_000, 1),
            (4, 1_000_000_000),
            (1_000_000_000, 4),
            (3, u32::MAX),
            (u32::MAX, u32::MAX),
            (2, 40_000_001),
            (1, 40_000_001),
            (40_000_001, 1),
            (1, 40_000_000),
            (7, 9_000_000),
            (0, u32::MAX),
        ] {
            let (cw, ch) = super::cap_pixels(w, h, max);
            assert!(cw >= 1 && ch >= 1, "{w}x{h} -> {cw}x{ch}");
            assert!(cw as u64 * ch as u64 <= max, "{w}x{h} -> {cw}x{ch} is over the cap");
            // Never grown.
            assert!(cw <= w.max(1) && ch <= h.max(1), "{w}x{h} -> {cw}x{ch} grew");
        }
        // The shrink is still the ratio's, not a blunt clamp, wherever it can be.
        assert_eq!(super::cap_pixels(1, 4_000_000_000, max), (1, 40_000_000));
        let (w, h) = super::cap_pixels(2, 4_000_000_000, max);
        assert!(w as u64 * h as u64 <= max && w >= 1, "{w}x{h}");
    }

    #[test]
    fn an_oversized_request_shrinks_with_its_aspect_ratio() {
        let (w, h) = super::cap_pixels(8000, 10_000, super::MAX_VIEW_PIXELS);
        assert!(w as u64 * h as u64 <= super::MAX_VIEW_PIXELS, "{w}x{h}");
        assert!((w as f64 / h as f64 - 0.8).abs() < 0.001, "{w}x{h}");
        assert!(w as u64 * h as u64 > super::MAX_VIEW_PIXELS * 99 / 100, "barely shrunk: {w}x{h}");
    }

    /// The reader shows the image pixel for pixel, so it must be exactly the size
    /// asked for — including sizes no whole-number DPI could produce (a Letter
    /// page at 208.2 % zoom on a 2× display is 2548 × 3298; 300 DPI gives 2550 × 3300).
    #[test]
    fn a_sized_render_is_exactly_the_requested_pixels() {
        let dir = scratch_dir();
        let path = dir.join("letter.pdf");
        write_pdf(&path, &[LETTER]);
        for (w, h) in [(2548u32, 3298u32), (2400, 3106), (1273, 1647)] {
            let png = super::render_pdf_page_png_sized(&path, 1, w, h).unwrap();
            assert_eq!(png_size(&png), (w, h));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A DPI request is pinned to a size only when it would pass the pixel cap, and
    /// is otherwise left exactly as the caller asked.
    #[test]
    fn a_dpi_request_is_sized_through_the_cap_only_when_it_needs_to_be() {
        let max = super::MAX_VIEW_PIXELS;
        // Letter at the usual DPIs, and at the 400 the command allows: kept as DPI.
        for dpi in [72, 150, 300, 400] {
            assert!(matches!(super::resolve_target(612.0, 792.0, Target::Dpi(dpi)), Target::Dpi(d) if d == dpi));
        }
        // A size request is never touched here.
        assert!(matches!(super::resolve_target(14_400.0, 14_400.0, Target::Size(5, 6)), Target::Size(5, 6)));
        // The PDF maximum page at 150 DPI is 30 000 × 30 000 (900 MP): pinned under the cap.
        let Target::Size(w, h) = super::resolve_target(14_400.0, 14_400.0, Target::Dpi(150)) else {
            panic!("a 900 MP request was left as a DPI one");
        };
        assert!(w as u64 * h as u64 <= max && w as u64 * h as u64 > max * 99 / 100, "{w}x{h}");
        assert_eq!(w, h, "a square page stays square");
        // A long thin strip keeps its shape, within the cap.
        let Target::Size(w, h) = super::resolve_target(100.0, 14_400.0, Target::Dpi(400)) else {
            panic!("a strip over the cap was left as a DPI one");
        };
        assert!(w as u64 * h as u64 <= max && h > w * 100, "{w}x{h}");
        // Absurd inputs neither wrap nor panic.
        for (pw, ph, dpi) in [(f32::MAX, f32::MAX, u32::MAX), (f32::NAN, 612.0, 150), (0.0, 0.0, 150), (1.0, 1.0e9, u32::MAX)] {
            match super::resolve_target(pw, ph, Target::Dpi(dpi)) {
                Target::Size(w, h) => assert!(w >= 1 && h >= 1 && w as u64 * h as u64 <= max, "{pw}x{ph}@{dpi} -> {w}x{h}"),
                Target::Dpi(_) => {}
            }
        }
    }

    /// The same through the real path: the largest page a PDF allows, at the MCP's
    /// 150 DPI, used to ask PDFium for a 3.6 GB bitmap.
    #[test]
    fn a_huge_page_at_a_dpi_is_rendered_within_the_pixel_cap() {
        let dir = scratch_dir();
        let path = dir.join("poster.pdf");
        write_pdf(&path, &[(14_400.0, 14_400.0)]);
        let png = render_pdf_page_png(&path, 1, 150).unwrap();
        let (w, h) = png_size(&png);
        assert!(w as u64 * h as u64 <= super::MAX_VIEW_PIXELS, "{w}x{h}");
        assert!(w > 1000, "{w}x{h}: shrunk far more than the cap needs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PDF rewritten in place must be re-read, not served from the cache. The page
    /// size is what shows it: at 72 DPI the PNG is the page's size in points.
    #[test]
    fn a_rewritten_pdf_is_not_served_stale() {
        let dir = scratch_dir();
        let path = dir.join("paper.pdf");

        write_pdf(&path, &[LETTER]);
        let first = png_size(&render_pdf_page_png(&path, 1, 72).unwrap());
        // Rendered again, the second time from the cache; same answer.
        assert_eq!(png_size(&render_pdf_page_png(&path, 1, 72).unwrap()), first);
        // Page 2 does not exist yet.
        assert!(render_with_pdfium(&path, 2, Target::Dpi(72)).is_err());

        // Replace the file with a longer, differently sized one.
        write_pdf(&path, &[A4, A4]);
        let second = png_size(&render_pdf_page_png(&path, 1, 72).unwrap());
        assert_ne!(second, first, "the cached Letter page was served after the file became A4");
        assert!(second.0 < first.0, "{second:?} vs {first:?}");
        // …and the new second page is there.
        assert_eq!(png_size(&render_pdf_page_png(&path, 2, 72).unwrap()), second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reason the cache key carries the modified time and not just the length:
    /// a same-sized rewrite must still count as a different file.
    #[test]
    fn the_cache_key_follows_the_modified_time_and_the_path() {
        let dir = scratch_dir();
        let path = dir.join("a.pdf");
        std::fs::write(&path, b"%PDF-1.4 same length").unwrap();
        let before = DocKey::of(&path).unwrap();
        assert_eq!(DocKey::of(&path).unwrap(), before, "an untouched file keeps its key");

        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.set_modified(before.modified.unwrap() + Duration::from_secs(5)).unwrap();
        drop(file);
        let touched = DocKey::of(&path).unwrap();
        assert_ne!(touched, before, "only the modified time changed");
        assert_eq!(touched.len, before.len);

        // Two spellings of one file share a key.
        let dotted = dir.join(".").join("a.pdf");
        assert_eq!(DocKey::of(&dotted).unwrap(), touched);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What the modified time and length cannot see: a same-sized file with its old
    /// modified time put back, as `cp -p`, `rsync -t` and a backup restore do.
    #[cfg(unix)]
    #[test]
    fn the_cache_key_notices_a_same_sized_file_whose_modified_time_was_restored() {
        use std::io::Write;
        let dir = scratch_dir();
        let path = dir.join("a.pdf");
        std::fs::write(&path, b"%PDF-1.4 aaaaaaaa").unwrap();
        let before = DocKey::of(&path).unwrap();
        let mtime = before.modified.unwrap();
        let triple = |k: &DocKey| (k.path.clone(), k.modified, k.len);

        // Replaced: another file, same bytes' length and mtime, renamed over the path.
        let other = dir.join("b.tmp");
        std::fs::write(&other, b"%PDF-1.4 bbbbbbbb").unwrap();
        std::fs::OpenOptions::new().write(true).open(&other).unwrap().set_modified(mtime).unwrap();
        std::fs::rename(&other, &path).unwrap();
        let replaced = DocKey::of(&path).unwrap();
        assert_eq!(triple(&replaced), triple(&before), "the old key could not tell these apart");
        assert_ne!(replaced, before, "a replaced file is a different file");

        // Overwritten in place, same inode, mtime put back: only the change time moves.
        std::thread::sleep(Duration::from_millis(50));
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.write_all(b"%PDF-1.4 cccccccc").unwrap();
        file.set_modified(mtime).unwrap();
        drop(file);
        let rewritten = DocKey::of(&path).unwrap();
        assert_eq!(triple(&rewritten), triple(&before));
        assert_ne!(rewritten, replaced, "an in-place rewrite is a different version");
        // And an untouched file still keeps its key.
        assert_eq!(DocKey::of(&path).unwrap(), rewritten);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_zero_page_is_an_error_not_a_crash() {
        let dir = scratch_dir();
        let path = dir.join("two.pdf");
        write_pdf(&path, &[LETTER, LETTER]);

        // Straight through the PDFium path, so the answer cannot come from poppler.
        let target = Target::Size(60, 78);
        let err = render_with_pdfium(&path, 3, target).unwrap_err();
        assert!(err.contains("第 3 页"), "{err}");
        // A page number past what a u16 index can hold must not wrap round to page 1.
        let err = render_with_pdfium(&path, 65_537, target).unwrap_err();
        assert!(err.contains("第 65537 页"), "{err}");
        assert_eq!(render_with_pdfium(&path, 0, target).unwrap_err(), "页码从 1 开始");
        let err = render_with_pdfium(&dir.join("absent.pdf"), 1, target).unwrap_err();
        assert!(err.contains("无法打开 PDF"), "{err}");

        // None of that disturbed the cached document.
        assert!(render_with_pdfium(&path, 2, target).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Opening is paid once per document, and no more than `CACHE_DOCS` stay open.
    /// One job, so no other test's render can slip between the steps.
    #[test]
    fn a_document_is_opened_once_and_the_cache_stays_bounded() {
        let dir = scratch_dir();
        let paths: Vec<PathBuf> = (0..CACHE_DOCS + 2)
            .map(|i| {
                let p = dir.join(format!("{i}.pdf"));
                write_pdf(&p, &[LETTER]);
                p
            })
            .collect();
        run_on_pdfium(move |pdfium, cache| {
            let target = Target::Size(60, 78);
            let start = cache.loads;
            for _ in 0..3 {
                render_bitmap(pdfium, cache, &paths[0], 1, target).unwrap();
            }
            assert_eq!(cache.loads - start, 1, "three renders of one file opened it more than once");

            for p in &paths[1..] {
                render_bitmap(pdfium, cache, p, 1, target).unwrap();
            }
            assert_eq!(cache.docs.len(), CACHE_DOCS);
            // The oldest fell out, so it has to be opened again…
            let before = cache.loads;
            render_bitmap(pdfium, cache, &paths[0], 1, target).unwrap();
            assert_eq!(cache.loads - before, 1);
            // …and the most recent one has not.
            let before = cache.loads;
            render_bitmap(pdfium, cache, paths.last().unwrap(), 1, target).unwrap();
            assert_eq!(cache.loads - before, 0);
        })
        .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn many_threads_rendering_at_once_neither_crash_nor_deadlock() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 6;
        let dir = scratch_dir();
        let paths: Vec<PathBuf> = (0..2)
            .map(|i| {
                let p = dir.join(format!("{i}.pdf"));
                write_pdf(&p, &[LETTER, A4, LETTER]);
                p
            })
            .collect();

        let (tx, rx) = mpsc::channel::<Result<((u32, u32), (u32, u32)), String>>();
        for t in 0..THREADS {
            let (tx, paths) = (tx.clone(), paths.clone());
            std::thread::spawn(move || {
                for i in 0..PER_THREAD {
                    let want = (120 + (t * 7 + i) as u32, 160 + i as u32);
                    let page = 1 + ((t + i) % 3) as u32;
                    let got = render_pdf_page_png_sized(&paths[(t + i) % 2], page, want.0, want.1)
                        .map(|png| (png_size(&png), want));
                    let _ = tx.send(got);
                }
            });
        }
        drop(tx);
        for n in 0..THREADS * PER_THREAD {
            // A deadlock would hang the suite; a minute is far beyond what 48 tiny renders need.
            let (got, want) = rx
                .recv_timeout(Duration::from_secs(60))
                .unwrap_or_else(|_| panic!("render {n} never finished: deadlock?"))
                .unwrap();
            assert_eq!(got, want);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A panic while rendering must cost that one call, not the reader.
    #[test]
    fn a_panicking_job_does_not_take_the_thread_down() {
        let err = run_on_pdfium(|_, _| -> u32 { panic!("deliberate: testing the PDFium thread's panic guard") });
        assert!(err.is_err());
        assert_eq!(run_on_pdfium(|_, _| 7u32).unwrap(), 7);
    }

    /// Wrap a closure as a job for [`serve_jobs`].
    fn job<F>(f: F) -> Job
    where
        F: for<'p> FnOnce(&'p Pdfium, &mut DocCache<'p>) + Send + 'static,
    {
        Box::new(f)
    }

    /// Cached documents are closed once the thread has had nothing to do for the idle
    /// period, and a thread that keeps getting jobs keeps them. The loop is run from
    /// inside a job (the only place a `Pdfium` exists) on its own channel and cache, fed
    /// by a helper thread, so the shared worker's cache and the other tests are untouched.
    #[test]
    fn idle_documents_are_released_and_busy_ones_are_kept() {
        let dir = scratch_dir();
        let path = dir.join("idle.pdf");
        write_pdf(&path, &[LETTER]);
        let idle = Duration::from_millis(400);

        let seen = run_on_pdfium(move |pdfium, _| {
            let (tx, rx) = mpsc::channel::<Job>();
            let (seen_tx, seen_rx) = mpsc::channel::<(&'static str, usize, usize)>();
            let feeder = std::thread::spawn(move || {
                let render = |tag: &'static str, path: PathBuf, seen_tx: mpsc::Sender<(&'static str, usize, usize)>| {
                    job(move |pdfium, cache| {
                        render_bitmap(pdfium, cache, &path, 1, Target::Size(60, 78)).unwrap();
                        seen_tx.send((tag, cache.docs.len(), cache.loads)).unwrap();
                    })
                };
                let look = |tag: &'static str, seen_tx: mpsc::Sender<(&'static str, usize, usize)>| {
                    job(move |_, cache| seen_tx.send((tag, cache.docs.len(), cache.loads)).unwrap())
                };
                tx.send(render("opened", path.clone(), seen_tx.clone())).unwrap();
                // Each gap is well inside the idle period but together they are longer
                // than it (3 × 150 ms > 400 ms): every job restarts the clock, so
                // nothing is released.
                for _ in 0..3 {
                    std::thread::sleep(Duration::from_millis(150));
                    tx.send(look("busy", seen_tx.clone())).unwrap();
                }
                // Then a pause several times the idle period.
                std::thread::sleep(idle * 3);
                tx.send(look("idle", seen_tx.clone())).unwrap();
                // The thread still works, and has to open the document again.
                tx.send(render("reopened", path, seen_tx)).unwrap();
            });
            let mut cache = DocCache::new();
            serve_jobs(&rx, pdfium, &mut cache, idle);
            feeder.join().unwrap();
            seen_rx.try_iter().collect::<Vec<_>>()
        })
        .unwrap();

        assert_eq!(
            seen,
            vec![
                ("opened", 1, 1),
                ("busy", 1, 1),
                ("busy", 1, 1),
                ("busy", 1, 1),
                ("idle", 0, 1),
                ("reopened", 1, 2),
            ]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only the symptom of a stale document earns a second, fresh open; anything else
    /// fails once and leaves the healthy cached document where it is.
    #[test]
    fn only_an_unreadable_page_retries_with_a_fresh_document() {
        let dir = scratch_dir();
        let path = dir.join("retry.pdf");
        write_pdf(&path, &[LETTER]);

        // (result is Ok, attempts, documents opened, cached documents left)
        type Outcome = (Result<(), String>, u32, usize, usize);
        let outcomes: Vec<Outcome> = run_on_pdfium(move |pdfium, cache| {
            let key = DocKey::of(&path).unwrap();
            let mut run = |cached: bool, script: &[fn() -> Result<(), Fail>]| -> Outcome {
                cache.clear();
                if cached {
                    render_bitmap(pdfium, cache, &path, 1, Target::Size(60, 78)).unwrap();
                }
                let loads = cache.loads;
                let mut calls = 0u32;
                let result = render_with_retry(pdfium, cache, &key, |_| {
                    calls += 1;
                    script[(calls as usize - 1).min(script.len() - 1)]()
                });
                (result, calls, cache.loads - loads, cache.docs.len())
            };
            let unreadable: fn() -> Result<(), Fail> = || Err(Fail::PageUnreadable("unreadable".into()));
            let render_failed: fn() -> Result<(), Fail> = || Err(Fail::RenderFailed("render failed".into()));
            let no_page: fn() -> Result<(), Fail> = || Err(Fail::NoSuchPage("no page".into()));
            let fine: fn() -> Result<(), Fail> = || Ok(());
            vec![
                // A render failure on a cached document: one attempt, no re-open, still cached.
                run(true, &[render_failed]),
                run(true, &[no_page]),
                // The stale-document symptom: re-opened once, and the retry's answer stands.
                run(true, &[unreadable, fine]),
                run(true, &[unreadable, unreadable]),
                // A document that was just opened has nothing to retry.
                run(false, &[unreadable]),
            ]
        })
        .unwrap();

        let msg = |o: &Outcome| o.0.clone().err().unwrap_or_default();
        assert_eq!((msg(&outcomes[0]), outcomes[0].1, outcomes[0].2, outcomes[0].3), ("render failed".into(), 1, 0, 1));
        assert_eq!((msg(&outcomes[1]), outcomes[1].1, outcomes[1].2, outcomes[1].3), ("no page".into(), 1, 0, 1));
        assert_eq!((outcomes[2].0.is_ok(), outcomes[2].1, outcomes[2].2, outcomes[2].3), (true, 2, 1, 1));
        assert_eq!((msg(&outcomes[3]), outcomes[3].1, outcomes[3].2, outcomes[3].3), ("unreadable".into(), 2, 1, 1));
        assert_eq!((msg(&outcomes[4]), outcomes[4].1, outcomes[4].2, outcomes[4].3), ("unreadable".into(), 1, 1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worker that is gone is replaced instead of failing every later render.
    #[test]
    fn a_stopped_thread_is_replaced_on_the_next_call() {
        let dir = scratch_dir();
        let path = dir.join("p.pdf");
        write_pdf(&path, &[LETTER]);
        assert!(render_pdf_page_png_sized(&path, 1, 60, 78).is_ok());
        shutdown_worker();
        assert!(render_pdf_page_png_sized(&path, 1, 60, 78).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A page of real content: text in a real font, so the encoder is tested on
    /// anti-aliased edges and not only on flat white.
    fn write_text_pdf(path: &Path) {
        let path = path.to_path_buf();
        run_on_pdfium(move |pdfium, _| {
            let mut doc = pdfium.create_new_pdf().unwrap();
            let font = doc.fonts_mut().times_roman();
            let mut page = doc
                .pages_mut()
                .create_page_at_end(PdfPagePaperSize::from_points(PdfPoints::new(612.0), PdfPoints::new(792.0)))
                .unwrap();
            for line in 0..30 {
                page.objects_mut()
                    .create_text_object(
                        PdfPoints::new(54.0),
                        PdfPoints::new(740.0 - line as f32 * 22.0),
                        format!("Line {line}: the quick brown fox jumps over the lazy dog, 0123456789."),
                        font,
                        PdfPoints::new(11.0),
                    )
                    .unwrap();
            }
            doc.save_to_file(&path).unwrap();
        })
        .unwrap();
    }

    /// The reader's hard requirement: speeding the encode up must not change a
    /// single pixel. Compares the decoded RGBA of the new encoder against the
    /// `image` crate's default one, on a real rendered text page and on a synthetic
    /// image with every alpha level and sharp edges.
    #[test]
    fn the_fast_png_decodes_to_the_same_pixels_as_the_default_png() {
        let dir = scratch_dir();
        let path = dir.join("text.pdf");
        write_text_pdf(&path);
        let rendered = run_on_pdfium({
            let path = path.clone();
            move |pdfium, cache| {
                render_bitmap(pdfium, cache, &path, 1, Target::Size(1224, 1584)).unwrap()
            }
        })
        .unwrap();
        assert!(
            rendered.to_rgba8().pixels().any(|p| p.0 != [255, 255, 255, 255]),
            "the text page came out blank"
        );

        let mut synthetic = image::RgbaImage::new(331, 257);
        let mut seed = 0x2545_F491u32;
        for (x, y, px) in synthetic.enumerate_pixels_mut() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = (seed >> 24) as u8;
            *px = image::Rgba([
                (x * 255 / 330) as u8,
                (y * 255 / 256) as u8,
                if (x / 8 + y / 8) % 2 == 0 { 0 } else { 255 },
                if x < 40 { noise } else { 255 - (y % 256) as u8 },
            ]);
        }

        for (name, img) in [
            ("rendered page", rendered),
            ("synthetic", DynamicImage::ImageRgba8(synthetic)),
        ] {
            let mut default_png = std::io::Cursor::new(Vec::new());
            img.write_to(&mut default_png, image::ImageFormat::Png).unwrap();
            let default_png = default_png.into_inner();
            let fast_png = encode_png(&img, 1).unwrap();

            let (old, new) = (decode_rgba(&default_png), decode_rgba(&fast_png));
            assert_eq!(new.dimensions(), (img.width(), img.height()), "{name}");
            assert_eq!(old.dimensions(), new.dimensions(), "{name}");
            assert!(old.as_raw() == new.as_raw(), "{name}: decoded pixels differ");
            assert!(old.as_raw() == img.to_rgba8().as_raw(), "{name}: not the source pixels");
            // Still an RGBA PNG, as before — the alpha channel is part of the output.
            assert_eq!(fast_png[25], 6, "{name}: colour type is not RGBA");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
