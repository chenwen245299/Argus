<script setup lang="ts">
import { ref, computed, watch, onMounted, onBeforeUnmount } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  libraries,
  snippets,
  createLibrary,
  addSnippet,
  pendingSnippet,
  type PendingSnippet,
} from '../stores/snippetLibrary'

const props = defineProps<{ pending: PendingSnippet }>()
const emit = defineEmits<{ close: [] }>()

const { t } = useI18n()

const SNIPPET_COLORS = [
  { labelKey: 'addSnippet.colorYellow', value: '#FFEB3B' },
  { labelKey: 'addSnippet.colorGreen', value: '#A5D6A7' },
  { labelKey: 'addSnippet.colorBlue', value: '#90CAF9' },
  { labelKey: 'addSnippet.colorPink', value: '#F48FB1' },
  { labelKey: 'addSnippet.colorOrange', value: '#FFCC80' },
  { labelKey: 'addSnippet.colorPurple', value: '#CE93D8' },
]

const selectedLibraryId = ref(libraries.value[0]?.id ?? '')
const showNewLibInput = ref(false)
const newLibName = ref('')
const tagInput = ref('')
const tagFocused = ref(false)
const tags = ref<string[]>([])
const note = ref('')
const selectedColor = ref(props.pending.color ?? '#CE93D8')

const sortedLibraries = computed(() => [...libraries.value])

// All distinct tags in the currently selected library
const libraryTags = computed(() => {
  if (!selectedLibraryId.value) return []
  const all = snippets.value
    .filter(s => s.libraryId === selectedLibraryId.value)
    .flatMap(s => s.tags)
  return [...new Set(all)]
})

// Filtered suggestions: match input, exclude already-added tags
const tagSuggestions = computed(() => {
  const q = tagInput.value.trim().toLowerCase()
  return libraryTags.value.filter(t =>
    !tags.value.includes(t) && (q === '' || t.toLowerCase().includes(q))
  )
})

const showSuggestions = computed(() => tagFocused.value && tagSuggestions.value.length > 0)

function pickSuggestion(tag: string) {
  if (!tags.value.includes(tag)) tags.value = [...tags.value, tag]
  tagInput.value = ''
}

function onTagBlur() {
  commitTag()
  setTimeout(() => { tagFocused.value = false }, 150)
}

function selectLibrary(id: string) {
  selectedLibraryId.value = id
  showNewLibInput.value = false
}

function startNewLib() {
  showNewLibInput.value = true
  newLibName.value = ''
}

async function submitNewLib() {
  const name = newLibName.value.trim()
  if (!name) { showNewLibInput.value = false; return }
  const lib = await createLibrary(name)
  selectedLibraryId.value = lib.id
  showNewLibInput.value = false
  newLibName.value = ''
}

function handleTagInput(e: KeyboardEvent) {
  // Ignore Enter/comma fired while an IME is composing (e.g. selecting Chinese
  // characters) — otherwise confirming the candidate commits a half-typed tag.
  if (e.isComposing || e.keyCode === 229) return
  if (e.key === 'Enter' || e.key === ',') {
    e.preventDefault()
    commitTag()
  } else if (e.key === 'Backspace' && tagInput.value === '' && tags.value.length > 0) {
    tags.value = tags.value.slice(0, -1)
  }
}

function commitTag() {
  const val = tagInput.value.replace(/,/g, '').trim()
  if (val && !tags.value.includes(val)) {
    tags.value = [...tags.value, val]
  }
  tagInput.value = ''
}

function removeTag(tag: string) {
  tags.value = tags.value.filter(t => t !== tag)
}

async function confirm() {
  if (!selectedLibraryId.value || !pendingSnippet.value) return
  commitTag()
  const pending = pendingSnippet.value
  await addSnippet({
    libraryId: selectedLibraryId.value,
    text: pending.text,
    tags: tags.value,
    note: note.value.trim(),
    paperId: pending.paperId,
    paperTitle: pending.paperTitle,
    page: pending.page,
    color: selectedColor.value,
  })
  // Trigger highlight creation in PdfViewer after successful save
  if (pending.rects?.length && pending.pageIndex !== undefined) {
    window.dispatchEvent(new CustomEvent('argus-snippet-highlight', {
      detail: {
        rects: pending.rects,
        pageIndex: pending.pageIndex,
        text: pending.text,
        color: selectedColor.value,
      }
    }))
  }
  emit('close')
}

function cancel() {
  emit('close')
}

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') cancel()
}

onMounted(() => window.addEventListener('keydown', onKeydown))
onBeforeUnmount(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <Teleport to="body">
    <div class="modal-overlay" @click.self="cancel">
      <div class="modal-card">
        <div class="modal-header">
          <span class="modal-title">{{ t('snippets.addToLibrary') }}</span>
          <button class="close-btn" @click="cancel">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        <!-- Original text preview with color highlight -->
        <div class="field-label">{{ t('snippets.sourceText') }}</div>
        <div class="text-preview" :style="{ background: selectedColor + '55', borderLeft: `3px solid ${selectedColor}` }">{{ pending.text }}</div>
        <div class="text-meta">{{ pending.paperTitle }}  ·  {{ t('addSnippet.page', { page: pending.page }) }}</div>

        <!-- Color picker -->
        <div class="color-row">
          <div
            v-for="c in SNIPPET_COLORS"
            :key="c.value"
            class="color-dot"
            :class="{ 'color-dot--active': selectedColor === c.value }"
            :style="{ background: c.value }"
            :title="t(c.labelKey)"
            @click="selectedColor = c.value"
          />
        </div>

        <!-- Library selector -->
        <div class="field-label">{{ t('snippets.selectLibrary') }}</div>
        <div class="lib-list">
          <button
            v-for="lib in sortedLibraries"
            :key="lib.id"
            class="lib-btn"
            :class="{ active: selectedLibraryId === lib.id }"
            @click="selectLibrary(lib.id)"
          >
            <span v-if="lib.emoji" class="lib-emoji">{{ lib.emoji }}</span>
            <svg v-else width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
            </svg>
            {{ lib.name }}
          </button>

          <template v-if="showNewLibInput">
            <input
              v-model="newLibName"
              class="new-lib-input"
              :placeholder="t('snippets.libraryName')"
              autofocus
              @keydown.enter="submitNewLib"
              @keydown.escape="showNewLibInput = false"
              @blur="submitNewLib"
            />
          </template>
          <button v-else class="lib-btn new-lib-btn" @click="startNewLib">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="12" y1="5" x2="12" y2="19"/><line x1="5" y1="12" x2="19" y2="12"/>
            </svg>
            {{ t('snippets.newLibrary') }}
          </button>
        </div>

        <!-- Tags -->
        <div class="field-label">{{ t('snippets.tags') }}</div>
        <div class="tag-editor-wrap">
          <div class="tag-editor" @click="($el as HTMLElement).querySelector('input')?.focus()">
            <span v-for="tag in tags" :key="tag" class="tag-chip">
              {{ tag }}
              <button class="tag-remove" @click.stop="removeTag(tag)">×</button>
            </span>
            <input
              v-model="tagInput"
              class="tag-input"
              :placeholder="tags.length === 0 ? t('snippets.tagsPlaceholder') : ''"
              @keydown="handleTagInput"
              @focus="tagFocused = true"
              @blur="onTagBlur"
            />
          </div>
          <!-- Suggestions dropdown -->
          <div v-if="showSuggestions" class="tag-suggestions">
            <button
              v-for="sug in tagSuggestions"
              :key="sug"
              class="tag-sug-item"
              @mousedown.prevent="pickSuggestion(sug)"
            >
              {{ sug }}
            </button>
          </div>
        </div>

        <!-- Note -->
        <div class="field-label">{{ t('snippets.note') }}</div>
        <textarea
          v-model="note"
          class="note-input"
          :placeholder="t('snippets.notePlaceholder')"
          rows="4"
        />

        <div class="modal-footer">
          <button class="btn-cancel" @click="cancel">{{ t('pdf.cancel') }}</button>
          <button class="btn-confirm" :disabled="!selectedLibraryId" @click="confirm">
            {{ t('snippets.addConfirm') }}
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.45);
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
}

.modal-card {
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: 12px;
  padding: 20px;
  width: 400px;
  max-width: calc(100vw - 40px);
  max-height: 80vh;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 10px;
  box-shadow: 0 8px 32px rgba(0,0,0,0.25);
}

.modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 4px;
}

.modal-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
}

.close-btn {
  color: var(--text-secondary);
  padding: 2px;
  border-radius: 4px;
}
.close-btn:hover { background: var(--bg-hover); color: var(--text-primary); }

.field-label {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-secondary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
  margin-top: 4px;
}

.text-preview {
  border-radius: 6px;
  padding: 8px 10px;
  font-size: 13px;
  color: var(--text-primary);
  line-height: 1.5;
  max-height: 100px;
  overflow-y: auto;
  white-space: pre-wrap;
  word-break: break-word;
}

.text-meta {
  font-size: 11px;
  color: var(--text-tertiary);
  margin-top: -4px;
}

.color-row {
  display: flex;
  gap: 8px;
  align-items: center;
  margin-top: -2px;
}

.color-dot {
  width: 18px;
  height: 18px;
  border-radius: 50%;
  cursor: pointer;
  border: 2px solid transparent;
  box-sizing: border-box;
  transition: transform 0.1s, border-color 0.1s;
}
.color-dot:hover { transform: scale(1.2); }
.color-dot--active {
  border-color: var(--text-primary);
  transform: scale(1.15);
}

.lib-list {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.lib-btn {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 4px 10px;
  border-radius: 20px;
  border: 1px solid var(--border-subtle);
  font-size: 12px;
  color: var(--text-secondary);
  background: transparent;
  cursor: pointer;
  transition: background 0.1s, color 0.1s, border-color 0.1s;
}
.lib-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.lib-btn.active {
  background: color-mix(in srgb, var(--accent) 15%, transparent);
  border-color: var(--accent);
  color: var(--accent);
}
.lib-btn.new-lib-btn {
  border-style: dashed;
}

.lib-emoji { font-size: 13px; }

.new-lib-input {
  padding: 4px 10px;
  border-radius: 20px;
  border: 1px solid var(--accent);
  font-size: 12px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  outline: none;
  width: 120px;
}

.tag-editor {
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  align-items: center;
  padding: 6px 8px;
  border: 1px solid var(--border-subtle);
  border-radius: 8px;
  background: var(--bg-secondary);
  min-height: 34px;
  cursor: text;
}
.tag-editor:focus-within { border-color: var(--accent); }

.tag-chip {
  display: flex;
  align-items: center;
  gap: 3px;
  padding: 2px 8px;
  border-radius: 12px;
  background: color-mix(in srgb, var(--accent) 15%, transparent);
  color: var(--accent);
  font-size: 12px;
  white-space: nowrap;
}

.tag-remove {
  font-size: 14px;
  line-height: 1;
  color: var(--accent);
  opacity: 0.7;
  padding: 0 1px;
}
.tag-remove:hover { opacity: 1; }

.tag-input {
  flex: 1;
  min-width: 80px;
  border: none;
  outline: none;
  background: transparent;
  font-size: 12px;
  color: var(--text-primary);
}
.tag-input::placeholder { color: var(--text-tertiary); }

.tag-editor-wrap {
  position: relative;
}

.tag-suggestions {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: 8px;
  box-shadow: var(--shadow-lg);
  z-index: 10;
  display: flex;
  flex-wrap: wrap;
  gap: 5px;
  padding: 8px;
  max-height: 100px;
  overflow-y: auto;
}

.tag-sug-item {
  padding: 3px 10px;
  border-radius: 12px;
  font-size: 12px;
  background: var(--bg-secondary);
  color: var(--text-secondary);
  border: 1px solid var(--border-subtle);
  cursor: pointer;
  transition: background 0.1s, color 0.1s;
}
.tag-sug-item:hover {
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
  border-color: var(--accent);
}

.note-input {
  width: 100%;
  padding: 8px 10px;
  border: 1px solid var(--border-subtle);
  border-radius: 8px;
  background: var(--bg-secondary);
  color: var(--text-primary);
  font: inherit;
  font-size: 13px;
  resize: none;
  outline: none;
  box-sizing: border-box;
}
.note-input:focus { border-color: var(--accent); }
.note-input::placeholder { color: var(--text-tertiary); }

.modal-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 6px;
}

.btn-cancel {
  padding: 6px 14px;
  border-radius: 6px;
  font-size: 13px;
  color: var(--text-secondary);
  border: 1px solid var(--border-subtle);
}
.btn-cancel:hover { background: var(--bg-hover); }

.btn-confirm {
  padding: 6px 14px;
  border-radius: 6px;
  font-size: 13px;
  background: var(--accent);
  color: #fff;
  font-weight: 500;
}
.btn-confirm:hover { opacity: 0.9; }
.btn-confirm:disabled { opacity: 0.4; cursor: default; }
</style>
