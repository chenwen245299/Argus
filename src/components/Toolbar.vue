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
import { titleInitialCaps } from '../utils/text'
import type { SearchHit } from '../types'
import TokenUsageModal from './TokenUsageModal.vue'

const { t } = useI18n()
const library = useLibraryStore()
const importStore = useImportStore()
const selection = useSelectionStore()
const paperTasks = usePaperTasksStore()
const { aiSummaryJobs, aiMetaSlug, aiMetaStage, abstractSlug } = storeToRefs(paperTasks)

const props = defineProps<{
  leftSidebarWidth?: number
  rightSidebarOpen?: boolean
  rightSidebarWidth?: number
  sidebarTab?: string
}>()

const emit = defineEmits<{
  'toggle-right-sidebar': []
  'update:sidebarTab': [tab: string]
}>()

type SidebarTabDef = { id: string; paths: string[]; label: string }

const sidebarTabs = computed((): SidebarTabDef[] => [
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
    : '40px',
}))

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
  if (!showUrlPopover.value) return
  const wrap = document.querySelector('.url-import-wrap')
  if (wrap && !wrap.contains(e.target as Node)) closeUrlPopover()
}

function submitUrl() {
  const url = urlInput.value.trim()
  if (!url) return
  const collectionId = selection.activeCollectionId
  if (!collectionId) {
    urlImportError.value = t('import.selectCollectionFirst')
    return
  }
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

// Token usage modal
const showUsage = ref(false)

interface PaperTaskStatusItem {
  id: string
  label: string
  detail?: string
  active?: boolean
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
    return {
      id: `${job.kind}:${slug}`,
      label: `${action} · ${aiSummaryStageLabel(job)}`,
      detail: aiSummaryDetail(slug, job),
      active: paperTasks.isAiSummaryActive(slug),
    }
  })

  if (aiMetaSlug.value) {
    items.push({
      id: `meta:${aiMetaSlug.value}`,
      label: `${t('paper.extractMetaAi')} · ${aiMetaStageLabel()}`,
      detail: paperTaskTitle(aiMetaSlug.value),
      active: true,
    })
  }

  if (abstractSlug.value) {
    items.push({
      id: `abstract:${abstractSlug.value}`,
      label: `${t('paper.extractAbstractAi')} · ${t('paper.extractAbstractAiIng')}`,
      detail: paperTaskTitle(abstractSlug.value),
      active: true,
    })
  }

  return items
})
const visiblePaperTaskItems = computed(() => paperTaskItems.value.slice(0, 2))
const hiddenPaperTaskCount = computed(() => Math.max(0, paperTaskItems.value.length - visiblePaperTaskItems.value.length))

// arXiv button state
const arxivNewCount = ref(0)
const arxivAnalyzing = ref(false)
const arxivFetching = ref(false)
const arxivProgress = ref({ done: 0, total: 0 })
const arxivLabelMode = ref<'name' | 'progress'>('name')
let unlistenArxiv: UnlistenFn | null = null
let unlistenArxivAnalysis: UnlistenFn | null = null
let unlistenArxivFetch: UnlistenFn | null = null
let labelToggleTimer: ReturnType<typeof setInterval> | null = null
let statusPollTimer: ReturnType<typeof setInterval> | null = null

const arxivBusy = computed(() => arxivAnalyzing.value || arxivFetching.value)

watch(arxivBusy, (busy) => {
  if (busy) {
    if (!labelToggleTimer) {
      arxivLabelMode.value = 'name'
      labelToggleTimer = setInterval(() => {
        arxivLabelMode.value = arxivLabelMode.value === 'name' ? 'progress' : 'name'
      }, 2200)
    }
  } else {
    if (labelToggleTimer) { clearInterval(labelToggleTimer); labelToggleTimer = null }
    arxivLabelMode.value = 'name'
  }
})

async function openArxiv() {
  try {
    await invoke('open_arxiv_window')
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
    if (status === 'started' || status === 'analyzing') {
      if (isBulk) arxivAnalyzing.value = true
    } else if (status === 'finished' || status === 'error') {
      if (isBulk) {
        arxivAnalyzing.value = false
        arxivProgress.value = { done, total }
      }
    }
    if (isBulk && (status === 'started' || status === 'analyzing')) {
      arxivProgress.value = { done, total }
    }
  })
  unlistenArxivFetch = await listen<{ status: string }>('arxiv-fetch', (e) => {
    arxivFetching.value = e.payload.status === 'fetching'
  })
  paperTasks.startListening()
  document.addEventListener('pointerdown', onDocClick, true)
  // Sync current analysis state immediately on mount (handles missed events)
  await syncArxivStatus()
  // Poll every 5 s so the indicator stays correct even if events were missed
  statusPollTimer = setInterval(syncArxivStatus, 5000)
})

onUnmounted(() => {
  if (unlistenArxiv) unlistenArxiv()
  if (unlistenArxivAnalysis) unlistenArxivAnalysis()
  if (unlistenArxivFetch) unlistenArxivFetch()
  if (labelToggleTimer) { clearInterval(labelToggleTimer); labelToggleTimer = null }
  if (statusPollTimer) { clearInterval(statusPollTimer); statusPollTimer = null }
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

    <div v-if="paperTaskItems.length" class="paper-task-strip">
      <span
        v-for="item in visiblePaperTaskItems"
        :key="item.id"
        class="paper-task-chip"
        :class="{ 'is-active': item.active }"
        :title="item.detail || item.label"
      >
        <span v-if="item.active" class="paper-task-spinner" />
        <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round">
          <polyline points="20 6 9 17 4 12"/>
        </svg>
        <span class="paper-task-label">{{ item.label }}</span>
      </span>
      <span v-if="hiddenPaperTaskCount" class="paper-task-more">+{{ hiddenPaperTaskCount }}</span>
    </div>

    <!-- Import status -->
    <div v-if="importStatusLabel" class="import-status">
      <span class="spinner" />
      <span>{{ importStatusLabel }}</span>
    </div>

    <!-- Import PDF -->
    <button
      v-if="library.currentPath"
      class="btn-outline"
      :disabled="activeJobs > 0 || !canImport"
      :title="importTitle"
      @click="pickAndImport"
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
        <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/>
        <polyline points="14 2 14 8 20 8"/>
        <line x1="12" y1="18" x2="12" y2="12"/>
        <polyline points="9 15 12 18 15 15"/>
      </svg>
      {{ t('import.fileImportBtn') }}
    </button>

    <!-- URL import -->
    <div v-if="library.currentPath" class="url-import-wrap">
      <button
        class="btn-outline"
        :disabled="activeJobs > 0 || !canImport"
        :title="t('import.urlImportTitle')"
        @click="openUrlPopover"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71"/>
          <path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71"/>
        </svg>
        {{ t('import.urlImportBtn') }}
      </button>

      <!-- Popover -->
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

    <!-- arXiv -->
    <button
      v-if="library.currentPath"
      class="tb-btn arxiv-btn rainbow-chip"
      :class="{ 'arxiv-busy': arxivBusy }"
      @click="openArxiv"
      :title="t('toolbar.arxivTitle')"
    >
      <span v-if="arxivBusy" class="arxiv-pulse-dot" />
      <Transition name="arxiv-flip" mode="out-in">
        <span
          v-if="arxivLabelMode === 'name' || arxivProgress.total === 0"
          key="name"
          class="arxiv-label-inner"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <path d="M12 2L2 7l10 5 10-5-10-5z"/>
            <path d="M2 17l10 5 10-5"/>
            <path d="M2 12l10 5 10-5"/>
          </svg>
          <span class="rainbow-chip-label">ArXiv</span>
        </span>
        <span v-else key="progress" class="arxiv-label-inner arxiv-progress-text">
          {{ arxivProgress.done }}/{{ arxivProgress.total }}
        </span>
      </Transition>
      <span v-if="arxivNewCount > 0" class="arxiv-badge">{{ arxivNewCount }}</span>
    </button>

    <button
      v-if="library.currentPath"
      class="tb-btn library-chat-btn rainbow-chip"
      :title="t('toolbar.libraryChatTitle')"
      @click="openLibraryChat"
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <path d="M21 15a4 4 0 0 1-4 4H8l-5 3V7a4 4 0 0 1 4-4h10a4 4 0 0 1 4 4z"/>
        <path d="M8 9h8"/>
        <path d="M8 13h5"/>
      </svg>
      <span class="rainbow-chip-label">{{ t('toolbar.libraryChat') }}</span>
    </button>

    <div v-if="library.currentPath" class="tb-sep global-feature-sep" />

    <!-- Token usage button -->
    <button
      v-if="library.currentPath"
      class="tb-btn usage-btn"
      :title="t('toolbar.aiUsageTitle')"
      :aria-label="t('toolbar.aiUsageTitle')"
      @click="showUsage = true"
    >
      <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <ellipse cx="12" cy="6" rx="7" ry="3"/>
        <path d="M5 6v5c0 1.7 3.1 3 7 3s7-1.3 7-3V6"/>
        <path d="M5 11v5c0 1.7 3.1 3 7 3 1.3 0 2.5-.1 3.5-.4"/>
        <path d="M18.5 14.5l.5 1.2 1.2.5-1.2.5-.5 1.2-.5-1.2-1.2-.5 1.2-.5.5-1.2z"/>
      </svg>
      <span class="usage-label">{{ t('toolbar.aiUsage') }}</span>
    </button>

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
        :class="{ active: props.rightSidebarOpen }"
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
  border-right: 1px solid var(--border-subtle);
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

.paper-task-strip {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
  min-width: 0;
  max-width: min(520px, 36vw);
  flex: 0 1 auto;
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

.paper-task-more {
  height: 22px;
  min-width: 22px;
  padding: 0 6px;
  border-radius: var(--radius-pill);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  background: var(--bg-hover);
  color: var(--text-tertiary);
  font-size: 11px;
  font-weight: 600;
  flex-shrink: 0;
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
.toolbar > .tb-sep {
  height: 24px;
  background: color-mix(in srgb, var(--text-tertiary) 34%, transparent);
}

/* arXiv button */
.arxiv-btn {
  width: auto;
  padding: 0 12px;
  gap: 5px;
  position: relative;
  color: var(--text-secondary);
  min-width: 82px;
  overflow: hidden;
}

.arxiv-btn.arxiv-busy {
  animation: arxiv-breathe 1.8s ease-in-out infinite;
}
@keyframes arxiv-breathe {
  0%, 100% { box-shadow: 0 0 0 0 rgba(255, 77, 125, 0); }
  50%       { box-shadow: 0 0 10px 3px rgba(255, 77, 125, 0.32), 0 0 6px 1px rgba(139, 92, 246, 0.2); }
}

.arxiv-pulse-dot {
  width: 6px; height: 6px;
  border-radius: 50%;
  background: #ff4d7d;
  flex-shrink: 0;
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

.arxiv-flip-enter-active,
.arxiv-flip-leave-active { transition: opacity 0.22s ease, transform 0.22s ease; }
.arxiv-flip-enter-from   { opacity: 0; transform: translateY(6px); }
.arxiv-flip-leave-to     { opacity: 0; transform: translateY(-6px); }
.library-chat-btn {
  width: auto;
  padding: 0 12px;
  gap: 5px;
  color: var(--text-secondary);
}
.usage-btn {
  width: auto;
  min-width: 0;
  height: auto;
  min-height: 0;
  box-sizing: border-box;
  padding: 4px 12px;
  gap: 5px;
  line-height: normal;
  border: 1px solid color-mix(in srgb, var(--accent) 34%, var(--border-default));
  border-radius: var(--radius-pill);
  background: var(--bg-secondary);
  color: color-mix(in srgb, var(--accent) 76%, #64748b);
  flex-shrink: 0;
}
.usage-btn:hover {
  color: color-mix(in srgb, var(--accent) 82%, #475569);
  border-color: color-mix(in srgb, var(--accent) 46%, var(--border-default));
  background: color-mix(in srgb, var(--accent) 9%, var(--bg-secondary));
}
.usage-label {
  font-weight: 600;
  white-space: nowrap;
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
  height: 100%;
  border-left: 1px solid var(--border-subtle);
  padding-left: 8px;
  padding-right: 10px;
  box-sizing: border-box;
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
  width: 36px;
  height: 32px;
  margin-left: 4px;
  border-radius: var(--radius-md);
  background: transparent;
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

/* URL import popover */
.url-import-wrap {
  position: relative;
  flex-shrink: 0;
}

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
