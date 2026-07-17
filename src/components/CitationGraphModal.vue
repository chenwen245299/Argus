<script setup lang="ts">
import { computed, ref, watch, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useLibraryStore } from '../stores/library'
import { useReaderStore } from '../stores/reader'
import { useSelectionStore } from '../stores/selection'
import type { CitationRef } from '../types'
import CitationGraph from './CitationGraph.vue'

const { t } = useI18n()
const library = useLibraryStore()
const reader = useReaderStore()
const selection = useSelectionStore()

const slug = computed(() => library.citationGraph?.slug ?? null)
const open = computed(() => !!slug.value)
const center = computed(() => library.papers.find(p => p.slug === slug.value) ?? null)

// Cache the trigger anchor so the close animation can still scale back toward
// the button after the store state has been cleared.
const lastAnchor = ref<{ x: number; y: number } | null>(null)
watch(() => library.citationGraph?.anchor, (a) => { if (a) lastAnchor.value = a })

// Set the panel's transform-origin to the trigger button so it grows/shrinks
// from/into that point.
function applyOrigin(el: Element) {
  const panel = el.querySelector('.cgm-panel') as HTMLElement | null
  if (!panel) return
  const a = lastAnchor.value
  const prev = panel.style.transform
  panel.style.transform = 'none'
  const r = panel.getBoundingClientRect()
  panel.style.transform = prev
  panel.style.transformOrigin = a
    ? `${a.x - r.left}px ${a.y - r.top}px`
    : 'center center'
}

const refs = ref<CitationRef[]>([])
const loading = ref(false)
const error = ref('')

// How many reference nodes to display — user-configurable, remembered.
const NODE_LIMIT_KEY = 'argus:citegraph-nodes'
function loadNodeLimit(): number {
  const v = Number(localStorage.getItem(NODE_LIMIT_KEY))
  return Number.isFinite(v) && v >= 5 ? Math.min(200, Math.round(v)) : 36
}
const nodeLimit = ref<number>(loadNodeLimit())
function onLimitChange() {
  nodeLimit.value = Math.min(200, Math.max(5, Math.round(nodeLimit.value || 36)))
  localStorage.setItem(NODE_LIMIT_KEY, String(nodeLimit.value))
}

// On open: show cached references instantly; only hit the network when there's
// no cache yet (Semantic Scholar rate-limits aggressively).
watch(slug, async (s) => {
  refs.value = []
  error.value = ''
  if (!s) return
  loading.value = true
  try {
    const cached = await invoke<CitationRef[]>('get_cached_references', { slug: s })
    if (cached.length) {
      refs.value = cached
    } else {
      refs.value = await invoke<CitationRef[]>('fetch_references', { slug: s })
    }
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}, { immediate: true })

async function refresh() {
  if (!slug.value || loading.value) return
  loading.value = true
  error.value = ''
  try {
    refs.value = await invoke<CitationRef[]>('fetch_references', { slug: slug.value })
  } catch (e) {
    error.value = String(e)
  } finally {
    loading.value = false
  }
}

function close() { library.closeCitationGraph() }

function onOpen(s: string) {
  const entry = library.papers.find(p => p.slug === s)
  selection.selectPaper(s)
  reader.openPaper(s, entry?.title ?? '', entry?.file_type)
  close()
}

function onKeydown(e: KeyboardEvent) { if (e.key === 'Escape') close() }
watch(open, (v) => {
  if (v) window.addEventListener('keydown', onKeydown)
  else window.removeEventListener('keydown', onKeydown)
})
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <Transition name="genie" :duration="360" @enter="applyOrigin" @before-leave="applyOrigin">
  <div v-if="open" class="cgm-backdrop" @click="close">
    <div class="cgm-panel" @click.stop>
      <div class="cgm-header">
        <span class="cgm-title">
          {{ t('citeGraph.title') }}
          <span v-if="refs.length" class="cgm-count">{{ refs.length }}</span>
        </span>
        <span class="cgm-center-title" :title="center?.title">{{ center?.title }}</span>
        <label class="cgm-limit" :title="t('citeGraph.nodeCountHint')">
          {{ t('citeGraph.nodeCount') }}
          <input
            type="number" min="5" max="200" step="1"
            v-model.number="nodeLimit"
            @change="onLimitChange"
            @keydown.enter.prevent="onLimitChange"
          />
        </label>
        <button class="cgm-refresh" :disabled="loading" :title="t('citeGraph.refresh')" @click="refresh">
          {{ loading ? t('citeGraph.loading') : t('citeGraph.refresh') }}
        </button>
        <button class="cgm-close" :title="t('related.close')" @click="close">×</button>
      </div>

      <div class="cgm-body">
        <div v-if="loading && !refs.length" class="cgm-status">{{ t('citeGraph.loading') }}</div>
        <div v-else-if="error" class="cgm-status cgm-error">
          {{ error }}
          <button class="cgm-retry" @click="refresh">{{ t('citeGraph.retry') }}</button>
        </div>
        <div v-else-if="!refs.length" class="cgm-status">{{ t('citeGraph.empty') }}</div>
        <CitationGraph
          v-else
          :center-title="center?.title ?? ''"
          :center-cite-count="center?.cite_count ?? null"
          :refs="refs"
          :max-nodes="nodeLimit"
          @open="onOpen"
        />
      </div>
    </div>
  </div>
  </Transition>
</template>

<style scoped>
/* Open: the window physically grows from the trigger button to full size.
   Close: it shrinks back into the button. The panel stays fully opaque the whole
   time (pure scale, no fade); only the dim backdrop fades — via background-color
   so it doesn't drag the panel's opacity down with it. */
.genie-enter-active { transition: background-color 0.3s ease; }
.genie-leave-active { transition: background-color 0.32s ease; }
.genie-enter-from, .genie-leave-to { background-color: transparent; }
.genie-enter-active .cgm-panel { transition: transform 0.32s cubic-bezier(0.16, 1, 0.3, 1); }
.genie-leave-active .cgm-panel { transition: transform 0.3s cubic-bezier(0.4, 0, 1, 1); }
.genie-enter-from .cgm-panel,
.genie-leave-to .cgm-panel { transform: scale(0.08); }

.cgm-backdrop {
  position: fixed;
  inset: 0;
  z-index: 3000;
  background: color-mix(in srgb, #000 32%, transparent);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
}
.cgm-panel {
  width: min(1040px, 92vw);
  height: min(760px, 88vh);
  display: flex;
  flex-direction: column;
  padding: 12px 14px 14px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-md);
  overflow: hidden;
}
.cgm-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}
.cgm-title {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-md);
  font-weight: 600;
  color: var(--text-primary);
  flex-shrink: 0;
}
.cgm-count {
  min-width: 18px;
  padding: 0 6px;
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
.cgm-center-title {
  flex: 1;
  min-width: 0;
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.cgm-limit {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}
.cgm-limit input {
  width: 52px;
  padding: 3px 6px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: var(--font-size-xs);
}
.cgm-refresh {
  flex-shrink: 0;
  border: 1px solid var(--border-default);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.cgm-refresh:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
.cgm-refresh:disabled { opacity: 0.5; cursor: default; }
.cgm-close {
  flex-shrink: 0;
  border: none;
  background: transparent;
  color: var(--text-tertiary);
  font-size: 20px;
  line-height: 1;
  cursor: pointer;
  padding: 0 4px;
}
.cgm-close:hover { color: var(--text-primary); }
.cgm-body {
  flex: 1;
  min-height: 0;
  display: flex;
  flex-direction: column;
}
.cgm-status {
  flex: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 12px;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
}
.cgm-error { color: var(--danger, #dc2626); }
.cgm-retry, .cgm-status .cgm-retry {
  border: 1px solid var(--border-default);
  background: transparent;
  color: var(--text-secondary);
  font-size: var(--font-size-xs);
  padding: 4px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
}
.cgm-retry:hover { background: var(--bg-hover); color: var(--text-primary); }
</style>
