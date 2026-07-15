<script setup lang="ts">
import { computed, ref, watch, nextTick, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import type { PaperIndexEntry, PaperMeta } from '../types'
import PaperPicker from './PaperPicker.vue'
import RelatedGraph from './RelatedGraph.vue'

const { t } = useI18n()
const library = useLibraryStore()
const reader = useReaderStore()
const selection = useSelectionStore()

const state = computed(() => library.relatedPopover)
const open = computed(() => !!state.value)
const currentSlug = computed(() => state.value?.slug ?? '')
const current = computed(() => library.papers.find(p => p.slug === currentSlug.value) ?? null)
const related = computed(() =>
  currentSlug.value ? library.relatedEntriesFor(currentSlug.value) : [])

const mode = ref<'list' | 'graph' | 'add'>('list')
// Reset to list mode each time the popover (re)opens for a paper.
watch(() => state.value?.slug, () => { mode.value = 'list' })

// ── Positioning ────────────────────────────────────────────────────────────
const panel = ref<HTMLElement | null>(null)
const posStyle = ref<Record<string, string>>({})
// Base size (list/graph/add share it so plain tab switches never resize). The
// graph can be enlarged to double the dimensions, clamped to the viewport.
const MARGIN = 12
const NORMAL_W = 360
const NORMAL_H = 420
const enlarged = ref(false)
const panelSize = computed(() => {
  if (enlarged.value) {
    return {
      w: Math.min(NORMAL_W * 2, window.innerWidth - MARGIN * 2),
      h: Math.min(NORMAL_H * 2, window.innerHeight - MARGIN * 2),
    }
  }
  return { w: NORMAL_W, h: NORMAL_H }
})

// Only transition size/position while the user is toggling enlarge — never on
// open or when the popover is (re)anchored.
const animateSize = ref(false)
let animateTimer: ReturnType<typeof setTimeout> | null = null
function toggleEnlarge() {
  animateSize.value = true
  enlarged.value = !enlarged.value
  if (animateTimer) clearTimeout(animateTimer)
  animateTimer = setTimeout(() => { animateSize.value = false }, 340)
}

async function reposition() {
  await nextTick()
  const { w, h } = panelSize.value
  const anchor = state.value?.anchor ?? null
  const vw = window.innerWidth
  const vh = window.innerHeight
  let left: number
  let top: number
  if (anchor) {
    // `anchor` is the button's bottom-right; grow the panel down-and-left from it.
    left = Math.min(Math.max(MARGIN, anchor.x - w), vw - w - MARGIN)
    top = Math.min(Math.max(MARGIN, anchor.y), vh - h - MARGIN)
  } else {
    left = Math.max(MARGIN, (vw - w) / 2)
    top = Math.max(MARGIN, (vh - h) / 2)
  }
  posStyle.value = { left: `${left}px`, top: `${top}px`, width: `${w}px`, height: `${h}px` }
}
watch(() => [state.value?.slug, mode.value, enlarged.value], () => { if (open.value) reposition() })

// ── Actions ──────────────────────────────────────────────────────────────
function close() { library.closeRelatedPopover() }

function openRelated(entry: PaperIndexEntry) {
  selection.selectPaper(entry.slug)
  reader.openPaper(entry.slug, entry.title, entry.file_type)
  close()
}

async function removeRelated(entry: PaperIndexEntry) {
  if (!currentSlug.value) return
  try {
    await library.unlinkRelated(currentSlug.value, entry.slug)
  } catch (e) {
    console.error('[related] unlink failed:', e)
  }
}

async function onPick(entry: PaperIndexEntry) {
  if (!currentSlug.value) return
  try {
    await library.linkRelated(currentSlug.value, entry.slug)
  } catch (e) {
    console.error('[related] link failed:', e)
  }
  mode.value = 'list'
}

function onKeydown(e: KeyboardEvent) {
  if (e.key !== 'Escape') return
  if (mode.value === 'add') mode.value = 'list'
  else close()
}
watch(open, (v) => {
  if (v) {
    window.addEventListener('keydown', onKeydown)
    reposition()
  } else {
    window.removeEventListener('keydown', onKeydown)
  }
})
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))

function authorLine(e: PaperIndexEntry) {
  const first = e.authors?.[0] ?? ''
  const etal = (e.authors?.length ?? 0) > 1 ? ' et al.' : ''
  const yr = e.year ? ` · ${e.year}` : ''
  return `${first}${etal}${yr}`
}

const excludeIds = computed(() =>
  current.value ? [current.value.id, ...(current.value.related_ids ?? [])] : [])

// ── Export: the direct (1-hop) similar papers, anchored on the current paper ─
const exporting = ref(false)

function buildMarkdown(anchor: PaperIndexEntry, metas: PaperMeta[]): string {
  const lines: string[] = []
  const anchorMeta = [(anchor.authors ?? []).join(', '), anchor.year, anchor.venue]
    .filter(Boolean).join(' · ')
  lines.push(`# ${anchor.title}`)
  if (anchorMeta) lines.push('', `_${anchorMeta}_`)
  lines.push('', `## ${t('related.title')} (${metas.length})`, '')
  metas.forEach((m, i) => {
    const bits = [(m.authors ?? []).join(', '), m.year, m.venue].filter(Boolean).join(' · ')
    lines.push(`${i + 1}. **${m.title}**`)
    if (bits) lines.push(`   ${bits}`)
    const links: string[] = []
    if (m.doi) links.push(`[DOI](https://doi.org/${m.doi})`)
    if (m.arxiv_id) links.push(`[arXiv](https://arxiv.org/abs/${m.arxiv_id})`)
    if (links.length) lines.push(`   ${links.join(' · ')}`)
    lines.push('')
  })
  return lines.join('\n')
}

async function exportList() {
  const anchor = current.value
  if (!anchor || !related.value.length || exporting.value) return
  exporting.value = true
  try {
    const metas = (await Promise.all(
      related.value.map(e =>
        invoke<PaperMeta>('get_paper_meta', { slug: e.slug }).catch(() => null)),
    )).filter((m): m is PaperMeta => !!m)
    if (!metas.length) return
    const safeName = (anchor.title || 'paper')
      .replace(/[\\/:*?"<>|]/g, '_').slice(0, 60).trim()
    const savePath = await dialogSave({
      title: t('related.exportTitle'),
      defaultPath: `${safeName} - similar.md`,
      filters: [{ name: 'Markdown', extensions: ['md'] }],
    })
    if (!savePath) return
    const bytes = Array.from(new TextEncoder().encode(buildMarkdown(anchor, metas)))
    await invoke('write_bytes_to_file', { path: savePath, bytes })
  } catch (e) {
    console.error('[related] export failed:', e)
  } finally {
    exporting.value = false
  }
}
</script>

<template>
  <div v-if="open" class="related-backdrop" @click="close">
    <div
      ref="panel"
      class="related-popover"
      :class="{ 'animate-size': animateSize }"
      :style="posStyle"
      @click.stop
    >
      <div class="rp-header">
        <span class="rp-title">
          {{ t('related.title') }}
          <span v-if="mode === 'list' && related.length" class="rp-count">{{ related.length }}</span>
        </span>
        <div v-if="mode !== 'add'" class="rp-seg">
          <button :class="{ active: mode === 'list' }" @click="mode = 'list'">{{ t('related.viewList') }}</button>
          <button :class="{ active: mode === 'graph' }" @click="mode = 'graph'">{{ t('related.viewGraph') }}</button>
        </div>
        <button
          v-if="mode !== 'add'"
          class="rp-icon-btn"
          :title="enlarged ? t('related.shrink') : t('related.enlarge')"
          @click="toggleEnlarge"
        >
          <svg v-if="!enlarged" width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="15 3 21 3 21 9" />
            <polyline points="9 21 3 21 3 15" />
            <line x1="21" y1="3" x2="14" y2="10" />
            <line x1="3" y1="21" x2="10" y2="14" />
          </svg>
          <svg v-else width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <polyline points="4 14 10 14 10 20" />
            <polyline points="20 10 14 10 14 4" />
            <line x1="14" y1="10" x2="21" y2="3" />
            <line x1="10" y1="14" x2="3" y2="21" />
          </svg>
        </button>
        <button class="rp-close" :title="t('related.close')" @click="close">×</button>
      </div>

      <div class="rp-body">
      <template v-if="mode === 'list'">
        <div v-if="related.length === 0" class="rp-empty">{{ t('related.empty') }}</div>
        <div v-else class="rp-list">
          <div
            v-for="e in related"
            :key="e.id"
            class="rp-item"
            @click="openRelated(e)"
          >
            <div class="rp-item-main">
              <div class="rp-item-title">{{ e.title }}</div>
              <div class="rp-item-sub">{{ authorLine(e) }}</div>
            </div>
            <button
              class="rp-item-remove"
              :title="t('related.remove')"
              @click.stop="removeRelated(e)"
            >×</button>
          </div>
        </div>
        <div class="rp-actions">
          <button class="rp-add" @click="mode = 'add'">＋ {{ t('related.add') }}</button>
          <button
            class="rp-export"
            :disabled="!related.length || exporting"
            :title="t('related.exportTitle')"
            @click="exportList"
          >
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
          </button>
        </div>
      </template>

      <RelatedGraph
        v-else-if="mode === 'graph'"
        :root-slug="currentSlug"
        @open="openRelated"
      />

      <PaperPicker
        v-else
        :current-slug="currentSlug"
        :exclude-ids="excludeIds"
        @select="onPick"
        @cancel="mode = 'list'"
      />
      </div>
    </div>
  </div>
</template>

<style scoped>
.related-backdrop {
  position: fixed;
  inset: 0;
  z-index: 3000;
  background: transparent;
}
.related-popover {
  position: fixed;
  /* Default size; overridden by the inline width/height from reposition()
     (which also handles the enlarged graph size). */
  width: 360px;
  height: 420px;
  display: flex;
  flex-direction: column;
  padding: 10px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  overflow: hidden;
}
.related-popover.animate-size {
  transition:
    width 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    height 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    left 0.3s cubic-bezier(0.4, 0, 0.2, 1),
    top 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}
.rp-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.rp-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 8px;
}
.rp-seg {
  display: inline-flex;
  margin-left: auto;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  overflow: hidden;
}
.rp-seg button {
  border: none;
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  padding: 3px 9px;
  cursor: pointer;
}
.rp-seg button.active {
  background: var(--accent);
  color: #fff;
}
.rp-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}
.rp-count {
  min-width: 18px;
  padding: 0 5px;
  height: 18px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-pill);
  background: var(--accent-light);
  color: var(--accent);
  font-size: var(--font-size-xs);
  font-weight: 600;
}
.rp-icon-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 24px;
  height: 24px;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.rp-icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.rp-close {
  margin-left: auto;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  font-size: 18px;
  line-height: 1;
  cursor: pointer;
  padding: 0 4px;
}
.rp-close:hover { color: var(--text-primary); }
.rp-empty {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px 8px;
  text-align: center;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}
.rp-list {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  min-height: 0;
}
.rp-item {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 7px 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.rp-item:hover { background: var(--bg-hover); }
.rp-item-main {
  flex: 1;
  min-width: 0;
}
.rp-item-title {
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.rp-item-sub {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.rp-item-remove {
  flex-shrink: 0;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  font-size: 15px;
  line-height: 1;
  cursor: pointer;
  padding: 2px 4px;
  border-radius: var(--radius-sm);
  opacity: 0;
}
.rp-item:hover .rp-item-remove { opacity: 1; }
.rp-item-remove:hover {
  background: var(--bg-secondary);
  color: var(--text-primary);
}
.rp-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
}
.rp-add {
  flex: 1;
  padding: 8px;
  border: 1px dashed var(--border-default);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--accent);
  font-size: var(--font-size-sm);
  cursor: pointer;
}
.rp-add:hover { background: var(--bg-hover); }
.rp-export {
  flex-shrink: 0;
  width: 38px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.rp-export:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--accent);
  border-color: var(--accent);
}
.rp-export:disabled {
  opacity: 0.4;
  cursor: default;
}
</style>
