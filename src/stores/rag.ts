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
      settings.value = await invoke<RagSettings>('get_rag_settings')
      loaded.value = true
    } catch { /* no library open */ }
  }

  async function save(s: RagSettings) {
    await invoke('save_rag_settings', { settings: s })
    settings.value = s
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
    if (collectionEmbedJobs.value[collectionId]) return
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
      if (collectionEmbedJobs.value[collectionId]?.status === 'running') {
        removeCollectionJob(collectionId)
      }
    }
  }

  return {
    settings,
    storeInfo,
    loaded,
    isConfigured,
    collectionEmbedJobs,
    load,
    save,
    loadStoreInfo,
    embedCollection,
  }
})
