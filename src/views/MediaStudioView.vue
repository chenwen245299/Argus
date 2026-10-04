<script setup lang="ts">
/**
 * 媒体工坊 — text-to-image, speech, transcription, sound design and music.
 *
 * Nothing in this file knows about any particular provider. The backend's
 * `list_media_capabilities` describes every configured provider's tasks, models
 * and knobs, and the form below is rendered from that description — so a second
 * provider with a media adapter appears here on its own, with its own fields,
 * without this component changing.
 */
import { ref, computed, onMounted } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { save as dialogSave } from '@tauri-apps/plugin-dialog'
import { providerLogo } from '../utils/providerLogo'
import type {
  MediaProviderCapabilities, MediaCapability, MediaModelSpec, MediaField,
  MediaArtifact, MediaResult, MediaKind,
} from '../types'

const { t } = useI18n()

const loading = ref(true)
const loadError = ref('')
const providers = ref<MediaProviderCapabilities[]>([])

const providerId = ref('')
const kind = ref<MediaKind | ''>('')
const modelId = ref('')
const prompt = ref('')
/** Field values keyed by `MediaField.key`, rebuilt from defaults on every model change. */
const options = ref<Record<string, unknown>>({})
/** Uploaded inputs, already read as data URIs so the backend never touches disk. */
const inputs = ref<{ name: string; dataUrl: string }[]>([])
const fileInputRef = ref<HTMLInputElement | null>(null)

const running = ref(false)
const runError = ref('')
const result = ref<MediaResult | null>(null)
/**
 * Bumped on every task/model change. A file read and a generation run both
 * outlive the selection they were started under, and both write into shared
 * state when they finish — so each captures this and drops its result if the
 * user has moved on. Without it a slowly-read image lands in `inputs` after
 * `selectModel` cleared it, and is then sent to whatever endpoint is selected
 * now.
 */
let selectionToken = 0

// ── Derived selection ─────────────────────────────────────────────────────────

const provider = computed(() => providers.value.find(p => p.providerId === providerId.value) ?? null)
const capability = computed<MediaCapability | null>(
  () => provider.value?.capabilities.find(c => c.kind === kind.value) ?? null
)
const model = computed<MediaModelSpec | null>(
  () => capability.value?.models.find(m => m.id === modelId.value) ?? null
)
const fields = computed<MediaField[]>(() => model.value?.fields ?? [])
/** Whether to show the file picker at all. */
const takesFile = computed(() => (model.value?.accepts?.length ?? 0) > 0)
/** Whether a missing file should grey out the run button. */
const needsFile = computed(() => takesFile.value && model.value?.fileRequired === true)
const accept = computed(() => model.value?.accepts?.join(',') ?? '')

const canRun = computed(() => {
  if (!model.value || running.value) return false
  if (model.value.promptRequired && !prompt.value.trim()) return false
  if (needsFile.value && inputs.value.length === 0) return false
  return true
})

// ── Cascading selection ───────────────────────────────────────────────────────
//
// Each level resets the ones under it, so a half-valid combination (a model from
// the previously selected task) can never be submitted.

function selectProvider(id: string) {
  providerId.value = id
  const first = provider.value?.capabilities[0]
  selectKind(first ? first.kind : '')
}

function selectKind(k: MediaKind | '') {
  kind.value = k
  // The main text box means something different in each task — a picture to
  // draw, a line to speak, hot words for a transcription, a style to compose in.
  // Carrying text across would quietly turn an image description into ASR hot
  // words. Switching *model* within a task keeps it, because the meaning is the
  // same there.
  prompt.value = ''
  selectModel(capability.value?.models[0]?.id ?? '')
}

function selectModel(id: string) {
  selectionToken += 1
  modelId.value = id
  // Defaults come from the field descriptions, so the form starts on values the
  // provider itself considers sensible rather than on empty strings.
  const next: Record<string, unknown> = {}
  for (const f of model.value?.fields ?? []) {
    if (f.default !== undefined && f.default !== null) next[f.key] = f.default
  }
  options.value = next
  // An input picked for one task is rarely the right input for another, and a
  // stale image silently riding along into a transcription would be worse than
  // asking again.
  inputs.value = []
  result.value = null
  runError.value = ''
}

// ── Inputs ────────────────────────────────────────────────────────────────────

function pickFile() {
  fileInputRef.value?.click()
}

function onFileChosen(e: Event) {
  const el = e.target as HTMLInputElement
  const file = el.files?.[0]
  el.value = ''
  if (!file) return
  const token = selectionToken
  const reader = new FileReader()
  reader.onload = () => {
    if (token !== selectionToken) return
    inputs.value = [{ name: file.name, dataUrl: reader.result as string }]
  }
  reader.onerror = () => {
    if (token !== selectionToken) return
    runError.value = t('mediaStudio.readFailed', { name: file.name })
  }
  reader.readAsDataURL(file)
}

// ── Running ───────────────────────────────────────────────────────────────────

async function run() {
  if (!canRun.value || !model.value) return
  const token = selectionToken
  running.value = true
  runError.value = ''
  result.value = null
  try {
    const produced = await invoke<MediaResult>('run_media_task', {
      request: {
        providerId: providerId.value,
        kind: kind.value,
        model: modelId.value,
        prompt: prompt.value,
        inputs: inputs.value,
        options: options.value,
      },
    })
    // A run outlives the task it was started under; showing a music clip under
    // the transcription form would be worse than showing nothing.
    if (token === selectionToken) result.value = produced
  } catch (e) {
    if (token === selectionToken) runError.value = String(e)
  } finally {
    running.value = false
  }
}

// ── Saving ────────────────────────────────────────────────────────────────────

async function saveArtifact(a: MediaArtifact) {
  const name = a.filename || 'output'
  const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1) : 'bin'
  // The dialog call is inside the try as well: if it rejects (an ACL that does
  // not cover this window, a platform refusal), an unguarded await would become
  // an unhandled rejection and the button would look like it did nothing.
  try {
    const path = await dialogSave({
      defaultPath: name,
      filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
    })
    if (!path) return

    // Branch on what the artifact *is*, not on which optional fields happen to
    // be set: an image returned as a hosted URL also carries `text` (its seed),
    // and keying off that would have written the word "seed 12345" into the .png.
    let bytes: number[]
    if (a.mime.startsWith('text/')) {
      bytes = Array.from(new TextEncoder().encode(a.text ?? ''))
    } else if (a.dataUrl) {
      const buf = await (await fetch(a.dataUrl)).arrayBuffer()
      bytes = Array.from(new Uint8Array(buf))
    } else if (a.url) {
      // Downloaded in Rust, not here: a cross-origin fetch from tauri://localhost
      // to the provider's CDN is blocked, and these links expire — which is the
      // whole reason saving matters.
      bytes = await invoke<number[]>('fetch_media_artifact', { url: a.url })
    } else {
      return
    }
    await invoke('write_bytes_to_file', { path, bytes })
  } catch (e) {
    runError.value = String(e)
  }
}

/**
 * `options` holds whatever the adapter declared, so its values are `unknown`.
 * An `<input>` needs a concrete string — and an empty one, rather than "null",
 * when the field has no value yet.
 */
function asText(v: unknown): string {
  return v === undefined || v === null ? '' : String(v)
}

async function copyText(text: string) {
  try { await navigator.clipboard.writeText(text) } catch { /* clipboard unavailable */ }
}

// ── Load ──────────────────────────────────────────────────────────────────────

onMounted(async () => {
  try {
    providers.value = await invoke<MediaProviderCapabilities[]>('list_media_capabilities')
    if (providers.value.length) selectProvider(providers.value[0].providerId)
  } catch (e) {
    loadError.value = String(e)
  } finally {
    loading.value = false
  }
})

const KIND_ICONS: Record<string, string> = {
  image_generate: 'fluent:image-sparkle-24-regular',
  image_edit: 'fluent:image-edit-24-regular',
  speech: 'fluent:speaker-2-24-regular',
  transcribe: 'fluent:text-bullet-list-square-24-regular',
  audio_generate: 'fluent:sound-wave-circle-24-regular',
  music: 'fluent:music-note-2-24-regular',
}
</script>

<template>
  <div class="media-studio">
    <div class="ms-titlebar" data-tauri-drag-region>
      <div class="tl-space" data-tauri-drag-region />
      <div class="ms-avatar" data-tauri-drag-region>
        <Icon icon="fluent:wand-24-regular" width="15" height="15" data-tauri-drag-region />
      </div>
      <div class="ms-title-block" data-tauri-drag-region>
        <span class="ms-title" data-tauri-drag-region>{{ t('mediaStudio.title') }}</span>
        <span class="ms-subtitle" data-tauri-drag-region>{{ t('mediaStudio.subtitle') }}</span>
      </div>
      <div class="ms-titlebar-fill" data-tauri-drag-region />
    </div>

    <div v-if="loading" class="ms-empty">{{ t('mediaStudio.loading') }}</div>
    <div v-else-if="loadError" class="ms-empty error">{{ loadError }}</div>
    <div v-else-if="!providers.length" class="ms-empty">
      <Icon icon="fluent:wand-24-regular" width="28" height="28" />
      <p>{{ t('mediaStudio.noProviders') }}</p>
    </div>

    <div v-else class="ms-body">
      <!-- Left: what to make -->
      <aside class="ms-side">
        <div class="ms-section">
          <div class="ms-section-label">{{ t('mediaStudio.provider') }}</div>
          <button
            v-for="p in providers"
            :key="p.providerId"
            class="ms-row"
            :class="{ active: p.providerId === providerId }"
            @click="selectProvider(p.providerId)"
          >
            <img v-if="providerLogo(p.providerName, p.baseUrl)" :src="providerLogo(p.providerName, p.baseUrl)" class="ms-row-logo" alt="" />
            <span>{{ p.providerName }}</span>
          </button>
        </div>

        <div v-if="provider" class="ms-section">
          <div class="ms-section-label">{{ t('mediaStudio.task') }}</div>
          <button
            v-for="c in provider.capabilities"
            :key="c.kind"
            class="ms-row"
            :class="{ active: c.kind === kind }"
            @click="selectKind(c.kind)"
          >
            <Icon :icon="KIND_ICONS[c.kind] ?? 'fluent:sparkle-24-regular'" width="15" height="15" />
            <span>{{ c.label }}</span>
          </button>
        </div>
      </aside>

      <!-- Middle: the form, rendered from the backend's field descriptions -->
      <main class="ms-main">
        <template v-if="capability">
          <div v-if="capability.note" class="ms-note top">
            <Icon icon="fluent:info-24-regular" width="13" height="13" />
            <span>{{ capability.note }}</span>
          </div>

          <label class="ms-field">
            <span class="ms-label">{{ t('mediaStudio.model') }}</span>
            <select class="ms-input" :value="modelId" @change="selectModel(($event.target as HTMLSelectElement).value)">
              <option v-for="m in capability.models" :key="m.id" :value="m.id">{{ m.displayName }}</option>
            </select>
          </label>
          <div v-if="model?.note" class="ms-note">{{ model.note }}</div>

          <div v-if="takesFile" class="ms-field">
            <span class="ms-label">
              {{ t('mediaStudio.inputFile') }}
              <em v-if="!needsFile">{{ t('mediaStudio.optional') }}</em>
            </span>
            <div class="ms-file-row">
              <button class="ms-btn ghost" @click="pickFile">
                <Icon icon="fluent:attach-24-regular" width="14" height="14" />
                {{ inputs.length ? t('mediaStudio.replaceFile') : t('mediaStudio.chooseFile') }}
              </button>
              <span v-if="inputs.length" class="ms-file-name">{{ inputs[0].name }}</span>
            </div>
            <input ref="fileInputRef" type="file" :accept="accept" style="display: none" @change="onFileChosen" />
          </div>

          <label class="ms-field">
            <span class="ms-label">
              {{ t('mediaStudio.prompt') }}
              <em v-if="!model?.promptRequired">{{ t('mediaStudio.optional') }}</em>
            </span>
            <textarea
              v-model="prompt"
              class="ms-input ms-textarea"
              rows="4"
              :placeholder="model?.promptPlaceholder ?? ''"
            />
          </label>

          <!-- One control per declared field. The backend named them; this loop
               only has to know the five input kinds. -->
          <div class="ms-grid">
            <label v-for="f in fields" :key="f.key" class="ms-field" :class="{ wide: f.kind === 'long_text' }">
              <span class="ms-label">{{ f.label }}</span>

              <select
                v-if="f.kind === 'select'"
                :value="asText(options[f.key])"
                class="ms-input"
                @change="options[f.key] = ($event.target as HTMLSelectElement).value"
              >
                <option v-for="o in f.options" :key="o.value" :value="o.value">{{ o.group ? `${o.group} · ${o.label}` : o.label }}</option>
              </select>

              <textarea
                v-else-if="f.kind === 'long_text'"
                :value="asText(options[f.key])"
                class="ms-input ms-textarea"
                rows="3"
                @input="options[f.key] = ($event.target as HTMLTextAreaElement).value"
              />

              <input
                v-else-if="f.kind === 'number'"
                :value="asText(options[f.key])"
                class="ms-input"
                type="number"
                :min="f.min"
                :max="f.max"
                :step="f.step"
                @input="options[f.key] = ($event.target as HTMLInputElement).value"
              />

              <label v-else-if="f.kind === 'toggle'" class="ms-toggle">
                <input
                  type="checkbox"
                  :checked="options[f.key] === true"
                  @change="options[f.key] = ($event.target as HTMLInputElement).checked"
                />
                <span>{{ f.note ?? '' }}</span>
              </label>

              <input
                v-else
                :value="asText(options[f.key])"
                class="ms-input"
                type="text"
                @input="options[f.key] = ($event.target as HTMLInputElement).value"
              />

              <span v-if="f.note && f.kind !== 'toggle'" class="ms-note">{{ f.note }}</span>
            </label>
          </div>

          <div class="ms-actions">
            <button class="ms-btn primary" :disabled="!canRun" @click="run">
              <Icon v-if="running" icon="fluent:spinner-ios-20-regular" class="spin" width="14" height="14" />
              <Icon v-else icon="fluent:sparkle-24-regular" width="14" height="14" />
              {{ running ? t('mediaStudio.running') : t('mediaStudio.run') }}
            </button>
            <span v-if="running" class="ms-note">{{ t('mediaStudio.runningNote') }}</span>
          </div>

          <div v-if="runError" class="ms-error">
            <Icon icon="fluent:warning-24-regular" width="14" height="14" />
            <span>{{ runError }}</span>
          </div>
        </template>
      </main>

      <!-- Right: what came back -->
      <section class="ms-result">
        <div class="ms-section-label">{{ t('mediaStudio.output') }}</div>
        <div v-if="!result" class="ms-empty small">{{ t('mediaStudio.noOutput') }}</div>
        <div v-for="(a, i) in result?.artifacts ?? []" :key="i" class="ms-artifact">
          <img v-if="a.mime.startsWith('image/')" :src="a.dataUrl ?? a.url" class="ms-image" alt="" />
          <audio v-else-if="a.mime.startsWith('audio/')" :src="a.dataUrl ?? a.url" controls class="ms-audio" />
          <pre v-else-if="a.text" class="ms-text">{{ a.text }}</pre>

          <div v-if="a.text && !a.mime.startsWith('text/')" class="ms-note">{{ a.text }}</div>
          <div class="ms-artifact-actions">
            <button class="ms-btn ghost xs" @click="saveArtifact(a)">
              <Icon icon="fluent:arrow-download-24-regular" width="13" height="13" />
              {{ t('mediaStudio.save') }}
            </button>
            <button v-if="a.text && a.mime.startsWith('text/')" class="ms-btn ghost xs" @click="copyText(a.text)">
              <Icon icon="fluent:copy-24-regular" width="13" height="13" />
              {{ t('mediaStudio.copy') }}
            </button>
          </div>
        </div>
      </section>
    </div>
  </div>
</template>

<style scoped>
.media-studio {
  display: flex;
  flex-direction: column;
  height: 100vh;
  background: var(--bg-primary, #f7f8fa);
  color: var(--text-primary, #1b1b1f);
  font-size: 13px;
  overflow: hidden;
}

.ms-titlebar {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 52px;
  padding: 0 14px;
  flex-shrink: 0;
  border-bottom: 1px solid var(--border-color, #e4e6eb);
  background: var(--bg-secondary, #fff);
}
.ms-titlebar .tl-space { width: 96px; flex-shrink: 0; }
.ms-avatar {
  display: grid;
  place-items: center;
  width: 26px;
  height: 26px;
  border-radius: 7px;
  background: var(--accent-soft, rgba(80, 140, 255, 0.12));
  color: var(--accent, #3b82f6);
  flex-shrink: 0;
}
.ms-title-block { display: flex; flex-direction: column; line-height: 1.25; }
.ms-title { font-weight: 600; font-size: 13px; }
.ms-subtitle { font-size: 11px; color: var(--text-secondary, #8a8f98); }
.ms-titlebar-fill { flex: 1; align-self: stretch; }

.ms-body {
  flex: 1;
  display: grid;
  grid-template-columns: 180px minmax(0, 1fr) minmax(260px, 0.8fr);
  min-height: 0;
}

.ms-side {
  border-right: 1px solid var(--border-color, #e4e6eb);
  padding: 12px 8px;
  overflow-y: auto;
}
.ms-section { margin-bottom: 18px; }
.ms-section-label {
  font-size: 11px;
  color: var(--text-secondary, #8a8f98);
  padding: 0 6px 6px;
  letter-spacing: 0.02em;
}
.ms-row {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 7px 8px;
  border: none;
  border-radius: 7px;
  background: transparent;
  color: inherit;
  font-size: 12.5px;
  text-align: left;
  cursor: pointer;
}
.ms-row:hover { background: var(--bg-hover, rgba(0, 0, 0, 0.04)); }
.ms-row.active {
  background: var(--accent-soft, rgba(80, 140, 255, 0.12));
  color: var(--accent, #3b82f6);
}
.ms-row-logo { width: 15px; height: 15px; object-fit: contain; }

.ms-main {
  padding: 16px 18px;
  overflow-y: auto;
  min-width: 0;
}
.ms-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
  gap: 12px;
}
.ms-field {
  display: flex;
  flex-direction: column;
  gap: 5px;
  margin-bottom: 12px;
  min-width: 0;
}
.ms-field.wide { grid-column: 1 / -1; }
.ms-label { font-size: 12px; color: var(--text-secondary, #6b7280); }
.ms-label em { font-style: normal; opacity: 0.7; margin-left: 4px; }
.ms-input {
  width: 100%;
  padding: 7px 9px;
  border: 1px solid var(--border-color, #e4e6eb);
  border-radius: 7px;
  background: var(--bg-secondary, #fff);
  color: inherit;
  font: inherit;
  box-sizing: border-box;
}
.ms-textarea { resize: vertical; line-height: 1.55; }
.ms-toggle { display: flex; align-items: center; gap: 6px; font-size: 12px; color: var(--text-secondary, #6b7280); }

.ms-note {
  font-size: 11.5px;
  color: var(--text-secondary, #8a8f98);
  line-height: 1.5;
}
.ms-note.top {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  padding: 8px 10px;
  margin-bottom: 14px;
  border-radius: 7px;
  background: var(--bg-hover, rgba(0, 0, 0, 0.035));
}

.ms-file-row { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; }
.ms-file-name { font-size: 12px; color: var(--text-secondary, #6b7280); word-break: break-all; }

.ms-actions { display: flex; align-items: center; gap: 10px; margin-top: 6px; flex-wrap: wrap; }
.ms-btn {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 7px 13px;
  border: 1px solid transparent;
  border-radius: 7px;
  font: inherit;
  cursor: pointer;
}
.ms-btn.primary { background: var(--accent, #3b82f6); color: #fff; }
.ms-btn.primary:disabled { opacity: 0.45; cursor: not-allowed; }
.ms-btn.ghost {
  background: var(--bg-secondary, #fff);
  border-color: var(--border-color, #e4e6eb);
  color: inherit;
}
.ms-btn.xs { padding: 4px 9px; font-size: 12px; }
.spin { animation: ms-spin 1s linear infinite; }
@keyframes ms-spin { to { transform: rotate(360deg); } }

.ms-error {
  display: flex;
  align-items: flex-start;
  gap: 6px;
  margin-top: 12px;
  padding: 9px 11px;
  border-radius: 7px;
  background: rgba(220, 70, 70, 0.10);
  color: #c0392b;
  font-size: 12.5px;
  line-height: 1.55;
}

.ms-result {
  border-left: 1px solid var(--border-color, #e4e6eb);
  padding: 12px;
  overflow-y: auto;
  min-width: 0;
}
.ms-artifact {
  margin-bottom: 14px;
  padding: 10px;
  border: 1px solid var(--border-color, #e4e6eb);
  border-radius: 9px;
  background: var(--bg-secondary, #fff);
}
.ms-image { width: 100%; border-radius: 6px; display: block; }
.ms-audio { width: 100%; }
.ms-text {
  margin: 0;
  max-height: 340px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-word;
  font-size: 12.5px;
  line-height: 1.6;
}
.ms-artifact-actions { display: flex; gap: 6px; margin-top: 8px; }

.ms-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  flex: 1;
  color: var(--text-secondary, #8a8f98);
  padding: 40px 20px;
  text-align: center;
}
.ms-empty.small { padding: 24px 10px; font-size: 12px; }
.ms-empty.error { color: #c0392b; }

@media (max-width: 900px) {
  .ms-body { grid-template-columns: 150px minmax(0, 1fr); }
  .ms-result { grid-column: 1 / -1; border-left: none; border-top: 1px solid var(--border-color, #e4e6eb); }
}
</style>
