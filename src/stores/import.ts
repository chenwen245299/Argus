import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { ImportJob, PaperMeta } from '../types'
import { useLibraryStore } from './library'
import { useReaderStore } from './reader'
import { useSelectionStore } from './selection'
import { useCollectionsStore } from './collections'
import { useRanksStore } from './ranks'
import { useSettingsStore } from './settings'
import { i18n } from '../i18n'
import { confirmDuplicateImport } from '../utils/duplicateImport'

/** A paper already in the library that matches the one being imported. */
interface DuplicateHit { existingSlug: string; title: string; incomingId?: string }
/** Result of a duplicate-aware import command (import_paper_url). */
interface ImportOutcome { status: 'imported' | 'duplicate'; slug?: string; existingSlug?: string; title?: string }

/** A duplicate that stopped an import, surfaced in the toolbar until dismissed. */
export interface DuplicateNotice {
  /** Title of the paper already in the library. */
  title: string
  /** Where to jump to see it; empty when the backend couldn't say. */
  existingSlug: string
  /** What the user tried to import (filename or URL). */
  source: string
}

export const useImportStore = defineStore('import', () => {
  const jobs = ref<ImportJob[]>([])
  const lastUrlError = ref<string | null>(null)
  const lastDuplicate = ref<DuplicateNotice | null>(null)

  function checkRoot(root: string | null) {
    if (useLibraryStore().currentPath !== root) throw new Error(i18n.global.t('import.libraryChanged'))
  }

  function scopedInvoke<T>(root: string | null, command: string, args: Record<string, unknown>): Promise<T> {
    checkRoot(root)
    return invoke<T>(command, { ...args, expectedRoot: root })
  }

  // Copying a batch must not wait for a full-library scan after every file.
  // Refresh both lists together, coalescing completions that land in one burst.
  let refreshTimer: ReturnType<typeof setTimeout> | undefined
  let refreshRoot: string | null = null
  function scheduleRefresh(root: string | null) {
    if (useLibraryStore().currentPath !== root) return
    refreshRoot = root
    if (refreshTimer) return
    refreshTimer = setTimeout(() => {
      refreshTimer = undefined
      if (useLibraryStore().currentPath !== refreshRoot) return
      void Promise.allSettled([useCollectionsStore().load(), useLibraryStore().refresh()])
    }, 150)
  }

  function _addJob(slug: string, filename: string): ImportJob {
    const job: ImportJob = {
      id: slug,
      slug,
      filename,
      status: 'importing',
    }
    jobs.value.push(job)
    return job
  }

  function _setStatus(slug: string, status: ImportJob['status'], error?: string) {
    const job = jobs.value.find(j => j.slug === slug)
    if (job) {
      job.status = status
      if (error) job.error = error
    }
  }

  function _updateSlug(oldSlug: string, newSlug: string) {
    const job = jobs.value.find(j => j.slug === oldSlug)
    if (job) {
      job.slug = newSlug
    }
  }

  /**
   * Only a user-declined duplicate is removed. Whatever was already written for the
   * half-imported paper — its folder, search-index entry, vector chunks and
   * collection membership — is removed by `delete_paper`, so a rejected import
   * leaves nothing behind, and the user is told which paper it matched.
   */
  async function _abortAsDuplicate(slug: string, source: string, dup: DuplicateHit, root: string | null) {
    const selection = useSelectionStore()
    _setStatus(slug, 'skipping')
    try {
      await scopedInvoke(root, 'delete_paper', { slug, ...(dup.incomingId ? { expectedId: dup.incomingId } : {}) })
    } catch {
      _setStatus(slug, 'error', i18n.global.t('import.duplicateCleanupFailed'))
      return
    }
    _setStatus(slug, 'skipped')
    if (useLibraryStore().currentPath !== root) return
    lastDuplicate.value = { title: dup.title, existingSlug: dup.existingSlug, source }
    // Only follow the user's focus: during a batch import the selection must not
    // jump around for every rejected file.
    if (selection.selectedSlug === slug && dup.existingSlug) selection.selectPaper(dup.existingSlug)
    scheduleRefresh(root)
  }

  const EBOOK_EXT_RE = /\.(epub|mobi|azw3|azw|fb2|txt|zip)$/i

  function canImportIntoCollection(collectionId: string): boolean {
    const collections = useCollectionsStore()
    if (!collections.canReceivePapers(collectionId)) {
      console.warn('Import blocked: select a sub-collection')
      return false
    }
    return true
  }

  // Local file persistence is independent of AI and duplicate decisions.
  interface LocalImportTask {
    sourcePath: string
    collectionId: string
    root: string | null
    jobId: string
    resolve: () => void
  }
  const fileQueue: LocalImportTask[] = []
  let fileWorkers = 0
  const FILE_CONCURRENCY = 2
  const copyWaiters: (() => void)[] = []
  const pendingDuplicates = new Map<string | null, Set<string>>()
  let duplicateWork: Promise<void> = Promise.resolve()

  function pendingFor(root: string | null) {
    let pending = pendingDuplicates.get(root)
    if (!pending) pendingDuplicates.set(root, pending = new Set())
    return pending
  }

  function waitForCopies() {
    return fileWorkers ? new Promise<void>(resolve => copyWaiters.push(resolve)) : Promise.resolve()
  }

  function checkDuplicateAndFinish(slug: string, source: string, root: string | null, finish: () => Promise<void>) {
    // Decisions (including cleanup) are ordered, while parsing/copying proceeds.
    // An undecided copy cannot be another file's "original": skipping both
    // sides of such a match used to remove every copy of a new same-title book.
    const work = duplicateWork.then(async () => {
      let dup: DuplicateHit | null
      do {
        await waitForCopies()
        dup = await scopedInvoke<DuplicateHit | null>(root, 'find_duplicate_paper', {
          slug, excludeSlugs: [...pendingFor(root)],
        })
      } while (fileWorkers)
      if (dup) {
        _setStatus(slug, 'awaiting_duplicate')
        const keep = await confirmDuplicateImport(dup.title, source)
        checkRoot(root)
        if (!keep) {
          await _abortAsDuplicate(slug, source, dup, root)
          pendingFor(root).delete(slug)
          return
        }
      }
      await finish()
      pendingFor(root).delete(slug)
    })
    duplicateWork = work.catch(() => {})
    return work
  }

  async function runFileWorker() {
    fileWorkers++
    try {
      while (fileQueue.length) {
        const task = fileQueue.shift()!
        const { sourcePath, collectionId, root, jobId } = task
        let cur = jobId
        let deferred = false
        try {
          _setStatus(cur, 'importing')
          const ebook = EBOOK_EXT_RE.test(sourcePath)
          const slug = await scopedInvoke<string>(root, ebook ? 'import_ebook' : 'import_pdf', { sourcePath, collectionId })
          _updateSlug(cur, slug)
          cur = slug
          pendingFor(root).add(slug)
          checkRoot(root)
          scheduleRefresh(root)
          useSelectionStore().selectPaper(slug)
          if (ebook) {
            // Releasing the copy/parse slot before asking is essential: pending
            // duplicate choices must not prevent the remaining books importing.
            deferred = true
            _setStatus(slug, 'queued')
            enqueueMetadata(slug, true, root, true, task.resolve)
          } else {
            _setStatus(slug, 'queued')
            enqueueMetadata(slug, true, root)
          }
        } catch (e) {
          _setStatus(cur, 'error', String(e))
        } finally {
          if (!deferred) task.resolve()
        }
      }
    } finally {
      fileWorkers--
      if (!fileWorkers) copyWaiters.splice(0).forEach(resolve => resolve())
    }
  }

  /**
   * Register a PDF/ebook, then copy and assign it before background recognition.
   * A collectionId is required so every imported paper belongs to a category.
   * Ebooks are parsed locally; PDF fulltext/OCR follows identification.
   */
  function importFile(sourcePath: string, collectionId: string): Promise<void> {
    if (!collectionId || !canImportIntoCollection(collectionId)) {
      return Promise.resolve()
    }
    const filename = sourcePath.split(/[/\\]/).pop() ?? sourcePath
    const jobId = `file_${crypto.randomUUID()}`
    _addJob(jobId, filename)
    _setStatus(jobId, 'queued')
    const done = new Promise<void>(resolve => fileQueue.push({ sourcePath, collectionId,
      root: useLibraryStore().currentPath, jobId, resolve }))
    if (fileWorkers < FILE_CONCURRENCY) void runFileWorker()
    return done
  }

  // ── Background metadata pipeline ──────────────────────────────────────────
  // Two metadata workers and a serial enrichment queue keep slow citation/OCR
  // work out of the identification path. Only papers receive scholarly enrichment.
  const metaQueue: { slug: string; checkDuplicate: boolean; root: string | null; ebook: boolean; settled?: () => void }[] = []
  let metaWorkers = 0
  const enrichmentQueue: { slug: string; root: string | null; isPaper: boolean }[] = []
  let enrichmentRunning = false

  function enqueueMetadata(slug: string, checkDuplicate = false, root = useLibraryStore().currentPath, ebook = false, settled?: () => void) {
    if (checkDuplicate) pendingFor(root).add(slug)
    metaQueue.push({ slug, checkDuplicate, root, ebook, settled })
    if (metaWorkers < 2) void runMetaWorker()
  }

  async function runMetaWorker() {
    metaWorkers++
    try {
      while (metaQueue.length) {
        const { slug, checkDuplicate, root, ebook, settled } = metaQueue.shift()!
        try {
          await runMetadataPipeline(slug, checkDuplicate, root, ebook, settled)
        } catch (e) {
          _setStatus(slug, 'error', String(e))
          settled?.()
        }
      }
    } finally {
      metaWorkers--
    }
  }

  // Opening pages → metadata → duplicate choice → rename → background fulltext
  // and paper-only enrichment. Optional failures retain the copied document.
  async function runMetadataPipeline(slug: string, checkDuplicate: boolean, root: string | null, ebook: boolean, settled?: () => void) {
    checkRoot(root)
    const cur = slug

    // Identify from five opening pages (ten only when needed). Full-book OCR
    // must never delay recognizing the rest of a batch, or run for skipped PDFs.
    _setStatus(cur, 'ai_meta')
    let aiOk = false
    let isPaper = false
    try {
      const result = await scopedInvoke<{ meta: PaperMeta; isPaper: boolean; isBook: boolean }>(root, 'extract_import_metadata', { slug: cur })
      checkRoot(root)
      const meta = result.meta
      isPaper = result.isPaper
      window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: cur, meta } }))
      aiOk = true
    } catch (e) {
      const job = jobs.value.find(j => j.slug === cur)
      if (job) job.warning = i18n.global.t('import.metadataRetained', { error: String(e) })
    }
    if (!aiOk && !ebook) {
      try {
        // A loose title search can replace a book's name with an unrelated
        // journal article. Fallback accepts identifiers printed in the PDF only.
        const meta = await scopedInvoke<PaperMeta>(root, 'fetch_metadata', { slug: cur, allowTitleSearch: false })
        checkRoot(root)
        isPaper = !!(meta.doi || meta.arxiv_id)
        window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: cur, meta } }))
      } catch { /* non-fatal */ }
    }

    // 1.5 Duplicate check, now that the title and authors are known: this is the
    //     first point where a local PDF can be matched against the library at
    //     all, and it also catches URL imports whose scraped title was wrong and
    //     only came out right after AI extraction. The user can keep an
    //     independent copy or skip it — see `_abortAsDuplicate`.
    if (checkDuplicate) {
      const source = jobs.value.find(j => j.slug === cur)?.filename ?? cur
      _setStatus(cur, 'awaiting_duplicate')
      void checkDuplicateAndFinish(cur, source, root, () => finishMetadata(cur, root, isPaper, ebook))
        .catch(e => _setStatus(cur, 'error', String(e)))
        .finally(() => settled?.())
      return
    }
    await finishMetadata(cur, root, isPaper, ebook)
    settled?.()
  }

  async function finishMetadata(slug: string, root: string | null, isPaper: boolean, ebook: boolean) {
    checkRoot(root)
    const readerStore = useReaderStore()
    const selection = useSelectionStore()
    let cur = slug

    // 2. Rename the folder to the canonical title-based slug.
    _setStatus(cur, 'renaming')
    try {
      const newSlug = await scopedInvoke<string>(root, 'rename_paper_folder', { slug: cur })
      checkRoot(root)
      if (newSlug !== cur) {
        readerStore.replacePaperSlug(cur, newSlug)
        if (selection.selectedSlug === cur) selection.selectPaper(newSlug)
        _updateSlug(cur, newSlug)
        cur = newSlug
      }
    } catch { /* non-fatal */ }

    checkRoot(root)
    scheduleRefresh(root)
    if (ebook) {
      _setStatus(cur, 'done')
      return
    }
    _setStatus(cur, 'queued')
    enrichmentQueue.push({ slug: cur, root, isPaper })
    if (!enrichmentRunning) void runEnrichmentWorker()
  }

  async function runEnrichmentWorker() {
    enrichmentRunning = true
    try {
      while (enrichmentQueue.length) {
        const { slug, root, isPaper } = enrichmentQueue.shift()!
        try {
          await enrichPaper(slug, root, isPaper)
        } catch (e) {
          _setStatus(slug, 'error', String(e))
        }
      }
    } finally {
      enrichmentRunning = false
    }
  }

  async function enrichPaper(cur: string, root: string | null, isPaper: boolean) {
    checkRoot(root)
    const ranks = useRanksStore()
    const settings = useSettingsStore()

    _setStatus(cur, 'extracting')
    try {
      await scopedInvoke(root, 'extract_fulltext', { slug: cur })
      checkRoot(root)
      window.dispatchEvent(new CustomEvent('argus-paper-fulltext-updated', { detail: { slug: cur } }))
    } catch (e) {
      const job = jobs.value.find(j => j.slug === cur)
      if (job) job.warning = [job.warning, i18n.global.t('import.fulltextIncomplete', { error: String(e) })].filter(Boolean).join('\n')
    }
    checkRoot(root)
    if (!isPaper) {
      _setStatus(cur, 'done')
      scheduleRefresh(root)
      return
    }

    // 3. Semantic Scholar — backfill venue/DOI/citation count, then references.
    _setStatus(cur, 'fetching_meta')
    let meta: PaperMeta | null = null
    try {
      meta = await scopedInvoke<PaperMeta>(root, 'fetch_citation_count', { slug: cur })
      checkRoot(root)
      window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: cur, meta } }))
    } catch { /* non-fatal */ }
    _setStatus(cur, 'fetching_refs')
    try {
      await scopedInvoke(root, 'fetch_references', { slug: cur })
      checkRoot(root)
      window.dispatchEvent(new CustomEvent('argus-references-updated', { detail: { slug: cur } }))
    } catch { /* non-fatal */ }

    // 4. easyScholar venue ranking. `ranks.fetchRank` reuses the library-wide
    //    venue cache, so a venue that's already been looked up costs no API call.
    _setStatus(cur, 'ranking')
    try {
      if (!settings.loaded) await settings.load()
      checkRoot(root)
      if (settings.easyscholarConfigured) {
        if (!meta) meta = await scopedInvoke<PaperMeta>(root, 'get_paper_meta', { slug: cur })
        checkRoot(root)
        const venue = meta?.venue?.trim()
        if (venue) await ranks.fetchRank(venue)
      }
    } catch { /* non-fatal */ }

    checkRoot(root)
    _setStatus(cur, 'done')
    scheduleRefresh(root)
  }

  /**
   * Import a paper by URL (arXiv, OpenReview, etc.).
   * Routes to the appropriate handler based on URL pattern.
   */
  async function importPaperUrl(url: string, collectionId: string) {
    if (!collectionId || !canImportIntoCollection(collectionId)) {
      return
    }

    const library = useLibraryStore()
    const root = library.currentPath
    const selection = useSelectionStore()
    const collections = useCollectionsStore()

    const jobId = `url_${crypto.randomUUID()}`
    const job: ImportJob = {
      id: jobId,
      slug: jobId,
      filename: url.trim(),
      status: 'downloading',
    }
    jobs.value.push(job)

    // The backend emits granular progress ("fetching" → "downloading" →
    // "importing") while it scrapes the source page and downloads the PDF — the
    // longest part of a URL import. Reflect it on the job so the toolbar shows a
    // live status throughout, rather than a single static "downloading". The
    // import button is disabled while any job is active, so this is the only
    // import in flight and every event belongs to it.
    const backendStatus: Record<string, ImportJob['status']> = {
      fetching: 'downloading',
      downloading: 'downloading',
      importing: 'importing',
    }
    const onBackendProgress = (e: { payload?: { status?: string } }) => {
      if (library.currentPath !== root) return
      const mapped = e.payload?.status ? backendStatus[e.payload.status] : undefined
      if (!mapped) return
      const j = jobs.value.find(x => x.id === jobId)
      if (j) j.status = mapped
    }
    const unlisten: UnlistenFn[] = []
    try {
      unlisten.push(await listen('arxiv-url-import', onBackendProgress))
      unlisten.push(await listen('paper-url-import', onBackendProgress))
    } catch { /* progress events are best-effort */ }

    try {
      let res = await scopedInvoke<ImportOutcome>(root, 'import_paper_url', {
        url: url.trim(),
        collectionId,
      })

      // The backend has not written a duplicate yet. Retry with force only
      // after the user opts into a new, independent copy.
      let acceptedDuplicate = false
      if (res.status === 'duplicate') {
        _setStatus(jobId, 'awaiting_duplicate')
        if (await confirmDuplicateImport(res.title ?? url.trim(), url.trim())) {
          acceptedDuplicate = true
          res = await scopedInvoke<ImportOutcome>(root, 'import_paper_url', { url: url.trim(), collectionId, force: true })
          if (res.status !== 'imported' || !res.slug) throw new Error('Duplicate import did not create a separate paper')
        } else {
          checkRoot(root)
          _setStatus(jobId, 'skipped')
          lastDuplicate.value = {
            title: res.title ?? url.trim(),
            existingSlug: res.existingSlug ?? '',
            source: url.trim(),
          }
          await library.refresh()
          checkRoot(root)
          if (res.existingSlug) selection.selectPaper(res.existingSlug)
          return
        }
      }

      if (res.status !== 'imported' || !res.slug) throw new Error('Import did not return a paper identity')
      const finalSlug = res.slug
      checkRoot(root)

      // Update job with real slug
      const j = jobs.value.find(j => j.id === jobId)
      if (j) {
        j.slug = finalSlug
        j.status = 'queued'
      }

      // Reload collections so the new assignment is visible in the collection view
      await collections.load()
      await library.refresh()
      checkRoot(root)
      selection.selectPaper(finalSlug)

      // Run the full metadata pipeline in the background — the same AI-metadata
      // extraction plus Semantic Scholar / easyScholar enrichment as a local PDF
      // import, so link imports end up with identical metadata coverage. The
      // duplicate check runs again unless the user already accepted a copy:
      // AI extraction may correct a scraped title into one that does match.
      enqueueMetadata(finalSlug, !acceptedDuplicate, root)
    } catch (e) {
      const j = jobs.value.find(j => j.id === jobId)
      if (j) {
        j.status = 'error'
        j.error = String(e)
      }
      lastUrlError.value = String(e)
    } finally {
      unlisten.forEach(u => u())
    }
  }

  /**
   * Run the full metadata pipeline for a paper that was added to the library
   * out-of-band (e.g. from the arXiv recommendations window). Registers a job so
   * the toolbar shows live progress, then runs the same extract + enrich pipeline
   * as a local PDF import. Idempotent: a slug already being processed is ignored.
   */
  function processAddedPaper(slug: string, filename?: string) {
    if (jobs.value.some(j => j.slug === slug)) return
    _addJob(slug, filename ?? slug)
    _setStatus(slug, 'queued')
    enqueueMetadata(slug)
  }

  /** Register the entire batch immediately; local IO uses a bounded shared pool. */
  async function importFiles(paths: string[], collectionId: string) {
    if (!collectionId || !canImportIntoCollection(collectionId)) {
      return
    }

    await Promise.allSettled(paths.map(p => importFile(p, collectionId)))
  }

  function clearDone() {
    jobs.value = jobs.value.filter(j => !['done', 'skipped', 'error'].includes(j.status))
  }

  const activeCount = computed(() => jobs.value.filter(j => !['done', 'skipped', 'error'].includes(j.status)).length)

  function clearUrlError() { lastUrlError.value = null }
  function clearDuplicateNotice() { lastDuplicate.value = null }

  return { jobs, importFile, importFiles, importPaperUrl, processAddedPaper, clearDone, activeCount, lastUrlError, clearUrlError, lastDuplicate, clearDuplicateNotice }
})
