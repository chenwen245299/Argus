<script setup lang="ts">
import { computed, ref, useId, watch, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { noteText, textStatistics, noteSelectionText } from '../utils/noteStatistics'

const props = defineProps<{
  markdown: string
  textRoot: HTMLElement | null
  active: boolean
  saveState: 'saved' | 'saving' | 'unsaved' | 'error' | 'deleted'
  saveError?: string
}>()
const { t, locale } = useI18n()
const bar = ref<HTMLElement | null>(null), expanded = ref(false), selectedText = ref('')
const id = useId()
const countedMarkdown = ref(props.markdown)
const statistics = computed(() => textStatistics(noteText(countedMarkdown.value)))
const selected = computed(() => selectedText.value ? textStatistics(selectedText.value) : null)
const number = (value: number) => value.toLocaleString(locale.value)
const countLabel = computed(() => selected.value
  ? t('libraryNote.selectionWords', { selected: number(selected.value.words), total: number(statistics.value.words) })
  : t('libraryNote.wordCount', { n: number(statistics.value.words) }))
const saveLabel = computed(() => t(props.saveState === 'saving' ? 'notes.saving'
  : props.saveState === 'error' ? 'libraryNote.saveIssue'
  : props.saveState === 'deleted' ? 'libraryNote.deletedStatus' : `libraryNote.${props.saveState}`))
let frame = 0
let countTimer: ReturnType<typeof setTimeout> | undefined
watch(() => props.markdown, value => {
  clearTimeout(countTimer)
  countTimer = setTimeout(() => { countedMarkdown.value = value }, 120)
})

function updateSelection() {
  cancelAnimationFrame(frame)
  frame = requestAnimationFrame(() => {
    const selection = window.getSelection()
    const inputFocused = document.activeElement?.matches('input,textarea')
    selectedText.value = props.active && !inputFocused && props.textRoot && selection?.rangeCount && !selection.isCollapsed
      ? noteSelectionText(props.textRoot, selection.getRangeAt(0)) : ''
  })
}
function outside(event: PointerEvent) {
  if (!bar.value?.contains(event.target as Node)) expanded.value = false
}
function onKey(event: KeyboardEvent) {
  if (event.key === 'Escape') expanded.value = false
}
watch([() => props.textRoot, () => props.active], () => { selectedText.value = ''; expanded.value = false; updateSelection() })
onMounted(() => {
  document.addEventListener('selectionchange', updateSelection)
  document.addEventListener('pointerdown', outside, true)
  window.addEventListener('keydown', onKey)
})
onBeforeUnmount(() => {
  cancelAnimationFrame(frame)
  clearTimeout(countTimer)
  document.removeEventListener('selectionchange', updateSelection)
  document.removeEventListener('pointerdown', outside, true)
  window.removeEventListener('keydown', onKey)
})
</script>

<template>
  <footer ref="bar" class="note-status-bar" :aria-label="t('libraryNote.statusBar')">
    <div class="note-status-metrics">
      <button :id="`${id}-button`" type="button" class="note-count-button" :aria-label="t('libraryNote.statistics')" :aria-expanded="expanded" :aria-controls="`${id}-statistics`" @mousedown.prevent @click="expanded = !expanded">
        <span>{{ countLabel }}</span><Icon icon="fluent:chevron-up-16-regular" width="12" :class="{ expanded }" />
      </button>
      <span class="note-character-count">{{ t('libraryNote.characterCount', { n: number(statistics.characters) }) }}</span>
      <span class="note-reading-time">{{ t('libraryNote.readingTime', { n: number(statistics.readingMinutes) }) }}</span>
    </div>
    <div class="note-status-document">
      <span class="note-save-state" :class="props.saveState" :title="saveError || saveLabel" role="status" aria-live="polite"><span class="note-save-dot" aria-hidden="true" />{{ saveLabel }}</span>
      <span class="note-document-format">Markdown</span>
    </div>
    <div v-if="expanded" :id="`${id}-statistics`" class="note-statistics-popover" role="region" :aria-labelledby="`${id}-button`">
      <div class="note-statistics-title">{{ t('libraryNote.statistics') }}</div>
      <dl>
        <div><dt>{{ t('libraryNote.wordsLabel') }}</dt><dd>{{ number(statistics.words) }}</dd></div>
        <div><dt>{{ t('libraryNote.charactersLabel') }}</dt><dd>{{ number(statistics.characters) }}</dd></div>
        <div><dt>{{ t('libraryNote.charactersWithSpacesLabel') }}</dt><dd>{{ number(statistics.charactersWithSpaces) }}</dd></div>
        <div><dt>{{ t('libraryNote.paragraphsLabel') }}</dt><dd>{{ number(statistics.paragraphs) }}</dd></div>
        <div><dt>{{ t('libraryNote.readingTimeLabel') }}</dt><dd>{{ t('libraryNote.minutes', { n: number(statistics.readingMinutes) }) }}</dd></div>
      </dl>
      <dl v-if="selected" class="note-selection-statistics">
        <div><dt>{{ t('libraryNote.selectedWordsLabel') }}</dt><dd>{{ number(selected.words) }}</dd></div>
        <div><dt>{{ t('libraryNote.selectedCharactersLabel') }}</dt><dd>{{ number(selected.characters) }}</dd></div>
      </dl>
      <p>{{ t('libraryNote.countingHint') }}</p>
    </div>
  </footer>
</template>

<style scoped>
.note-status-bar { position: relative; display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 4px 16px; min-height: 30px; padding: 3px 14px; flex-shrink: 0; border-top: 1px solid var(--border-subtle); background: var(--bg-primary); color: var(--text-tertiary); font-size: var(--font-size-xs); font-variant-numeric: tabular-nums; container-type: inline-size; }
.note-status-metrics,.note-status-document { display: flex; align-items: center; flex-wrap: wrap; gap: 6px 16px; min-width: 0; }
.note-count-button { display: inline-flex; align-items: center; gap: 5px; padding: 3px 5px; margin-left: -5px; color: var(--text-secondary); border-radius: var(--radius-sm); font-size: inherit; white-space: nowrap; }
.note-count-button:hover,.note-count-button[aria-expanded="true"] { background: var(--bg-secondary); color: var(--text-primary); }
.note-count-button:focus-visible { outline: 2px solid var(--accent); outline-offset: 1px; }
.note-count-button .expanded { color: var(--accent); }
.note-save-state { display: inline-flex; align-items: center; gap: 5px; white-space: nowrap; }
.note-save-dot { width: 5px; height: 5px; border-radius: 50%; background: currentColor; }
.note-save-state.saved { color: var(--text-tertiary); }
.note-save-state.saving { color: var(--accent); }
.note-save-state.unsaved { color: var(--text-secondary); }
.note-save-state.error,.note-save-state.deleted { color: var(--color-danger, #c33); }
.note-statistics-popover { position: absolute; left: 10px; bottom: calc(100% + 6px); z-index: 40; width: 270px; max-width: calc(100% - 20px); box-sizing: border-box; padding: 14px; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); background: var(--bg-primary); color: var(--text-secondary); box-shadow: var(--shadow-md); }
.note-statistics-title { color: var(--text-primary); font-weight: 600; margin-bottom: 10px; }
dl { display: flex; flex-direction: column; gap: 7px; margin: 0; }
dl > div { display: flex; justify-content: space-between; gap: 12px; }
dd { margin: 0; color: var(--text-primary); }
.note-selection-statistics { margin-top: 10px; padding-top: 10px; border-top: 1px solid var(--border-subtle); }
.note-statistics-popover p { margin: 12px 0 0; line-height: 1.5; font-size: 11px; color: var(--text-tertiary); }
@container (max-width: 600px) { .note-character-count,.note-document-format { display: none; } }
@container (max-width: 400px) { .note-reading-time { display: none; } }
</style>
