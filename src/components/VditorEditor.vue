<script setup lang="ts">
import { onMounted, onBeforeUnmount, ref, nextTick, watch } from 'vue'
import type VditorType from 'vditor'

const props = defineProps<{ initialContent: string }>()
const emit = defineEmits<{ change: [markdown: string] }>()

const containerEl = ref<HTMLDivElement | null>(null)
let vd: VditorType | null = null
let destroyed = false

onMounted(async () => {
  // Vditor (JS + CSS) is heavyweight — load it when an editor actually mounts
  // instead of in every window's startup bundle.
  const [{ default: Vditor }] = await Promise.all([
    import('vditor'),
    import('vditor/dist/index.css'),
  ])
  await nextTick()
  if (!containerEl.value || destroyed) return

  const content = props.initialContent

  vd = new Vditor(containerEl.value, {
    mode: 'ir',
    cdn: '/vditor',
    lang: 'zh_CN',
    value: content,
    minHeight: 200,
    toolbarConfig: { hide: true },
    preview: {
      math: {
        engine: 'KaTeX',
        inlineDigit: true,
      },
    },
    counter: { enable: false },
    resize: { enable: false },
    cache: { enable: false },
    after() {
      requestAnimationFrame(() => {
        if (content && vd?.getValue().trim() === '') {
          vd.setValue(content, true)
        }
      })
    },
    input(val) {
      emit('change', val)
    },
  })
})

onBeforeUnmount(() => {
  destroyed = true
  vd?.destroy()
  vd = null
  clearHighlights()
})

// ── Find bar ──────────────────────────────────────────────────────────────────
const findVisible = ref(false)
const findQuery = ref('')
const findInput = ref<HTMLInputElement | null>(null)
const matchCount = ref(0)
const matchIndex = ref(0) // 1-based

let allRanges: Range[] = []

// CSS Custom Highlight API — no DOM mutation, no focus side-effects
function cssHighlights(): HighlightRegistry | null {
  return (typeof CSS !== 'undefined' && 'highlights' in CSS)
    ? (CSS as unknown as { highlights: HighlightRegistry }).highlights
    : null
}

function applyHighlights() {
  const h = cssHighlights()
  if (!h) return
  h.delete('vditor-find-all')
  h.delete('vditor-find-current')
  if (allRanges.length === 0) return
  h.set('vditor-find-all', new Highlight(...allRanges))
  if (matchIndex.value > 0) {
    h.set('vditor-find-current', new Highlight(allRanges[matchIndex.value - 1]))
  }
}

function clearHighlights() {
  const h = cssHighlights()
  if (!h) return
  h.delete('vditor-find-all')
  h.delete('vditor-find-current')
}

function scrollToMatch(range: Range) {
  range.startContainer.parentElement?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
}

function getEditorRoot(): HTMLElement | null {
  return containerEl.value?.querySelector('.vditor-ir .vditor-reset') ?? null
}

function gatherRanges(query: string): Range[] {
  const root = getEditorRoot()
  if (!root || !query) return []

  const ranges: Range[] = []
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT)
  const lower = query.toLowerCase()

  let node: Node | null
  while ((node = walker.nextNode())) {
    const text = (node.textContent ?? '').toLowerCase()
    let pos = 0
    while ((pos = text.indexOf(lower, pos)) !== -1) {
      const r = document.createRange()
      r.setStart(node, pos)
      r.setEnd(node, pos + query.length)
      ranges.push(r)
      pos += lower.length
    }
  }
  return ranges
}

function runSearch() {
  allRanges = gatherRanges(findQuery.value)
  matchCount.value = allRanges.length
  matchIndex.value = allRanges.length > 0 ? 1 : 0
  applyHighlights()
  if (allRanges.length > 0) scrollToMatch(allRanges[0])
}

function findNext() {
  if (allRanges.length === 0) return
  matchIndex.value = (matchIndex.value % allRanges.length) + 1
  applyHighlights()
  scrollToMatch(allRanges[matchIndex.value - 1])
}

function findPrev() {
  if (allRanges.length === 0) return
  matchIndex.value = ((matchIndex.value - 2 + allRanges.length) % allRanges.length) + 1
  applyHighlights()
  scrollToMatch(allRanges[matchIndex.value - 1])
}

function openFind() {
  findVisible.value = true
  nextTick(() => {
    findInput.value?.focus()
    findInput.value?.select()
    if (findQuery.value) runSearch()
  })
}

function closeFind() {
  clearHighlights()
  allRanges = []
  matchCount.value = 0
  matchIndex.value = 0
  findQuery.value = ''
  findVisible.value = false
  // Re-set content so Vditor replaces all Text nodes, invalidating any stale Highlight ranges.
  // Pass `clearStack=true` so this internal reset does NOT emit `input` — otherwise merely
  // opening/closing the find bar would mark the note dirty and trigger an autosave.
  if (vd) vd.setValue(vd.getValue(), true)
}

// Use @input on the input element instead of a watcher to avoid async flush races

function onWrapperKeydown(e: KeyboardEvent) {
  if ((e.metaKey || e.ctrlKey) && e.key === 'f') {
    e.preventDefault()
    e.stopPropagation()
    openFind()
  }
}

function onFindKeydown(e: KeyboardEvent) {
  if (e.key === 'Enter') {
    e.preventDefault()
    e.stopPropagation()
    if (e.shiftKey) findPrev()
    else findNext()
  } else if (e.key === 'Escape') {
    e.stopPropagation()
    closeFind()
  }
}
</script>

<template>
  <div class="vditor-wrapper" @keydown="onWrapperKeydown">
    <!-- Find bar -->
    <Transition name="find-bar">
      <div v-if="findVisible" class="find-bar">
        <input
          ref="findInput"
          v-model="findQuery"
          class="find-input"
          placeholder="搜索..."
          @input="runSearch"
          @keydown="onFindKeydown"
        />
        <span class="find-count">
          {{ findQuery ? (matchCount === 0 ? '无结果' : `${matchIndex} / ${matchCount}`) : '' }}
        </span>
        <button class="find-nav" :disabled="matchCount === 0" title="上一个 (Shift+Enter)" @mousedown.prevent @click="findPrev">
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="18 15 12 9 6 15"/></svg>
        </button>
        <button class="find-nav" :disabled="matchCount === 0" title="下一个 (Enter)" @mousedown.prevent @click="findNext">
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"/></svg>
        </button>
        <button class="find-close" title="关闭 (Esc)" @mousedown.prevent @click="closeFind">
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>
        </button>
      </div>
    </Transition>

    <div ref="containerEl" class="vditor-host" />
  </div>
</template>

<style scoped>
.vditor-wrapper {
  position: absolute;
  inset: 0;
  overflow: hidden;
}

.vditor-host {
  position: absolute;
  inset: 0;
  overflow-y: auto;
}

/* Find bar */
.find-bar {
  position: absolute;
  top: 8px;
  right: 12px;
  z-index: 100;
  display: flex;
  align-items: center;
  gap: 4px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  padding: 4px 6px;
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
}

.find-input {
  width: 160px;
  height: 22px;
  font-size: 12px;
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  padding: 0 6px;
  outline: none;
}
.find-input:focus {
  border-color: var(--accent);
}

.find-count {
  font-size: 11px;
  color: var(--text-tertiary);
  min-width: 52px;
  text-align: center;
  flex-shrink: 0;
}

.find-nav {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.find-nav:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.find-nav:disabled {
  opacity: 0.35;
  cursor: default;
}

.find-close {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.find-close:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}

/* Transition */
.find-bar-enter-active,
.find-bar-leave-active {
  transition: opacity 0.12s, transform 0.12s;
}
.find-bar-enter-from,
.find-bar-leave-to {
  opacity: 0;
  transform: translateY(-4px);
}
</style>

<!-- CSS Custom Highlight API — global, not scoped -->
<style>
::highlight(vditor-find-all) {
  background-color: rgba(255, 200, 0, 0.4);
  color: inherit;
}
::highlight(vditor-find-current) {
  background-color: rgba(255, 140, 0, 0.7);
  color: #000;
}
</style>
