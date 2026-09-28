import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { RagSettings, VectorStoreInfo, PaperIndexEntry, PaperVectorizeInput, ChunkInput } from '../types'
import { buildChunks } from '../utils/chunker'
import { useLibraryStore } from './library'

export interface CollectionEmbedJob {
  done: number
  total: number
  failed: number
  status: 'running' | 'done'
}

// Papers currently being embedded by any job — prevents double work when two
// jobs overlap (a paper can be assigned to several collections).
const inFlightSlugs = new Set<string>()

/** 同步缺失 embeds only papers not yet vectorized; 完整重建 embeds every paper. */
export type VectorRebuildMode = 'full' | 'missing'

export interface VectorRebuildProgress {
  done: number
  total: number
  failed: number
}

/**
 * How the last 同步缺失 / 完整重建 run ended. Kept as data rather than a
 * translated string, so the settings panel words it in the current locale.
 */
export type VectorRebuildOutcome =
  | { kind: 'nothing'; mode: VectorRebuildMode }   // no papers to embed
  | { kind: 'paused'; done: number; total: number } // cancelled by the user
  | { kind: 'done'; done: number; total: number; failed: number }
  | { kind: 'error'; message: string }

/** Field-by-field, so key order and object identity do not count as a change. */
export function sameRagSettings(a: RagSettings, b: RagSettings): boolean {
  return (a.provider_id ?? null) === (b.provider_id ?? null)
    && (a.embedding_model ?? null) === (b.embedding_model ?? null)
    && a.chunk_size === b.chunk_size
    && a.chunk_overlap === b.chunk_overlap
    && a.top_k === b.top_k
    && a.enabled === b.enabled
}

export const useRagStore = defineStore('rag', () => {
  const settings = ref<RagSettings>({
    provider_id: null,
    embedding_model: null,
    chunk_size: 800,
    chunk_overlap: 100,
    top_k: 5,
    enabled: true,
  })
  const storeInfo = ref<VectorStoreInfo | null>(null)
  const loaded = ref(false)

  /** Per-collection embed progress, keyed by collection id. */
  const collectionEmbedJobs = ref<Record<string, CollectionEmbedJob>>({})

  // The settings panel's 同步缺失 / 完整重建 run. It lives here, not in the
  // panel, so closing the settings modal or switching its tab neither stops it
  // nor loses sight of it: a remounted panel reads the same state and shows
  // the progress and the 取消 button again. One run at a time, per window.
  const rebuilding = ref(false)
  const rebuildProgress = ref<VectorRebuildProgress>({ done: 0, total: 0, failed: 0 })
  const rebuildCurrentPaper = ref('')
  const rebuildOutcome = ref<VectorRebuildOutcome | null>(null)
  let rebuildCancelRequested = false

  const isConfigured = computed(
    () =>
      settings.value.enabled &&
      !!settings.value.provider_id &&
      settings.value.provider_id.length > 0 &&
      !!settings.value.embedding_model &&
      settings.value.embedding_model.length > 0
  )

  async function load() {
    try {
      const next = await invoke<RagSettings>('get_rag_settings')
      // A reload that finds nothing new keeps the current object: the settings
      // form copies every change of `settings` and auto-saves it, so a no-op
      // reload (another window saved, `rag-settings-changed`) must not look
      // like an edit.
      if (!loaded.value || !sameRagSettings(next, settings.value)) settings.value = next
      loaded.value = true
    } catch { /* no library open */ }
  }

  async function save(s: RagSettings) {
    // Snapshot before the await: what is written is what gets recorded as
    // saved. Copying `s` afterwards would also take in an edit made to the
    // form while the write was in flight, marking it saved when it was not.
    // Being a copy, it also keeps the form object and the store from aliasing.
    const snapshot: RagSettings = { ...s }
    await invoke('save_rag_settings', { settings: snapshot })
    settings.value = snapshot
  }

  async function loadStoreInfo() {
    try {
      storeInfo.value = await invoke<VectorStoreInfo>('get_vector_store_info')
    } catch { storeInfo.value = null }
  }

  function setCollectionJob(collectionId: string, patch: Partial<CollectionEmbedJob>) {
    const existing = collectionEmbedJobs.value[collectionId]
    collectionEmbedJobs.value = {
      ...collectionEmbedJobs.value,
      [collectionId]: {
        done: patch.done ?? existing?.done ?? 0,
        total: patch.total ?? existing?.total ?? 0,
        failed: patch.failed ?? existing?.failed ?? 0,
        status: patch.status ?? existing?.status ?? 'running',
      },
    }
  }

  function removeCollectionJob(collectionId: string) {
    const { [collectionId]: _removed, ...rest } = collectionEmbedJobs.value
    collectionEmbedJobs.value = rest
  }

  /**
   * Embed all not-yet-vectorized papers of a collection (papers come from the
   * caller, typically `collections.listAllPapersInTree`). Skips papers another
   * job is already embedding. Progress is exposed via `collectionEmbedJobs`.
   */
  async function embedCollection(collectionId: string, papers: PaperIndexEntry[]) {
    // Only block a truly in-progress job. A finished job lingers ~2s for UI
    // feedback; re-embedding during that window must be allowed.
    if (collectionEmbedJobs.value[collectionId]?.status === 'running') return
    // Claim the slot before any await so a rapid double-click can't start twice.
    setCollectionJob(collectionId, { done: 0, total: 0, failed: 0, status: 'running' })

    const library = useLibraryStore()
    const startPath = library.currentPath
    // Declared outside try{} so the finally cleanup can always reach them.
    const queue: PaperIndexEntry[] = []

    try {
      if (!loaded.value) await load()
      if (!isConfigured.value || !startPath) {
        removeCollectionJob(collectionId)
        return
      }

      const targets = papers.filter(
        p => !p.status.vectorized && !inFlightSlugs.has(p.slug)
      )

      // Nothing to do — flash a brief "all embedded" state so the click has feedback.
      if (targets.length === 0) {
        setCollectionJob(collectionId, { done: 0, total: 0, failed: 0, status: 'done' })
        setTimeout(() => removeCollectionJob(collectionId), 2000)
        return
      }

      for (const p of targets) inFlightSlugs.add(p.slug)
      queue.push(...targets)
      let done = 0, failed = 0
      setCollectionJob(collectionId, { done, total: targets.length, failed, status: 'running' })

      const s = settings.value
      // Small worker pool — embedding API latency dominates each paper.
      const CONCURRENCY = 3
      const workers = Array.from({ length: Math.min(CONCURRENCY, queue.length) }, async () => {
        // Stop dispatching when the user switches libraries: the remaining
        // slugs belong to the old library and must not hit the new one.
        while (library.currentPath === startPath) {
          const paper = queue.shift()
          if (!paper) break
          try {
            const input = await invoke<PaperVectorizeInput>('get_paper_vectorize_input', { slug: paper.slug })
            const chunks: ChunkInput[] = await buildChunks(input, s.chunk_size ?? 512, s.chunk_overlap ?? 50)
            if (chunks.length === 0) { failed++ } else {
              await invoke('embed_and_store_chunks', {
                slug: paper.slug, paperId: input.paper_id, paperTitle: input.paper_title, chunks,
              })
              paper.status.vectorized = true
              done++
            }
          } catch {
            failed++
          } finally {
            inFlightSlugs.delete(paper.slug)
          }
          setCollectionJob(collectionId, { done, failed })
        }
      })
      await Promise.all(workers)

      if (library.currentPath !== startPath) {
        removeCollectionJob(collectionId)
        return
      }

      setCollectionJob(collectionId, { done, failed, status: 'done' })
      loadStoreInfo().catch(() => {})
      setTimeout(() => removeCollectionJob(collectionId), failed > 0 ? 5000 : 2500)
    } finally {
      // Release any queued-but-unprocessed papers (early exit / library switch)
      // so a later job can still embed them…
      for (const p of queue) inFlightSlugs.delete(p.slug)
      // …and never leave a job stuck in 'running' (it would disable the menu
      // item and pin the progress badge forever).
      const finalStatus: string | undefined =
        collectionEmbedJobs.value[collectionId]?.status
      if (finalStatus === 'running') {
        removeCollectionJob(collectionId)
      }
    }
  }

  /**
   * Run 同步缺失 (`missing`: only papers not yet vectorized — resumes a paused
   * run and catches up new imports) or 完整重建 (`full`: every paper). Chunk
   * sizes are the panel form's, so an edit not auto-saved yet still applies.
   * Ignored while a run is in progress; state is in `rebuilding`,
   * `rebuildProgress`, `rebuildCurrentPaper` and `rebuildOutcome`.
   */
  async function rebuildVectors(
    mode: VectorRebuildMode,
    formChunkSize?: number | null,
    formChunkOverlap?: number | null,
  ) {
    // Claimed before any await, so a double click cannot start two runs.
    if (rebuilding.value) return
    rebuilding.value = true
    rebuildCancelRequested = false
    rebuildOutcome.value = null
    rebuildCurrentPaper.value = ''
    rebuildProgress.value = { done: 0, total: 0, failed: 0 }

    // The run now outlives the settings panel, so the library can be switched
    // under it (the collection jobs above guard the same way): the remaining
    // slugs belong to the old library and must not hit the new one.
    const library = useLibraryStore()
    const startPath = library.currentPath
    const libraryChanged = () => library.currentPath !== startPath

    try {
      // Re-derive each paper's `vectorized` flag from the vector store first: it
      // drifts (a model switch, a partition deleted), and "missing" trusts it. The
      // chat window used to do this before its own sync button; that button is
      // gone, so this is where it happens now.
      if (mode === 'missing') await invoke('sync_vectorized_flags').catch(() => {})
      const allPapers = await invoke<PaperIndexEntry[]>('list_papers')
      const papers = mode === 'missing'
        ? allPapers.filter(p => !p.status.vectorized)
        : allPapers

      const total = papers.length
      let done = 0, failed = 0
      rebuildProgress.value = { done, total, failed }

      if (total === 0) {
        rebuildOutcome.value = { kind: 'nothing', mode }
        return
      }

      const chunkSize: number = formChunkSize || 800
      const chunkOverlap: number = formChunkOverlap || 100

      // Small worker pool: the embedding API call dominates each paper's wall
      // time, so a few in-flight papers give a near-linear speedup.
      const CONCURRENCY = 3
      const queue = [...papers]
      const workers = Array.from({ length: Math.min(CONCURRENCY, queue.length) }, async () => {
        while (!rebuildCancelRequested && !libraryChanged()) {
          const paper = queue.shift()
          if (!paper) break
          rebuildCurrentPaper.value = paper.title

          try {
            const input = await invoke<PaperVectorizeInput>('get_paper_vectorize_input', { slug: paper.slug })
            const chunks: ChunkInput[] = await buildChunks(input, chunkSize, chunkOverlap)
            if (chunks.length === 0) { failed++; rebuildProgress.value = { done, total, failed }; continue }
            await invoke('embed_and_store_chunks', {
              slug: paper.slug,
              paperId: input.paper_id,
              paperTitle: input.paper_title,
              chunks,
            })
            done++
          } catch {
            failed++
          }

          rebuildProgress.value = { done, total, failed }
        }
      })
      await Promise.all(workers)

      // Counts from the old library mean nothing in the new one.
      if (libraryChanged()) return

      if (rebuildCancelRequested) {
        rebuildOutcome.value = { kind: 'paused', done, total }
      } else {
        rebuildOutcome.value = { kind: 'done', done, total, failed }
      }
      await loadStoreInfo()
    } catch (e) {
      rebuildOutcome.value = { kind: 'error', message: String(e) }
    } finally {
      rebuilding.value = false
      rebuildCurrentPaper.value = ''
    }
  }

  /** Each worker finishes the paper it is on and stops; nothing is half-written. */
  function cancelRebuild() {
    if (rebuilding.value) rebuildCancelRequested = true
  }

  return {
    settings,
    storeInfo,
    loaded,
    isConfigured,
    collectionEmbedJobs,
    rebuilding,
    rebuildProgress,
    rebuildCurrentPaper,
    rebuildOutcome,
    load,
    save,
    loadStoreInfo,
    embedCollection,
    rebuildVectors,
    cancelRebuild,
  }
})
