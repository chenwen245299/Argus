<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useReaderStore } from '../../stores/reader'
import { useLibraryStore } from '../../stores/library'
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
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M12 20h9"/>
        <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/>
      </svg>
      <p>{{ t('hl.openPdf') }}</p>
      <span>{{ t('hl.doubleClick') }}</span>
    </div>

    <div v-else-if="sortedHighlights.length === 0" class="empty">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M12 20h9"/>
        <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z"/>
      </svg>
      <p>{{ t('hl.noHighlights') }}</p>
      <span>{{ t('hl.selectToHighlight') }}</span>
    </div>

    <div v-else class="list">
      <div v-for="hl in sortedHighlights" :key="hl.id" class="highlight-item">
        <div class="hl-color-bar" :style="{ background: hl.color }" />
        <div class="hl-body">
          <p class="hl-text" :style="{ background: colorStyle(hl.color) }">{{ hl.text }}</p>
          <div class="hl-meta">
            <span v-if="!isEbook" class="hl-page">p.{{ hl.page }}</span>
            <span v-if="hl.note" class="hl-note">{{ hl.note }}</span>
          </div>

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
              <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <path d="M5 12h14M12 5l7 7-7 7"/>
              </svg>
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
.hl-note { color: var(--text-secondary); font-style: italic; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }

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
  resize: none;
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
