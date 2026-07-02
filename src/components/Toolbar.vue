<script setup lang="ts">
import { ref, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { useI18n } from 'vue-i18n'
import { storeToRefs } from 'pinia'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useLibraryStore } from '../stores/library'
import { useImportStore } from '../stores/import'
import { useSelectionStore } from '../stores/selection'
import { usePaperTasksStore, type AiSummaryJob } from '../stores/paperTasks'
import { useCollectionsStore } from '../stores/collections'
import { useRagStore } from '../stores/rag'
import { titleInitialCaps } from '../utils/text'
import type { SearchHit, Note, PaperStatus } from '../types'
import TokenUsageModal from './TokenUsageModal.vue'

const { t } = useI18n()
const library = useLibraryStore()
const importStore = useImportStore()
const selection = useSelectionStore()
const paperTasks = usePaperTasksStore()
const collectionsStore = useCollectionsStore()
const ragStore = useRagStore()

// Aggregate embed-vector progress across all running collection jobs so the
// toolbar shows a single status chip while embeddings are being built.
const embedJobAgg = computed(() => {
  const jobs = Object.values(ragStore.collectionEmbedJobs)
  if (!jobs.length) return null
  let done = 0, total = 0, failed = 0, running = false
  for (const j of jobs) {
    done += j.done
    total += j.total
    failed += j.failed
    if (j.status === 'running') running = true
  }
  return { done, total, failed, running }
})

const embedJobText = computed(() => {
  const a = embedJobAgg.value
  if (!a) return ''
  const processed = a.done + a.failed
  if (a.running) return t('toolbar.embedBuilding', { done: processed, total: a.total })
  if (a.total === 0) return t('toolbar.embedUpToDate')
  if (a.failed > 0) return t('toolbar.embedDoneFailed', { done: a.done, total: a.total, failed: a.failed })
  return t('toolbar.embedDone', { done: a.done, total: a.total })
})
const { aiSummaryJobs, aiMetaSlug, aiMetaStage, abstractSlug } = storeToRefs(paperTasks)

const props = defineProps<{
  leftSidebarWidth?: number
  rightSidebarOpen?: boolean
  rightSidebarWidth?: number
  sidebarTab?: string
  canvasMode?: boolean
}>()

const emit = defineEmits<{
  'toggle-right-sidebar': []
  'update:sidebarTab': [tab: string]
}>()

type SidebarTabDef = { id: string; paths: string[]; label: string }

const drawTabDef = computed((): SidebarTabDef => ({
  id: 'draw',
  label: t('toolbarTabs.draw'),
  // pen / edit icon
  paths: ['M12 19l7-7 3 3-7 7-3-3z', 'M18 13l-1.5-7.5L2 2l3.5 14.5L13 18l5-5z', 'M2 2l7.586 7.586', 'M11 11a2 2 0 1 0 4 0 2 2 0 0 0-4 0z'],
}))

const sidebarTabs = computed((): SidebarTabDef[] => {
  // In canvas/graph mode, hide PDF-only tabs (translations/highlights) and
  // surface the drawing properties tab to the left of notes.
  if (props.canvasMode) {
    return baseSidebarTabs.value.filter(t => t.id === 'notes' || t.id === 'ai' || t.id === 'metadata')
      .reduce<SidebarTabDef[]>((acc, t) => {
        if (t.id === 'notes') acc.push(drawTabDef.value)
        acc.push(t)
        return acc
      }, [])
  }
  return baseSidebarTabs.value
})

const baseSidebarTabs = computed((): SidebarTabDef[] => [
  {
    id: 'translations',
    label: t('toolbarTabs.translations'),
    paths: ['m5 8 6 6', 'm4 14 6-6 2-3', 'M2 5h12', 'M7 2h1', 'm22 22-5-10-5 10', 'M14 18h6'],
  },
  {
    id: 'notes',
    label: t('toolbarTabs.notes'),
    paths: [
      'M4 4.5A2.5 2.5 0 0 1 6.5 2H18a2 2 0 0 1 2 2v17H6.5A2.5 2.5 0 0 0 4 18.5z',
      'M8 6h8', 'M8 10h7',
    ],
  },
  {
    id: 'highlights',
    label: t('toolbarTabs.highlights'),
    paths: ['m15 5 4 4-8.5 8.5H6.5l-.5-4z', 'm13 7 4 4', 'M4 21h12'],
  },
  {
    id: 'ai',
    label: t('toolbarTabs.ai'),
    paths: [
      'M12 3 13.7 8.3 19 10l-5.3 1.7L12 17l-1.7-5.3L5 10l5.3-1.7z',
      'M19 15v4', 'M17 17h4',
    ],
  },
  {
    id: 'metadata',
    label: t('toolbarTabs.metadata'),
    paths: ['M12 22a10 10 0 1 0 0-20 10 10 0 0 0 0 20z', 'M12 16v-4', 'M12 8h.01'],
  },
])

const searchQuery = ref('')
let searchTimer: ReturnType<typeof setTimeout> | null = null

function shortPath(p: string): string {
  const parts = p.replace(/\\/g, '/').split('/')
  if (parts.length <= 3) return p
  return '…/' + parts.slice(-2).join('/')
}

async function pickAndImport() {
  const collectionId = selection.activeCollectionId
  if (!library.currentPath || !collectionId) return
  try {
    const paths = await invoke<string[]>('pick_pdf_files')
    if (paths.length > 0) {
      importStore.importFiles(paths, collectionId)
    }
  } catch (e) {
    console.error('Import pick failed:', e)
  }
}

const activeJobs = computed(() => importStore.activeCount)
const canImport = computed(() => !!library.currentPath && !!selection.activeCollectionId)
const importTitle = computed(() => canImport.value ? t('import.btnTitle') : t('import.selectCollectionTitle'))
const leftReserveStyle = computed(() => ({
  width: `${Math.max(140, props.leftSidebarWidth ?? 200)}px`,
}))
const rightReserveStyle = computed(() => ({
  width: props.rightSidebarOpen
    ? `${Math.max(40, (props.rightSidebarWidth ?? 300) + 1)}px`
    : '50px',
}))

// ── Import dropdown menu ─────────────────────────────────────────────────────
const showImportMenu = ref(false)

function toggleImportMenu() {
  if (showImportMenu.value) {
    showImportMenu.value = false
    return
  }
  showUrlPopover.value = false
  showAiMenu.value = false
  showImportMenu.value = true
}

// ── AI hub dropdown menu (arXiv + library chat) ─────────────────────────────
const showAiMenu = ref(false)

function toggleAiMenu() {
  if (showAiMenu.value) {
    showAiMenu.value = false
    return
  }
  showImportMenu.value = false
  showUrlPopover.value = false
  showAiMenu.value = true
}

function chooseArxiv() {
  showAiMenu.value = false
  openArxiv()
}

function chooseLibraryChat() {
  showAiMenu.value = false
  openLibraryChat()
}

function chooseEmbeddingMap() {
  showAiMenu.value = false
  invoke('open_embedding_map_window').catch((e) => console.error('Open embedding map window:', e))
}

function chooseUsage() {
  showAiMenu.value = false
  showUsage.value = true
}

function chooseFileImport() {
  showImportMenu.value = false
  pickAndImport()
}

function chooseUrlImport() {
  showImportMenu.value = false
  openUrlPopover()
}

// ── URL import popover ───────────────────────────────────────────────────────
const showUrlPopover = ref(false)
const urlInput = ref('')
const urlImportError = ref('')

function openUrlPopover() {
  showUrlPopover.value = true
  urlInput.value = ''
  urlImportError.value = ''
  nextTick(() => {
    const el = document.querySelector('.popover-input') as HTMLInputElement | null
    el?.focus()
  })
}

function closeUrlPopover() {
  showUrlPopover.value = false
}

function onDocClick(e: MouseEvent) {
  if (showUrlPopover.value || showImportMenu.value) {
    const wrap = document.querySelector('.import-wrap')
    if (wrap && !wrap.contains(e.target as Node)) {
      closeUrlPopover()
      showImportMenu.value = false
    }
  }
  if (showAiMenu.value) {
    const wrap = document.querySelector('.ai-hub-wrap')
    if (wrap && !wrap.contains(e.target as Node)) showAiMenu.value = false
  }
  if (showBatchDetail.value) {
    const strip = document.querySelector('.batch-progress-strip')
    if (strip && !strip.contains(e.target as Node)) showBatchDetail.value = false
  }
  if (showPaperTaskDetail.value) {
    const strip = document.querySelector('.paper-task-strip')
    if (strip && !strip.contains(e.target as Node)) showPaperTaskDetail.value = false
  }
}

function submitUrl() {
  const url = urlInput.value.trim()
  if (!url) return
  const collectionId = selection.activeCollectionId
  if (!collectionId) {
    urlImportError.value = t('import.selectCollectionFirst')
    return
  }
  importStore.clearUrlError()
  importStore.importPaperUrl(url, collectionId)
  closeUrlPopover()
}

const importStatusLabel = computed(() => {
  const jobs = importStore.jobs
  if (!jobs.length) return ''
  const active = jobs.filter(j => j.status !== 'done' && j.status !== 'error')
  if (active.length) {
    const first = active[0]
    if (first.status === 'downloading') return t('import.downloading')
    if (first.status === 'importing') return t('import.importing')
    if (first.status === 'fetching_meta') return t('import.fetchingMeta')
    if (first.status === 'renaming') return t('import.renaming')
  }
  return ''
})

function onSearchInput() {
  if (searchTimer) clearTimeout(searchTimer)
  const q = searchQuery.value.trim()
  if (!q) {
    selection.clearSearch()
    return
  }
  searchTimer = setTimeout(() => runSearch(q), 300)
}

async function runSearch(q: string) {
  if (!library.currentPath) return
  try {
    const hits = await invoke<SearchHit[]>('search_fulltext', { query: q })
    selection.setSearchResults(q, hits)
  } catch (e) {
    console.error('Search failed:', e)
  }
}

function clearSearch() {
  searchQuery.value = ''
  selection.clearSearch()
}

// ── Batch AI analysis ────────────────────────────────────────────────────────
const batchRunning = ref(false)
const batchStopping = ref(false) // queue cleared, waiting for in-flight to finish
const batchDone = ref(0)
const batchTotal = ref(0)
const showBatchDetail = ref(false)
let batchCancelled = false

watch(batchRunning, (running) => {
  if (!running) showBatchDetail.value = false
})

async function refreshSinglePaperStatus(slug: string) {
  try {
    const status = await invoke<PaperStatus>('get_paper_status', { slug })
    const idx = library.papers.findIndex(p => p.slug === slug)
    if (idx >= 0) {
      library.papers[idx] = { ...library.papers[idx], status }
    }
  } catch (e) {
    console.error('[batch] failed to refresh paper status:', e)
  }
}

async function analyzeOnePaper(slug: string): Promise<void> {
  if (batchCancelled) return
  paperTasks.setAiSummaryJob(slug, { kind: 'summary', stage: 'queued', generatedChars: 0, message: undefined })

  const eventSafeSlug = slug.replace(/[^A-Za-z0-9:_/-]/g, '-')
  const unlistenStream = await listen<{ delta?: string; done?: boolean }>(`ai-summary-${eventSafeSlug}`, (ev) => {
    const job = aiSummaryJobs.value[slug]
    if (!job) return
    if (ev.payload.delta) {
      paperTasks.setAiSummaryJob(slug, {
        stage: 'ai',
        generatedChars: job.generatedChars + ev.payload.delta.length,
      })
    }
    if (ev.payload.done && job.stage === 'ai') {
      paperTasks.setAiSummaryJob(slug, { stage: 'saving' })
    }
  })

  try {
    await invoke<Note>('generate_summary', { slug, providerId: null, modelId: null })
    paperTasks.setAiSummaryJob(slug, { stage: 'done' })
    await refreshSinglePaperStatus(slug)
    window.dispatchEvent(new CustomEvent('argus-notes-updated', { detail: { slug } }))
  } catch (e: unknown) {
    paperTasks.setAiSummaryJob(slug, { stage: 'error', message: String(e) })
  } finally {
    unlistenStream()
    const delay = aiSummaryJobs.value[slug]?.stage === 'error' ? 5000 : 1800
    setTimeout(() => {
      const job = aiSummaryJobs.value[slug]
      if (job?.stage === 'done' || job?.stage === 'error') paperTasks.removeAiSummaryJob(slug)
    }, delay)
  }
}

async function startBatchAnalysis() {
  if (batchRunning.value) {
    if (!batchStopping.value) {
      batchCancelled = true
      batchStopping.value = true
    }
    return
  }

  // Get papers for current view. Use the recursive tree walk (same as PaperList)
  // so a top-level category picks up papers held only in its sub-collections —
  // list_papers_in_collection returns only directly-assigned papers and would
  // yield nothing for a parent category, aborting the batch before it starts.
  const collId = selection.activeCollectionId
  let papers = collId
    ? collectionsStore.listAllPapersInTree(collId)
    : [...library.papers]

  // Apply tag filter consistent with PaperList
  const nav = selection.activeNav
  const activeTag = selection.tagFilter ?? (nav.startsWith('tag:') ? nav.slice(4) : null)
  if (activeTag) papers = papers.filter(p => p.tags?.includes(activeTag))

  const toAnalyze = papers.filter(p => !p.status.ai_summary_done && !paperTasks.isAiSummaryActive(p.slug))
  if (toAnalyze.length === 0) return

  batchRunning.value = true
  batchStopping.value = false
  batchCancelled = false
  batchDone.value = 0
  batchTotal.value = toAnalyze.length

  const CONCURRENCY = 5
  const queue = [...toAnalyze]
  let active = 0

  await new Promise<void>((resolve) => {
    function drain() {
      if (active === 0 && (queue.length === 0 || batchCancelled)) { resolve(); return }
      while (active < CONCURRENCY && queue.length > 0 && !batchCancelled) {
        const paper = queue.shift()!
        active++
        analyzeOnePaper(paper.slug).finally(() => {
          active--
          batchDone.value++
          drain()
        })
      }
    }
    drain()
  })

  batchRunning.value = false
  batchStopping.value = false
  batchCancelled = false
  await library.refresh()
}

// Token usage modal
const showUsage = ref(false)

interface PaperTaskStatusItem {
  id: string
  label: string
  detail?: string
  active?: boolean
  stage: string
}

function paperTaskTitle(slug: string) {
  return titleInitialCaps(library.papers.find(p => p.slug === slug)?.title ?? slug)
}

function formatSummaryCount(n?: number) {
  if (!n) return ''
  if (n >= 10000) return `${Math.round(n / 1000)}k`
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return String(n)
}

function aiMetaStageLabel(): string {
  switch (aiMetaStage.value) {
    case 'text': return t('paper.extractMetaAiStageText')
    case 'ai':   return t('paper.extractMetaAiStageAi')
    default:     return t('paper.extractMetaAiIng')
  }
}

function aiSummaryStageLabel(job?: AiSummaryJob): string {
  if (!job) return t('paper.summarizeAiIng')
  const isExtract = job.kind === 'extract'
  switch (job.stage) {
    case 'queued': return isExtract ? t('extraction.extracting') : t('paper.summaryStageQueued')
    case 'fulltext': return isExtract ? t('extraction.stageFulltext') : t('paper.summaryStageFulltext')
    case 'text': return isExtract ? t('extraction.stageText') : t('paper.summaryStageText')
    case 'fulltext_ready':
    case 'preparing': {
      const count = formatSummaryCount(job.fulltextChars)
      if (isExtract) {
        return count
          ? t('extraction.stageDoneWithCount', { count })
          : t('extraction.done')
      }
      return count
        ? t('paper.summaryStagePreparingWithCount', { count })
        : t('paper.summaryStagePreparing')
    }
    case 'ai': {
      const count = formatSummaryCount(job.generatedChars)
      return count
        ? t('paper.summaryStageGeneratingWithCount', { count })
        : t('paper.summaryStageAi')
    }
    case 'saving': return isExtract ? t('extraction.stageIndexing') : t('paper.summaryStageSaving')
    case 'done': return isExtract ? t('extraction.done') : t('paper.summaryStageDone')
    case 'error': return isExtract ? t('extraction.failed') : t('paper.summaryStageError')
    default: return t('paper.summarizeAiIng')
  }
}

function aiSummaryDetail(slug: string, job: AiSummaryJob): string {
  const parts = [aiSummaryStageLabel(job)]
  if (job.provider || job.model) parts.push([job.provider, job.model].filter(Boolean).join(' / '))
  if (job.fulltextChars) parts.push(t('paper.summaryFulltextChars', { count: formatSummaryCount(job.fulltextChars) }))
  if (job.contextChars) parts.push(t('paper.summaryContextChars', { count: formatSummaryCount(job.contextChars) }))
  if (job.truncated) parts.push(t('paper.summaryTruncated'))
  if (job.message) parts.push(job.message)
  return `${paperTaskTitle(slug)}\n${parts.filter(Boolean).join('\n')}`
}

const paperTaskItems = computed<PaperTaskStatusItem[]>(() => {
  const items = Object.entries(aiSummaryJobs.value).map(([slug, job]) => {
    const action = job.kind === 'extract' ? t('extraction.reExtract') : t('paper.summarizeAi')
    const stage = aiSummaryStageLabel(job)
    return {
      id: `${job.kind}:${slug}`,
      label: `${action} · ${stage}`,
      detail: aiSummaryDetail(slug, job),
      active: paperTasks.isAiSummaryActive(slug),
      stage,
    }
  })

  if (aiMetaSlug.value) {
    const stage = aiMetaStageLabel()
    items.push({
      id: `meta:${aiMetaSlug.value}`,
      label: `${t('paper.extractMetaAi')} · ${stage}`,
      detail: paperTaskTitle(aiMetaSlug.value),
      active: true,
      stage,
    })
  }

  if (abstractSlug.value) {
    const stage = t('paper.extractAbstractAiIng')
    items.push({
      id: `abstract:${abstractSlug.value}`,
      label: `${t('paper.extractAbstractAi')} · ${stage}`,
      detail: paperTaskTitle(abstractSlug.value),
      active: true,
      stage,
    })
  }

  return items
})
// When several manual tasks run at once, merge them into one chip (like batch)
// instead of one badge per task, which eats toolbar space.
const paperTaskActiveCount = computed(() => paperTaskItems.value.filter(i => i.active).length)
const paperTaskDoneCount = computed(() => paperTaskItems.value.length - paperTaskActiveCount.value)
const showPaperTaskDetail = ref(false)
// Collapse the detail popover once we're back to fewer than 2 tasks.
watch(() => paperTaskItems.value.length, len => { if (len < 2) showPaperTaskDetail.value = false })

// arXiv button state
const arxivNewCount = ref(0)
const arxivWindowOpen = ref(false)
const arxivAnalyzing = ref(false)
const arxivFetching = ref(false)
const arxivProgress = ref({ done: 0, total: 0 })
let unlistenArxiv: UnlistenFn | null = null
let unlistenArxivAnalysis: UnlistenFn | null = null
let unlistenArxivFetch: UnlistenFn | null = null
let unlistenArxivWinOpen: UnlistenFn | null = null
let unlistenArxivWinClose: UnlistenFn | null = null
let statusPollTimer: ReturnType<typeof setInterval> | null = null

// RAG embed progress state
const ragEmbedSyncing = ref(false)
const ragEmbedProgress = ref({ done: 0, total: 0 })
let unlistenRagEmbed: UnlistenFn | null = null

const arxivBusy = computed(() => arxivAnalyzing.value || arxivFetching.value)

// AI hub button: flips between its name and the active task's progress
const aiBusy = computed(() => arxivBusy.value || ragEmbedSyncing.value)
const aiLabelMode = ref<'name' | 'progress'>('name')
let aiLabelToggleTimer: ReturnType<typeof setInterval> | null = null

const aiProgressText = computed(() => {
  if (arxivAnalyzing.value && arxivProgress.value.total > 0) {
    return t('toolbar.arxivProgress', { done: arxivProgress.value.done, total: arxivProgress.value.total })
  }
  if (ragEmbedSyncing.value && ragEmbedProgress.value.total > 0) {
    return t('toolbar.ragProgress', { done: ragEmbedProgress.value.done, total: ragEmbedProgress.value.total })
  }
  return ''
})

const aiShowName = computed(() => aiLabelMode.value === 'name' || !aiProgressText.value)

watch(aiBusy, (busy) => {
  if (busy) {
    if (!aiLabelToggleTimer) {
      aiLabelMode.value = 'name'
      aiLabelToggleTimer = setInterval(() => {
        aiLabelMode.value = aiLabelMode.value === 'name' ? 'progress' : 'name'
      }, 2200)
    }
  } else {
    if (aiLabelToggleTimer) { clearInterval(aiLabelToggleTimer); aiLabelToggleTimer = null }
    aiLabelMode.value = 'name'
  }
})

async function openArxiv() {
  try {
    await invoke('open_arxiv_window')
    arxivWindowOpen.value = true
    arxivNewCount.value = 0
  } catch (e) {
    console.error('Open arXiv window:', e)
  }
}

async function openLibraryChat() {
  try {
    await invoke('open_library_chat_window')
  } catch (e) {
    console.error('Open library chat window:', e)
  }
}

async function syncArxivStatus() {
  try {
    const s = await invoke<{ analyzing: boolean; fetching: boolean; analyzed_count: number; total_pending: number }>(
      'get_arxiv_schedule_status'
    )
    arxivAnalyzing.value = s.analyzing
    arxivFetching.value = s.fetching
    if (s.analyzing) {
      arxivProgress.value = { done: s.analyzed_count, total: s.analyzed_count + s.total_pending }
    }
  } catch { /* no library open yet */ }
}

onMounted(async () => {
  unlistenArxiv = await listen<{ count: number }>('arxiv-new-recommendations', (e) => {
    arxivNewCount.value = e.payload.count
  })
  unlistenArxivAnalysis = await listen<{
    status: string; done: number; total: number; bulk?: boolean
  }>('arxiv-analysis', (e) => {
    const { status, done, total } = e.payload
    // Only update bulk-analysis indicator for bulk events (total > 1 or explicit bulk flag)
    // Single-paper events (total === 1) should not reset bulk analysis state
    const isBulk = total > 1 || e.payload.bulk === true
    if (!isBulk) return

    if (total > 0 || status === 'started' || status === 'finished') {
      arxivProgress.value = { done, total }
    }

    if (status === 'finished' || status === 'error') {
      arxivAnalyzing.value = false
    } else {
      arxivAnalyzing.value = true
    }
  })
  unlistenArxivFetch = await listen<{ status: string }>('arxiv-fetch', (e) => {
    arxivFetching.value = e.payload.status === 'fetching'
  })
  paperTasks.startListening()
  document.addEventListener('pointerdown', onDocClick, true)
  unlistenRagEmbed = await listen<{ syncing: boolean; done: number; total: number }>('rag-embed-progress', (e) => {
    ragEmbedSyncing.value = e.payload.syncing
    ragEmbedProgress.value = { done: e.payload.done, total: e.payload.total }
  })
  unlistenArxivWinOpen = await listen('arxiv-window-opened', () => {
    arxivWindowOpen.value = true
  })
  unlistenArxivWinClose = await listen('arxiv-window-closed', () => {
    arxivWindowOpen.value = false
  })
  // Sync current analysis state immediately on mount (handles missed events)
  await syncArxivStatus()
  // Poll every 5 s so the indicator stays correct even if events were missed
  statusPollTimer = setInterval(syncArxivStatus, 5000)
})

onUnmounted(() => {
  if (unlistenArxiv) unlistenArxiv()
  if (unlistenArxivAnalysis) unlistenArxivAnalysis()
  if (unlistenArxivFetch) unlistenArxivFetch()
  if (unlistenRagEmbed) unlistenRagEmbed()
  if (unlistenArxivWinOpen) unlistenArxivWinOpen()
  if (unlistenArxivWinClose) unlistenArxivWinClose()
  if (aiLabelToggleTimer) { clearInterval(aiLabelToggleTimer); aiLabelToggleTimer = null }
  if (statusPollTimer) { clearInterval(statusPollTimer); statusPollTimer = null }
  if (searchTimer) { clearTimeout(searchTimer); searchTimer = null }
  document.removeEventListener('pointerdown', onDocClick, true)
})
</script>

<template>
  <div class="toolbar">
    <div class="left-toolbar-reserve" :style="leftReserveStyle">
      <!-- Left: vault picker button -->
      <button class="lib-path-btn" @click="library.pickAndOpen()" :title="t('toolbar.switchTitle')">
        <!-- Home icon -->
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" class="vault-icon">
          <path d="M3 9l9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>
          <polyline points="9 22 9 12 15 12 15 22"/>
        </svg>
        <span class="path-text">
          {{ library.currentPath ? shortPath(library.currentPath) : t('toolbar.noLibrary') }}
        </span>
        <span v-if="library.isRefreshing" class="scan-dot" title="正在同步…" />
        <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="chevron-icon">
          <polyline points="6 9 12 15 18 9"/>
        </svg>
      </button>
    </div>

    <!-- Center: shrinkable section — clips horizontally so the right reserve
         (and its border) stays aligned with the sidebar below on narrow windows -->
    <div class="toolbar-center">

    <!-- Center: search -->
    <div v-if="library.currentPath" class="search-box">
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" class="search-icon">
        <circle cx="11" cy="11" r="8"/>
        <line x1="21" y1="21" x2="16.65" y2="16.65"/>
      </svg>
      <input
        v-model="searchQuery"
        class="search-input"
        :placeholder="t('search.placeholder')"
        @input="onSearchInput"
        @keydown.escape="clearSearch"
      />
      <button v-if="searchQuery" class="search-clear" @click="clearSearch">
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3">
          <line x1="18" y1="6" x2="6" y2="18"/>
          <line x1="6" y1="6" x2="18" y2="18"/>
        </svg>
      </button>
    </div>
    <div v-else class="search-placeholder" />

    <div class="spacer" />

    <!-- Batch analysis progress chip + detail popover -->
    <div v-if="batchRunning" class="batch-progress-strip">
      <span
        class="paper-task-chip is-active batch-chip-clickable"
        @click.stop="showBatchDetail = !showBatchDetail"
      >
        <span class="paper-task-spinner" />
        <span class="paper-task-label">{{ batchStopping ? t('toolbar.batchStopping') : t('toolbar.batchAnalysisRunning') }} {{ batchDone }}/{{ batchTotal }}</span>
        <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round">
          <polyline :points="showBatchDetail ? '18 15 12 9 6 15' : '6 9 12 15 18 9'"/>
        </svg>
      </span>

      <Transition name="batch-detail">
        <div v-if="showBatchDetail" class="batch-detail-popover" @click.stop>
          <div class="batch-detail-header">
            <span class="batch-detail-title-text">{{ t('toolbar.batchAnalysisRunning') }}</span>
            <span class="batch-detail-count">{{ batchDone }}/{{ batchTotal }}</span>
          </div>
          <div class="batch-detail-list">
            <div
              v-for="item in paperTaskItems"
              :key="item.id"
              class="batch-detail-item"
            >
              <span v-if="item.active" class="paper-task-spinner batch-item-spinner" />
              <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                <polyline points="20 6 9 17 4 12"/>
              </svg>
              <span class="batch-item-name">{{ item.detail?.split('\n')[0] ?? item.id }}</span>
              <span class="batch-item-stage">{{ item.stage }}</span>
            </div>
            <div v-if="!paperTaskItems.length" class="batch-detail-empty">正在准备…</div>
          </div>
        </div>
      </Transition>
    </div>

    <!-- Manual AI tasks: single chip when one, merged chip + popover when many -->
    <div v-if="paperTaskItems.length && !batchRunning" class="paper-task-strip">
      <!-- Single task → plain chip -->
      <span
        v-if="paperTaskItems.length === 1"
        :key="paperTaskItems[0].id"
        class="paper-task-chip"
        :class="{ 'is-active': paperTaskItems[0].active }"
        :title="paperTaskItems[0].detail || paperTaskItems[0].label"
      >
        <span v-if="paperTaskItems[0].active" class="paper-task-spinner" />
        <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12"/>
        </svg>
        <span class="paper-task-label">{{ paperTaskItems[0].label }}</span>
      </span>

      <!-- Multiple tasks → one merged chip with a detail popover -->
      <template v-else>
        <span
          class="paper-task-chip batch-chip-clickable"
          :class="{ 'is-active': paperTaskActiveCount > 0 }"
          @click.stop="showPaperTaskDetail = !showPaperTaskDetail"
        >
          <span v-if="paperTaskActiveCount > 0" class="paper-task-spinner" />
          <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="20 6 9 17 4 12"/>
          </svg>
          <span class="paper-task-label">{{ t('paper.summarizeAi') }} {{ paperTaskDoneCount }}/{{ paperTaskItems.length }}</span>
          <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.8" stroke-linecap="round">
            <polyline :points="showPaperTaskDetail ? '18 15 12 9 6 15' : '6 9 12 15 18 9'"/>
          </svg>
        </span>

        <Transition name="batch-detail">
          <div v-if="showPaperTaskDetail" class="batch-detail-popover" @click.stop>
            <div class="batch-detail-header">
              <span class="batch-detail-title-text">{{ t('paper.summarizeAi') }}</span>
              <span class="batch-detail-count">{{ paperTaskDoneCount }}/{{ paperTaskItems.length }}</span>
            </div>
            <div class="batch-detail-list">
              <div
                v-for="item in paperTaskItems"
                :key="item.id"
                class="batch-detail-item"
              >
                <span v-if="item.active" class="paper-task-spinner batch-item-spinner" />
                <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <polyline points="20 6 9 17 4 12"/>
                </svg>
                <span class="batch-item-name">{{ item.detail?.split('\n')[0] ?? item.id }}</span>
                <span class="batch-item-stage">{{ item.stage }}</span>
              </div>
            </div>
          </div>
        </Transition>
      </template>
    </div>

    <!-- Embed-vector build progress (started from a collection's context menu) -->
    <div v-if="embedJobAgg" class="embed-progress-strip">
      <span class="paper-task-chip" :class="{ 'is-active': embedJobAgg.running }">
        <span v-if="embedJobAgg.running" class="paper-task-spinner" />
        <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12"/>
        </svg>
        <span class="paper-task-label">{{ embedJobText }}</span>
      </span>
    </div>

    <!-- Import status -->
    <div v-if="importStatusLabel" class="import-status">
      <span class="spinner" />
      <span>{{ importStatusLabel }}</span>
    </div>

    <!-- Import error toast -->
    <div
      v-if="importStore.lastUrlError"
      class="import-error-toast"
      :title="importStore.lastUrlError"
      @click="importStore.clearUrlError()"
    >
      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <circle cx="12" cy="12" r="10"/><line x1="12" y1="8" x2="12" y2="12"/><line x1="12" y1="16" x2="12.01" y2="16"/>
      </svg>
      <span class="import-error-text">导入失败：{{ importStore.lastUrlError }}</span>
      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
        <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
      </svg>
    </div>

    <!-- Import (file / URL) -->
    <div v-if="library.currentPath" class="import-wrap">
      <button
        class="btn-outline"
        :disabled="activeJobs > 0 || !canImport"
        :title="importTitle"
        @click="toggleImportMenu"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
          <polyline points="14 2 14 8 20 8"/>
          <line x1="12" y1="18" x2="12" y2="12"/>
          <polyline points="9 15 12 18 15 15"/>
        </svg>
        {{ t('import.btn') }}
        <svg class="import-caret" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
          <polyline points="6 9 12 15 18 9"/>
        </svg>
      </button>

      <!-- Dropdown: choose import method -->
      <Transition name="popover">
        <div v-if="showImportMenu" class="import-menu">
          <button class="import-menu-item" :title="t('import.btnTitle')" @click="chooseFileImport">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
              <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
              <polyline points="14 2 14 8 20 8"/>
              <line x1="12" y1="18" x2="12" y2="12"/>
              <polyline points="9 15 12 18 15 15"/>
            </svg>
            {{ t('import.fileImportBtn') }}
          </button>
          <button class="import-menu-item" :title="t('import.urlImportTitle')" @click="chooseUrlImport">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
              <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>
              <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>
            </svg>
            {{ t('import.urlImportBtn') }}
          </button>
        </div>
      </Transition>

      <!-- URL import popover -->
      <Transition name="popover">
        <div v-if="showUrlPopover" class="url-import-popover" @keydown.escape="closeUrlPopover">
          <div class="popover-header">
            <span class="popover-title">{{ t('import.urlImportTitle') }}</span>
            <button class="popover-close" @click="closeUrlPopover">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
            </button>
          </div>
          <input
            v-model="urlInput"
            class="popover-input"
            :placeholder="t('import.urlImportPlaceholder')"
            @keydown.enter="submitUrl"
          />
          <p v-if="urlImportError" class="popover-error">{{ urlImportError }}</p>
          <div class="popover-actions">
            <button class="btn-ghost-sm" @click="closeUrlPopover">{{ t('import.cancel') }}</button>
            <button
              class="btn-accent-sm"
              :disabled="!urlInput.trim()"
              @click="submitUrl"
            >
              {{ t('import.urlImportConfirm') }}
            </button>
          </div>
        </div>
      </Transition>
    </div>

    <!-- Divider -->
    <div class="tb-sep" />

    <!-- AI hub: arXiv recommendations + library chat -->
    <div v-if="library.currentPath" class="ai-hub-wrap">
      <button
        class="tb-btn ai-hub-btn rainbow-chip"
        :class="{ 'ai-busy': aiBusy }"
        :title="t('toolbar.aiHubTitle')"
        @click="toggleAiMenu"
      >
        <span v-if="aiBusy" class="arxiv-pulse-dot" />
        <!-- Both states stay rendered (stacked) so the button width never
             changes while flipping between name and progress -->
        <span class="ai-flip-stack">
          <span class="arxiv-label-inner ai-flip-item" :class="{ 'is-on': aiShowName }">
            <!-- GitHub Copilot (octicon) -->
            <svg width="13" height="13" viewBox="0 0 16 16" fill="currentColor">
              <path d="M7.998 15.035c-4.562 0-7.873-2.914-7.998-3.749V9.338c.085-.628.677-1.686 1.588-2.065.013-.07.024-.143.036-.218.029-.183.06-.384.126-.612-.201-.508-.254-1.084-.254-1.656 0-.87.128-1.769.693-2.484.579-.733 1.494-1.124 2.724-1.261 1.206-.134 2.262.034 2.944.765.05.053.096.108.139.165.044-.057.094-.112.143-.165.682-.731 1.738-.899 2.944-.765 1.23.137 2.145.528 2.724 1.261.566.715.693 1.614.693 2.484 0 .572-.053 1.148-.254 1.656.066.228.098.429.126.612.012.076.024.148.037.218.924.385 1.522 1.471 1.591 2.095v1.872c0 .766-3.351 3.795-8.002 3.795Zm0-1.485c2.28 0 4.584-1.11 5.002-1.433V7.862l-.023-.116c-.49.21-1.075.291-1.727.291-1.146 0-2.059-.327-2.71-.991A3.222 3.222 0 0 1 8 6.303a3.24 3.24 0 0 1-.544.743c-.65.664-1.563.991-2.71.991-.652 0-1.236-.081-1.727-.291l-.023.116v4.255c.419.323 2.722 1.433 5.002 1.433ZM6.762 2.83c-.193-.206-.637-.413-1.682-.297-1.019.113-1.479.404-1.713.7-.247.312-.369.789-.369 1.554 0 .793.129 1.171.308 1.371.162.181.519.379 1.442.379.853 0 1.339-.235 1.638-.54.315-.322.527-.827.617-1.553.117-.935-.037-1.395-.241-1.614Zm4.155-.297c-1.044-.116-1.488.091-1.681.297-.204.219-.359.679-.242 1.614.091.726.303 1.231.618 1.553.299.305.784.54 1.638.54.922 0 1.28-.198 1.442-.379.179-.2.308-.578.308-1.371 0-.765-.123-1.242-.37-1.554-.233-.296-.693-.587-1.713-.7Z"/>
              <path d="M6.25 9.037a.75.75 0 0 1 .75.75v1.501a.75.75 0 0 1-1.5 0V9.787a.75.75 0 0 1 .75-.75Zm4.25.75v1.501a.75.75 0 0 1-1.5 0V9.787a.75.75 0 0 1 1.5 0Z"/>
            </svg>
            <span class="rainbow-chip-label">{{ t('toolbar.aiHub') }}</span>
          </span>
          <span class="arxiv-label-inner arxiv-progress-text ai-flip-item" :class="{ 'is-on': !aiShowName }">
            {{ aiProgressText }}
          </span>
        </span>
        <svg class="import-caret" width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
          <polyline points="6 9 12 15 18 9"/>
        </svg>
        <span v-if="!arxivWindowOpen && arxivNewCount > 0" class="arxiv-badge">{{ arxivNewCount }}</span>
      </button>

      <!-- Dropdown: arXiv / library chat -->
      <Transition name="popover">
        <div v-if="showAiMenu" class="import-menu ai-hub-menu">
          <button class="import-menu-item" :title="t('toolbar.arxivTitle')" @click="chooseArxiv">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M12 2L2 7l10 5 10-5-10-5z"/>
              <path d="M2 17l10 5 10-5"/>
              <path d="M2 12l10 5 10-5"/>
            </svg>
            {{ t('toolbar.arxivMenuItem') }}
            <span v-if="!arxivWindowOpen && arxivNewCount > 0" class="menu-badge">{{ arxivNewCount }}</span>
            <span v-else-if="arxivAnalyzing && arxivProgress.total > 0" class="menu-meta">{{ arxivProgress.done }}/{{ arxivProgress.total }}</span>
          </button>
          <button class="import-menu-item" :title="t('toolbar.libraryChatTitle')" @click="chooseLibraryChat">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"/>
              <path d="M8 9h8"/>
              <path d="M8 13h5"/>
            </svg>
            {{ t('toolbar.libraryChat') }}
            <span v-if="ragEmbedSyncing && ragEmbedProgress.total > 0" class="menu-meta">{{ ragEmbedProgress.done }}/{{ ragEmbedProgress.total }}</span>
          </button>
          <button class="import-menu-item" :title="t('toolbar.embeddingMapTitle')" @click="chooseEmbeddingMap">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
              <circle cx="5.5" cy="6" r="2.6"/>
              <circle cx="18" cy="5" r="2.2"/>
              <circle cx="12" cy="13.5" r="2.8"/>
              <circle cx="6" cy="19" r="2"/>
              <circle cx="19" cy="18" r="2.4"/>
              <path d="M7.8 7.5 10 11.4M14.6 12.2 16.4 6.9M10 15.6 7.3 17.5M14.5 15 17 16.6"/>
            </svg>
            {{ t('toolbar.embeddingMap') }}
          </button>
          <div class="menu-divider" />
          <button class="import-menu-item" :title="t('toolbar.aiUsageTitle')" @click="chooseUsage">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <ellipse cx="12" cy="6" rx="7" ry="3"/>
              <path d="M5 6v5c0 1.7 3.1 3 7 3s7-1.3 7-3V6"/>
              <path d="M5 11v5c0 1.7 3.1 3 7 3 1.3 0 2.5-.1 3.5-.4"/>
              <path d="M18.5 14.5l.5 1.2 1.2.5-1.2.5-.5 1.2-.5-1.2-1.2-.5 1.2-.5.5-1.2z"/>
            </svg>
            {{ t('toolbar.aiUsage') }}
          </button>
        </div>
      </Transition>
    </div>

    <div v-if="library.currentPath" class="tb-sep global-feature-sep" />

    <!-- Batch analysis button -->
    <button
      v-if="library.currentPath"
      class="tb-btn batch-btn"
      :class="{ 'batch-running': batchRunning, 'batch-stopping': batchStopping }"
      :disabled="batchStopping"
      :title="batchStopping ? t('toolbar.batchStopping') : batchRunning ? t('toolbar.batchStopTitle') : t('toolbar.batchAnalysisTitle')"
      @click="startBatchAnalysis"
    >
      <!-- Sparkle icon when idle -->
      <svg v-if="!batchRunning" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3 13.7 8.3 19 10l-5.3 1.7L12 17l-1.7-5.3L5 10l5.3-1.7z"/>
        <path d="M19 15v4"/>
        <path d="M17 17h4"/>
      </svg>
      <!-- Spinner when stopping -->
      <span v-else-if="batchStopping" class="paper-task-spinner" />
      <!-- Stop icon when running -->
      <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
        <rect x="4" y="4" width="16" height="16" rx="2"/>
      </svg>
      <span class="batch-btn-label">{{ batchStopping ? t('toolbar.batchStopping') : batchRunning ? t('toolbar.batchStop') : t('toolbar.batchAnalysis') }}</span>
    </button>

    </div><!-- /toolbar-center -->

    <!-- Token usage modal -->
    <Teleport to="body">
      <Transition name="usage-fade">
        <TokenUsageModal v-if="showUsage" @close="showUsage = false" />
      </Transition>
    </Teleport>

    <div v-if="library.currentPath" class="right-toolbar-reserve" :style="rightReserveStyle">
      <!-- Sidebar tab icons (shown when sidebar is open) -->
      <template v-if="props.rightSidebarOpen">
        <button
          v-for="tab in sidebarTabs"
          :key="tab.id"
          class="tb-btn sidebar-tab-btn"
          :class="{ active: props.sidebarTab === tab.id }"
          :title="tab.label"
          @click="emit('update:sidebarTab', tab.id)"
        >
          <svg class="sidebar-tab-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path v-for="p in tab.paths" :key="p" :d="p"/>
          </svg>
          <span class="sidebar-tab-label">{{ tab.label }}</span>
        </button>
        <div class="tb-sep" />
      </template>

      <!-- Right sidebar toggle -->
      <button
        class="tb-btn sidebar-toggle-btn"
        :class="{ active: props.rightSidebarOpen, 'sidebar-collapsed': !props.rightSidebarOpen }"
        :title="props.rightSidebarOpen ? t('pdf.hideSidebar') : t('pdf.showSidebar')"
        @click="emit('toggle-right-sidebar')"
      >
        <svg class="sidebar-toggle-icon" width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
          <rect x="3.5" y="4" width="17" height="16" rx="4"/>
          <path d="M14.5 4v16"/>
          <path d="M17.5 9h.01"/>
          <path d="M17.5 12h.01"/>
        </svg>
      </button>
    </div>
  </div>
</template>

<style scoped>
.toolbar {
  height: var(--toolbar-height);
  display: flex;
  align-items: center;
  padding: 0;
  gap: 4px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.left-toolbar-reserve {
  display: flex;
  align-items: center;
  flex-shrink: 0;
  min-width: 0;
  height: 100%;
  padding: 0 10px 0 14px;
  border-right: 1px solid var(--border-default);
  box-sizing: border-box;
}

/* Vault picker button (left side) */
.lib-path-btn {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--text-secondary);
  font-size: 13px;
  font-weight: 500;
  padding: 5px 9px;
  border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s;
  flex-shrink: 0;
  max-width: 210px;
  min-width: 0;
}
.lib-path-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.vault-icon { flex-shrink: 0; color: var(--accent); }
.scan-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--accent);
  flex-shrink: 0;
  animation: scan-pulse 1.2s ease-in-out infinite;
  opacity: 0.8;
}
@keyframes scan-pulse {
  0%, 100% { opacity: 0.5; transform: scale(0.85); }
  50% { opacity: 1; transform: scale(1); }
}
.path-text {
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  min-width: 0;
}
.chevron-icon { flex-shrink: 0; opacity: 0.45; }

/* Search */
.search-box {
  position: relative;
  display: flex;
  align-items: center;
  width: 220px;
  flex-shrink: 0;
}
.search-placeholder { width: 220px; flex-shrink: 0; }

.search-icon {
  position: absolute;
  left: 9px;
  color: var(--text-tertiary);
  pointer-events: none;
  z-index: 1;
}
.search-input {
  width: 100%;
  padding: 6px 28px 6px 28px;
  font-size: var(--font-size-sm);
  border: none;
  border-radius: var(--radius-pill);
  background: var(--input-bg);
  color: var(--text-primary);
  transition: background 0.15s, box-shadow 0.15s;
}
.search-input::placeholder { color: var(--text-tertiary); }
.search-input:focus {
  background: var(--bg-primary);
  box-shadow: 0 0 0 3px var(--accent-light);
}
.search-clear {
  position: absolute;
  right: 8px;
  width: 16px;
  height: 16px;
  border-radius: 50%;
  background: var(--text-tertiary);
  color: var(--bg-primary);
  display: flex;
  align-items: center;
  justify-content: center;
  opacity: 0.7;
  transition: opacity 0.1s;
  flex-shrink: 0;
}
.search-clear:hover { opacity: 1; }

.spacer { flex: 1; min-width: 12px; }

/* Shrinkable middle section: clips its own overflow (horizontally only) so the
   fixed-width right reserve never gets pushed past the window edge — keeps the
   reserve border aligned with the right sidebar below on narrow windows.
   justify-content: flex-end keeps the action buttons visible and lets the
   search box clip first. */
.toolbar-center {
  flex: 1;
  min-width: 0;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 4px;
  overflow-x: clip;
  overflow-y: visible;
}

.paper-task-strip {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
  min-width: 0;
  max-width: min(520px, 36vw);
  flex: 0 1 auto;
}

.embed-progress-strip {
  display: flex;
  align-items: center;
  flex-shrink: 0;
}

.paper-task-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  min-width: 0;
  max-width: 250px;
  height: 26px;
  padding: 0 10px;
  border-radius: var(--radius-pill);
  border: 1px solid color-mix(in srgb, var(--accent) 26%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 8%, var(--bg-primary));
  color: color-mix(in srgb, var(--accent) 78%, var(--text-secondary));
  font-size: 12px;
  font-weight: 600;
  line-height: 1;
  white-space: nowrap;
  flex-shrink: 1;
}

.paper-task-chip.is-active {
  background: color-mix(in srgb, var(--accent) 10%, var(--bg-secondary));
}

.paper-task-label {
  overflow: hidden;
  text-overflow: ellipsis;
}

.paper-task-spinner {
  width: 10px;
  height: 10px;
  border: 1.5px solid color-mix(in srgb, var(--accent) 22%, transparent);
  border-top-color: currentColor;
  border-radius: 50%;
  flex-shrink: 0;
  animation: spin 0.75s linear infinite;
}

/* Import status */
.import-status {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  flex-shrink: 0;
}

.import-error-toast {
  display: flex;
  align-items: center;
  gap: 5px;
  max-width: 320px;
  padding: 4px 8px;
  border-radius: var(--radius-md);
  background: color-mix(in srgb, #cc3333 10%, var(--bg-primary));
  border: 1px solid color-mix(in srgb, #cc3333 30%, transparent);
  color: #cc3333;
  font-size: var(--font-size-xs);
  cursor: pointer;
  flex-shrink: 0;
}
.import-error-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 260px;
}

.spinner {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 1.5px solid var(--border-default);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}

/* Accent button (Import) — flat style */
.btn-accent {
  display: flex;
  align-items: center;
  gap: 5px;
  background: var(--accent);
  color: #fff;
  font-size: var(--font-size-sm);
  font-weight: 500;
  padding: 5px 13px;
  border-radius: var(--radius-pill);
  transition: opacity 0.12s, background 0.12s;
  white-space: nowrap;
  flex-shrink: 0;
}
.btn-accent:hover { opacity: 0.85; }
.btn-accent:active { opacity: 0.70; }
.btn-accent:disabled { opacity: 0.35; cursor: not-allowed; }

/* Regular toolbar buttons (icon-only style) */
.tb-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 5px;
  width: 32px;
  height: 32px;
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  border-radius: var(--radius-md);
  transition: background 0.12s, color 0.12s;
  flex-shrink: 0;
}
.tb-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

/* Separator */
.tb-sep {
  width: 1px;
  height: 18px;
  background: var(--border-subtle);
  margin: 0 4px;
  flex-shrink: 0;
}
.toolbar-center > .tb-sep {
  height: 24px;
  background: color-mix(in srgb, var(--text-tertiary) 34%, transparent);
}

/* AI hub button (merged arXiv + library chat) */
.ai-hub-wrap {
  position: relative;
  flex-shrink: 0;
}

.ai-hub-btn {
  width: auto;
  /* Reserve the left lane for the status dot in EVERY state (not just when
     busy) so the button width is identical with or without a running task —
     otherwise the button grows when a task starts and shifts its neighbours,
     risking misclicks. The dot itself is absolutely positioned (no width). */
  padding: 0 10px 0 18px;
  gap: 4px;
  position: relative;
  color: var(--text-secondary);
  overflow: hidden;
}

.ai-hub-btn.ai-busy {
  animation: arxiv-breathe 1.8s ease-in-out infinite;
}

.ai-hub-menu {
  min-width: 188px;
}

.menu-badge {
  margin-left: auto;
  min-width: 16px;
  height: 15px;
  background: #ef4444;
  color: #fff;
  font-size: 10px;
  font-weight: 700;
  border-radius: 8px;
  padding: 0 4px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  line-height: 1;
  flex-shrink: 0;
}

.menu-meta {
  margin-left: auto;
  font-size: 11px;
  font-weight: 600;
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
  flex-shrink: 0;
}

.menu-divider {
  height: 1px;
  background: var(--border-subtle);
  margin: 3px 6px;
  flex-shrink: 0;
}
@keyframes arxiv-breathe {
  0%, 100% { box-shadow: 0 0 0 0 rgba(255, 77, 125, 0); }
  50%       { box-shadow: 0 0 10px 3px rgba(255, 77, 125, 0.32), 0 0 6px 1px rgba(139, 92, 246, 0.2); }
}

.arxiv-pulse-dot {
  /* status indicator: out of flex flow so it never affects button width,
     vertically centered in the reserved left lane */
  position: absolute;
  top: 50%;
  left: 7px;
  margin-top: -3px; /* half the dot height → true vertical center */
  width: 6px; height: 6px;
  border-radius: 50%;
  background: #ff4d7d;
  animation: dot-pulse 1.8s ease-in-out infinite;
}
@keyframes dot-pulse {
  0%, 100% { opacity: 0.45; transform: scale(0.8); }
  50%       { opacity: 1;    transform: scale(1.2); }
}

.arxiv-label-inner {
  display: flex; align-items: center; gap: 5px;
}

.arxiv-progress-text {
  font-size: 11px; font-weight: 700; white-space: nowrap;
  background: var(--rainbow-chip-gradient);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}

/* Name/progress flip: both layers stay in the grid so the wider one defines
   a stable button width — flipping never resizes the button */
.ai-flip-stack {
  display: inline-grid;
  align-items: center;
  justify-items: center;
}
.ai-flip-item {
  grid-area: 1 / 1;
  transition: opacity 0.22s ease, transform 0.22s ease;
}
.ai-flip-item:not(.is-on) {
  opacity: 0;
  transform: translateY(6px);
  pointer-events: none;
}
.batch-progress-strip {
  position: relative;
  display: flex;
  align-items: center;
  flex-shrink: 0;
}

.batch-chip-clickable {
  cursor: pointer;
  user-select: none;
}
.batch-chip-clickable:hover {
  background: color-mix(in srgb, var(--accent) 14%, var(--bg-primary));
}

.batch-detail-popover {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  left: auto;
  z-index: 300;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  min-width: 320px;
  max-width: min(440px, calc(100vw - 16px));
  max-height: 360px;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.batch-detail-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 14px 9px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}
.batch-detail-title-text {
  font-size: 12px;
  font-weight: 700;
  color: var(--text-primary);
}
.batch-detail-count {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-tertiary);
}

.batch-detail-list {
  overflow-y: auto;
  padding: 4px 0;
}

.batch-detail-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 14px;
  font-size: 12px;
  min-width: 0;
}
.batch-item-spinner {
  flex-shrink: 0;
}
.batch-item-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  color: var(--text-primary);
}
.batch-item-stage {
  flex-shrink: 0;
  color: var(--text-tertiary);
  font-size: 11px;
  white-space: nowrap;
}

.batch-detail-empty {
  padding: 14px;
  font-size: 12px;
  color: var(--text-tertiary);
  text-align: center;
}

.batch-detail-enter-active,
.batch-detail-leave-active { transition: opacity 0.15s ease, transform 0.15s ease; }
.batch-detail-enter-from,
.batch-detail-leave-to { opacity: 0; transform: translateY(-4px); }

.batch-btn {
  width: auto;
  min-width: 0;
  height: auto;
  min-height: 0;
  box-sizing: border-box;
  padding: 4px 11px;
  gap: 5px;
  line-height: normal;
  border: 1px solid color-mix(in srgb, var(--accent) 34%, var(--border-default));
  border-radius: var(--radius-pill);
  background: var(--bg-secondary);
  color: color-mix(in srgb, var(--accent) 76%, #64748b);
  flex-shrink: 0;
}
.batch-btn:hover {
  color: color-mix(in srgb, var(--accent) 82%, #475569);
  border-color: color-mix(in srgb, var(--accent) 46%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 9%, var(--bg-secondary));
}
.batch-btn.batch-running {
  color: #ef4444;
  border-color: color-mix(in srgb, #ef4444 40%, var(--border-default));
  animation: batch-breathe 1.8s ease-in-out infinite;
}
.batch-btn.batch-stopping,
.batch-btn.batch-stopping:hover {
  color: var(--text-tertiary);
  border-color: var(--border-default);
  background: var(--bg-secondary);
  animation: none;
  cursor: not-allowed;
  opacity: 0.7;
}
.batch-btn.batch-running:hover {
  background: color-mix(in srgb, #ef4444 9%, var(--bg-secondary));
  border-color: color-mix(in srgb, #ef4444 55%, var(--border-default));
}
@keyframes batch-breathe {
  0%, 100% { box-shadow: 0 0 0 0 rgba(239, 68, 68, 0); }
  50%       { box-shadow: 0 0 8px 2px rgba(239, 68, 68, 0.22); }
}
.batch-btn-label {
  font-weight: 600;
  white-space: nowrap;
  font-size: var(--font-size-sm);
}

.usage-fade-enter-active, .usage-fade-leave-active { transition: opacity 0.18s ease; }
.usage-fade-enter-from, .usage-fade-leave-to { opacity: 0; }
.rainbow-chip {
  --rainbow-chip-gradient: linear-gradient(110deg, #ff5b7f, #f59e0b, #16a34a, #0284c7, #7c3aed, #ff5b7f);
  height: auto;
  min-height: 0;
  box-sizing: border-box;
  border: 1px solid transparent;
  border-radius: var(--radius-pill);
  padding-top: 4px;
  padding-bottom: 4px;
  line-height: normal;
  background:
    linear-gradient(var(--bg-secondary), var(--bg-secondary)) padding-box,
    linear-gradient(110deg, #ff4d7d, #ffb84d, #35d07f, #4da3ff, #8b5cf6, #ff4d7d) border-box;
  box-shadow: 0 0 0 0 color-mix(in srgb, var(--accent) 0%, transparent);
}
.rainbow-chip svg {
  color: color-mix(in srgb, var(--accent) 70%, #8b5cf6);
}
.rainbow-chip-label {
  color: transparent;
  background: var(--rainbow-chip-gradient);
  -webkit-background-clip: text;
  background-clip: text;
  font-weight: 600;
}
.rainbow-chip:hover,
.rainbow-chip.active {
  color: var(--text-primary);
  background:
    linear-gradient(color-mix(in srgb, var(--bg-secondary) 88%, var(--accent)), color-mix(in srgb, var(--bg-secondary) 88%, var(--accent))) padding-box,
    linear-gradient(110deg, #ff4d7d, #ffb84d, #35d07f, #4da3ff, #8b5cf6, #ff4d7d) border-box;
}
.global-feature-sep {
  margin-left: 6px;
  margin-right: 6px;
}
.arxiv-badge {
  position: absolute;
  top: 2px; right: 2px;
  min-width: 14px; height: 14px;
  background: #ef4444; color: #fff;
  font-size: 9px; font-weight: 700;
  border-radius: 7px; padding: 0 3px;
  display: flex; align-items: center; justify-content: center;
  line-height: 1;
}


.right-toolbar-reserve {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  flex-shrink: 0;
  position: relative;
  height: 100%;
  padding-left: 8px;
  padding-right: 10px;
  box-sizing: border-box;
}
.right-toolbar-reserve::before {
  content: '';
  position: absolute;
  left: 0;
  top: 50%;
  transform: translateY(-50%);
  width: 1px;
  height: 18px;
  background: var(--border-subtle);
}

.sidebar-tab-btn.active {
  color: var(--accent);
  background: var(--accent-light);
}
.tb-btn.sidebar-tab-btn {
  width: auto;
  min-width: 52px;
  height: 32px;
  gap: 4px;
  padding: 0 7px;
}
.sidebar-tab-icon {
  width: 17px;
  height: 17px;
  flex-shrink: 0;
}
.sidebar-tab-label {
  font-size: 11px;
  font-weight: 500;
  line-height: 1;
  white-space: nowrap;
  letter-spacing: 0;
}

.sidebar-toggle-btn.active {
  color: var(--accent);
}

.tb-btn.sidebar-toggle-btn {
  width: 32px;
  height: 32px;
  margin-left: 4px;
  border-radius: var(--radius-md);
  background: transparent;
}
.tb-btn.sidebar-toggle-btn.sidebar-collapsed {
  margin-left: 0;
}
.tb-btn.sidebar-toggle-btn:hover {
  color: var(--accent);
  background: transparent;
}

/* Outline button (arXiv URL) */
.btn-outline {
  display: flex;
  align-items: center;
  gap: 5px;
  background: transparent;
  color: var(--accent);
  border: 1px solid var(--accent);
  font-size: var(--font-size-sm);
  font-weight: 500;
  padding: 4px 12px;
  border-radius: var(--radius-pill);
  transition: background 0.12s, opacity 0.12s;
  white-space: nowrap;
  flex-shrink: 0;
}
.btn-outline:hover { background: var(--accent-light); }
.btn-outline:disabled { opacity: 0.35; cursor: not-allowed; }

/* Import button wrapper (dropdown + URL popover anchor) */
.import-wrap {
  position: relative;
  flex-shrink: 0;
}

.import-caret {
  margin-left: -1px;
  opacity: 0.65;
  flex-shrink: 0;
}

/* Import method dropdown */
.import-menu {
  position: absolute;
  top: calc(100% + 6px);
  right: 0;
  min-width: 148px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
  padding: 4px;
  z-index: 200;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.import-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
  border-radius: var(--radius-sm);
  text-align: left;
  white-space: nowrap;
  transition: background 0.12s;
}
.import-menu-item:hover { background: var(--bg-hover); }
.import-menu-item svg { color: var(--accent); flex-shrink: 0; }

.url-import-popover {
  position: absolute;
  top: calc(100% + 8px);
  right: 0;
  width: 340px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  padding: 14px 16px;
  z-index: 200;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.popover-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}
.popover-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
}
.popover-close {
  color: var(--text-tertiary);
  border-radius: var(--radius-sm);
  padding: 2px;
  width: 20px;
  height: 20px;
  display: flex;
  align-items: center;
  justify-content: center;
}
.popover-close:hover { background: var(--bg-hover); color: var(--text-primary); }

.popover-input {
  width: 100%;
  box-sizing: border-box;
  padding: 7px 10px;
  font-size: 13px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  background: var(--bg-secondary);
  color: var(--text-primary);
  transition: border-color 0.12s;
}
.popover-input:focus {
  outline: none;
  border-color: var(--accent);
  background: var(--bg-primary);
}
.popover-input::placeholder { color: var(--text-tertiary); }

.popover-error {
  font-size: var(--font-size-xs);
  color: #cc3333;
  margin: 0;
  line-height: 1.4;
  word-break: break-word;
}

.popover-actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

.btn-ghost-sm {
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  transition: background 0.12s;
}
.btn-ghost-sm:hover:not(:disabled) { background: var(--bg-hover); }
.btn-ghost-sm:disabled { opacity: 0.4; cursor: not-allowed; }

.btn-accent-sm {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 14px;
  font-size: 12px;
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-md);
  transition: opacity 0.12s;
}
.btn-accent-sm:hover:not(:disabled) { opacity: 0.85; }
.btn-accent-sm:disabled { opacity: 0.4; cursor: not-allowed; }

.spinner-sm {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 1.5px solid rgba(255,255,255,0.4);
  border-top-color: #fff;
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
  flex-shrink: 0;
}

/* Popover transition */
.popover-enter-active,
.popover-leave-active {
  transition: opacity 0.12s ease, transform 0.12s ease;
}
.popover-enter-from,
.popover-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}
.spin { animation: spin 0.7s linear infinite; }
</style>
