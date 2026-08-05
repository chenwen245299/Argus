<script setup lang="ts">
import { computed, ref } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { useReaderStore } from '../../stores/reader'
import { useLibraryStore } from '../../stores/library'
import { renderMarkdown } from '../../utils/renderMarkdown'
import { isEbookFileType } from '../../types'

const { t } = useI18n()
const reader = useReaderStore()
const library = useLibraryStore()

const isReaderActive = computed(() => !!reader.openSlug)

// Ebook highlights anchor to spine chapters (cover/preface files count too),
// so the raw number would mislead — hide it and rely on the jump button.
const isEbook = computed(() => {
  const slug = reader.openSlug
  if (!slug) return false
  const tab = reader.tabs.find(tb => tb.slug === slug)
  return isEbookFileType(tab?.fileType ?? library.papers.find(p => p.slug === slug)?.file_type)
})

const sortedHighlights = computed(() => {
  return [...reader.highlights].sort((a, b) => {
    if (a.page !== b.page) return a.page - b.page
    return (a.rects[0]?.y ?? 0) - (b.rects[0]?.y ?? 0)
  })
})

function jumpTo(id: string) {
  reader.jumpToHighlight(id)
}

function deleteHighlight(id: string) {
  reader.removeHighlight(id)
}

const editingNoteId = ref<string | null>(null)
const editingNoteValue = ref('')

function startEditNote(id: string) {
  const hl = reader.highlights.find(h => h.id === id)
  if (!hl) return
  editingNoteId.value = id
  editingNoteValue.value = hl.note ?? ''
}

function saveNote(id: string) {
  reader.updateHighlight(id, { note: editingNoteValue.value || undefined })
  editingNoteId.value = null
}

function cancelNote() {
  editingNoteId.value = null
}

// Same markdown + $TeX$ rendering as the reader's note popup, so a formula reads
// the same on both sides instead of showing raw source here.
function noteHtml(note: string): string {
  return renderMarkdown(note)
}

function colorStyle(color: string, alpha = 0.35): string {
  const h = color.replace('#', '')
  const r = parseInt(h.substring(0, 2), 16)
  const g = parseInt(h.substring(2, 4), 16)
  const b = parseInt(h.substring(4, 6), 16)
  return `rgba(${r},${g},${b},${alpha})`
}
</script>

<template>
  <div class="highlights-tab">
    <div v-if="!isReaderActive" class="empty">
      <Icon icon="fluent:highlight-24-regular" width="28" height="28" />
      <p>{{ t('hl.openPdf') }}</p>
      <span>{{ t('hl.doubleClick') }}</span>
    </div>

    <div v-else-if="sortedHighlights.length === 0" class="empty">
      <Icon icon="fluent:highlight-24-regular" width="28" height="28" />
      <p>{{ t('hl.noHighlights') }}</p>
      <span>{{ t('hl.selectToHighlight') }}</span>
    </div>

    <div v-else class="list">
      <div v-for="hl in sortedHighlights" :key="hl.id" class="highlight-item">
        <div class="hl-color-bar" :style="{ background: hl.color }" />
        <div class="hl-body">
          <p class="hl-text" :style="{ background: colorStyle(hl.color) }">{{ hl.text }}</p>
          <div v-if="!isEbook" class="hl-meta">
            <span class="hl-page">p.{{ hl.page }}</span>
          </div>
          <div v-if="hl.note" class="hl-note" v-html="noteHtml(hl.note)" />

          <div v-if="editingNoteId === hl.id" class="note-editor">
            <textarea
              v-model="editingNoteValue"
              class="note-input"
              rows="2"
              :placeholder="t('hl.notePlaceholder')"
              @keydown.enter.ctrl="saveNote(hl.id)"
              @keydown.escape="cancelNote"
            />
            <div class="note-actions">
              <button class="note-save" @click="saveNote(hl.id)">{{ t('hl.save') }}</button>
              <button class="note-cancel" @click="cancelNote">{{ t('hl.cancel') }}</button>
            </div>
          </div>

          <div class="hl-actions" v-if="editingNoteId !== hl.id">
            <button class="act-btn" @click="jumpTo(hl.id)">
              <Icon icon="fluent:arrow-right-24-regular" width="12" height="12" />
              {{ t('hl.go') }}
            </button>
            <button class="act-btn" @click="startEditNote(hl.id)">{{ t('hl.note') }}</button>
            <button class="act-btn danger" @click="deleteHighlight(hl.id)">{{ t('hl.delete') }}</button>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.highlights-tab { height: 100%; overflow-y: auto; display: flex; flex-direction: column; }

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

.list { padding: 8px; flex: 1; }

.highlight-item {
  display: flex;
  gap: 8px;
  padding: 8px;
  border-radius: var(--radius-md);
  margin-bottom: 6px;
  border: 1px solid var(--border-subtle);
  background: var(--bg-secondary);
}

.hl-color-bar { width: 4px; border-radius: 2px; flex-shrink: 0; min-height: 20px; }
.hl-body { flex: 1; min-width: 0; }

.hl-text {
  font-size: var(--font-size-sm);
  line-height: 1.5;
  color: var(--text-primary);
  padding: 2px 4px;
  border-radius: 2px;
  margin-bottom: 4px;
  display: -webkit-box;
  -webkit-line-clamp: 4;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.hl-meta { display: flex; gap: 8px; font-size: var(--font-size-xs); color: var(--text-tertiary); margin-bottom: 4px; }
.hl-page { flex-shrink: 0; }

/* Rendered note preview: markdown + KaTeX, kept tight so list rows stay compact. */
.hl-note {
  font-size: var(--font-size-xs);
  line-height: 1.5;
  color: var(--text-secondary);
  word-break: break-word;
  margin-bottom: 6px;
}
.hl-note :deep(> *:first-child) { margin-top: 0; }
.hl-note :deep(> *:last-child) { margin-bottom: 0; }
.hl-note :deep(p) { margin: 0 0 4px; }
.hl-note :deep(ul),
.hl-note :deep(ol) { margin: 0 0 4px; padding-left: 16px; }
.hl-note :deep(li) { margin: 1px 0; }
.hl-note :deep(h1),
.hl-note :deep(h2),
.hl-note :deep(h3),
.hl-note :deep(h4) { font-size: var(--font-size-xs); font-weight: 600; margin: 4px 0 2px; color: var(--text-primary); }
.hl-note :deep(blockquote) { margin: 0 0 4px; padding-left: 6px; border-left: 2px solid var(--border-subtle); }
.hl-note :deep(img) { max-width: 100%; height: auto; }
.hl-note :deep(table) { display: block; overflow-x: auto; max-width: 100%; }
.hl-note :deep(pre) { max-width: 100%; overflow-x: auto; }
.hl-note :deep(.md-code-block) { margin: 4px 0; }
.hl-note :deep(.katex) { font-size: 1.05em; }
.hl-note :deep(.katex-display) { margin: 4px 0; overflow-x: auto; overflow-y: hidden; }
.hl-note :deep(.katex-display > .katex) { font-size: 1.1em; }

.hl-actions { display: flex; gap: 4px; flex-wrap: wrap; }

.act-btn {
  display: inline-flex;
  align-items: center;
  gap: 3px;
  font-size: var(--font-size-xs);
  padding: 2px 7px;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
  background: var(--bg-primary);
  color: var(--text-secondary);
  transition: background 0.1s, color 0.1s;
  cursor: pointer;
}
.act-btn:hover { background: var(--bg-tertiary); color: var(--text-primary); }
.act-btn.danger { color: #cc3333; }
.act-btn.danger:hover { background: #fff0f0; border-color: #ffcccc; }

.note-editor { margin-top: 6px; }
.note-input {
  width: 100%;
  font-size: var(--font-size-xs);
  padding: 4px 6px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  resize: vertical;
  min-height: 44px;
  font-family: inherit;
  line-height: 1.5;
  box-sizing: border-box;
}

.note-actions { display: flex; gap: 4px; margin-top: 4px; }

.note-save {
  font-size: var(--font-size-xs);
  padding: 2px 8px;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-sm);
  transition: background 0.1s;
}
.note-save:hover { background: var(--accent-hover); }

.note-cancel {
  font-size: var(--font-size-xs);
  padding: 2px 8px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
}
</style>
