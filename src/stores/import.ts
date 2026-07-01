import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { ImportJob, PaperMeta } from '../types'
import { useLibraryStore } from './library'
import { useReaderStore } from './reader'
import { useSelectionStore } from './selection'
import { useCollectionsStore } from './collections'

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

  /**
   * Import a single PDF file path.
   * Pipeline: copy → move to collection → fetch_metadata → rename → extract.
   * A collectionId is required so every imported paper belongs to a category.
   */
  async function importFile(sourcePath: string, collectionId: string) {
    if (!collectionId) {
      console.warn('Import blocked: no collection selected')
      return
    }

    const filename = sourcePath.split(/[/\\]/).pop() ?? sourcePath
    const library = useLibraryStore()
    const reader = useReaderStore()
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

    // 2. Fetch metadata (async, non-blocking)
    _setStatus(tempSlug, 'fetching_meta')
    let updatedMeta: PaperMeta | null = null
    try {
      updatedMeta = await invoke<PaperMeta>('fetch_metadata', { slug: tempSlug })
    } catch {
      // Network/API failure is non-fatal — keep temp slug with filename title
    }

    // 3. Rename folder if we got metadata
    let finalSlug = tempSlug
    _setStatus(tempSlug, 'renaming')
    try {
      const newSlug = await invoke<string>('rename_paper_folder', { slug: tempSlug })
      if (newSlug !== tempSlug) {
        // Update all reader tabs + selection pointing at the old slug
        reader.replacePaperSlug(tempSlug, newSlug, updatedMeta?.title)
        if (selection.selectedSlug === tempSlug) {
          selection.selectPaper(newSlug)
        }
        _updateSlug(tempSlug, newSlug)
        finalSlug = newSlug
      }
    } catch {
      // Rename failure is non-fatal
    }

    // 4. Extract fulltext (best-effort).
    try {
      await invoke('extract_fulltext', { slug: finalSlug })
      await invoke('index_paper_search', { slug: finalSlug })
    } catch {
      // Extraction failure is non-fatal for imports.
    }

    _setStatus(finalSlug, 'done')
    await library.refresh()

    // Auto-select the newly imported paper
    selection.selectPaper(finalSlug)
  }

  /**
   * Import a paper by URL (arXiv, OpenReview, etc.).
   * Routes to the appropriate handler based on URL pattern.
   */
  async function importPaperUrl(url: string, collectionId: string) {
    if (!collectionId) {
      console.warn('URL import blocked: no collection selected')
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
        j.status = 'done'
      }

      // Reload collections so the new assignment is visible in the collection view
      await collections.load()
      await library.refresh()
      selection.selectPaper(finalSlug)
    } catch (e) {
      const j = jobs.value.find(j => j.id === jobId)
      if (j) {
        j.status = 'error'
        j.error = String(e)
      }
      lastUrlError.value = String(e)
    }
  }

  /** Import multiple files, one at a time (to avoid API rate limiting). */
  async function importFiles(paths: string[], collectionId: string) {
    if (!collectionId) {
      console.warn('Import blocked: no collection selected')
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
