<script setup lang="ts">
/**
 * 设置 → AI 随航 → 朗读: which speech model reads a selection aloud, and how.
 *
 * Nothing here knows a provider. The backend lists, per provider, the speech models
 * it offers and the knobs each one takes (voice, speed, format ...) as data, and
 * this panel renders that: the provider chips come from what is configured, the
 * model list from the provider, the fields from the model. A provider that gains a
 * speech adapter shows up here, with its own fields, without this file changing.
 */
import { computed, onMounted } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { providerLogo } from '../../utils/providerLogo'
import { charCount } from '../../utils/speechText'
import { effectiveOptions, PREVIEW_TEXT, useSpeechStore } from '../../stores/speech'
import { useSettingsStore } from '../../stores/settings'
import MediaFieldInput from './MediaFieldInput.vue'

const { t } = useI18n()
const speech = useSpeechStore()
const settingsStore = useSettingsStore()

// A fresh answer every time the panel opens: a provider may have been added or
// had its key removed since the last look. The saved voice is re-read too, like
// the other settings panels do: speech_* travel inside the whole settings object
// and every window keeps its own copy, so a window that has been open for a while
// would otherwise show (and, on its next save, write back) a stale choice made in
// another one.
onMounted(() => {
  void settingsStore.load()
  void speech.loadCapabilities()
})

const model = computed(() => speech.selectedModel)
const fields = computed(() => model.value?.fields ?? [])
/** Stored values over the model's defaults: what the form shows and what a read sends. */
const values = computed(() => effectiveOptions(model.value, speech.config.options))

/** The saved choice is no longer offered (deleted, disabled, key removed, model retired). */
const choiceGone = computed(() => speech.notConfiguredReason === 'provider-missing')

function pickProvider(providerId: string) {
  if (providerId === speech.providerId && model.value) return
  const provider = speech.providers.find((p) => p.providerId === providerId)
  const first = provider?.capabilities.find((c) => c.kind === 'speech')?.models[0]
  if (first) void speech.select(providerId, first.id)
}

function pickModel(modelId: string) {
  if (modelId && modelId !== speech.modelId) void speech.select(speech.providerId, modelId)
}

const previewing = computed(() => speech.isReading && speech.activeSource === 'preview')
const previewLoading = computed(() => previewing.value && speech.state === 'loading')
const previewError = computed(() => (speech.state === 'error' && !speech.setupPrompt ? speech.errorMessage : ''))

function togglePreview() {
  // `read` toggles: pressing it again while the preview plays stops it.
  void speech.preview()
}

function goProviders() { speech.openSettings('ai') }
</script>

<template>
  <div class="settings-section">
    <div v-if="!speech.loaded && !speech.loadError" class="state-note">
      <Icon icon="fluent:spinner-ios-20-regular" class="spin" width="15" height="15" />
      {{ t('speech.loading') }}
    </div>

    <div v-else-if="!speech.loaded" class="settings-card">
      <p class="error-text">{{ t('speech.loadFailed', { message: speech.loadError }) }}</p>
      <button class="ghost-btn self-start" @click="speech.loadCapabilities()">{{ t('speech.retry') }}</button>
    </div>

    <!-- No provider can speak at all: say why and where to fix it. -->
    <div v-else-if="speech.providers.length === 0" class="settings-card empty-card">
      <Icon icon="fluent:speaker-2-24-regular" width="30" height="30" class="empty-icon" />
      <div class="empty-title">{{ t('speech.noProvidersTitle') }}</div>
      <p class="setting-hint center">{{ t('speech.noProvidersBody') }}</p>
      <button class="primary-btn" @click="goProviders">{{ t('speech.goProviders') }}</button>
    </div>

    <template v-else>
      <div class="settings-card">
        <div v-if="choiceGone" class="notice warn">
          <Icon icon="fluent:warning-24-regular" width="14" height="14" />
          <span>{{ t('speech.providerMissing') }}</span>
        </div>
        <div v-else-if="speech.notConfiguredReason === 'unset'" class="notice">
          <Icon icon="fluent:info-24-regular" width="14" height="14" />
          <span>{{ t('speech.unsetHint') }}</span>
        </div>

        <div class="group">
          <label class="setting-label">{{ t('speech.providerLabel') }}</label>
          <div class="chips">
            <button
              v-for="p in speech.providers"
              :key="p.providerId"
              class="chip"
              :class="{ active: p.providerId === speech.providerId && !choiceGone }"
              @click="pickProvider(p.providerId)"
            >
              <img
                v-if="providerLogo(p.providerName, p.baseUrl)"
                :src="providerLogo(p.providerName, p.baseUrl)"
                class="chip-logo"
                alt=""
              />
              <span>{{ p.providerName }}</span>
            </button>
          </div>
        </div>

        <div v-if="speech.selectedCapability" class="group">
          <label class="setting-label" for="speech-model">{{ t('speech.modelLabel') }}</label>
          <select
            id="speech-model"
            class="field-input"
            :value="speech.modelId"
            @change="pickModel(($event.target as HTMLSelectElement).value)"
          >
            <option v-if="!model" value="" disabled>—</option>
            <option v-for="m in speech.selectedCapability.models" :key="m.id" :value="m.id">{{ m.displayName }}</option>
          </select>
          <p v-if="model?.note" class="setting-hint">{{ model.note }}</p>
          <p v-if="model?.maxPromptChars" class="setting-hint">
            {{ t('speech.maxChars', { n: model.maxPromptChars }) }}
          </p>
        </div>

        <!-- One control per field the model declares; the backend named them. -->
        <div v-if="model && fields.length" class="group">
          <label class="setting-label">{{ t('speech.voiceLabel') }}</label>
          <div class="fields">
            <MediaFieldInput
              v-for="f in fields"
              :key="`${speech.providerId}/${speech.modelId}/${f.key}`"
              :field="f"
              :model-value="values[f.key]"
              @update:model-value="speech.setOption(f.key, $event)"
            />
          </div>
        </div>
      </div>

      <div class="settings-card">
        <div class="field-row">
          <div>
            <label class="setting-label">{{ t('speech.skipCitations') }}</label>
            <p class="setting-hint">{{ t('speech.skipCitationsHint') }}</p>
          </div>
          <label class="toggle">
            <input
              type="checkbox"
              :checked="speech.skipCitations"
              @change="speech.save({ skipCitations: !speech.skipCitations })"
            />
            <span class="toggle-track" />
          </label>
        </div>

        <div class="divider" />

        <div class="preview-row">
          <button class="primary-btn" :disabled="!speech.isConfigured" @click="togglePreview">
            <Icon v-if="previewLoading" icon="fluent:spinner-ios-20-regular" class="spin" width="14" height="14" />
            <Icon v-else-if="previewing" icon="fluent:stop-24-filled" width="14" height="14" />
            <Icon v-else icon="fluent:speaker-2-24-regular" width="14" height="14" />
            {{ previewLoading ? t('speech.previewing') : previewing ? t('speech.previewStop') : t('speech.preview') }}
          </button>
          <p class="setting-hint grow">{{ t('speech.previewNote', { n: charCount(PREVIEW_TEXT) }) }}</p>
        </div>
        <p v-if="previewError" class="error-text">{{ previewError }}</p>
      </div>
    </template>
  </div>
</template>

<style scoped>
.settings-section {
  display: flex;
  flex-direction: column;
  gap: 18px;
  max-width: 760px;
  padding-bottom: 8px;
  box-sizing: border-box;
}

.settings-card {
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 18px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary));
}

.group { display: flex; flex-direction: column; gap: 7px; }
.field-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
.setting-label { font-size: 13px; font-weight: 600; color: var(--text-primary); display: block; }
.setting-hint { font-size: 12px; color: var(--text-tertiary); margin: 0; line-height: 1.55; }
.setting-hint.center { text-align: center; max-width: 440px; }
.setting-hint.grow { flex: 1; }
.divider { height: 1px; background: var(--border-subtle); margin: 2px 0; }

.state-note {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12.5px;
  color: var(--text-tertiary);
  padding: 6px 2px;
}

.empty-card { align-items: center; text-align: center; gap: 10px; padding: 30px 22px; }
.empty-icon { color: var(--text-tertiary); }
.empty-title { font-size: 14px; font-weight: 600; color: var(--text-primary); }

.notice {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  padding: 9px 11px;
  font-size: 12px;
  line-height: 1.55;
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
  background: var(--bg-hover);
}
.notice svg { flex-shrink: 0; margin-top: 2px; }
.notice.warn {
  color: var(--text-primary);
  background: color-mix(in srgb, #f59e0b 12%, var(--bg-primary));
  border: 1px solid color-mix(in srgb, #f59e0b 30%, transparent);
}
.notice.warn svg { color: #b45309; }

.chips { display: flex; flex-wrap: wrap; gap: 6px; }
.chip {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  padding: 6px 12px;
  font-size: 12.5px;
  font-weight: 500;
  color: var(--text-secondary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-pill);
}
.chip:hover { background: var(--bg-hover); color: var(--text-primary); }
.chip.active {
  color: var(--accent);
  background: var(--accent-light);
  border-color: color-mix(in srgb, var(--accent) 40%, transparent);
}
.chip-logo { width: 15px; height: 15px; object-fit: contain; }

.field-input {
  width: 100%;
  padding: 7px 10px;
  font-size: 12.5px;
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  box-sizing: border-box;
}
.field-input:focus { outline: none; border-color: var(--accent); }

.fields {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
  gap: 14px;
}

.toggle { display: inline-flex; align-items: center; cursor: pointer; flex-shrink: 0; }
.toggle input { display: none; }
.toggle-track { width: 32px; height: 18px; background: var(--border-default); border-radius: 9px; position: relative; transition: background 0.15s; }
.toggle input:checked + .toggle-track { background: var(--accent); }
.toggle-track::after { content: ''; position: absolute; width: 12px; height: 12px; border-radius: 50%; background: #fff; top: 3px; left: 3px; transition: left 0.15s; }
.toggle input:checked + .toggle-track::after { left: 17px; }

.preview-row { display: flex; align-items: center; gap: 12px; }
.ghost-btn, .primary-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 6px 13px;
  font-size: 12px;
  font-weight: 500;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
  color: var(--text-secondary);
  background: var(--bg-secondary);
  flex-shrink: 0;
}
.ghost-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.primary-btn { background: var(--accent); border-color: var(--accent); color: #fff; }
.primary-btn:hover:not(:disabled) { background: var(--accent-hover); }
.primary-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.self-start { align-self: flex-start; }

.error-text { font-size: 12px; color: #dc2626; margin: 0; line-height: 1.5; }

.spin { animation: sp-spin 1s linear infinite; }
@keyframes sp-spin { to { transform: rotate(360deg); } }
</style>
