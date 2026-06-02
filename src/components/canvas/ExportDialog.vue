<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { save as saveDialog } from '@tauri-apps/plugin-dialog'
import { toPng, toSvg } from 'html-to-image'

const props = defineProps<{
  canvasName: string
  flowEl: HTMLElement | null  // the .vue-flow element to capture
}>()

const emit = defineEmits<{ close: [] }>()

const { t } = useI18n()

const format = ref<'png' | 'svg'>('png')
const bg = ref<'white' | 'transparent'>('white')
const scale = ref<1 | 2>(2)
const loading = ref(false)
const statusMsg = ref('')
const isError = ref(false)

async function doExport() {
  if (!props.flowEl) {
    statusMsg.value = t('canvas.exportFail') + ': canvas not ready'
    isError.value = true
    return
  }

  loading.value = true
  statusMsg.value = t('canvas.exporting')
  isError.value = false

  try {
    // Get save path from user
    const ext = format.value
    const savePath = await saveDialog({
      defaultPath: `${props.canvasName || 'canvas'}.${ext}`,
      filters: [{ name: ext.toUpperCase(), extensions: [ext] }],
    })

    if (!savePath) {
      loading.value = false
      statusMsg.value = ''
      return
    }

    // Render canvas to image
    const bgColor = bg.value === 'white' ? '#ffffff' : undefined
    const pixelRatio = scale.value

    let imageData: string
    if (format.value === 'png') {
      imageData = await toPng(props.flowEl, {
        backgroundColor: bgColor,
        pixelRatio,
      })
    } else {
      imageData = await toSvg(props.flowEl, {
        backgroundColor: bgColor,
      })
    }

    // Write file via Rust
    await invoke('export_canvas_image', {
      format: format.value,
      imageData,
      savePath,
    })

    statusMsg.value = t('canvas.exportDone')
    isError.value = false
    setTimeout(() => emit('close'), 1200)
  } catch (e) {
    statusMsg.value = `${t('canvas.exportFail')}: ${String(e)}`
    isError.value = true
  } finally {
    loading.value = false
  }
}
</script>

<template>
  <Teleport to="body">
    <div class="export-overlay" @mousedown.self="emit('close')">
      <div class="export-dialog">
        <div class="export-header">
          <span class="export-title">{{ t('canvas.exportImage') }}</span>
          <button class="close-btn" @click="emit('close')">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>

        <div class="export-body">
          <!-- Format -->
          <div class="field-group">
            <label class="field-label">格式</label>
            <div class="radio-row">
              <label class="radio-item">
                <input type="radio" v-model="format" value="png" />
                {{ t('canvas.exportPng') }}
              </label>
              <label class="radio-item">
                <input type="radio" v-model="format" value="svg" />
                {{ t('canvas.exportSvg') }}
              </label>
            </div>
          </div>

          <!-- Background -->
          <div class="field-group">
            <label class="field-label">背景</label>
            <div class="radio-row">
              <label class="radio-item">
                <input type="radio" v-model="bg" value="white" />
                {{ t('canvas.exportBgWhite') }}
              </label>
              <label class="radio-item">
                <input type="radio" v-model="bg" value="transparent" />
                {{ t('canvas.exportBgTransparent') }}
              </label>
            </div>
          </div>

          <!-- Scale (PNG only) -->
          <div v-if="format === 'png'" class="field-group">
            <label class="field-label">分辨率</label>
            <div class="radio-row">
              <label class="radio-item">
                <input type="radio" v-model="scale" :value="1" />
                {{ t('canvas.exportScale1x') }}
              </label>
              <label class="radio-item">
                <input type="radio" v-model="scale" :value="2" />
                {{ t('canvas.exportScale2x') }}
              </label>
            </div>
          </div>

          <!-- Status -->
          <p v-if="statusMsg" class="status-msg" :class="{ 'status-error': isError }">{{ statusMsg }}</p>

          <div class="export-actions">
            <button class="cancel-btn" @click="emit('close')" :disabled="loading">取消</button>
            <button class="export-btn" :disabled="loading" @click="doExport">
              <span v-if="loading" class="spinner" />
              {{ loading ? t('canvas.exporting') : t('canvas.exportImage') }}
            </button>
          </div>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.export-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0,0,0,0.3);
  backdrop-filter: blur(4px);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 2000;
}
.export-dialog {
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
  width: 320px;
  overflow: hidden;
}
.export-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid var(--border-subtle);
}
.export-title { font-size: 14px; font-weight: 600; color: var(--text-primary); }
.close-btn { color: var(--text-tertiary); border-radius: var(--radius-sm); padding: 3px; display: flex; }
.close-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.export-body { padding: 16px; display: flex; flex-direction: column; gap: 14px; }
.field-group { display: flex; flex-direction: column; gap: 6px; }
.field-label { font-size: 11px; font-weight: 600; color: var(--text-tertiary); text-transform: uppercase; letter-spacing: 0.04em; }
.radio-row { display: flex; gap: 14px; }
.radio-item { display: flex; align-items: center; gap: 6px; font-size: 13px; color: var(--text-primary); cursor: pointer; }
.radio-item input { accent-color: var(--accent); }
.status-msg { font-size: 12px; color: var(--text-secondary); }
.status-error { color: #e53e3e; }
.export-actions { display: flex; gap: 8px; justify-content: flex-end; }
.cancel-btn {
  padding: 6px 14px; font-size: 13px; border: 1px solid var(--border-default);
  border-radius: var(--radius-md); color: var(--text-secondary); background: var(--bg-secondary);
}
.cancel-btn:hover:not(:disabled) { background: var(--bg-hover); }
.export-btn {
  display: flex; align-items: center; gap: 6px;
  padding: 6px 14px; font-size: 13px; font-weight: 600;
  background: var(--accent); color: #fff; border-radius: var(--radius-md);
  transition: opacity 0.12s;
}
.export-btn:disabled { opacity: 0.4; cursor: not-allowed; }
.spinner {
  display: inline-block; width: 10px; height: 10px;
  border: 1.5px solid rgba(255,255,255,0.4); border-top-color: #fff;
  border-radius: 50%; animation: spin 0.7s linear infinite;
}
@keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
</style>
