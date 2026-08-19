<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { invoke } from '@tauri-apps/api/core'

const { t } = useI18n()

interface ExternalServer {
  id: string
  name: string
  command: string
  args: string[]
  env: Record<string, string>
  enabled: boolean
}

interface AgentSettingsView {
  maxRounds: number
  keepCacheWarm: boolean
  systemPrompt: string
  defaultSystemPrompt: string
  servers: ExternalServer[]
  builtinToolCount: number
  minRounds: number
  maxRoundsLimit: number
  maxServers: number
}

interface ToolInfo {
  name: string
  description?: string
  /** The tool's own `readOnlyHint`. Unstated counts as writing. */
  readOnly: boolean
}

interface ProbeResult {
  ok: boolean
  tools: ToolInfo[]
  error: string | null
}

/** Split a tool list the only way that matters before letting a model loose on
 *  it: what can change things, and what cannot. */
function byAccess(tools: ToolInfo[]) {
  return {
    readOnly: tools.filter(t => t.readOnly),
    writes: tools.filter(t => !t.readOnly),
  }
}

const view = ref<AgentSettingsView>({
  maxRounds: 50,
  keepCacheWarm: true,
  systemPrompt: '',
  defaultSystemPrompt: '',
  servers: [],
  builtinToolCount: 0,
  minRounds: 1,
  maxRoundsLimit: 500,
  maxServers: 20,
})
const builtinTools = ref<ToolInfo[]>([])

/** "只读 17" / "只读 15 · 可写 2" — the count alone did not say the thing worth
 *  knowing about a tool surface. */
const builtinAccess = computed(() => {
  const { readOnly, writes } = byAccess(builtinTools.value)
  const parts = []
  if (readOnly.length) parts.push(`${t('agentSettings.readOnly')} ${readOnly.length}`)
  if (writes.length) parts.push(`${t('agentSettings.canWrite')} ${writes.length}`)
  return parts.join(' · ') || t('agentSettings.builtinTools', { n: view.value.builtinToolCount })
})
const showBuiltinTools = ref(false)
const error = ref('')

// ── Round budget ──────────────────────────────────────────────────────────────

/** Plain-language read on what the current budget buys, so the number means
 *  something without the user having to run an experiment. */
const roundsVerdict = computed(() => {
  const n = view.value.maxRounds
  if (n <= 4) return t('agentSettings.roundsFew')
  if (n <= 15) return t('agentSettings.roundsBalanced')
  if (n <= 60) return t('agentSettings.roundsMany')
  return t('agentSettings.roundsDeep')
})

function setRounds(value: number) {
  const n = Math.round(Number(value))
  view.value.maxRounds = Number.isFinite(n)
    ? Math.min(Math.max(n, view.value.minRounds), view.value.maxRoundsLimit)
    : 50
  persist()
}

// ── System prompt ─────────────────────────────────────────────────────────────

/** Empty means "use the built-in one", so clearing the box restores the default
 *  rather than leaving the model with no instructions. */
const usingDefaultPrompt = computed(() => !view.value.systemPrompt.trim())

function resetPrompt() {
  view.value.systemPrompt = ''
  persist()
}

/** Start from the built-in text so editing it is a change, not a rewrite from
 *  a blank page. */
function forkDefaultPrompt() {
  if (usingDefaultPrompt.value) view.value.systemPrompt = view.value.defaultSystemPrompt
}

// ── Server editing ────────────────────────────────────────────────────────────

/** The row being edited, as free text. Args and env are multi-line strings here
 *  and only become arrays/objects on save — editing a list in a textarea is what
 *  every MCP client does, and it survives a half-typed line. */
interface Draft {
  id: string | null
  name: string
  command: string
  argsText: string
  envText: string
  enabled: boolean
}

const draft = ref<Draft | null>(null)
const draftError = ref('')
const probing = ref(false)
const probeResult = ref<ProbeResult | null>(null)

function blankDraft(): Draft {
  return { id: null, name: '', command: '', argsText: '', envText: '', enabled: true }
}

function toDraft(server: ExternalServer): Draft {
  return {
    id: server.id,
    name: server.name,
    command: server.command,
    argsText: server.args.join('\n'),
    envText: Object.entries(server.env).map(([k, v]) => `${k}=${v}`).join('\n'),
    enabled: server.enabled,
  }
}

function fromDraft(d: Draft): ExternalServer {
  const env: Record<string, string> = {}
  for (const line of d.envText.split('\n')) {
    const trimmed = line.trim()
    if (!trimmed) continue
    // Only the first '=' separates; values routinely contain more.
    const eq = trimmed.indexOf('=')
    if (eq <= 0) continue
    env[trimmed.slice(0, eq).trim()] = trimmed.slice(eq + 1).trim()
  }
  return {
    id: d.id ?? crypto.randomUUID(),
    name: d.name.trim(),
    command: d.command.trim(),
    args: d.argsText.split('\n').map(s => s.trim()).filter(Boolean),
    env,
    enabled: d.enabled,
  }
}

function startAdd() {
  draft.value = blankDraft()
  draftError.value = ''
  probeResult.value = null
}

function startEdit(server: ExternalServer) {
  draft.value = toDraft(server)
  draftError.value = ''
  probeResult.value = null
}

function cancelEdit() {
  draft.value = null
  draftError.value = ''
  probeResult.value = null
}

const canSaveDraft = computed(() =>
  !!draft.value?.name.trim() && !!draft.value?.command.trim())

async function saveDraft() {
  if (!draft.value) return
  if (!canSaveDraft.value) {
    draftError.value = t('agentSettings.nameRequired')
    return
  }
  const server = fromDraft(draft.value)
  const existing = view.value.servers.findIndex(s => s.id === server.id)
  if (existing >= 0) view.value.servers[existing] = server
  else view.value.servers = [...view.value.servers, server]
  draft.value = null
  probeResult.value = null
  await persist()
}

async function toggleServer(server: ExternalServer) {
  server.enabled = !server.enabled
  await persist()
}

async function removeServer(server: ExternalServer) {
  if (!confirm(t('agentSettings.removeConfirm'))) return
  view.value.servers = view.value.servers.filter(s => s.id !== server.id)
  if (draft.value?.id === server.id) cancelEdit()
  await persist()
}

/** Launch the server as configured and report what it offers. Nothing is saved
 *  — the point is to find out before committing to it. */
async function testDraft() {
  if (!draft.value || probing.value) return
  probing.value = true
  probeResult.value = null
  try {
    probeResult.value = await invoke<ProbeResult>('agent_probe_server', {
      server: fromDraft(draft.value),
    })
  } catch (e) {
    probeResult.value = { ok: false, tools: [], error: String(e) }
  } finally {
    probing.value = false
  }
}

// ── Persistence ───────────────────────────────────────────────────────────────

async function persist() {
  error.value = ''
  try {
    view.value = await invoke<AgentSettingsView>('agent_save_settings', {
      settings: {
        maxRounds: view.value.maxRounds,
        keepCacheWarm: view.value.keepCacheWarm,
        systemPrompt: view.value.systemPrompt,
        servers: view.value.servers,
      },
    })
  } catch (e) {
    error.value = String(e)
  }
}

async function refresh() {
  try {
    view.value = await invoke<AgentSettingsView>('agent_get_settings')
    builtinTools.value = await invoke<ToolInfo[]>('agent_list_builtin_tools')
  } catch (e) {
    error.value = String(e)
  }
}

onMounted(refresh)
</script>

<template>
  <div class="settings-section">
    <!-- Tool call budget -->
    <div class="settings-card">
      <div class="field-row">
        <label class="setting-label">{{ t('agentSettings.rounds') }}</label>
        <div class="rounds-value">
          <input
            type="number"
            class="rounds-input"
            :min="view.minRounds"
            :max="view.maxRoundsLimit"
            :value="view.maxRounds"
            @change="setRounds(($event.target as HTMLInputElement).valueAsNumber)"
          />
          <span class="rounds-unit">{{ t('agentSettings.roundsUnit') }}</span>
        </div>
      </div>
      <input
        type="range"
        class="rounds-slider"
        :min="view.minRounds"
        :max="view.maxRoundsLimit"
        :value="view.maxRounds"
        @input="view.maxRounds = ($event.target as HTMLInputElement).valueAsNumber"
        @change="setRounds(($event.target as HTMLInputElement).valueAsNumber)"
      />
      <p class="rounds-verdict">
        <Icon icon="fluent:lightbulb-24-regular" width="13" height="13" />
        <span>{{ roundsVerdict }}</span>
      </p>

      <div class="divider" />

      <div class="divider" />

      <div class="field-row">
        <div>
          <label class="setting-label">{{ t('agentSettings.systemPrompt') }}</label>
          <p class="setting-hint">{{ t('agentSettings.systemPromptHint') }}</p>
        </div>
        <button
          class="ghost-btn"
          :disabled="usingDefaultPrompt"
          @click="resetPrompt"
        >{{ t('agentSettings.resetPrompt') }}</button>
      </div>
      <textarea
        class="prompt-area"
        :class="{ inherited: usingDefaultPrompt }"
        rows="10"
        :value="usingDefaultPrompt ? view.defaultSystemPrompt : view.systemPrompt"
        :placeholder="view.defaultSystemPrompt"
        @focus="forkDefaultPrompt"
        @input="view.systemPrompt = ($event.target as HTMLTextAreaElement).value"
        @change="persist()"
      />
      <p class="prompt-state">
        {{ usingDefaultPrompt ? t('agentSettings.usingDefault') : t('agentSettings.customPrompt') }}
      </p>

      <div class="divider" />

      <div class="field-row">
        <label class="setting-label">{{ t('agentSettings.keepWarm') }}</label>
        <label class="toggle">
          <input
            type="checkbox"
            :checked="view.keepCacheWarm"
            @change="view.keepCacheWarm = !view.keepCacheWarm; persist()"
          />
          <span class="toggle-track" />
        </label>
      </div>
    </div>

    <!-- MCP servers -->
    <div class="settings-card">
      <label class="setting-label">{{ t('agentSettings.servers') }}</label>

      <!-- Built-in, always present -->
      <div class="server-row builtin">
        <span class="server-dot on" />
        <div class="server-body">
          <div class="server-title">
            <span class="server-name">{{ t('agentSettings.builtin') }}</span>
            <span class="badge">{{ t('agentSettings.builtinBadge') }}</span>
            <span class="tool-count">{{ builtinAccess }}</span>
          </div>
          <div class="server-sub">{{ t('agentSettings.builtinDesc') }}</div>
        </div>
        <button class="ghost-btn" @click="showBuiltinTools = !showBuiltinTools">
          {{ showBuiltinTools ? t('agentSettings.hideTools') : t('agentSettings.viewTools') }}
        </button>
      </div>
      <div v-if="showBuiltinTools" class="tool-groups">
        <div v-for="group in [
               { key: 'ro', label: t('agentSettings.readOnly'), note: t('agentSettings.readOnlyHint'), tools: byAccess(builtinTools).readOnly },
               { key: 'rw', label: t('agentSettings.canWrite'), note: t('agentSettings.builtinWriteHint'), tools: byAccess(builtinTools).writes },
             ]"
             :key="group.key"
             v-show="group.tools.length"
             class="tool-group"
        >
          <div class="tool-group-head" :class="{ writes: group.key === 'rw' }">
            <Icon
              :icon="group.key === 'ro' ? 'fluent:eye-24-regular' : 'fluent:pen-24-regular'"
              width="12" height="12"
            />
            <span>{{ group.label }}</span>
            <span class="tool-group-count">{{ group.tools.length }}</span>
            <span class="tool-group-note">{{ group.note }}</span>
          </div>
          <div class="tool-grid">
            <div
              v-for="tool in group.tools"
              :key="tool.name"
              class="tool-chip"
              :class="{ writes: group.key === 'rw' }"
              :title="tool.description"
            >
              <code>{{ tool.name }}</code>
            </div>
          </div>
        </div>
      </div>

      <!-- User-configured servers -->
      <div v-for="server in view.servers" :key="server.id" class="server-row">
        <span class="server-dot" :class="{ on: server.enabled }" />
        <div class="server-body">
          <div class="server-title">
            <span class="server-name">{{ server.name }}</span>
            <span class="state" :class="{ off: !server.enabled }">
              {{ server.enabled ? t('agentSettings.enabled') : t('agentSettings.disabled') }}
            </span>
          </div>
          <div class="server-sub mono">{{ server.command }} {{ server.args.join(' ') }}</div>
        </div>
        <label class="toggle">
          <input type="checkbox" :checked="server.enabled" @change="toggleServer(server)" />
          <span class="toggle-track" />
        </label>
        <button class="icon-btn" :title="t('agentSettings.edit')" @click="startEdit(server)">
          <Icon icon="fluent:edit-24-regular" width="14" height="14" />
        </button>
        <button class="icon-btn danger" :title="t('agentSettings.remove')" @click="removeServer(server)">
          <Icon icon="fluent:delete-24-regular" width="14" height="14" />
        </button>
      </div>

      <p v-if="view.servers.length === 0 && !draft" class="empty-note">
        {{ t('agentSettings.noServers') }}
      </p>

      <!-- Add / edit form -->
      <div v-if="draft" class="draft-form">
        <label class="form-label">{{ t('agentSettings.serverName') }}</label>
        <input v-model="draft.name" class="text-input" :placeholder="t('agentSettings.serverNamePlaceholder')" />

        <label class="form-label">{{ t('agentSettings.command') }}</label>
        <input v-model="draft.command" class="text-input mono" :placeholder="t('agentSettings.commandPlaceholder')" />
        <p class="form-hint">{{ t('agentSettings.commandHint') }}</p>

        <label class="form-label">{{ t('agentSettings.args') }}</label>
        <textarea v-model="draft.argsText" class="text-area mono" rows="3" :placeholder="t('agentSettings.argsPlaceholder')" />

        <label class="form-label">{{ t('agentSettings.env') }}</label>
        <textarea v-model="draft.envText" class="text-area mono" rows="2" :placeholder="t('agentSettings.envPlaceholder')" />

        <p v-if="draftError" class="error-text">{{ draftError }}</p>

        <div
          v-if="probeResult"
          class="probe-result"
          :class="{ bad: !probeResult.ok }"
        >
          <Icon
            :icon="probeResult.ok ? 'fluent:checkmark-circle-24-regular' : 'fluent:dismiss-circle-24-regular'"
            width="14" height="14"
          />
          <div>
            <div>
              {{ probeResult.ok
                ? t('agentSettings.testOk', { n: probeResult.tools.length })
                : t('agentSettings.testFail') }}
            </div>
            <div v-if="probeResult.error" class="probe-detail">{{ probeResult.error }}</div>
            <div v-else-if="probeResult.tools.length" class="probe-groups">
              <div v-for="group in [
                     { key: 'ro', label: t('agentSettings.readOnly'), tools: byAccess(probeResult.tools).readOnly },
                     { key: 'rw', label: t('agentSettings.canWrite'), tools: byAccess(probeResult.tools).writes },
                   ]"
                   :key="group.key"
                   v-show="group.tools.length"
                   class="probe-group"
              >
                <span class="probe-group-label" :class="{ writes: group.key === 'rw' }">
                  {{ group.label }} {{ group.tools.length }}
                </span>
                <span class="probe-detail mono">{{ group.tools.map(x => x.name).join(', ') }}</span>
              </div>
            </div>
          </div>
        </div>

        <div class="form-actions">
          <button class="ghost-btn" :disabled="probing || !canSaveDraft" @click="testDraft">
            {{ probing ? t('agentSettings.testing') : t('agentSettings.test') }}
          </button>
          <div class="spacer" />
          <button class="ghost-btn" @click="cancelEdit">{{ t('agentSettings.cancel') }}</button>
          <button class="primary-btn" :disabled="!canSaveDraft" @click="saveDraft">
            {{ t('agentSettings.save') }}
          </button>
        </div>
      </div>

      <button
        v-else-if="view.servers.length < view.maxServers"
        class="add-btn"
        @click="startAdd"
      >
        <Icon icon="fluent:add-24-regular" width="14" height="14" />
        {{ t('agentSettings.addServer') }}
      </button>
      <p v-else class="empty-note">{{ t('agentSettings.limitReached', { n: view.maxServers }) }}</p>

      <p v-if="error" class="error-text">{{ error }}</p>
    </div>
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

.field-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
.setting-label { font-size: 13px; font-weight: 600; color: var(--text-primary); display: block; }
.setting-hint { font-size: 12px; color: var(--text-tertiary); margin: 0; line-height: 1.55; }

/* Rounds */
.rounds-value { display: inline-flex; align-items: baseline; gap: 5px; flex-shrink: 0; }
.rounds-input {
  width: 62px;
  padding: 5px 8px;
  font-size: 14px;
  font-weight: 600;
  text-align: center;
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
}
.rounds-unit { font-size: 12px; color: var(--text-tertiary); }
.rounds-slider { width: 100%; accent-color: var(--accent); }
.rounds-verdict {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 0;
  font-size: 12px;
  color: var(--text-secondary);
}
.rounds-verdict svg { color: var(--accent); flex-shrink: 0; }
.divider { height: 1px; background: var(--border-subtle); margin: 2px 0; }

.prompt-area {
  width: 100%;
  padding: 10px 12px;
  font-size: 12px;
  line-height: 1.6;
  font-family: var(--font-mono, ui-monospace, monospace);
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  box-sizing: border-box;
  resize: vertical;
  white-space: pre-wrap;
}
.prompt-area:focus { outline: none; border-color: var(--accent); }
/* Greyed while it is the built-in text: it is being shown, not edited. Focus
   copies it in and this class drops away. */
.prompt-area.inherited { color: var(--text-tertiary); }
.prompt-state { font-size: 11px; color: var(--text-tertiary); margin: -6px 0 0; }

/* Server rows */
.server-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 11px 13px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
}
.server-row.builtin { border-color: color-mix(in srgb, var(--accent) 26%, var(--border-subtle)); }
.server-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--border-default);
  flex-shrink: 0;
}
.server-dot.on { background: #22c55e; box-shadow: 0 0 0 3px color-mix(in srgb, #22c55e 18%, transparent); }
.server-body { flex: 1; min-width: 0; }
.server-title { display: flex; align-items: center; gap: 7px; flex-wrap: wrap; }
.server-name { font-size: 13px; font-weight: 600; color: var(--text-primary); }
.badge {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: var(--radius-pill);
  background: color-mix(in srgb, var(--accent) 14%, transparent);
  color: var(--accent);
}
.tool-count, .state { font-size: 11px; color: var(--text-tertiary); }
.state.off { color: var(--text-tertiary); opacity: 0.7; }
.server-sub {
  font-size: 11.5px;
  color: var(--text-tertiary);
  margin-top: 3px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mono { font-family: var(--font-mono, ui-monospace, monospace); }

.toggle { display: inline-flex; align-items: center; cursor: pointer; flex-shrink: 0; }
.toggle input { display: none; }
.toggle-track { width: 32px; height: 18px; background: var(--border-default); border-radius: 9px; position: relative; transition: background 0.15s; }
.toggle input:checked + .toggle-track { background: var(--accent); }
.toggle-track::after { content: ''; position: absolute; width: 12px; height: 12px; border-radius: 50%; background: #fff; top: 3px; left: 3px; transition: left 0.15s; }
.toggle input:checked + .toggle-track::after { left: 17px; }

.icon-btn {
  display: inline-flex;
  padding: 5px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  flex-shrink: 0;
}
.icon-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.icon-btn.danger:hover { color: #dc2626; }

.tool-groups { display: flex; flex-direction: column; gap: 12px; }
.tool-group { display: flex; flex-direction: column; gap: 6px; }
.tool-group-head {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11.5px;
  font-weight: 600;
  color: #15803d;
}
/* Writing tools are the ones worth noticing, so they get the warning colour
   rather than the same grey as everything else. */
.tool-group-head.writes { color: #b45309; }
.tool-group-count {
  padding: 0 6px;
  border-radius: var(--radius-pill);
  font-size: 10.5px;
  font-variant-numeric: tabular-nums;
  background: color-mix(in srgb, currentColor 14%, transparent);
}
.tool-group-note { font-weight: 400; color: var(--text-tertiary); }

.tool-grid { display: flex; flex-wrap: wrap; gap: 5px; }
.tool-chip {
  padding: 3px 8px;
  font-size: 11px;
  border-radius: var(--radius-sm);
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  color: var(--text-secondary);
  cursor: default;
}
.tool-chip.writes {
  border-color: color-mix(in srgb, #f59e0b 34%, var(--border-subtle));
  background: color-mix(in srgb, #f59e0b 8%, var(--bg-primary));
}
.tool-chip code { font-family: var(--font-mono, ui-monospace, monospace); }

.probe-groups { display: flex; flex-direction: column; gap: 4px; margin-top: 4px; }
.probe-group { display: flex; align-items: baseline; gap: 6px; }
.probe-group-label {
  flex-shrink: 0;
  font-size: 10.5px;
  font-weight: 600;
  color: #15803d;
}
.probe-group-label.writes { color: #b45309; }

.empty-note { font-size: 12px; color: var(--text-tertiary); margin: 0; }

/* Draft form */
.draft-form {
  display: flex;
  flex-direction: column;
  gap: 6px;
  padding: 14px;
  border: 1px solid color-mix(in srgb, var(--accent) 24%, var(--border-subtle));
  border-radius: var(--radius-md);
  background: var(--bg-primary);
}
.form-label { font-size: 12px; font-weight: 600; color: var(--text-secondary); margin-top: 6px; }
.form-label:first-child { margin-top: 0; }
.form-hint { font-size: 11px; color: var(--text-tertiary); margin: 2px 0 0; line-height: 1.5; }
.text-input, .text-area {
  width: 100%;
  padding: 7px 10px;
  font-size: 12.5px;
  color: var(--text-primary);
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-sm);
  box-sizing: border-box;
}
.text-area { resize: vertical; line-height: 1.5; white-space: pre; }
.text-input:focus, .text-area:focus { outline: none; border-color: var(--accent); }

.form-actions { display: flex; align-items: center; gap: 8px; margin-top: 10px; }
.spacer { flex: 1; }
.ghost-btn, .primary-btn, .add-btn {
  padding: 6px 13px;
  font-size: 12px;
  font-weight: 500;
  border-radius: var(--radius-sm);
  border: 1px solid var(--border-subtle);
  color: var(--text-secondary);
  background: var(--bg-secondary);
}
.ghost-btn:hover:not(:disabled), .add-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.ghost-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.primary-btn { background: var(--accent); border-color: var(--accent); color: #fff; }
.primary-btn:disabled { opacity: 0.5; cursor: not-allowed; }
.add-btn { display: inline-flex; align-items: center; gap: 6px; align-self: flex-start; }

.probe-result {
  display: flex;
  align-items: flex-start;
  gap: 8px;
  margin-top: 8px;
  padding: 9px 11px;
  font-size: 12px;
  line-height: 1.55;
  border-radius: var(--radius-sm);
  background: color-mix(in srgb, #22c55e 9%, var(--bg-secondary));
  border: 1px solid color-mix(in srgb, #22c55e 26%, transparent);
  color: var(--text-primary);
}
.probe-result svg { flex-shrink: 0; margin-top: 2px; color: #15803d; }
.probe-result.bad {
  background: color-mix(in srgb, #ef4444 8%, var(--bg-secondary));
  border-color: color-mix(in srgb, #ef4444 26%, transparent);
}
.probe-result.bad svg { color: #b91c1c; }
.probe-detail {
  margin-top: 3px;
  font-size: 11px;
  color: var(--text-tertiary);
  word-break: break-word;
  white-space: pre-wrap;
}

.error-text { font-size: 12px; color: #dc2626; margin: 0; line-height: 1.5; }
</style>
