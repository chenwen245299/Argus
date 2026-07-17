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

export const useImportStore = defineStore('import', () => {
  const jobs = ref<ImportJob[]>([])
  const lastUrlError = ref<string | null>(null)

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
      job.id = newSlug
    }
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

  /**
   * Import a single ebook file path. The whole pipeline (parse → copy →
   * metadata from the book itself → sections → fulltext → index → rename)
   * runs inside the `import_ebook` command; no online metadata fetch.
   */
  async function importEbookFile(sourcePath: string, collectionId: string) {
    const filename = sourcePath.split(/[/\\]/).pop() ?? sourcePath
    const library = useLibraryStore()
    const selection = useSelectionStore()
    const collections = useCollectionsStore()

    const jobId = `ebook_${Date.now()}`
    _addJob(jobId, filename)
    try {
      const finalSlug = await invoke<string>('import_ebook', { sourcePath, collectionId })
      _updateSlug(jobId, finalSlug)
      _setStatus(finalSlug, 'done')
      await collections.load()
      await library.refresh()
      selection.selectPaper(finalSlug)
    } catch (e) {
      _setStatus(jobId, 'error', String(e))
    }
  }

  /**
   * Import a single PDF file path.
   * Pipeline: copy → move to collection → fetch_metadata → rename → extract.
   * A collectionId is required so every imported paper belongs to a category.
   * Ebook paths are routed to their own (fully backend-side) pipeline.
   */
  async function importFile(sourcePath: string, collectionId: string) {
    if (!collectionId || !canImportIntoCollection(collectionId)) {
      return
    }

    if (EBOOK_EXT_RE.test(sourcePath)) {
      await importEbookFile(sourcePath, collectionId)
      return
    }

    const filename = sourcePath.split(/[/\\]/).pop() ?? sourcePath
    const library = useLibraryStore()
    const selection = useSelectionStore()
    const collections = useCollectionsStore()

    // 1. Copy PDF + create scaffold
    let tempSlug: string
    try {
      tempSlug = await invoke<string>('import_pdf', { sourcePath })
    } catch (e) {
      const phantom = `err_${Date.now()}`
      _addJob(phantom, filename)
      _setStatus(phantom, 'error', String(e))
      return
    }

    _addJob(tempSlug, filename)
    // Immediately refresh so the paper appears in the list. With the trailing
    // refresh guard in the library store this awaits a scan that includes the
    // just-imported paper.
    await library.refresh()

    // Locate the freshly-imported paper's id. It should be present after the
    // refresh above; fall back to invoke in case the scan raced the copy.
    let tempPaperId = library.papers.find(p => p.slug === tempSlug)?.id ?? null
    if (!tempPaperId) {
      try {
        const entry = await invoke<PaperMeta>('get_paper_meta', { slug: tempSlug })
        tempPaperId = entry?.id ?? null
      } catch {
        // Fallback unavailable — skip the move; the paper still imported fine.
      }
    }
    if (tempPaperId) {
      await collections.movePaper(tempPaperId, collectionId)
    }

    // The paper now exists in the library and is selectable. Everything else —
    // AI metadata, Semantic Scholar venue/references, easyScholar ranking — runs
    // asynchronously in the background so the import UI never blocks.
    _setStatus(tempSlug, 'queued')
    selection.selectPaper(tempSlug)
    enqueueMetadata(tempSlug)
  }

  // ── Background metadata pipeline ──────────────────────────────────────────
  // Papers are processed one at a time (a shared FIFO queue) so we never fire a
  // burst of Semantic Scholar / easyScholar requests in parallel and trip their
  // rate limits, even when many files are imported at once.
  // `extract` = run fulltext + AI-metadata extraction + rename (the steps that
  // derive metadata from the file itself). URL imports already arrive with
  // source metadata and a canonical folder name, so they only need the online
  // enrichment (Semantic Scholar venue/references + easyScholar rank).
  interface MetaOpts { extract: boolean }
  interface MetaJob { slug: string; opts: MetaOpts }
  const metaQueue: MetaJob[] = []
  let metaRunning = false

  function enqueueMetadata(slug: string, opts: Partial<MetaOpts> = {}) {
    metaQueue.push({ slug, opts: { extract: true, ...opts } })
    if (!metaRunning) void runMetaWorker()
  }

  async function runMetaWorker() {
    metaRunning = true
    try {
      while (metaQueue.length) {
        const job = metaQueue.shift()!
        await runMetadataPipeline(job.slug, job.opts)
      }
    } finally {
      metaRunning = false
    }
  }

  // Order (per the paper's lifecycle): fulltext → AI metadata → rename →
  // Semantic Scholar (venue + references) → easyScholar (venue rank, cache-aware).
  // Every step is best-effort; a failure never aborts the rest.
  async function runMetadataPipeline(slug: string, opts: MetaOpts) {
    const library = useLibraryStore()
    const readerStore = useReaderStore()
    const selection = useSelectionStore()
    const ranks = useRanksStore()
    const settings = useSettingsStore()

    let cur = slug

    if (opts.extract) {
      // 0. Fulltext + search index (prerequisite for AI metadata extraction).
      _setStatus(cur, 'extracting')
      try {
        await invoke('extract_fulltext', { slug: cur })
        await invoke('index_paper_search', { slug: cur })
        window.dispatchEvent(new CustomEvent('argus-paper-fulltext-updated', { detail: { slug: cur } }))
      } catch { /* non-fatal */ }

      // 1. AI metadata (title/authors/year/venue from the PDF). When AI is not
      //    configured or fails, fall back to the id/title-based online lookup so
      //    non-AI setups still get metadata.
      _setStatus(cur, 'ai_meta')
      let aiOk = false
      try {
        const meta = await invoke<PaperMeta>('extract_metadata_ai', { slug: cur })
        window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: cur, meta } }))
        aiOk = true
      } catch { /* AI unavailable / no fulltext / error */ }
      if (!aiOk) {
        try {
          const meta = await invoke<PaperMeta>('fetch_metadata', { slug: cur })
          window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: cur, meta } }))
        } catch { /* non-fatal */ }
      }

      // 2. Rename the folder to the canonical title-based slug.
      _setStatus(cur, 'renaming')
      try {
        const newSlug = await invoke<string>('rename_paper_folder', { slug: cur })
        if (newSlug !== cur) {
          readerStore.replacePaperSlug(cur, newSlug)
          if (selection.selectedSlug === cur) selection.selectPaper(newSlug)
          _updateSlug(cur, newSlug)
          cur = newSlug
        }
      } catch { /* non-fatal */ }
    }

    // 3. Semantic Scholar — backfill venue/DOI/citation count, then references.
    _setStatus(cur, 'fetching_meta')
    let meta: PaperMeta | null = null
    try {
      meta = await invoke<PaperMeta>('fetch_citation_count', { slug: cur })
      window.dispatchEvent(new CustomEvent('argus-paper-meta-updated', { detail: { slug: cur, meta } }))
    } catch { /* non-fatal */ }
    _setStatus(cur, 'fetching_refs')
    try {
      await invoke('fetch_references', { slug: cur })
      window.dispatchEvent(new CustomEvent('argus-references-updated', { detail: { slug: cur } }))
    } catch { /* non-fatal */ }

    // 4. easyScholar venue ranking. `ranks.fetchRank` reuses the library-wide
    //    venue cache, so a venue that's already been looked up costs no API call.
    _setStatus(cur, 'ranking')
    try {
      if (!settings.loaded) await settings.load()
      if (settings.easyscholarConfigured) {
        if (!meta) meta = await invoke<PaperMeta>('get_paper_meta', { slug: cur })
        const venue = meta?.venue?.trim()
        if (venue) await ranks.fetchRank(venue)
      }
    } catch { /* non-fatal */ }

    _setStatus(cur, 'done')
    await library.refresh()
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
    const selection = useSelectionStore()
    const collections = useCollectionsStore()

    const jobId = `url_${Date.now()}`
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
      const finalSlug = await invoke<string>('import_paper_url', {
        url: url.trim(),
        collectionId,
      })

      // Update job with real slug
      const j = jobs.value.find(j => j.id === jobId)
      if (j) {
        j.slug = finalSlug
        j.id = finalSlug
        j.status = 'queued'
      }

      // Reload collections so the new assignment is visible in the collection view
      await collections.load()
      await library.refresh()
      selection.selectPaper(finalSlug)

      // Backfill online enrichment (Semantic Scholar venue/references +
      // easyScholar rank) in the background. The URL import already produced
      // source metadata and a canonical folder, so skip the file-extraction steps.
      enqueueMetadata(finalSlug, { extract: false })
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

  /** Import multiple files, one at a time (to avoid API rate limiting). */
  async function importFiles(paths: string[], collectionId: string) {
    if (!collectionId || !canImportIntoCollection(collectionId)) {
      return
    }

    for (const p of paths) {
      await importFile(p, collectionId)
    }
  }

  function clearDone() {
    jobs.value = jobs.value.filter(j => j.status !== 'done')
  }

  const activeCount = computed(() => jobs.value.filter(j => j.status !== 'done' && j.status !== 'error').length)

  function clearUrlError() { lastUrlError.value = null }

  return { jobs, importFile, importFiles, importPaperUrl, clearDone, activeCount, lastUrlError, clearUrlError }
})
