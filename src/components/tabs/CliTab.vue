<script setup lang="ts">
import { ref, watch, computed, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { useCliStore } from '../../stores/cli'
import type { CliAnalysisEntry, CliAnalysisEvent, CliPromptTemplate } from '../../types'
import { marked } from 'marked'

const props = defineProps<{ slug: string }>()

const { t } = useI18n()
const cliStore = useCliStore()

// ── View state ────────────────────────────────────────────────────────────────

type View = 'history' | 'run'
const view = ref<View>('history')

// ── History ───────────────────────────────────────────────────────────────────

const analyses = ref<CliAnalysisEntry[]>([])
const viewingEntry = ref<CliAnalysisEntry | null>(null)
const viewingContent = ref('')

async function loadHistory() {
  try {
    analyses.value = await invoke<CliAnalysisEntry[]>('list_cli_analyses', { slug: props.slug })
  } catch {
    analyses.value = []
  }
}

async function viewEntry(entry: CliAnalysisEntry) {
  viewingEntry.value = entry
  try {
    viewingContent.value = await invoke<string>('get_cli_analysis', {
      slug: props.slug, filename: entry.filename,
    })
  } catch (e: any) {
    viewingContent.value = String(e)
  }
}

function closeViewer() {
  viewingEntry.value = null
  viewingContent.value = ''
}

const viewingHtml = computed(() => {
  if (!viewingContent.value) return ''
  try { return marked(viewingContent.value) as string } catch { return viewingContent.value }
})

// ── Run panel ─────────────────────────────────────────────────────────────────

const templates = ref<CliPromptTemplate[]>([])
const selectedToolId = ref('')
const selectedTemplateId = ref('')
const promptText = ref('')
const isCustomPrompt = ref(false)

const running = ref(false)
const runId = ref('')
const outputChunks = ref<{ text: string; source: string }[]>([])
const runDone = ref(false)
const runCancelled = ref(false)
const exitCode = ref<number | null>(null)
const outputEl = ref<HTMLElement | null>(null)

let unlisten: UnlistenFn | null = null
const saveName = ref('')
const saving = ref(false)
const saved = ref(false)

async function openRunPanel() {
  templates.value = await cliStore.loadTemplates()
  if (templates.value.length > 0 && !selectedTemplateId.value) {
    selectedTemplateId.value = templates.value[0].id
    applyTemplate(templates.value[0].id)
  }
  if (cliStore.enabledTools.length > 0 && !selectedToolId.value) {
    selectedToolId.value = cliStore.enabledTools[0].id
  }
  view.value = 'run'
  resetRunState()
}

function applyTemplate(id: string) {
  const tpl = templates.value.find(t => t.id === id)
  if (tpl) { promptText.value = tpl.prompt_template; isCustomPrompt.value = false }
}

function onTemplateChange() {
  if (selectedTemplateId.value === 'custom') {
    isCustomPrompt.value = true
  } else {
    applyTemplate(selectedTemplateId.value)
    isCustomPrompt.value = false
  }
}

function resetRunState() {
  outputChunks.value = []
  runDone.value = false
  runCancelled.value = false
  exitCode.value = null
  saveName.value = ''
  saved.value = false
  runId.value = ''
}

async function startRun() {
  if (!selectedToolId.value || !promptText.value.trim()) return

  resetRunState()
  running.value = true

  try {
    runId.value = await invoke<string>('run_cli_analysis', {
      slug: props.slug,
      toolId: selectedToolId.value,
      prompt: promptText.value,
    })

    const eventName = `cli-analysis-${runId.value}`
    unlisten = await listen<CliAnalysisEvent>(eventName, (event) => {
      const { chunk, source, done, exit_code, cancelled } = event.payload

      if (chunk) {
        outputChunks.value.push({ text: chunk, source })
        // Auto-scroll
        if (outputEl.value) {
          outputEl.value.scrollTop = outputEl.value.scrollHeight
        }
      }
      if (done) {
        running.value = false
        runDone.value = true
        runCancelled.value = cancelled
        exitCode.value = exit_code

        const template = templates.value.find(t => t.id === selectedTemplateId.value)
        saveName.value = template?.name ?? t('cliTab.analysisResult')

        unlisten?.()
        unlisten = null
      }
    })
  } catch (e: any) {
    outputChunks.value.push({ text: String(e), source: 'error' })
    running.value = false
    runDone.value = true
  }
}

async function cancelRun() {
  if (!runId.value) return
  try {
    await invoke('cancel_cli_analysis', { runId: runId.value })
  } catch { /* already done */ }
}

async function saveResult() {
  const content = outputChunks.value.map(c => c.text).join('')
  if (!content || !saveName.value) return
  saving.value = true
  try {
    await invoke('save_cli_analysis_result', {
      slug: props.slug,
      name: saveName.value,
      content,
    })
    saved.value = true
    await loadHistory()
  } catch (e: any) {
    alert(String(e))
  } finally {
    saving.value = false
  }
}

function discardAndBack() {
  view.value = 'history'
  resetRunState()
  unlisten?.()
  unlisten = null
}

// ── Computed output ───────────────────────────────────────────────────────────

const outputText = computed(() => outputChunks.value.map(c => c.text).join(''))

const hasError = computed(() =>
  runDone.value && !runCancelled.value && exitCode.value !== 0 && exitCode.value !== null
)

// ── Lifecycle ─────────────────────────────────────────────────────────────────

watch(() => props.slug, async () => {
  view.value = 'history'
  resetRunState()
  unlisten?.()
  unlisten = null
  await loadHistory()
  await cliStore.load()
}, { immediate: true })

onUnmounted(() => {
  unlisten?.()
  unlisten = null
})
</script>

<template>
  <div class="cli-tab">

    <!-- No tools configured guard -->
    <div v-if="cliStore.enabledTools.length === 0" class="no-tools">
      <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M14.7 6.3a1 1 0 0 0 0 1.4l1.6 1.6a1 1 0 0 0 1.4 0l3.77-3.77a6 6 0 0 1-7.94 7.94l-6.91 6.91a2.12 2.12 0 0 1-3-3l6.91-6.91a6 6 0 0 1 7.94-7.94l-3.76 3.76z"/>
      </svg>
      <p>{{ t('cliTab.noTools') }}</p>
      <span>{{ t('cliTab.noToolsHint') }}</span>
    </div>

    <!-- History view -->
    <template v-else-if="view === 'history' && !viewingEntry">
      <div class="tab-toolbar">
        <span class="toolbar-title">{{ t('cliTab.title') }}</span>
        <button class="btn-run" @click="openRunPanel">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
            <polygon points="5 3 19 12 5 21 5 3"/>
          </svg>
          {{ t('cliTab.newAnalysis') }}
        </button>
      </div>

      <div v-if="analyses.length === 0" class="empty-history">
        <p>{{ t('cliTab.noHistory') }}</p>
        <span>{{ t('cliTab.noHistoryHint') }}</span>
      </div>

      <div v-else class="analysis-list">
        <button
          v-for="entry in analyses"
          :key="entry.filename"
          class="analysis-item"
          @click="viewEntry(entry)"
        >
          <div class="analysis-name">{{ entry.name }}</div>
          <div class="analysis-date">{{ entry.created_at }}</div>
        </button>
      </div>
    </template>

    <!-- Viewing saved analysis -->
    <template v-else-if="viewingEntry">
      <div class="tab-toolbar">
        <button class="btn-back" @click="closeViewer">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="15 18 9 12 15 6"/>
          </svg>
        </button>
        <span class="toolbar-title ellipsis">{{ viewingEntry.name }}</span>
      </div>
      <div class="output-viewer markdown-body" v-html="viewingHtml" />
    </template>

    <!-- Run panel -->
    <template v-else-if="view === 'run'">
      <div class="tab-toolbar">
        <button class="btn-back" @click="discardAndBack">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <polyline points="15 18 9 12 15 6"/>
          </svg>
        </button>
        <span class="toolbar-title">{{ t('cliTab.newAnalysis') }}</span>
      </div>

      <!-- Setup section (hidden while running) -->
      <div v-if="!running && !runDone" class="setup-section">
        <div class="field-group">
          <label class="field-label">{{ t('cliTab.selectTool') }}</label>
          <select v-model="selectedToolId" class="field-select">
            <option v-for="tool in cliStore.enabledTools" :key="tool.id" :value="tool.id">
              {{ tool.name }}
            </option>
          </select>
        </div>

        <div class="field-group">
          <label class="field-label">{{ t('cliTab.selectTemplate') }}</label>
          <select v-model="selectedTemplateId" class="field-select" @change="onTemplateChange">
            <option v-for="tpl in templates" :key="tpl.id" :value="tpl.id">{{ tpl.name }}</option>
            <option value="custom">{{ t('cliTab.customPrompt') }}</option>
          </select>
        </div>

        <div class="field-group">
          <label class="field-label">{{ t('cliTab.prompt') }}</label>
          <textarea
            v-model="promptText"
            class="prompt-textarea"
            :placeholder="t('cliTab.promptPh')"
            rows="5"
          />
        </div>

        <button class="btn-run-lg" @click="startRun" :disabled="!selectedToolId || !promptText.trim()">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="currentColor">
            <polygon points="5 3 19 12 5 21 5 3"/>
          </svg>
          {{ t('cliTab.run') }}
        </button>
      </div>

      <!-- Running state / output -->
      <div v-if="running || runDone" class="run-section">
        <div class="run-status-bar">
          <div class="run-status-indicator" :class="{ running, done: runDone, error: hasError, cancelled: runCancelled }">
            <span v-if="running">{{ t('cliTab.running') }}</span>
            <span v-else-if="runCancelled">{{ t('cliTab.cancelled') }}</span>
            <span v-else-if="hasError">{{ t('cliTab.failed', { code: exitCode }) }}</span>
            <span v-else>{{ t('cliTab.done') }}</span>
          </div>
          <button v-if="running" class="btn-cancel" @click="cancelRun">{{ t('cliTab.cancel') }}</button>
        </div>

        <div ref="outputEl" class="output-stream">
          <template v-for="(chunk, i) in outputChunks" :key="i">
            <span :class="{ 'err-text': chunk.source === 'stderr' || chunk.source === 'error' }">{{ chunk.text }}</span>
          </template>
          <span v-if="running" class="cursor-blink">▍</span>
        </div>

        <!-- Save panel after completion -->
        <div v-if="runDone && !runCancelled && !hasError" class="save-panel">
          <input v-model="saveName" class="save-name-input" :placeholder="t('cliTab.saveName')" />
          <button class="btn-save" :disabled="saving || saved || !saveName" @click="saveResult">
            {{ saved ? t('cliTab.saved') : (saving ? t('cliTab.saving') : t('cliTab.save')) }}
          </button>
          <button class="btn-ghost-sm" @click="discardAndBack">{{ t('cliTab.discard') }}</button>
        </div>
        <div v-if="runDone && (runCancelled || hasError)" class="post-run-actions">
          <button class="btn-ghost-sm" @click="discardAndBack">{{ t('cliTab.back') }}</button>
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.cli-tab {
  display: flex;
  flex-direction: column;
  height: 100%;
  overflow: hidden;
}

/* No tools guard */
.no-tools {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--text-tertiary);
  padding: 20px;
  text-align: center;
}
.no-tools p { font-size: var(--font-size-sm); color: var(--text-secondary); margin: 0; }
.no-tools span { font-size: var(--font-size-xs); }

/* Toolbar */
.tab-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.toolbar-title {
  flex: 1;
  font-size: var(--font-size-sm);
  font-weight: 500;
  color: var(--text-primary);
}
.toolbar-title.ellipsis {
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}

.btn-back {
  display: flex;
  align-items: center;
  color: var(--text-secondary);
  padding: 3px;
  border-radius: var(--radius-sm);
}
.btn-back:hover { background: var(--bg-hover); color: var(--text-primary); }

.btn-run {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 4px 10px;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-sm);
  font-size: var(--font-size-xs);
  font-weight: 500;
  flex-shrink: 0;
}
.btn-run:hover { background: var(--accent-hover); }

/* History */
.empty-history {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  flex: 1;
  gap: 6px;
  color: var(--text-tertiary);
  text-align: center;
  padding: 20px;
}
.empty-history p { font-size: var(--font-size-sm); color: var(--text-secondary); margin: 0; }
.empty-history span { font-size: var(--font-size-xs); }

.analysis-list {
  flex: 1;
  overflow-y: auto;
  padding: 8px 0;
}

.analysis-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  width: 100%;
  padding: 8px 14px;
  text-align: left;
  border-radius: 0;
  transition: background 0.1s;
}
.analysis-item:hover { background: var(--bg-hover); }

.analysis-name {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-primary);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
}
.analysis-date {
  font-size: 10px;
  color: var(--text-tertiary);
}

/* Saved analysis viewer */
.output-viewer {
  flex: 1;
  overflow-y: auto;
  padding: 14px 16px;
}

/* Setup section */
.setup-section {
  padding: 12px 14px;
  flex-shrink: 0;
  overflow-y: auto;
}

.field-group { margin-bottom: 10px; }

.field-label {
  display: block;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  margin-bottom: 4px;
}

.field-select {
  width: 100%;
  padding: 5px 8px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  font-size: var(--font-size-sm);
  color: var(--text-primary);
}
.field-select:focus { outline: none; border-color: var(--accent); }

.prompt-textarea {
  width: 100%;
  padding: 6px 9px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  font-size: var(--font-size-xs);
  color: var(--text-primary);
  resize: vertical;
  min-height: 80px;
  font-family: inherit;
}
.prompt-textarea:focus { outline: none; border-color: var(--accent); }

.btn-run-lg {
  display: flex;
  align-items: center;
  gap: 6px;
  width: 100%;
  justify-content: center;
  padding: 8px 0;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-sm);
  font-size: var(--font-size-sm);
  font-weight: 500;
  margin-top: 4px;
}
.btn-run-lg:hover { background: var(--accent-hover); }
.btn-run-lg:disabled { opacity: 0.5; cursor: default; }

/* Run section */
.run-section {
  display: flex;
  flex-direction: column;
  flex: 1;
  overflow: hidden;
}

.run-status-bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.run-status-indicator {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-secondary);
}
.run-status-indicator.running { color: var(--accent); }
.run-status-indicator.done    { color: var(--badge-extracted-fg); }
.run-status-indicator.error   { color: #c62828; }
.run-status-indicator.cancelled { color: var(--text-tertiary); }

.btn-cancel {
  font-size: var(--font-size-xs);
  padding: 3px 10px;
  color: #c62828;
  border: 1px solid #fca5a5;
  border-radius: var(--radius-sm);
  background: #fef2f2;
}
.btn-cancel:hover { background: #fca5a5; }

.output-stream {
  flex: 1;
  overflow-y: auto;
  padding: 10px 12px;
  font-family: monospace;
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-primary);
  white-space: pre-wrap;
  word-break: break-word;
  background: var(--bg-secondary);
}

.err-text { color: #c62828; }

@keyframes blink { 0%, 100% { opacity: 1 } 50% { opacity: 0 } }
.cursor-blink { animation: blink 1s step-end infinite; }

.save-panel {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
  background: var(--bg-primary);
}

.save-name-input {
  flex: 1;
  padding: 5px 9px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  font-size: var(--font-size-xs);
  color: var(--text-primary);
}
.save-name-input:focus { outline: none; border-color: var(--accent); }

.btn-save {
  padding: 5px 12px;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-sm);
  font-size: var(--font-size-xs);
  font-weight: 500;
  flex-shrink: 0;
}
.btn-save:hover { background: var(--accent-hover); }
.btn-save:disabled { opacity: 0.6; cursor: default; }

.post-run-actions {
  display: flex;
  gap: 8px;
  padding: 8px 12px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.btn-ghost-sm {
  padding: 4px 10px;
  font-size: var(--font-size-xs);
  color: var(--text-secondary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
}
.btn-ghost-sm:hover { background: var(--bg-hover); color: var(--text-primary); }
</style>

<style>
/* Global markdown body styles for the saved analysis viewer */
.cli-tab .markdown-body h1,
.cli-tab .markdown-body h2,
.cli-tab .markdown-body h3 {
  font-weight: 600;
  color: var(--text-primary);
  margin: 0.9em 0 0.4em;
}
.cli-tab .markdown-body h1 { font-size: 1.15em; }
.cli-tab .markdown-body h2 { font-size: 1.05em; }
.cli-tab .markdown-body h3 { font-size: 1em; }
.cli-tab .markdown-body p  { margin: 0.5em 0; line-height: 1.6; font-size: var(--font-size-sm); color: var(--text-primary); }
.cli-tab .markdown-body ul,
.cli-tab .markdown-body ol { padding-left: 1.4em; margin: 0.4em 0; }
.cli-tab .markdown-body li { font-size: var(--font-size-sm); color: var(--text-primary); margin: 0.2em 0; }
.cli-tab .markdown-body code {
  font-family: monospace; font-size: 0.88em;
  background: var(--bg-secondary); border-radius: 3px; padding: 1px 4px;
}
.cli-tab .markdown-body pre {
  background: var(--bg-secondary); border-radius: var(--radius-sm);
  padding: 10px 12px; overflow-x: auto; margin: 0.6em 0;
}
.cli-tab .markdown-body pre code { background: none; padding: 0; }
.cli-tab .markdown-body hr { border: none; border-top: 1px solid var(--border-subtle); margin: 1em 0; }
.cli-tab .markdown-body blockquote {
  border-left: 3px solid var(--border-default);
  padding-left: 0.8em; color: var(--text-secondary);
  margin: 0.5em 0; font-style: italic;
}
</style>
