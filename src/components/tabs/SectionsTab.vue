<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useReaderStore } from '../../stores/reader'
import { useLibraryStore } from '../../stores/library'
import { computeSections, detectSectionsForSlug } from '../../utils/sections'
import type { PaperSections } from '../../types'
import { isEbookFileType } from '../../types'

const props = defineProps<{ slug: string }>()

const { t } = useI18n()
const reader = useReaderStore()
const library = useLibraryStore()

// Ebook papers: sections come from the book's own TOC, jump targets are
// chapters, and re-detect must never touch reader.pdfDoc — that can hold
// another tab's PDF document. The stored `page` is a SPINE index (counts
// cover/preface files too), so it is hidden in the UI for ebooks.
const isEbook = computed(() =>
  isEbookFileType(
    reader.tabs.find(tb => tb.slug === props.slug)?.fileType
      ?? library.papers.find(p => p.slug === props.slug)?.file_type
  )
)

const data = ref<PaperSections | null>(null)
const loading = ref(false)
const detecting = ref(false)
const splitting = ref(false)
const error = ref('')

async function load(slug: string) {
  data.value = null
  error.value = ''
  if (!slug) return
  loading.value = true
  try {
    data.value = await invoke<PaperSections | null>('get_sections', { slug })
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

watch(() => props.slug, (slug) => load(slug), { immediate: true })

function onSectionsUpdated(e: Event) {
  const detail = (e as CustomEvent<{ slug: string }>).detail
  if (detail?.slug === props.slug) load(props.slug)
}

onMounted(() => window.addEventListener('argus-sections-updated', onSectionsUpdated))
onBeforeUnmount(() => window.removeEventListener('argus-sections-updated', onSectionsUpdated))

function jumpTo(page: number) {
  if (page <= 0) return
  // Open the paper first if it isn't the active tab; the viewer consumes the
  // pending jump once it has loaded (see PdfViewer.loadPdf / EbookViewer.loadBook).
  if (reader.activeSlug !== props.slug) {
    const entry = library.papers.find(p => p.slug === props.slug)
    reader.openPaper(props.slug, entry?.title ?? props.slug, entry?.file_type)
  }
  reader.pendingPageJump = page
}

// Manual structural re-detection (embedded outline → heading heuristic).
// Prefers the already-open document; otherwise loads the PDF just for this.
// Ebooks branch FIRST — reader.pdfDoc may hold a different tab's document.
async function runDetect() {
  if (detecting.value || !props.slug) return
  detecting.value = true
  error.value = ''
  try {
    if (isEbook.value) {
      data.value = await invoke<PaperSections>('regen_ebook_sections', { slug: props.slug })
      window.dispatchEvent(new CustomEvent('argus-sections-updated', { detail: { slug: props.slug } }))
      return
    }
    const result =
      reader.openSlug === props.slug && reader.pdfDoc
        ? await computeSections(reader.pdfDoc)
        : await detectSectionsForSlug(props.slug)
    if (result && result.sections.length) {
      await invoke('save_sections', { slug: props.slug, data: result })
      data.value = result
      window.dispatchEvent(new CustomEvent('argus-sections-updated', { detail: { slug: props.slug } }))
    } else {
      error.value = t('sectionsTab.detectEmpty')
    }
  } catch (e) {
    error.value = String(e)
  } finally {
    detecting.value = false
  }
}

async function runAiSplit() {
  if (splitting.value || !props.slug) return
  splitting.value = true
  error.value = ''
  try {
    data.value = await invoke<PaperSections>('ai_split_sections', {
      slug: props.slug,
      providerId: null,
      modelId: null,
    })
    window.dispatchEvent(new CustomEvent('argus-sections-updated', { detail: { slug: props.slug } }))
  } catch (e) {
    error.value = String(e)
  } finally {
    splitting.value = false
  }
}

function sourceLabel(source: string): string {
  if (source === 'outline') return t('sectionsTab.sourceOutline')
  if (source === 'ai') return t('sectionsTab.sourceAi')
  return t('sectionsTab.sourceHeuristic')
}
</script>

<template>
  <div class="sections-tab">
    <!-- No paper -->
    <div v-if="!props.slug" class="empty">
      <Icon icon="fluent:text-bullet-list-24-regular" width="28" height="28" />
      <p>{{ t('sectionsTab.selectPaper') }}</p>
    </div>

    <!-- Have sections -->
    <template v-else-if="data && data.sections.length">
      <div class="sections-header">
        <span class="source-tag" :class="`source-${data.source}`">{{ sourceLabel(data.source) }}</span>
        <div class="header-actions">
          <button class="ai-split-btn" :disabled="detecting" :title="t('sectionsTab.detectHint')" @click="runDetect">
            <span v-if="detecting" class="spinner" />
            <Icon v-else icon="fluent:arrow-sync-24-regular" width="13" height="13" />
            {{ detecting ? t('sectionsTab.detecting') : t('sectionsTab.reDetect') }}
          </button>
          <button class="ai-split-btn" :disabled="splitting" @click="runAiSplit">
            <span v-if="splitting" class="spinner" />
            <Icon v-else icon="fluent:sparkle-24-regular" width="13" height="13" />
            {{ splitting ? t('sectionsTab.aiSplitting') : t('sectionsTab.aiReSplit') }}
          </button>
        </div>
      </div>
      <p v-if="error" class="error-text">{{ error }}</p>
      <div class="list">
        <button
          v-for="(sec, i) in data.sections"
          :key="i"
          class="section-item"
          :class="[`level-${sec.level}`, { 'no-jump': sec.page <= 0 }]"
          :title="sec.title"
          @click="jumpTo(sec.page)"
        >
          <span class="section-title">{{ sec.title }}</span>
          <span v-if="sec.page > 0 && !isEbook" class="section-page">p.{{ sec.page }}</span>
        </button>
      </div>
    </template>

    <!-- No structure detected -->
    <div v-else-if="!loading" class="empty">
      <Icon icon="fluent:text-bullet-list-24-regular" width="28" height="28" />
      <p>{{ t('sectionsTab.noSections') }}</p>
      <span>{{ t('sectionsTab.noSectionsHint') }}</span>
      <p v-if="error" class="error-text">{{ error }}</p>
      <div class="empty-actions">
        <button class="ai-split-btn primary" :disabled="detecting" @click="runDetect">
          <span v-if="detecting" class="spinner" />
          <Icon v-else icon="fluent:arrow-sync-24-regular" width="14" height="14" />
          {{ detecting ? t('sectionsTab.detecting') : t('sectionsTab.detect') }}
        </button>
        <button class="ai-split-btn" :disabled="splitting" @click="runAiSplit">
          <span v-if="splitting" class="spinner" />
          <Icon v-else icon="fluent:sparkle-24-regular" width="14" height="14" />
          {{ splitting ? t('sectionsTab.aiSplitting') : t('sectionsTab.aiSplit') }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.sections-tab { height: 100%; overflow-y: auto; display: flex; flex-direction: column; }

.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  padding: 24px;
  text-align: center;
}
.empty p { color: var(--text-secondary); font-weight: 500; }
.empty span { font-size: var(--font-size-xs); }

.sections-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 10px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.header-actions { display: flex; align-items: center; gap: 6px; }
.empty-actions { display: flex; flex-direction: column; align-items: center; gap: 8px; margin-top: 4px; }

.source-tag {
  font-size: var(--font-size-xs);
  font-weight: 600;
  padding: 2px 8px;
  border-radius: var(--radius-pill);
  color: var(--text-secondary);
  background: var(--bg-tertiary);
}
.source-tag.source-ai { color: var(--accent); background: var(--accent-light); }

.ai-split-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: var(--font-size-xs);
  padding: 4px 9px;
  border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle);
  background: var(--bg-primary);
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.1s, color 0.1s;
}
.ai-split-btn:hover:not(:disabled) { background: var(--bg-tertiary); color: var(--text-primary); }
.ai-split-btn:disabled { opacity: 0.6; cursor: default; }
.ai-split-btn.primary {
  background: var(--accent);
  color: #fff;
  border-color: transparent;
  padding: 6px 12px;
  font-size: var(--font-size-sm);
  margin-top: 4px;
}
.ai-split-btn.primary:hover:not(:disabled) { background: var(--accent-hover); color: #fff; }

.list { padding: 6px; flex: 1; }

.section-item {
  display: flex;
  align-items: baseline;
  gap: 8px;
  width: 100%;
  text-align: left;
  padding: 5px 8px;
  border-radius: var(--radius-sm);
  color: var(--text-primary);
  cursor: pointer;
  transition: background 0.1s;
}
.section-item:hover { background: var(--bg-hover); }
.section-item.no-jump { cursor: default; }
.section-item.no-jump:hover { background: transparent; }

.section-item.level-1 { font-weight: 600; font-size: var(--font-size-sm); }
.section-item.level-2 { padding-left: 22px; font-size: var(--font-size-sm); color: var(--text-secondary); }
.section-item.level-3 { padding-left: 38px; font-size: var(--font-size-xs); color: var(--text-tertiary); }

.section-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.section-page {
  flex-shrink: 0;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  font-variant-numeric: tabular-nums;
}

.error-text { color: #cc3333; font-size: var(--font-size-xs); padding: 0 10px; text-align: center; }

.spinner {
  display: inline-block;
  width: 11px;
  height: 11px;
  border: 1.5px solid var(--border-default);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
}
@keyframes spin { to { transform: rotate(360deg); } }
</style>
