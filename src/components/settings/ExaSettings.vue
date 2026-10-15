<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'

interface ExaSettings {
  enabled: boolean
  hasKey: boolean
  libraryRoot: string
}
const { t } = useI18n()
const settings = ref<ExaSettings>({ enabled: false, hasKey: false, libraryRoot: '' })
const apiKey = ref('')
const loaded = ref(false)
const saving = ref(false)
const error = ref('')
const saved = ref(false)

async function save(clearKey = false) {
  if (!loaded.value || saving.value) return
  saving.value = true
  error.value = ''
  saved.value = false
  try {
    settings.value = await invoke<ExaSettings>('exa_save_settings', {
      enabled: settings.value.enabled,
      apiKey: clearKey ? null : apiKey.value.trim() || null,
      clearKey,
      expectedRoot: settings.value.libraryRoot,
    })
    apiKey.value = ''
    saved.value = true
  } catch (e) {
    error.value = String(e)
  } finally {
    saving.value = false
  }
}

async function openDashboard() {
  try {
    await invoke('open_url', { url: 'https://dashboard.exa.ai/api-keys' })
  } catch (e) {
    error.value = String(e)
  }
}

onMounted(async () => {
  try {
    settings.value = await invoke<ExaSettings>('exa_get_settings')
    loaded.value = true
  } catch (e) {
    error.value = String(e)
  }
})
</script>

<template>
  <section class="exa-settings" :aria-label="t('exaSettings.title')">
    <div class="exa-heading">
      <label class="exa-title" for="exa-enabled">{{ t('exaSettings.title') }}</label>
      <input id="exa-enabled" v-model="settings.enabled" type="checkbox" :disabled="!loaded || saving" @change="saved = false" />
    </div>
    <p class="exa-hint">{{ t('exaSettings.description') }}</p>
    <p class="exa-permission">{{ t('exaSettings.permission') }}</p>
    <label class="exa-key-label" for="exa-api-key">Exa API Key</label>
    <input
      id="exa-api-key"
      v-model="apiKey"
      class="exa-key"
      type="password"
      autocomplete="off"
      spellcheck="false"
      :disabled="!loaded || saving"
      :placeholder="t(settings.hasKey ? 'exaSettings.keySaved' : 'exaSettings.keyPlaceholder')"
      @input="saved = false"
      @keydown.enter.prevent="save()"
    />
    <p class="exa-hint">{{ t('exaSettings.keyHint') }}</p>
    <p v-if="error" class="exa-error" role="alert">{{ error }}</p>
    <p v-else-if="saved" class="exa-hint" role="status">{{ t('exaSettings.saved') }}</p>
    <div class="exa-actions">
      <button class="exa-ghost" @click="openDashboard">{{ t('exaSettings.getKey') }}</button>
      <button v-if="settings.hasKey" class="exa-ghost" :disabled="saving" @click="save(true)">{{ t('exaSettings.clearKey') }}</button>
      <button class="exa-save" :disabled="!loaded || saving || (settings.enabled && !settings.hasKey && !apiKey.trim())" @click="save()">
        {{ t(saving ? 'exaSettings.saving' : 'agentSettings.save') }}
      </button>
    </div>
  </section>
</template>

<style scoped>
.exa-settings { display: flex; flex-direction: column; gap: 12px; padding: 18px; border: 1px solid var(--border-subtle); border-radius: var(--radius-lg); background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary)); }
.exa-heading { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.exa-heading input { accent-color: var(--accent); }
.exa-title, .exa-key-label { font-size: 13px; font-weight: 600; color: var(--text-primary); }
.exa-hint { margin: 0; font-size: 12px; line-height: 1.55; color: var(--text-tertiary); }
.exa-permission { margin: 0; font-size: 12px; line-height: 1.55; color: var(--text-secondary); }
.exa-key { width: 100%; min-width: 0; box-sizing: border-box; padding: 8px 10px; border: 1px solid var(--border-subtle); border-radius: var(--radius-sm); background: var(--bg-primary); color: var(--text-primary); font-size: 12px; }
.exa-actions { display: flex; align-items: center; flex-wrap: wrap; gap: 10px; }
.exa-actions button { padding: 6px 10px; border-radius: var(--radius-sm); font-size: 12px; cursor: pointer; }
.exa-ghost { color: var(--text-secondary); border: 1px solid var(--border-subtle); }
.exa-save { margin-left: auto; background: var(--accent); color: var(--bg-primary); }
.exa-error { margin: 0; color: var(--text-primary); font-size: 12px; overflow-wrap: anywhere; }
button:disabled { opacity: 0.5; cursor: default; }
</style>
