<script setup lang="ts">
import { computed, ref, watch, nextTick, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import MarkdownBody from './MarkdownBody.vue'
import { useReaderStore } from '../stores/reader'
import { HIGHLIGHT_COLORS } from '../utils/highlightColors'
import { markdownTextIndex, markdownSelection, markdownTextRects, markdownRange, locateMarkdownQuote } from '../utils/markdownAnchors'
import { selectionPopupAnchor, popupShift } from '../utils/popupFit'
import { triggerAskAi, runTranslation } from '../stores/translationHistory'
import { openAddSnippetModal } from '../stores/snippetLibrary'
import { renderMarkdown } from '../utils/renderMarkdown'
import type { Highlight } from '../types'

const props = defineProps<{
  slug: string; markdown: string; editing?: boolean; editorRoot?: HTMLElement | null
  color: string; markStyle: 'highlight' | 'underline'; fontSize: number
}>()
const emit = defineEmits<{ 'update:color': [value: string]; 'update:markStyle': [value: 'highlight' | 'underline'] }>()
const { t } = useI18n(), reader = useReaderStore()
const body = ref<HTMLElement | null>(null), scroller = ref<HTMLElement | null>(null), toolbar = ref<HTMLElement | null>(null)
const editorOverlay = ref<HTMLElement | null>(null)
const root = computed(() => props.editing ? props.editorRoot : body.value)
const origin = computed(() => props.editing ? editorOverlay.value : body.value)
const active = computed(() => reader.activeSlug === props.slug)
const marks = computed(() => reader.highlightsFor(props.slug))
const selected = ref<(NonNullable<ReturnType<typeof markdownSelection>> & { x: number; y: number }) | null>(null)
const editingId = ref<string | null>(null), note = ref('')
const color = computed({ get: () => props.color, set: value => emit('update:color', value) })
const style = computed({ get: () => props.markStyle, set: value => emit('update:markStyle', value) })
const paint = ref<{ id: string; key: string; style: Record<string, string> }[]>([])
const missing = ref(0)
const findVisible = ref(false), query = ref(''), findInput = ref<HTMLInputElement | null>(null)
const matchCount = ref(0), matchIndex = ref(0)
const searchPaint = ref<{ key: string; style: Record<string, string> }[]>([])
let resize: ResizeObserver | undefined, mutations: MutationObserver | undefined, frame = 0, disposed = false

function queuePaint() {
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(paintMarks)
}
function locate(index: ReturnType<typeof markdownTextIndex>, hl: Highlight) {
  let match = locateMarkdownQuote(index.text, hl)
  if (!match && hl.start_offset == null) {
    // AI quotes are exact Markdown source; formatted excerpts have different visible text.
    const el = document.createElement('div'); el.innerHTML = renderMarkdown(hl.text)
    const text = markdownTextIndex(el).text.trim()
    if (text !== hl.text) match = locateMarkdownQuote(index.text, { text })
  }
  return match
}
function paintMarks() {
  if (!root.value || !origin.value || !active.value || disposed) return
  const box = origin.value.getBoundingClientRect(), index = markdownTextIndex(root.value)
  const rects: typeof paint.value = []; let absent = 0
  for (const hl of marks.value) {
    const match = locate(index, hl)
    if (!match) { absent++; continue }
    for (const [i, r] of markdownTextRects(index, match.start, match.end).entries()) {
      if (!r.width || !r.height) continue
      const underline = hl.style === 'underline'
      rects.push({ id: hl.id, key: `${hl.id}:${i}`, style: {
        left: `${r.left - box.left}px`, top: `${r.top - box.top}px`, width: `${r.width}px`, height: `${r.height}px`,
        background: underline ? 'transparent' : `color-mix(in srgb, ${hl.color} 40%, transparent)`,
        borderBottom: underline ? `2px solid ${hl.color}` : 'none',
      } })
    }
  }
  paint.value = rects; missing.value = absent
  const found: { start: number; end: number }[] = []
  if (findVisible.value && query.value) {
    const escaped = query.value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
    for (const m of index.text.matchAll(new RegExp(escaped, 'giu'))) found.push({ start: m.index!, end: m.index! + m[0].length })
  }
  matchCount.value = found.length
  matchIndex.value = found.length ? Math.max(1, Math.min(matchIndex.value, found.length)) : 0
  searchPaint.value = found.flatMap((match, i) => markdownTextRects(index, match.start, match.end).map((r, j) => ({ key: `${i}:${j}`, style: {
    left: `${r.left - box.left}px`, top: `${r.top - box.top}px`, width: `${r.width}px`, height: `${r.height}px`,
    background: i === matchIndex.value - 1 ? '#ff980080' : '#ffeb3b60',
  } })))
}
function openFind() { findVisible.value = true; void nextTick(() => findInput.value?.focus()) }
function closeFind() { findVisible.value = false; query.value = ''; queuePaint() }
function findNext(step = 1) {
  if (!root.value || !matchCount.value) return
  matchIndex.value = (matchIndex.value - 1 + step + matchCount.value) % matchCount.value + 1
  paintMarks()
  const rect = searchPaint.value.find(r => r.key.startsWith(`${matchIndex.value - 1}:`))
  if (rect && origin.value) {
    const top = origin.value.getBoundingClientRect().top + parseFloat(rect.style.top)
    const scroll = props.editing ? root.value : scroller.value
    if (scroll) scroll.scrollTop += top - scroll.getBoundingClientRect().top - 100
  }
}
watch(query, () => { matchIndex.value = 1; void nextTick(() => { paintMarks(); findNext(0) }) })
async function selectText(event: MouseEvent) {
  const target = event.target instanceof Element ? event.target : null
  if (!active.value || !root.value || target?.closest('.md-selection,.md-annotation-editor,[data-markdown-overlay],.md-note-find,.note-header')) return
  const selection = window.getSelection()
  if (!selection?.rangeCount || selection.isCollapsed) { selected.value = null; return }
  const range = selection.getRangeAt(0), quote = markdownSelection(root.value, range)
  if (!quote) return
  const anchor = selectionPopupAnchor(Array.from(range.getClientRects()), { width: innerWidth, height: innerHeight })
  if (!anchor) return
  selected.value = { ...quote, ...anchor }; editingId.value = null
  await nextTick()
  if (toolbar.value && selected.value) {
    const { dx, dy } = popupShift(toolbar.value.getBoundingClientRect(), { width: innerWidth, height: innerHeight }, { flipGap: 12 })
    selected.value = { ...selected.value, x: selected.value.x + dx, y: selected.value.y + dy }
  }
}
function addMark(annotation = false, chosenColor = color.value) {
  const quote = selected.value
  if (!quote || !active.value) return
  color.value = chosenColor
  const existing = marks.value.find(h => h.text === quote.text && h.start_offset === quote.start && h.end_offset === quote.end && h.color === chosenColor && (h.style ?? 'highlight') === style.value)
  const hl: Highlight = { id: existing?.id ?? crypto.randomUUID(), page: 1, rects: [], text: quote.text, color: chosenColor,
    style: style.value, created_at: new Date().toISOString(), start_offset: quote.start, end_offset: quote.end,
    anchor_prefix: quote.prefix, anchor_suffix: quote.suffix }
  if (!existing) reader.addHighlight(hl)
  selected.value = null; window.getSelection()?.removeAllRanges()
  if (annotation) openAnnotation(hl.id)
}
function openAnnotation(id: string) {
  const mark = marks.value.find(h => h.id === id)
  editingId.value = id; note.value = mark?.note ?? ''; selected.value = null
  color.value = mark?.color ?? color.value; style.value = mark?.style ?? 'highlight'
}
function saveAnnotation() {
  if (active.value && editingId.value) reader.updateHighlight(editingId.value, { note: note.value || undefined, color: color.value, style: style.value })
  editingId.value = null
}
function selectedAction(action: 'ai' | 'translate' | 'snippet') {
  const text = selected.value?.text
  if (!text) return
  selected.value = null
  if (action === 'ai') triggerAskAi(text)
  else if (action === 'translate') void runTranslation(text)
  else openAddSnippetModal({ text, paperId: props.slug, paperTitle: reader.tabs.find(tab => tab.slug === props.slug)?.title ?? props.slug, page: 1 })
}
function outside(event: PointerEvent) {
  const target = event.target instanceof Element ? event.target : null
  if (!target?.closest('.md-selection,.md-annotation-editor,[data-markdown-overlay],.note-header')) selected.value = null
}
function onScroll() { selected.value = null; queuePaint() }
function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape') { selected.value = null; editingId.value = null; closeFind() }
  const target = event.target instanceof HTMLElement ? event.target : null
  if (active.value && (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'f'
    && (!target?.closest('input,textarea,[contenteditable="true"]') || !!root.value?.contains(target))) {
    event.preventDefault(); event.stopPropagation(); openFind()
  }
}
function onKeySelection(event: KeyboardEvent) {
  if (event.shiftKey || ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'a')) {
    void selectText({ target: event.target } as MouseEvent)
  }
}
watch([() => props.markdown, () => props.fontSize, marks, active], () => { void nextTick(queuePaint) }, { deep: true })
watch(active, value => { if (!value) { selected.value = null; editingId.value = null; closeFind() } })
watch([root, origin], ([el, layer]) => {
  resize?.disconnect(); mutations?.disconnect(); selected.value = null
  resize = new ResizeObserver(queuePaint)
  if (el) resize.observe(el)
  if (layer && layer !== el) resize.observe(layer)
  if (el && props.editing) {
    mutations = new MutationObserver(queuePaint)
    mutations.observe(el, { childList: true, characterData: true, attributes: true, attributeFilter: ['class', 'style', 'hidden'], subtree: true })
  }
  queuePaint()
}, { flush: 'post' })
watch([() => reader.scrollToHighlightId, active, root], async ([id]) => {
  if (!id || !active.value || !root.value || !marks.value.some(h => h.id === id)) return
  const el = root.value
  reader.scrollToHighlightId = null
  await nextTick()
  if (disposed || !active.value || root.value !== el) return
  paintMarks()
  const index = markdownTextIndex(el), mark = marks.value.find(h => h.id === id)
  if (!mark) return
  const match = locate(index, mark)
  if (match) markdownRange(index, match.start, match.end)?.startContainer.parentElement?.scrollIntoView({ block: 'center' })
  openAnnotation(id)
})
watch([() => reader.pendingPageJump, active, root], async ([page]) => {
  if (page == null || !active.value || !root.value) return
  const el = root.value
  reader.pendingPageJump = null
  await nextTick()
  if (disposed || !active.value || root.value !== el) return
  el.querySelectorAll<HTMLElement>('h1,h2,h3,h4,h5,h6')[page - 1]?.scrollIntoView({ block: 'start' })
})
onMounted(() => {
  document.addEventListener('pointerdown', outside, true)
  document.addEventListener('scroll', onScroll, true)
  window.addEventListener('mouseup', selectText, true); window.addEventListener('keydown', onKey, true); window.addEventListener('keyup', onKeySelection)
  queuePaint()
})
onBeforeUnmount(() => {
  disposed = true; resize?.disconnect(); mutations?.disconnect(); cancelAnimationFrame(frame)
  document.removeEventListener('pointerdown', outside, true)
  document.removeEventListener('scroll', onScroll, true)
  window.removeEventListener('mouseup', selectText, true); window.removeEventListener('keydown', onKey, true); window.removeEventListener('keyup', onKeySelection)
})
defineExpose({ openFind, highlight: (chosenColor: string) => { color.value = chosenColor; addMark(false, chosenColor) }, getText: () => root.value ? markdownTextIndex(root.value).text : '' })
</script>

<template>
  <div v-if="!editing" ref="scroller" class="md-note-reading" @load.capture="queuePaint">
    <p v-if="missing" class="md-anchor-warning">{{ t('libraryNote.missingMarks', { n: missing }) }}</p>
    <div class="md-reading-page">
      <article ref="body" class="markdown-body md-reading-body" :style="{ fontSize: fontSize + 'px' }"><MarkdownBody :content="markdown" /></article>
      <div class="md-mark-layer" data-markdown-overlay>
        <span v-for="rect in searchPaint" :key="rect.key" class="md-search-match" :style="rect.style" />
        <button v-for="rect in paint" :key="rect.key" class="md-mark" :data-hl-id="rect.id" :style="rect.style"
          :aria-label="t('hl.note')" @click="openAnnotation(rect.id)" />
      </div>
    </div>
  </div>
  <div v-else ref="editorOverlay" class="md-mark-layer md-editor-overlay" data-markdown-overlay>
    <span v-for="rect in searchPaint" :key="rect.key" class="md-search-match" :style="rect.style" />
    <button v-for="rect in paint" :key="rect.key" class="md-mark" :data-hl-id="rect.id" :style="rect.style" :aria-label="t('hl.note')" @click="openAnnotation(rect.id)" />
  </div>
  <div v-if="findVisible" class="md-note-find" :class="{ 'md-editor-find': editing }" data-markdown-overlay>
    <input ref="findInput" v-model="query" :placeholder="t('ebook.search')" :aria-label="t('ebook.search')" @keydown.enter.prevent="findNext($event.shiftKey ? -1 : 1)" />
    <span>{{ matchIndex }} / {{ matchCount }}</span>
    <button :disabled="!matchCount" @click="findNext(-1)">↑</button><button :disabled="!matchCount" @click="findNext(1)">↓</button><button @click="closeFind">×</button>
  </div>
  <Teleport to="body">
      <div v-if="selected && active" ref="toolbar" class="md-selection" :style="{ left: selected.x + 'px', top: selected.y + 'px' }" @mousedown.prevent>
        <div class="md-mark-colors"><button v-for="c in HIGHLIGHT_COLORS" :key="c.value" :title="t(c.labelKey)" :aria-label="t(c.labelKey)" :style="{ background: c.value }" :class="{ chosen: color === c.value }" @click="addMark(false, c.value)" /></div>
        <button @click="style = style === 'highlight' ? 'underline' : 'highlight'">{{ t(style === 'highlight' ? 'libraryNote.highlight' : 'libraryNote.underline') }}</button>
        <button @click="addMark()">{{ t('libraryNote.mark') }}</button>
        <button @click="addMark(true)">{{ t('hl.note') }}</button>
        <button @click="selectedAction('ai')">{{ t('pdf.askAi') }}</button>
        <button @click="selectedAction('translate')">{{ t('pdf.translate') }}</button>
        <button @click="selectedAction('snippet')">{{ t('pdf.snippet') }}</button>
      </div>
      <div v-if="editingId && active" class="md-annotation-editor">
        <div class="md-mark-colors"><button v-for="c in HIGHLIGHT_COLORS" :key="c.value" :title="t(c.labelKey)" :style="{ background: c.value }" :class="{ chosen: color === c.value }" @click="color = c.value" /></div>
        <button @click="style = style === 'highlight' ? 'underline' : 'highlight'">{{ t(style === 'highlight' ? 'libraryNote.highlight' : 'libraryNote.underline') }}</button>
        <textarea v-model="note" :aria-label="t('hl.notePlaceholder')" :placeholder="t('hl.notePlaceholder')" @keydown.ctrl.enter="saveAnnotation" @keydown.meta.enter="saveAnnotation" />
        <button @click="saveAnnotation">{{ t('hl.save') }}</button><button @click="editingId = null">{{ t('hl.cancel') }}</button>
        <button @click="reader.removeHighlight(editingId!); editingId = null">{{ t('hl.delete') }}</button>
      </div>
  </Teleport>
</template>

<style scoped>
.md-note-reading { flex: 1; min-height: 0; overflow: auto; padding: 24px; }
.md-reading-page { position: relative; max-width: 900px; margin: auto; }
.md-reading-body { position: relative; font-size: 16px; line-height: 1.8; overflow-wrap: anywhere; user-select: text; -webkit-user-select: text; }
.md-reading-body :deep(*) { user-select: text; -webkit-user-select: text; }
.md-reading-body :deep(img) { max-width: 100%; }
.md-mark-layer { position: absolute; inset: 0; pointer-events: none; }
.md-editor-overlay { overflow: hidden; z-index: 2; }
.md-search-match { position: absolute; pointer-events: none; }
.md-note-find { position: absolute; top: calc(var(--content-header-height) + 8px); right: 12px; z-index: 10; display: flex; align-items: center; gap: 6px; padding: 6px; border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); background: var(--bg-secondary); box-shadow: var(--shadow-md); font-size: 12px; }
.md-note-find input { width: 160px; background: var(--bg-primary); color: var(--text-primary); padding: 4px; user-select: text; }
.md-editor-find { top: 8px; }
.md-mark { position: absolute; pointer-events: auto; border: 0; padding: 0; border-radius: 2px; cursor: pointer; }
.md-selection,.md-annotation-editor { position: fixed; z-index: 1000; display: flex; flex-wrap: wrap; gap: 8px; padding: 10px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--bg-primary); box-shadow: var(--shadow-md); max-width: calc(100vw - 20px); color: var(--text-primary); }
.md-selection button,.md-annotation-editor button { font-size: 12px; padding: 4px; }
.md-mark-colors { display: flex; gap: 5px; align-items: center; }
.md-mark-colors button { width: 17px; height: 17px; border-radius: 50%; }
.md-mark-colors button.chosen { outline: 2px solid var(--text-primary); outline-offset: 1px; }
.md-annotation-editor { right: 24px; bottom: 24px; width: 360px; }
.md-annotation-editor textarea { flex: 1 1 100%; min-height: 100px; background: var(--bg-secondary); color: var(--text-primary); padding: 8px; resize: vertical; user-select: text; }
.md-anchor-warning { font-size: 12px; color: var(--text-secondary); }
</style>
