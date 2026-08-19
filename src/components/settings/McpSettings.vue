<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'
import { copyText } from '../../utils/clipboard'

const { t } = useI18n()

interface McpStatus {
  enabled: boolean
}

interface ClientConfig {
  executable: string
  claudeCode: string
  desktopConfigPath: string
  desktopSnippet: string
  codexConfigPath: string
  codexSnippet: string
}

const status = ref<McpStatus>({ enabled: false })
const config = ref<ClientConfig>({
  executable: '',
  claudeCode: '',
  desktopConfigPath: '',
  desktopSnippet: '',
  codexConfigPath: '',
  codexSnippet: '',
})
const activeClient = ref<'code' | 'desktop' | 'codex'>('code')
const busy = ref(false)
const error = ref('')
const copiedKey = ref('')

const clientTabs = [
  { id: 'code', label: 'Claude Code' },
  { id: 'desktop', label: 'Claude Desktop' },
  { id: 'codex', label: 'Codex' },
] as const

/** Claude Desktop and Codex differ only in file, format and restart wording, so
 *  they share one step renderer. */
const fileSteps = computed(() => {
  const desktop = activeClient.value === 'desktop'
  return [
    {
      text: desktop ? t('mcpSettings.desktopStep1') : t('mcpSettings.codexStep1'),
      code: desktop ? config.value.desktopConfigPath : config.value.codexConfigPath,
      key: 'path',
    },
    {
      text: t('mcpSettings.fileStep2'),
      code: desktop ? config.value.desktopSnippet : config.value.codexSnippet,
      key: 'snippet',
    },
    {
      text: desktop ? t('mcpSettings.desktopStep3') : t('mcpSettings.codexStep3'),
      code: '',
      key: '',
    },
  ]
})

/** "How do I know it worked" differs per client — same slot, different sentence. */
const verifyNote = computed(() => {
  if (activeClient.value === 'code') return t('mcpSettings.verifyCode')
  if (activeClient.value === 'desktop') return t('mcpSettings.verifyDesktop')
  return t('mcpSettings.verifyCodex')
})

/** The doodle in the card corner follows the tab: a terminal for the one-liner,
 *  a config file for the two clients that need one edited. */
const clientDoodle = computed(() => (activeClient.value === 'code' ? 'terminal' : 'config-file'))

async function refresh() {
  try {
    status.value = await invoke<McpStatus>('mcp_get_status')
    config.value = await invoke<ClientConfig>('mcp_get_client_config')
  } catch (e) {
    error.value = String(e)
  }
}

async function toggle() {
  busy.value = true
  error.value = ''
  try {
    status.value = await invoke<McpStatus>('mcp_set_enabled', { enabled: !status.value.enabled })
  } catch (e) {
    error.value = String(e)
  } finally {
    busy.value = false
  }
}

async function copy(key: string, text: string) {
  if (await copyText(text)) {
    copiedKey.value = key
    setTimeout(() => { if (copiedKey.value === key) copiedKey.value = '' }, 1600)
  }
}

onMounted(refresh)
</script>

<template>
  <div class="settings-section">
    <div class="section-heading">
      <h2 class="section-title">{{ t('mcpSettings.title') }}</h2>
      <p class="section-desc">{{ t('mcpSettings.desc') }}</p>
    </div>

    <!-- Master switch -->
    <div class="settings-card">
      <div class="field-row">
        <div>
          <label class="setting-label">{{ t('mcpSettings.enable') }}</label>
          <p class="setting-hint">{{ t('mcpSettings.enableHint') }}</p>
        </div>
        <label class="toggle">
          <input type="checkbox" :checked="status.enabled" :disabled="busy" @change="toggle" />
          <span class="toggle-track" />
        </label>
      </div>
      <p v-if="error" class="error-text">{{ error }}</p>
    </div>

    <!-- What the endpoint is for -->
    <div class="settings-card">
      <div class="field-row">
        <div>
          <label class="setting-label">{{ t('mcpSettings.what') }}</label>
          <p class="setting-hint">{{ t('mcpSettings.whatHint') }}</p>
        </div>
      </div>
    </div>

    <!-- How to connect -->
    <div class="settings-card">
      <div class="field-row connect-row">
        <div>
          <label class="setting-label">{{ t('mcpSettings.connect') }}</label>
          <p class="setting-hint">{{ t('mcpSettings.connectHint') }}</p>
        </div>
        <!-- Fills the empty top-right of the card: a drawing of whatever the
             current tab asks you to open, with a scribbled pointer down to it.
             Positioned out of flow so its height never leaves a blank band
             above the tabs; `.connect-row` reserves the gutter it sits in. -->
        <div class="corner-doodle">
          <Icon :icon="`doodle:${clientDoodle}`" width="38" height="38" />
          <span class="corner-hand">{{ t('mcpSettings.connectDoodle') }}</span>
          <Icon class="corner-arrow" icon="doodle:curved-arrow" width="34" height="25" />
        </div>
      </div>

      <div class="client-tabs">
        <button
          v-for="tab in clientTabs"
          :key="tab.id"
          class="client-tab"
          :class="{ active: activeClient === tab.id }"
          @click="activeClient = tab.id"
        >
          {{ tab.label }}
        </button>
      </div>

      <!-- Claude Code: one command -->
      <ol v-if="activeClient === 'code'" class="steps">
        <li>
          <span>{{ t('mcpSettings.codeStep1') }}</span>
          <div class="snippet-wrap">
            <pre class="snippet">{{ config.claudeCode }}</pre>
            <button class="snippet-copy" @click="copy('code', config.claudeCode)">
              <Icon :icon="copiedKey === 'code' ? 'fluent:checkmark-24-regular' : 'fluent:copy-24-regular'" width="14" height="14" />
              <span>{{ copiedKey === 'code' ? t('mcpSettings.copied') : t('mcpSettings.copy') }}</span>
            </button>
          </div>
        </li>
      </ol>

      <!-- Claude Desktop: edit a config file -->
      <ol v-else class="steps">
        <li v-for="step in fileSteps" :key="step.text">
          <span>{{ step.text }}</span>
          <div v-if="step.code" class="snippet-wrap">
            <pre class="snippet">{{ step.code }}</pre>
            <button class="snippet-copy" @click="copy(step.key, step.code)">
              <Icon :icon="copiedKey === step.key ? 'fluent:checkmark-24-regular' : 'fluent:copy-24-regular'" width="14" height="14" />
              <span>{{ copiedKey === step.key ? t('mcpSettings.copied') : t('mcpSettings.copy') }}</span>
            </button>
          </div>
        </li>
      </ol>

      <!-- Verifying is the step people ask about most, so it gets its own block
           rather than a line buried in the steps above. -->
      <div class="verify-block">
        <span class="verify-head">{{ t('mcpSettings.verify') }}</span>
        <p class="verify-line">{{ verifyNote }}</p>
      </div>
    </div>

    <!-- What it can and cannot see -->
    <div class="settings-card">
      <div class="field-row">
        <div>
          <label class="setting-label">{{ t('mcpSettings.scope') }}</label>
          <p class="setting-hint">{{ t('mcpSettings.scopeHint') }}</p>
        </div>
        <Icon class="card-mark" icon="doodle:shield-lock" width="40" height="40" />
      </div>
      <div class="scope-grid">
        <div class="scope-col allow">
          <span class="scope-head">
            <Icon icon="fluent:checkmark-circle-24-regular" width="14" height="14" />
            {{ t('mcpSettings.canRead') }}
          </span>
          <ul>
            <li>{{ t('mcpSettings.allowPapers') }}</li>
            <li>{{ t('mcpSettings.allowNotes') }}</li>
            <li>{{ t('mcpSettings.allowCanvas') }}</li>
            <li>{{ t('mcpSettings.allowSnippets') }}</li>
            <li>{{ t('mcpSettings.allowChats') }}</li>
          </ul>
        </div>
        <div class="scope-col deny">
          <span class="scope-head">
            <Icon icon="fluent:prohibited-24-regular" width="14" height="14" />
            {{ t('mcpSettings.cannotRead') }}
          </span>
          <ul>
            <li>{{ t('mcpSettings.denyKeys') }}</li>
            <li>{{ t('mcpSettings.denyProviders') }}</li>
            <li>{{ t('mcpSettings.denyCost') }}</li>
            <li>{{ t('mcpSettings.denyWrite') }}</li>
          </ul>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-section {
  display: flex;
  flex-direction: column;
  gap: 18px;
  max-width: 760px;
  padding: 24px 28px;
  box-sizing: border-box;
}

.section-heading { display: flex; flex-direction: column; gap: 8px; }
.section-title { font-size: 18px; font-weight: 600; color: var(--text-primary); margin: 0; }
.section-desc { font-size: 13px; color: var(--text-secondary); margin: 0; line-height: 1.5; }

.settings-card {
  position: relative;
  display: flex;
  flex-direction: column;
  gap: 14px;
  padding: 18px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary));
}

.field-row { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
/* Reserve the top-right gutter the (absolutely positioned) corner-doodle sits in,
   so the description text keeps its old width instead of flowing under the mark. */
.connect-row { padding-right: 168px; }
.setting-label { font-size: 13px; font-weight: 600; color: var(--text-primary); display: block; margin-bottom: 5px; }
.setting-hint { font-size: 12px; color: var(--text-tertiary); margin: 0; line-height: 1.5; }

.toggle { display: inline-flex; align-items: center; cursor: pointer; flex-shrink: 0; }
.toggle input { display: none; }
.toggle-track { width: 36px; height: 20px; background: var(--border-default); border-radius: 10px; position: relative; transition: background 0.15s; }
.toggle input:checked + .toggle-track { background: var(--accent); }
.toggle-track::after { content: ''; position: absolute; width: 14px; height: 14px; border-radius: 50%; background: #fff; top: 3px; left: 3px; transition: left 0.15s; }
.toggle input:checked + .toggle-track::after { left: 19px; }

.error-text { font-size: 12px; color: #dc2626; margin: 0; line-height: 1.5; }

.client-tabs { display: flex; gap: 4px; }
.client-tab {
  padding: 5px 12px;
  font-size: 12px;
  font-weight: 500;
  color: var(--text-secondary);
  border-radius: var(--radius-pill);
}
.client-tab:hover { background: var(--bg-hover); }
.client-tab.active { background: var(--accent); color: #fff; }

.steps {
  margin: 0;
  padding-left: 18px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.steps li { font-size: 12.5px; color: var(--text-primary); line-height: 1.6; }
.steps li > span { display: block; margin-bottom: 6px; }

.snippet-wrap { position: relative; }
.snippet {
  margin: 0;
  padding: 12px 14px;
  padding-right: 84px;
  font-size: 11.5px;
  line-height: 1.6;
  font-family: var(--font-mono, ui-monospace, monospace);
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  overflow-x: auto;
  white-space: pre;
}
.snippet-copy {
  position: absolute;
  top: 8px;
  right: 8px;
  display: inline-flex;
  align-items: center;
  gap: 5px;
  padding: 4px 9px;
  font-size: 11px;
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
}
.snippet-copy:hover { color: var(--text-primary); background: var(--bg-hover); }

/* ── Hand-drawn tutorial bits ── */

/* Corner drawing in the "how to connect" card, pointing at the tabs below.
   Absolutely placed so it decorates the corner without adding to the row height
   (which is what left the blank band above the tabs). */
.corner-doodle {
  position: absolute;
  top: 18px;
  right: 18px;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 2px;
  width: 152px;
  color: color-mix(in srgb, var(--accent) 70%, var(--text-tertiary));
  transform: rotate(-1.5deg);
  /* Purely decorative overlay — never let it capture clicks meant for the tabs. */
  pointer-events: none;
}
.corner-hand {
  font-family: var(--font-hand);
  font-size: 11.5px;
  line-height: 1.4;
  text-align: center;
  letter-spacing: 0.2px;
  color: var(--text-tertiary);
}
.corner-arrow { transform: rotate(105deg) scaleY(-1); opacity: 0.75; margin-top: -2px; }

/* Faint drawing that labels a card without competing with its text. */
.card-mark {
  flex-shrink: 0;
  color: var(--text-tertiary);
  opacity: 0.55;
  transform: rotate(-4deg);
}

.verify-block {
  padding: 11px 13px;
  border: 1px solid color-mix(in srgb, #22c55e 30%, var(--border-subtle));
  border-radius: var(--radius-md);
  background: color-mix(in srgb, #22c55e 5%, var(--bg-primary));
}
.verify-head {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 5px;
}
.verify-line { margin: 0; font-size: 12px; line-height: 1.65; color: var(--text-secondary); }

.scope-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px; }
.scope-col {
  padding: 12px 14px;
  border-radius: var(--radius-md);
  border: 1px solid var(--border-subtle);
  background: var(--bg-primary);
}
.scope-col.allow { border-color: color-mix(in srgb, #22c55e 26%, var(--border-subtle)); }
.scope-col.deny { border-color: color-mix(in srgb, #ef4444 22%, var(--border-subtle)); }
.scope-head {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  font-size: 12px;
  font-weight: 600;
  margin-bottom: 8px;
}
.scope-col.allow .scope-head { color: #15803d; }
.scope-col.deny .scope-head { color: #b91c1c; }
.scope-col ul { margin: 0; padding-left: 16px; }
.scope-col li { font-size: 12px; color: var(--text-secondary); line-height: 1.7; }

@media (max-width: 760px) {
  .scope-grid { grid-template-columns: 1fr; }
  /* No doodle at this width, so its gutter would only be dead space. */
  .corner-doodle { display: none; }
  .connect-row { padding-right: 0; }
}
</style>
