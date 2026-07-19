<script setup lang="ts">
// First-run onboarding: a welcome overlay with a short checklist that guides the
// user to configure the two things required for full functionality — a default
// AI model and (recommended) a Semantic Scholar API key. Each item links to the
// existing settings panel via the `argus-open-settings` window event, and its
// status tick flips automatically as the underlying store state becomes valid.
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { useAiStore } from '../stores/ai'
import { useSettingsStore } from '../stores/settings'
import { ensureFluentIcons, fluentReady, fluentIconFor } from '../utils/fluentEmoji'

const { t } = useI18n()
const aiStore = useAiStore()
const settingsStore = useSettingsStore()

// `dontRemind` when true (or once everything is configured) tells the parent to
// stop showing the guide on subsequent launches. Otherwise it re-appears next
// time, since setup is still incomplete.
const emit = defineEmits<{ close: [dontRemind: boolean] }>()
const dontRemind = ref(false)

// A default model counts as configured only when the stored selection actually
// points at an existing, enabled model — not merely when some provider exists.
const modelDone = computed(() => !!aiStore.findModel(aiStore.defaultSelection))
const ssDone = computed(() => settingsStore.semanticScholarConfigured)
const allDone = computed(() => modelDone.value && ssDone.value)

const waveIcon = computed(() => (fluentReady.value ? fluentIconFor('👋') : null))

function openSettings(section: 'ai' | 'general') {
  window.dispatchEvent(new CustomEvent('argus-open-settings', { detail: { section } }))
}

function close() {
  emit('close', dontRemind.value || allDone.value)
}

onMounted(() => { ensureFluentIcons() })
</script>

<template>
  <Teleport to="body">
    <div class="ob-overlay">
      <Transition name="ob" appear>
        <div class="ob-card">
          <!-- Header -->
          <div class="ob-header">
            <div class="ob-emoji">
              <Icon v-if="waveIcon" :icon="waveIcon" width="44" height="44" />
              <span v-else class="ob-emoji-native">👋</span>
            </div>
            <h2 class="ob-title">{{ t('onboarding.title') }}</h2>
            <p class="ob-subtitle">{{ t('onboarding.subtitle') }}</p>
          </div>

          <!-- Checklist -->
          <div class="ob-list">
            <!-- 1. Default AI model (required) -->
            <div class="ob-item" :class="{ done: modelDone }">
              <div class="ob-check">
                <Icon v-if="modelDone" icon="fluent:checkmark-24-regular" width="14" height="14" />
                <span v-else>1</span>
              </div>
              <div class="ob-body">
                <div class="ob-item-title">
                  {{ t('onboarding.modelTitle') }}
                  <span class="ob-badge required">{{ t('onboarding.required') }}</span>
                </div>
                <div class="ob-item-desc">{{ t('onboarding.modelDesc') }}</div>
              </div>
              <button v-if="!modelDone" class="ob-config-btn" @click="openSettings('ai')">
                {{ t('onboarding.configure') }} →
              </button>
              <span v-else class="ob-done-tag">{{ t('onboarding.configured') }}</span>
            </div>

            <!-- 2. Semantic Scholar API key (recommended) -->
            <div class="ob-item" :class="{ done: ssDone }">
              <div class="ob-check">
                <Icon v-if="ssDone" icon="fluent:checkmark-24-regular" width="14" height="14" />
                <span v-else>2</span>
              </div>
              <div class="ob-body">
                <div class="ob-item-title">
                  {{ t('onboarding.ssTitle') }}
                  <span class="ob-badge recommended">{{ t('onboarding.recommended') }}</span>
                </div>
                <div class="ob-item-desc">{{ t('onboarding.ssDesc') }}</div>
              </div>
              <button v-if="!ssDone" class="ob-config-btn" @click="openSettings('general')">
                {{ t('onboarding.configure') }} →
              </button>
              <span v-else class="ob-done-tag">{{ t('onboarding.configured') }}</span>
            </div>
          </div>

          <!-- Footer -->
          <div class="ob-footer">
            <label v-if="!allDone" class="ob-dismiss">
              <input type="checkbox" v-model="dontRemind" />
              <span>{{ t('onboarding.dontRemind') }}</span>
            </label>
            <span class="ob-footer-spacer" />
            <button class="ob-primary-btn" :class="{ ready: allDone }" @click="close">
              {{ allDone ? t('onboarding.finish') : t('onboarding.later') }}
            </button>
          </div>
        </div>
      </Transition>
    </div>
  </Teleport>
</template>

<style scoped>
.ob-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.34);
  backdrop-filter: blur(10px) saturate(1.15);
  -webkit-backdrop-filter: blur(10px) saturate(1.15);
  display: flex;
  align-items: center;
  justify-content: center;
  /* Below the settings modal (1000) so "Configure" can open it on top. */
  z-index: 900;
}

.ob-card {
  background: var(--bg-primary);
  border-radius: 16px;
  box-shadow: var(--shadow-lg);
  border: 1px solid var(--border-subtle);
  width: min(540px, 92vw);
  max-height: 88vh;
  overflow-y: auto;
  padding: 32px 30px 22px;
  font-family: var(--font-sans);
  color: var(--text-primary);
  user-select: none;
  -webkit-user-select: none;
}

/* Header */
.ob-header {
  text-align: center;
  margin-bottom: 24px;
}
.ob-emoji {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 48px;
  margin-bottom: 10px;
}
.ob-emoji-native { font-size: 40px; line-height: 1; }
.ob-title {
  font-size: 20px;
  font-weight: 650;
  color: var(--text-primary);
  margin-bottom: 6px;
}
.ob-subtitle {
  font-size: 13px;
  color: var(--text-secondary);
  line-height: 1.5;
}

/* Checklist */
.ob-list {
  display: flex;
  flex-direction: column;
  gap: 10px;
  margin-bottom: 24px;
}
.ob-item {
  display: flex;
  align-items: flex-start;
  gap: 12px;
  padding: 14px 16px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  transition: border-color 0.18s, background 0.18s;
}
.ob-item.done {
  border-color: color-mix(in srgb, #28C840 45%, var(--border-subtle));
  background: color-mix(in srgb, #28C840 8%, var(--bg-secondary));
}

.ob-check {
  flex-shrink: 0;
  width: 22px;
  height: 22px;
  border-radius: 50%;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 12px;
  font-weight: 600;
  margin-top: 1px;
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
  transition: background 0.18s, color 0.18s;
}
.ob-item.done .ob-check {
  background: #28C840;
  color: #fff;
}

.ob-body { flex: 1; min-width: 0; }
.ob-item-title {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 14px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 3px;
}
.ob-item-desc {
  font-size: 12px;
  line-height: 1.5;
  color: var(--text-secondary);
}
.ob-badge {
  font-size: 10px;
  font-weight: 500;
  padding: 1px 7px;
  border-radius: var(--radius-pill);
  white-space: nowrap;
}
.ob-badge.required {
  background: color-mix(in srgb, var(--accent) 15%, transparent);
  color: var(--accent);
}
.ob-badge.recommended {
  background: var(--bg-tertiary);
  color: var(--text-tertiary);
}

.ob-config-btn {
  flex-shrink: 0;
  align-self: center;
  padding: 6px 13px;
  font-size: 12px;
  font-weight: 550;
  color: var(--accent);
  background: var(--accent-light);
  border: none;
  border-radius: var(--radius-md);
  cursor: pointer;
  white-space: nowrap;
  transition: filter 0.12s;
}
.ob-config-btn:hover { filter: brightness(0.96); }
.ob-config-btn:active { filter: brightness(0.9); }

.ob-done-tag {
  flex-shrink: 0;
  align-self: center;
  font-size: 12px;
  font-weight: 550;
  color: #1a9e37;
  white-space: nowrap;
}

/* Footer */
.ob-footer {
  display: flex;
  align-items: center;
  gap: 10px;
}
.ob-footer-spacer { flex: 1; }
.ob-dismiss {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
  cursor: pointer;
  user-select: none;
}
/* Custom checkbox — the native control renders as an opaque black box in the
   webview, so draw our own to match the app's theme. */
.ob-dismiss input {
  appearance: none;
  -webkit-appearance: none;
  width: 15px;
  height: 15px;
  margin: 0;
  flex-shrink: 0;
  border: 1.5px solid var(--border-default);
  border-radius: 4px;
  background: var(--bg-primary);
  cursor: pointer;
  position: relative;
  transition: background 0.12s, border-color 0.12s;
}
.ob-dismiss input:hover { border-color: var(--accent); }
.ob-dismiss input:checked {
  background: var(--accent);
  border-color: var(--accent);
}
.ob-dismiss input:checked::after {
  content: '';
  position: absolute;
  left: 4px;
  top: 1px;
  width: 4px;
  height: 8px;
  border: solid #fff;
  border-width: 0 2px 2px 0;
  transform: rotate(45deg);
}
.ob-dismiss:hover { color: var(--text-primary); }

.ob-primary-btn {
  padding: 8px 20px;
  font-size: 13px;
  font-weight: 600;
  color: #fff;
  background: var(--accent);
  border: none;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: filter 0.12s, transform 0.12s;
}
.ob-primary-btn:hover { filter: brightness(1.05); }
.ob-primary-btn:active { transform: scale(0.98); }
.ob-primary-btn.ready {
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 25%, transparent);
}

/* Transition */
.ob-enter-active { animation: ob-in 0.2s cubic-bezier(0.34, 1.56, 0.64, 1); }
.ob-leave-active { animation: ob-in 0.14s ease reverse; }
@keyframes ob-in {
  from { opacity: 0; transform: scale(0.94) translateY(12px); }
  to   { opacity: 1; transform: scale(1) translateY(0); }
}
</style>
