<script setup lang="ts">
import { ref, watch, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useReaderStore } from '../../stores/reader'
import { useLibraryStore } from '../../stores/library'
import { computeSections, detectSectionsForSlug } from '../../utils/sections'
import type { PaperSections } from '../../types'

const props = defineProps<{ slug: string }>()

const { t } = useI18n()
const reader = useReaderStore()
const library = useLibraryStore()

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
  // pending jump once it has loaded (see PdfViewer.loadPdf).
  if (reader.activeSlug !== props.slug) {
    const title = library.papers.find(p => p.slug === props.slug)?.title ?? props.slug
    reader.openPaper(props.slug, title)
  }
  reader.pendingPageJump = page
}

// Manual structural re-detection (embedded outline → heading heuristic).
// Prefers the already-open document; otherwise loads the PDF just for this.
async function runDetect() {
  if (detecting.value || !props.slug) return
  detecting.value = true
  error.value = ''
  try {
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
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <line x1="8" y1="6" x2="21" y2="6" /><line x1="8" y1="12" x2="21" y2="12" /><line x1="8" y1="18" x2="21" y2="18" />
        <line x1="3" y1="6" x2="3.01" y2="6" /><line x1="3" y1="12" x2="3.01" y2="12" /><line x1="3" y1="18" x2="3.01" y2="18" />
      </svg>
      <p>{{ t('sectionsTab.selectPaper') }}</p>
    </div>

    <!-- Have sections -->
    <template v-else-if="data && data.sections.length">
      <div class="sections-header">
        <span class="source-tag" :class="`source-${data.source}`">{{ sourceLabel(data.source) }}</span>
        <div class="header-actions">
          <button class="ai-split-btn" :disabled="detecting" :title="t('sectionsTab.detectHint')" @click="runDetect">
            <span v-if="detecting" class="spinner" />
            <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M23 4v6h-6" /><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
            </svg>
            {{ detecting ? t('sectionsTab.detecting') : t('sectionsTab.reDetect') }}
          </button>
          <button class="ai-split-btn" :disabled="splitting" @click="runAiSplit">
            <span v-if="splitting" class="spinner" />
            <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M12 3 13.7 8.3 19 10l-5.3 1.7L12 17l-1.7-5.3L5 10l5.3-1.7z" /><path d="M19 15v4" /><path d="M17 17h4" />
            </svg>
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
          <span v-if="sec.page > 0" class="section-page">p.{{ sec.page }}</span>
        </button>
      </div>
    </template>

    <!-- No structure detected -->
    <div v-else-if="!loading" class="empty">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <line x1="8" y1="6" x2="21" y2="6" /><line x1="8" y1="12" x2="21" y2="12" /><line x1="8" y1="18" x2="21" y2="18" />
        <line x1="3" y1="6" x2="3.01" y2="6" /><line x1="3" y1="12" x2="3.01" y2="12" /><line x1="3" y1="18" x2="3.01" y2="18" />
      </svg>
      <p>{{ t('sectionsTab.noSections') }}</p>
      <span>{{ t('sectionsTab.noSectionsHint') }}</span>
      <p v-if="error" class="error-text">{{ error }}</p>
      <div class="empty-actions">
        <button class="ai-split-btn primary" :disabled="detecting" @click="runDetect">
          <span v-if="detecting" class="spinner" />
          <svg v-else width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M23 4v6h-6" /><path d="M20.49 15a9 9 0 1 1-2.12-9.36L23 10" />
          </svg>
          {{ detecting ? t('sectionsTab.detecting') : t('sectionsTab.detect') }}
        </button>
        <button class="ai-split-btn" :disabled="splitting" @click="runAiSplit">
          <span v-if="splitting" class="spinner" />
          <svg v-else width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M12 3 13.7 8.3 19 10l-5.3 1.7L12 17l-1.7-5.3L5 10l5.3-1.7z" /><path d="M19 15v4" /><path d="M17 17h4" />
          </svg>
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
