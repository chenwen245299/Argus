<script setup lang="ts">
import { computed, ref, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { useLibraryStore } from '../../stores/library'
import { useSettingsStore } from '../../stores/settings'
import { setLocale, type Locale } from '../../i18n'
import type { ThemeId } from '../../types'

const { t, locale } = useI18n()
const library = useLibraryStore()
const settingsStore = useSettingsStore()

const usdToCnyRate = ref('7.2')
const billingSaveStatus = ref<'' | 'saving' | 'saved'>('')

onMounted(async () => {
  await settingsStore.load()
  usdToCnyRate.value = formatRate(settingsStore.settings.usd_to_cny_rate)
})

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

interface ThemeOption {
  id: ThemeId
  label: string
  preview: { bg: string; sidebar: string; accent: string; text: string }
}

const languageOptions: { id: Locale; label: string }[] = [
  { id: 'zh', label: '中文' },
  { id: 'en', label: 'English' },
]

const themes = computed<ThemeOption[]>(() => [
  {
    id: 'system',
    label: t('settings.themeSystem'),
    preview: { bg: 'linear-gradient(135deg, #ffffff 50%, #1c1c1e 50%)', sidebar: 'rgba(0,0,0,0.08)', accent: '#007aff', text: '#888888' },
  },
  {
    id: 'light',
    label: t('settings.themeLight'),
    preview: { bg: '#ffffff', sidebar: '#f2f2f7', accent: '#007aff', text: '#1c1c1e' },
  },
  {
    id: 'dark',
    label: t('settings.themeDark'),
    preview: { bg: '#1c1c1e', sidebar: '#2c2c2e', accent: '#0a84ff', text: '#f5f5f7' },
  },
  {
    id: 'warm',
    label: t('settings.themeWarm'),
    preview: { bg: '#faf8f5', sidebar: '#f2ede5', accent: '#d97706', text: '#1c1a16' },
  },
  {
    id: 'forest',
    label: t('settings.themeForest'),
    preview: { bg: '#f6f9f4', sidebar: '#ecf2e8', accent: '#2d7a4f', text: '#182418' },
  },
  {
    id: 'rose',
    label: t('settings.themeRose'),
    preview: { bg: '#fdf8fc', sidebar: '#f5eef5', accent: '#9333ea', text: '#1c1820' },
  },
])

async function setTheme(id: ThemeId) {
  await settingsStore.save({ appearance: id })
}

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

    <div class="setting-group">
      <div class="setting-label">{{ t('settings.aiBilling') }}</div>
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

    <div class="setting-group">
      <div class="setting-label">{{ t('settings.appearance') }}</div>
      <div class="theme-grid">
        <button
          v-for="theme in themes"
          :key="theme.id"
          class="theme-card"
          :class="{ active: settingsStore.settings.appearance === theme.id }"
          @click="setTheme(theme.id)"
        >
          <!-- Mini app preview -->
          <div class="theme-preview" :style="{ background: theme.preview.bg }">
            <div class="preview-sidebar" :style="{ background: theme.preview.sidebar }" />
            <div class="preview-body">
              <div class="preview-bar" :style="{ background: theme.preview.accent, opacity: '0.85' }" />
              <div class="preview-line" :style="{ background: theme.preview.text, opacity: '0.18' }" />
              <div class="preview-line short" :style="{ background: theme.preview.text, opacity: '0.12' }" />
            </div>
          </div>
          <!-- Label + active check -->
          <div class="theme-footer">
            <span class="theme-name">{{ theme.label }}</span>
            <svg v-if="settingsStore.settings.appearance === theme.id"
              class="check-icon" width="12" height="12" viewBox="0 0 24 24"
              fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round">
              <polyline points="20 6 9 17 4 12"/>
            </svg>
          </div>
        </button>
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

/* ── Theme grid ── */
.theme-grid {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 10px;
}

.theme-card {
  display: flex;
  flex-direction: column;
  border: 2px solid var(--border-default);
  border-radius: var(--radius-lg);
  overflow: hidden;
  cursor: pointer;
  transition: border-color 0.15s, box-shadow 0.15s, transform 0.1s;
  background: var(--bg-secondary);
  text-align: left;
}
.theme-card:hover {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light);
  transform: translateY(-1px);
}
.theme-card.active {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light);
}

/* Mini preview area */
.theme-preview {
  height: 64px;
  display: flex;
  flex-shrink: 0;
  overflow: hidden;
  background-size: cover;
}

.preview-sidebar {
  width: 28%;
  flex-shrink: 0;
  border-right: 1px solid rgba(0,0,0,0.08);
}

.preview-body {
  flex: 1;
  padding: 8px 8px 6px;
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.preview-bar {
  height: 6px;
  border-radius: 3px;
  width: 55%;
}

.preview-line {
  height: 4px;
  border-radius: 2px;
  width: 90%;
  background: #000;
}
.preview-line.short { width: 65%; }

/* Footer */
.theme-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 7px 10px;
  gap: 4px;
}

.theme-name {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-secondary);
}
.theme-card.active .theme-name { color: var(--accent); font-weight: 600; }

.check-icon {
  color: var(--accent);
  flex-shrink: 0;
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
