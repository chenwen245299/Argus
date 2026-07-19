<script setup lang="ts">
import { ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { useLibraryStore } from '../../stores/library'
import { useSettingsStore } from '../../stores/settings'
import { setLocale, type Locale } from '../../i18n'

const { t, locale } = useI18n()
const library = useLibraryStore()
const settingsStore = useSettingsStore()

const usdToCnyRate = ref('7.2')
const billingSaveStatus = ref<'' | 'saving' | 'saved'>('')

const semanticScholarKey = ref('')
const semanticScholarSaveStatus = ref<'' | 'saving' | 'saved'>('')

const easyScholarKey = ref('')
const easyScholarSaveStatus = ref<'' | 'saving' | 'saved'>('')

onMounted(async () => {
  await settingsStore.load()
  usdToCnyRate.value = formatRate(settingsStore.settings.usd_to_cny_rate)
})

// External links must go through the backend opener — a bare <a target="_blank">
// does nothing inside the Tauri webview.
function openUrl(url: string) {
  invoke('open_url', { url }).catch(console.error)
}

// Stored keys are encrypted at rest and never sent back to the UI, so each input
// starts empty; typing a value replaces the stored key.
async function saveSemanticScholarKey() {
  const key = semanticScholarKey.value.trim()
  if (!key) return
  semanticScholarSaveStatus.value = 'saving'
  await settingsStore.setSemanticScholarKey(key)
  semanticScholarKey.value = ''
  semanticScholarSaveStatus.value = 'saved'
  setTimeout(() => { semanticScholarSaveStatus.value = '' }, 2000)
}
async function clearSemanticScholarKey() {
  await settingsStore.setSemanticScholarKey('')
  semanticScholarKey.value = ''
}

async function saveEasyScholarKey() {
  const key = easyScholarKey.value.trim()
  if (!key) return
  easyScholarSaveStatus.value = 'saving'
  await settingsStore.setEasyscholarKey(key)
  easyScholarKey.value = ''
  easyScholarSaveStatus.value = 'saved'
  setTimeout(() => { easyScholarSaveStatus.value = '' }, 2000)
}
async function clearEasyScholarKey() {
  await settingsStore.setEasyscholarKey('')
  easyScholarKey.value = ''
}

function formatRate(rate: unknown) {
  const n = Number(rate)
  return Number.isFinite(n) && n > 0 ? String(n) : '7.2'
}

function parseRate(raw: string) {
  const v = Number(raw)
  return Number.isFinite(v) && v > 0 ? v : 7.2
}

async function saveBillingSettings() {
  const rate = parseRate(usdToCnyRate.value)
  usdToCnyRate.value = formatRate(rate)
  if (settingsStore.settings.usd_to_cny_rate === rate) return
  billingSaveStatus.value = 'saving'
  await settingsStore.save({ usd_to_cny_rate: rate })
  billingSaveStatus.value = 'saved'
  setTimeout(() => { billingSaveStatus.value = '' }, 2000)
}

const languageOptions: { id: Locale; label: string }[] = [
  { id: 'zh', label: '中文' },
  { id: 'en', label: 'English' },
]

function selectLanguage(id: Locale) {
  setLocale(id)
}

function shortPath(p: string): string {
  const parts = p.replace(/\\/g, '/').split('/')
  return parts.length <= 3 ? p : '…/' + parts.slice(-2).join('/')
}
</script>

<template>
  <div class="settings-panel">
    <h2>{{ t('settings.general') }}</h2>

    <div class="setting-group">
      <div class="setting-label">{{ t('settings.libraryPath') }}</div>
      <div class="setting-row">
        <code class="path-display" :title="library.currentPath ?? ''">
          {{ library.currentPath ? shortPath(library.currentPath) : '—' }}
        </code>
        <button class="btn-secondary" @click="library.pickAndOpen()">
          {{ t('settings.switchLibrary') }}
        </button>
      </div>
    </div>

    <div class="setting-group">
      <div class="setting-label">{{ t('settings.language') }}</div>
      <div class="segmented-control">
        <button
          v-for="option in languageOptions"
          :key="option.id"
          class="segment-btn"
          :class="{ active: locale === option.id }"
          @click="selectLanguage(option.id)"
        >
          {{ option.label }}
        </button>
      </div>
    </div>

    <div class="setting-group api-card">
      <div class="card-title">{{ t('settings.aiBilling') }}</div>
      <div class="card-field">
        <div class="setting-row billing-row">
          <span class="billing-desc">{{ t('settings.aiBillingDesc') }}</span>
          <label class="billing-rate-field">
            <span>USD/CNY</span>
            <input
              v-model="usdToCnyRate"
              class="text-input sm"
              type="number"
              min="0.0001"
              step="0.01"
              @blur="saveBillingSettings"
              @keydown.enter.prevent="saveBillingSettings"
            />
          </label>
          <span v-if="billingSaveStatus" class="billing-save">
            {{ billingSaveStatus === 'saving' ? '…' : t('settings.saved') }}
          </span>
        </div>
      </div>
    </div>

    <!-- Grouped external data services (both are third-party API keys). -->
    <div class="setting-group api-card">
      <div class="card-title">{{ t('settings.dataServices') }}</div>

      <!-- Semantic Scholar: powers automatic paper-metadata fetching -->
      <div class="card-field">
        <div class="setting-label">{{ t('settings.semanticScholar') }}</div>
        <div class="setting-row">
          <input
            v-model="semanticScholarKey"
            class="text-input"
            type="password"
            autocomplete="off"
            spellcheck="false"
            :placeholder="settingsStore.semanticScholarConfigured ? t('settings.semanticScholarConfigured') : t('settings.semanticScholarPlaceholder')"
            @blur="saveSemanticScholarKey"
            @keydown.enter.prevent="saveSemanticScholarKey"
          />
          <button
            v-if="settingsStore.semanticScholarConfigured"
            class="btn-secondary"
            @click="clearSemanticScholarKey"
          >
            {{ t('settings.semanticScholarClear') }}
          </button>
          <span v-if="semanticScholarSaveStatus" class="billing-save">
            {{ semanticScholarSaveStatus === 'saving' ? '…' : t('settings.saved') }}
          </span>
        </div>
        <div class="setting-hint">
          {{ t('settings.semanticScholarDesc') }}
          <a class="ext-link" @click="openUrl('https://www.semanticscholar.org/product/api')">
            {{ t('settings.semanticScholarLink') }}
          </a>
        </div>
      </div>

      <!-- easyScholar: journal/venue ranks -->
      <div class="card-field">
        <div class="setting-label">{{ t('settings.easyScholar') }}</div>
        <div class="setting-row">
          <input
            v-model="easyScholarKey"
            class="text-input"
            type="password"
            autocomplete="off"
            spellcheck="false"
            :placeholder="settingsStore.easyscholarConfigured ? t('settings.easyScholarConfigured') : t('settings.easyScholarPlaceholder')"
            @blur="saveEasyScholarKey"
            @keydown.enter.prevent="saveEasyScholarKey"
          />
          <button
            v-if="settingsStore.easyscholarConfigured"
            class="btn-secondary"
            @click="clearEasyScholarKey"
          >
            {{ t('settings.easyScholarClear') }}
          </button>
          <span v-if="easyScholarSaveStatus" class="billing-save">
            {{ easyScholarSaveStatus === 'saving' ? '…' : t('settings.saved') }}
          </span>
        </div>
        <div class="setting-hint">
          {{ t('settings.easyScholarDesc') }}
          <a class="ext-link" @click="openUrl('https://www.easyscholar.cc/')">
            {{ t('settings.easyScholarLink') }}
          </a>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-panel { padding: 24px 28px; }
h2 { font-size: 18px; font-weight: 600; margin-bottom: 24px; color: var(--text-primary); }

.setting-group { margin-bottom: 28px; }
.setting-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-secondary);
  margin-bottom: 10px;
  letter-spacing: 0;
}

.setting-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.setting-hint {
  margin-top: 8px;
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  line-height: 1.5;
}
.ext-link {
  color: var(--accent);
  cursor: pointer;
}
.ext-link:hover { text-decoration: underline; }

/* Grouped card for the external data-service API keys — sets them apart from
   the plain language / appearance rows and shows the two belong together. */
.api-card {
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--bg-secondary) 55%, var(--bg-primary));
  padding: 16px 18px 18px;
}
.api-card .card-title {
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.4px;
  color: var(--text-tertiary);
  margin-bottom: 16px;
}
.api-card .card-field + .card-field {
  margin-top: 18px;
  padding-top: 18px;
  border-top: 1px solid var(--border-subtle);
}

.path-display {
  flex: 1;
  font-family: var(--font-mono);
  font-size: var(--font-size-sm);
  background: var(--bg-tertiary);
  padding: 6px 10px;
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.btn-secondary {
  padding: 6px 14px;
  font-size: var(--font-size-sm);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  transition: background 0.12s;
  white-space: nowrap;
}
.btn-secondary:hover { background: var(--bg-hover); color: var(--text-primary); }

.segmented-control {
  display: inline-flex;
  gap: 2px;
  padding: 3px;
  background: var(--bg-tertiary);
  border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle);
}

.segment-btn {
  min-width: 86px;
  padding: 6px 12px;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
  transition: background 0.12s, color 0.12s, box-shadow 0.12s;
}
.segment-btn:hover { color: var(--text-primary); }
.segment-btn.active {
  background: var(--bg-primary);
  color: var(--text-primary);
  box-shadow: var(--shadow-sm);
}

.billing-row {
  justify-content: space-between;
  gap: 16px;
}

.billing-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  flex: 1;
  min-width: 0;
}

.billing-rate-field {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.billing-rate-field span {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
}

.billing-rate-field .text-input.sm {
  width: 92px;
  height: 28px;
  padding: 4px 8px;
}

.billing-save {
  font-size: var(--font-size-xs);
  color: var(--text-tertiary);
  width: 34px;
  text-align: right;
  flex-shrink: 0;
}

.text-input {
  width: 100%;
  padding: 8px 10px;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  transition: border-color 0.12s, box-shadow 0.12s;
}

.text-input:focus {
  outline: none;
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light);
}

.text-input.sm {
  width: 92px;
  height: 28px;
  padding: 4px 8px;
}
</style>
