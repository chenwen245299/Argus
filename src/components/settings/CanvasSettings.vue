<script setup lang="ts">
import { onMounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { useCanvasStore } from '../../stores/canvas'

const { t } = useI18n()
const canvasStore = useCanvasStore()

const hoverOptions = [
  { value: 'notes', labelKey: 'canvasSettings.sourceNotes', icon: 'fluent:notebook-24-regular' },
  { value: 'summary', labelKey: 'canvasSettings.sourceSummary', icon: 'fluent:sparkle-24-regular' },
  { value: 'abstract', labelKey: 'canvasSettings.sourceAbstract', icon: 'fluent:text-align-left-24-regular' },
  { value: 'none', labelKey: 'canvasSettings.sourceNone', icon: 'fluent:prohibited-24-regular' },
] as const

onMounted(async () => {
  await canvasStore.loadSettings()
})

watch(() => canvasStore.settings.hover_content_source, () => {
  canvasStore.saveSettings()
})
</script>

<template>
  <div class="settings-section">
    <div class="section-heading">
      <h2 class="section-title">{{ t('canvasSettings.title') }}</h2>
      <p class="section-desc">{{ t('canvasSettings.desc') }}</p>
    </div>

    <div class="settings-card">
      <div class="card-head">
        <div>
          <label class="setting-label">{{ t('canvasSettings.hoverContent') }}</label>
          <p class="setting-hint">{{ t('canvasSettings.hoverContentDesc') }}</p>
        </div>
      </div>

      <div class="option-grid">
        <label
          v-for="option in hoverOptions"
          :key="option.value"
          class="option-card"
          :class="{ active: canvasStore.settings.hover_content_source === option.value }"
        >
          <input
            type="radio"
            :value="option.value"
            v-model="canvasStore.settings.hover_content_source"
          />
          <span class="option-icon">
            <Icon :icon="option.icon" width="19" height="19" />
          </span>
          <span class="option-label">{{ t(option.labelKey) }}</span>
          <span class="option-check">
            <Icon icon="fluent:checkmark-24-filled" width="14" height="14" />
          </span>
        </label>
      </div>

      <div class="setting-actions">
        <span v-if="canvasStore.settingsSaved" class="saved-pill">{{ t('canvasSettings.saved') }}</span>
        <button
          class="save-btn"
          :disabled="canvasStore.settingsSaving"
          @click="canvasStore.saveSettings()"
        >
          <Icon v-if="canvasStore.settingsSaving" icon="fluent:arrow-clockwise-24-regular" class="spin" width="14" height="14" />
          <span>{{ canvasStore.settingsSaving ? t('canvasSettings.saving') : t('canvasSettings.save') }}</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-section {
  display: flex;
  flex-direction: column;
  gap: 22px;
  max-width: 760px;
  padding: 24px 28px;
  box-sizing: border-box;
}

.section-heading {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.section-title {
  font-size: 18px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
  letter-spacing: 0;
}

.section-desc {
  font-size: 13px;
  color: var(--text-secondary);
  margin: 0;
  line-height: 1.5;
}

.settings-card {
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 18px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  background: color-mix(in srgb, var(--bg-secondary) 72%, var(--bg-primary));
  box-shadow: inset 0 1px 0 color-mix(in srgb, #fff 52%, transparent);
}

.card-head {
  display: flex;
  justify-content: space-between;
  gap: 14px;
}

.setting-label {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  display: block;
  margin-bottom: 5px;
}

.setting-hint {
  font-size: 12px;
  color: var(--text-tertiary);
  margin: 0;
  line-height: 1.5;
}

.option-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.option-card {
  position: relative;
  display: flex;
  align-items: center;
  min-width: 0;
  gap: 10px;
  min-height: 46px;
  padding: 10px 36px 10px 12px;
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-md);
  background: var(--bg-primary);
  font-size: 13px;
  color: var(--text-primary);
  cursor: pointer;
  box-sizing: border-box;
  transition:
    border-color 0.14s ease,
    background 0.14s ease,
    box-shadow 0.14s ease,
    transform 0.14s ease;
}

.option-card:hover {
  border-color: color-mix(in srgb, var(--accent) 38%, var(--border-subtle));
  background: color-mix(in srgb, var(--bg-primary) 84%, var(--accent-light));
}

.option-card.active {
  border-color: color-mix(in srgb, var(--accent) 70%, var(--border-subtle));
  background: color-mix(in srgb, var(--accent-light) 62%, var(--bg-primary));
  box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 12%, transparent);
}

.option-card input {
  position: absolute;
  opacity: 0;
  pointer-events: none;
}

.option-icon {
  width: 28px;
  height: 28px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: var(--radius-md);
  color: var(--text-secondary);
  background: var(--bg-tertiary);
  flex-shrink: 0;
}

.option-card.active .option-icon {
  color: var(--accent);
  background: color-mix(in srgb, var(--accent-light) 76%, var(--bg-primary));
}

.option-label {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-weight: 500;
}

.option-check {
  position: absolute;
  right: 12px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: #fff;
  background: var(--accent);
  opacity: 0;
  transform: scale(0.86);
  transition: opacity 0.12s ease, transform 0.12s ease;
}

.option-card.active .option-check {
  opacity: 1;
  transform: scale(1);
}

.setting-actions {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 10px;
  padding-top: 2px;
  border-top: 1px solid var(--border-subtle);
  padding-top: 14px;
}

.saved-pill {
  font-size: 12px;
  color: #15803d;
  background: color-mix(in srgb, #22c55e 14%, var(--bg-primary));
  border: 1px solid color-mix(in srgb, #22c55e 22%, transparent);
  border-radius: var(--radius-pill);
  padding: 4px 9px;
}

.save-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  min-width: 96px;
  height: 34px;
  padding: 0 16px;
  font-size: 13px;
  font-weight: 600;
  background: var(--accent);
  color: #fff;
  border-radius: var(--radius-pill);
  transition: opacity 0.12s;
  white-space: nowrap;
}
.save-btn:hover:not(:disabled) { opacity: 0.85; }
.save-btn:disabled { opacity: 0.4; cursor: not-allowed; }

.spin {
  animation: spin 0.7s linear infinite;
}

@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

@media (max-width: 760px) {
  .option-grid {
    grid-template-columns: 1fr;
  }
}
</style>
