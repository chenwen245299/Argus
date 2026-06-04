import { reactive, ref } from 'vue'

export interface TranslationEntry {
  id: string
  sourceText: string
  result: string
  createdAt: string
}

const MAX_HISTORY = 200
const STORAGE_KEY = 'argus:translation-history'

function loadFromStorage(): TranslationEntry[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return []
    return JSON.parse(raw) as TranslationEntry[]
  } catch {
    return []
  }
}

function saveToStorage(entries: TranslationEntry[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(entries))
  } catch {}
}

export const translationHistory = reactive<{ entries: TranslationEntry[] }>({
  entries: loadFromStorage(),
})

// Current in-progress (or most recent) translation — drives the tab's main view
export const currentTranslation = reactive({
  sourceText: '',
  result: '',
  loading: false,
  error: '',
})

// Signal MainView to switch to translations tab
export const switchToTranslationsTab = ref(false)

// Signal MainView + AiTab to open AI chat with pre-filled text
export const askAiText = ref<string | null>(null)

export function triggerAskAi(text: string) {
  askAiText.value = text
}

export function startTranslation(sourceText: string) {
  currentTranslation.sourceText = sourceText
  currentTranslation.result = ''
  currentTranslation.loading = true
  currentTranslation.error = ''
  switchToTranslationsTab.value = true
}

export function appendTranslationChunk(delta: string) {
  currentTranslation.result += delta
  currentTranslation.loading = false
}

export function finishTranslation() {
  currentTranslation.loading = false
  if (!currentTranslation.result) return
  const entry: TranslationEntry = {
    id: crypto.randomUUID(),
    sourceText: currentTranslation.sourceText,
    result: currentTranslation.result,
    createdAt: new Date().toISOString(),
  }
  translationHistory.entries.unshift(entry)
  if (translationHistory.entries.length > MAX_HISTORY) {
    translationHistory.entries.splice(MAX_HISTORY)
  }
  saveToStorage(translationHistory.entries)
}

export function failTranslation(error: string) {
  currentTranslation.loading = false
  currentTranslation.error = error
}

export function deleteTranslationEntry(id: string) {
  const idx = translationHistory.entries.findIndex(e => e.id === id)
  if (idx !== -1) translationHistory.entries.splice(idx, 1)
  saveToStorage(translationHistory.entries)
}

export function clearTranslationHistory() {
  translationHistory.entries = []
  saveToStorage([])
}
