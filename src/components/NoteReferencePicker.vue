<script setup lang="ts">
import { computed, nextTick, onMounted, onBeforeUnmount, ref, useId, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getReferenceTrigger, insertReference, referenceTriggerValid, restoreReferenceCaret, type ReferenceTrigger } from '../utils/noteReferenceTrigger'
import { installPaperReferenceNavigation, matchingReferences, referenceMarkdown, type ReferencePaper } from '../utils/paperReferences'
import { popupShift } from '../utils/popupFit'

const { t } = useI18n()
const trigger = ref<ReferenceTrigger | null>(null), papers = ref<ReferencePaper[]>([])
const popup = ref<HTMLElement | null>(null), loading = ref(false), choosing = ref(false), error = ref(''), toast = ref('')
const searchInput = ref<HTMLInputElement | null>(null), query = ref(''), stage = ref<'command' | 'papers'>('command')
const activeIndex = ref(0), x = ref(0), y = ref(0), listId = useId()
const matches = computed(() => matchingReferences(papers.value, query.value))
const options = computed(() => matches.value.slice(0, 20))
let root: string | null = null, revision = 0, insertionRevision = 0, frame = 0, disposed = false
let dismissedEditor: HTMLElement | null = null
let unlisten: UnlistenFn | undefined, toastTimer: ReturnType<typeof setTimeout> | undefined

function close() {
  revision++; insertionRevision++; choosing.value = false
  trigger.value = null; root = null; papers.value = []; error.value = ''; loading.value = false
  stage.value = 'command'; query.value = ''; activeIndex.value = 0
}
function dismiss() {
  const old = trigger.value
  if (!old) return
  dismissedEditor = old.editor
  close()
  restoreReferenceCaret(old)
}
async function position() {
  const current = trigger.value
  if (!current) return
  const rects = current.range ? Array.from(current.range.getClientRects()) : []
  const rect = rects[rects.length - 1]
  x.value = Math.max(8, Math.min(rect?.left ?? current.x, innerWidth - Math.min(stage.value === 'papers' ? 420 : 280, innerWidth - 16) - 8))
  y.value = (rect?.bottom ?? current.y) + 8
  await nextTick()
  if (!popup.value || trigger.value !== current) return
  const { dx, dy } = popupShift(popup.value.getBoundingClientRect(), { width: innerWidth, height: innerHeight }, { flipGap: 8 })
  x.value += dx; y.value += dy
}
async function update() {
  if (disposed) return
  if (trigger.value && stage.value === 'papers') {
    // Search has its own input. Keep the source range, rather than interpreting
    // the popup's caret as a new note insertion position.
    const active = document.activeElement
    const ownsFocus = !!active && (popup.value?.contains(active) || trigger.value.editor.contains(active))
    if (ownsFocus && referenceTriggerValid(trigger.value)) { void position(); return }
    close()
  }
  const next = getReferenceTrigger()
  if (!next) { if (trigger.value) close(); return }
  if (dismissedEditor === next.editor) return
  trigger.value = next
  void position()
  if (next.kind === 'reference') void openPapers()
}
async function openPapers() {
  const next = getReferenceTrigger() ?? trigger.value
  if (!next || !referenceTriggerValid(next)) { close(); return }
  trigger.value = next
  const old = trigger.value
  stage.value = 'papers'; query.value = old.query
  activeIndex.value = 0
  const current = ++revision
  papers.value = []; loading.value = true; error.value = ''
  await nextTick()
  if (disposed || current !== revision || trigger.value !== old) return
  searchInput.value?.focus({ preventScroll: true })
  void position()
  try {
    const currentRoot = await invoke<string | null>('get_current_library')
    if (disposed || current !== revision) return
    const expected = old.editor.closest<HTMLElement>('[data-note-library-root]')?.dataset.noteLibraryRoot
    if (!currentRoot || expected && expected !== currentRoot) throw new Error(t('noteReference.libraryChanged'))
    const catalog = await invoke<ReferencePaper[]>('list_note_reference_papers', { expectedRoot: currentRoot })
    if (disposed || current !== revision) return
    root = currentRoot; papers.value = catalog; void position()
  } catch (cause) {
    if (!disposed && current === revision) { error.value = String(cause); void position() }
  } finally {
    if (current === revision) loading.value = false
  }
}
function schedule(event?: Event) {
  if (event?.type === 'input' && !popup.value?.contains(event.target as Node)) {
    // Cancelling /ref must let the user continue ordinary prose. Only a new
    // slash (or removal of the old trigger) arms command suggestions again.
    if ((event as InputEvent).data?.includes('/') || !getReferenceTrigger()) dismissedEditor = null
  }
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(() => { void update() })
}
async function choose(paper: ReferencePaper) {
  const old = trigger.value, expectedRoot = root
  if (!old || !expectedRoot || choosing.value) return
  const operation = insertionRevision
  choosing.value = true
  try {
    const resolved = await invoke<ReferencePaper>('resolve_note_reference', { paperId: paper.id, expectedRoot })
    if (disposed || operation !== insertionRevision || trigger.value !== old) return
    if (!referenceTriggerValid(old)) { close(); return }
    if (await invoke<string | null>('get_current_library') !== expectedRoot) throw new Error(t('noteReference.libraryChanged'))
    // The popup owns focus, but the original note and exact trigger must still
    // match after both IPC reads. Query text never becomes note content.
    if (operation !== insertionRevision) return
    if (!referenceTriggerValid(old)) { close(); return }
    if (!insertReference(old, referenceMarkdown(resolved))) throw new Error(t('noteReference.insertFailed'))
    close()
  } catch (cause) { if (operation === insertionRevision) { error.value = String(cause); void position() } }
  finally { if (operation === insertionRevision) choosing.value = false }
}
function onKey(event: KeyboardEvent) {
  if (!trigger.value || event.isComposing || event.keyCode === 229) return
  const target = event.target as Node | null
  if (!target || !popup.value?.contains(target) && !trigger.value.editor.contains(target)) { close(); return }
  if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); dismiss(); return }
  if (event.metaKey || event.ctrlKey || event.altKey) return
  if (stage.value === 'command') {
    if (['Enter', 'Tab', 'ArrowDown', 'ArrowUp'].includes(event.key)) {
      event.preventDefault(); event.stopImmediatePropagation()
      if (event.key === 'Enter' || event.key === 'Tab') void openPapers()
    }
    return
  }
  if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
    event.preventDefault(); event.stopImmediatePropagation()
    if (!options.value.length || choosing.value) return
    activeIndex.value = (activeIndex.value + (event.key === 'ArrowDown' ? 1 : options.value.length - 1)) % options.value.length
    void nextTick(() => popup.value?.querySelector('[aria-selected="true"]')?.scrollIntoView({ block: 'nearest' }))
  } else if ((event.key === 'Enter' && !event.shiftKey) || event.key === 'Tab') {
    event.preventDefault(); event.stopImmediatePropagation()
    const paper = options.value[activeIndex.value] ?? options.value[0]
    if (paper && !choosing.value) void choose(paper)
  }
}
function outside(event: PointerEvent) {
  const target = event.target as Node
  if (trigger.value && !popup.value?.contains(target)
    && (stage.value === 'papers' || !trigger.value.editor.contains(target))) dismiss()
}
function keepInputFocus(event: MouseEvent | PointerEvent) {
  if (!(event.target as Element).closest('input')) event.preventDefault()
}
watch(query, () => {
  activeIndex.value = 0; void position()
  void nextTick(() => { const list = popup.value?.querySelector('.note-reference-list'); if (list) list.scrollTop = 0 })
})
function showError(event: Event) {
  toast.value = t('noteReference.openFailed', { error: (event as CustomEvent<string>).detail })
  clearTimeout(toastTimer); toastTimer = setTimeout(() => { toast.value = '' }, 6000)
}
onMounted(async () => {
  installPaperReferenceNavigation()
  document.addEventListener('input', schedule, true)
  document.addEventListener('keyup', schedule, true)
  document.addEventListener('selectionchange', schedule)
  document.addEventListener('focusin', schedule)
  document.addEventListener('keydown', onKey, true)
  document.addEventListener('pointerdown', outside, true)
  document.addEventListener('scroll', schedule, true)
  window.addEventListener('resize', schedule)
  window.addEventListener('argus-note-reference-error', showError)
  unlisten = await listen('library-changed', () => { dismissedEditor = trigger.value?.editor ?? null; cancelAnimationFrame(frame); close() })
  if (disposed) unlisten()
})
onBeforeUnmount(() => {
  disposed = true; close(); cancelAnimationFrame(frame); clearTimeout(toastTimer); unlisten?.()
  document.removeEventListener('input', schedule, true)
  document.removeEventListener('keyup', schedule, true)
  document.removeEventListener('selectionchange', schedule)
  document.removeEventListener('focusin', schedule)
  document.removeEventListener('keydown', onKey, true)
  document.removeEventListener('pointerdown', outside, true)
  document.removeEventListener('scroll', schedule, true)
  window.removeEventListener('resize', schedule)
  window.removeEventListener('argus-note-reference-error', showError)
})
</script>

<template>
  <Teleport to="body">
    <div v-if="trigger" ref="popup" class="note-reference-popup" :class="{ 'command-menu': stage === 'command' }" :style="{ left: `${x}px`, top: `${y}px` }" @pointerdown="keepInputFocus" @mousedown="keepInputFocus">
      <template v-if="stage === 'command'">
        <button type="button" tabindex="-1" class="note-reference-command" @click="openPapers">
          <span class="note-reference-command-name">/ref</span><span>{{ t('noteReference.title') }}</span>
        </button>
        <div class="note-reference-footer">{{ t('noteReference.commandKeyboard') }}</div>
      </template>
      <template v-else>
        <div class="note-reference-heading"><strong>{{ t('noteReference.title') }}</strong><button type="button" tabindex="-1" :aria-label="t('noteReference.close')" @click="dismiss">×</button></div>
        <div class="note-reference-search-wrap">
          <input ref="searchInput" v-model="query" class="note-reference-search" :placeholder="t('noteReference.hint')" :aria-label="t('noteReference.hint')" autocomplete="off" :spellcheck="false" :readonly="choosing" role="combobox" aria-expanded="true" :aria-controls="listId" :aria-activedescendant="options.length ? `${listId}-${activeIndex}` : undefined" />
        </div>
        <div v-if="error" class="note-reference-feedback error" role="alert">{{ error }}</div>
        <div v-else-if="loading" class="note-reference-feedback" role="status">{{ t('noteReference.loading') }}</div>
        <div v-else-if="!options.length" class="note-reference-feedback" role="status">{{ t('noteReference.empty') }}</div>
        <div :id="listId" class="note-reference-list" role="listbox" :aria-label="t('noteReference.title')" :aria-busy="loading || choosing">
          <button v-for="(paper, index) in options" :id="`${listId}-${index}`" :key="paper.id + paper.slug" type="button" tabindex="-1" role="option" :aria-selected="index === activeIndex" class="note-reference-option" :disabled="choosing" @mouseenter="activeIndex = index" @click="choose(paper)">
            <span class="note-reference-paper-title">{{ paper.title }}</span>
            <span v-if="paper.authors.length || paper.year" class="note-reference-details"><span>{{ paper.authors.slice(0, 2).join(' · ') }}</span><span v-if="paper.year" class="note-reference-year">{{ paper.year }}</span></span>
          </button>
        </div>
        <div class="note-reference-footer">{{ matches.length > 20 ? t('noteReference.more') : t('noteReference.keyboard') }}</div>
      </template>
    </div>
    <div v-if="toast" class="note-reference-toast" role="alert">{{ toast }}<button type="button" :aria-label="t('noteReference.close')" @click="toast = ''">×</button></div>
  </Teleport>
</template>

<style scoped>
.note-reference-popup { position: fixed; z-index: 20000; display: flex; flex-direction: column; width: 420px; max-width: calc(100vw - 16px); max-height: calc(100vh - 16px); border: 1px solid var(--border-default); border-radius: var(--radius-md); background: var(--bg-primary); color: var(--text-primary); box-shadow: var(--shadow-lg); font-size: var(--font-size-sm); overflow: hidden; }
.note-reference-popup.command-menu { width: 280px; }
.note-reference-command { display: flex; align-items: center; gap: 14px; margin: 4px; padding: 10px; border-radius: var(--radius-sm); color: var(--text-primary); background: var(--accent-light); text-align: left; }
.note-reference-command-name { font-family: var(--font-mono); color: var(--accent); }
.note-reference-heading { display: flex; align-items: center; justify-content: space-between; gap: 10px; padding: 10px 12px; border-bottom: 1px solid var(--border-subtle); }
.note-reference-heading button { color: var(--text-tertiary); font-size: 18px; line-height: 1; padding: 2px; }
.note-reference-search-wrap { padding: 10px 12px 6px; }
.note-reference-search { box-sizing: border-box; width: 100%; min-width: 0; padding: 8px 10px; border: 1px solid var(--border-default); border-radius: var(--radius-sm); background: var(--bg-secondary); color: var(--text-primary); font-size: inherit; }
.note-reference-search:focus { border-color: var(--accent); outline: 1px solid var(--accent); }
.note-reference-search::placeholder { color: var(--text-tertiary); }
.note-reference-list { min-height: 0; max-height: min(280px, 45vh); overflow-y: auto; overscroll-behavior: contain; padding: 4px; }
.note-reference-option { display: flex; flex-direction: column; gap: 4px; width: 100%; padding: 9px 10px; text-align: left; border-radius: var(--radius-sm); color: var(--text-primary); }
.note-reference-option[aria-selected="true"] { background: var(--accent-light); }
.note-reference-details { display: flex; justify-content: space-between; gap: 12px; color: var(--text-tertiary); font-size: var(--font-size-xs); }
.note-reference-details > span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.note-reference-year { flex-shrink: 0; color: var(--text-tertiary); }
.note-reference-paper-title { display: -webkit-box; -webkit-box-orient: vertical; -webkit-line-clamp: 2; overflow: hidden; line-height: 1.5; overflow-wrap: anywhere; }
.note-reference-feedback { padding: 16px 12px; color: var(--text-secondary); overflow-wrap: anywhere; }
.note-reference-feedback.error { color: var(--color-danger, #c33); }
.note-reference-footer { padding: 7px 12px; border-top: 1px solid var(--border-subtle); color: var(--text-tertiary); font-size: var(--font-size-xs); }
.note-reference-toast { position: fixed; z-index: 21000; bottom: 24px; left: 50%; transform: translateX(-50%); display: flex; align-items: center; gap: 12px; max-width: calc(100vw - 32px); padding: 12px 16px; border: 1px solid var(--border-default); border-radius: var(--radius-md); background: var(--bg-primary); box-shadow: var(--shadow-lg); color: var(--text-primary); font-size: var(--font-size-sm); overflow-wrap: anywhere; }
.note-reference-toast button { font-size: 18px; flex-shrink: 0; color: var(--text-secondary); }
</style>
