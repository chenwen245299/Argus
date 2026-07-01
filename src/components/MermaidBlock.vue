<template>
  <div class="mermaid-block">
    <div v-if="status === 'pending'" class="mermaid-loading">{{ t('mermaid.rendering') }}</div>
    <div v-else-if="status === 'done'" class="mermaid-svg-wrap">
      <div class="mermaid-inner" v-html="svgHtml" />
      <button class="mermaid-copy-btn" :title="t('mermaid.copyImage')" @click="copyAsImage">
        <svg xmlns="http://www.w3.org/2000/svg" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect width="14" height="14" x="8" y="8" rx="2" ry="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/></svg>
      </button>
    </div>
    <pre v-else class="mermaid-error">{{ src }}</pre>
    <Teleport to="body">
      <div v-if="toastText" class="mermaid-toast">{{ toastText }}</div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { ref, watch, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'
import DOMPurify from 'dompurify'
import { svgStringToPngBlob } from '../utils/svgToPng'
import { copyPngBlobToClipboard } from '../utils/clipboard'

const { t } = useI18n()

const props = defineProps<{ src: string; streaming?: boolean }>()

const status  = ref<'pending' | 'done' | 'error'>('pending')
const svgHtml = ref('')
let   rendered = false

const toastText = ref('')
let toastTimer: ReturnType<typeof setTimeout> | null = null

function showToast(text: string) {
  toastText.value = text
  if (toastTimer) clearTimeout(toastTimer)
  toastTimer = setTimeout(() => { toastText.value = '' }, 2500)
}

async function copyAsImage() {
  if (!svgHtml.value) return
  try {
    const pngBlob = await svgStringToPngBlob(svgHtml.value)
    await copyPngBlobToClipboard(pngBlob)
    showToast(t('mermaid.copied'))
  } catch {
    showToast(t('mermaid.copyFailed'))
  }
}

type MermaidMod = typeof import('mermaid').default
let _mermaid: MermaidMod | null = null
async function loadMermaid(): Promise<MermaidMod> {
  if (!_mermaid) {
    const mod = await import('mermaid')
    _mermaid = mod.default
    _mermaid.initialize({ startOnLoad: false, theme: 'neutral', fontFamily: '"Helvetica Neue", Arial, sans-serif' })
  }
  return _mermaid
}

async function tryRender() {
  if (props.streaming || rendered) return
  try {
    const m = await loadMermaid()
    const stage = document.createElement('div')
    stage.style.cssText = 'position:fixed;top:0;left:-9999px;width:900px;height:600px;overflow:hidden;visibility:hidden;pointer-events:none'
    document.body.appendChild(stage)
    const id = 'mermaid-blk-' + Math.random().toString(36).slice(2, 10)
    try {
      const { svg } = await Promise.race([
        m.render(id, props.src, stage),
        new Promise<never>((_, reject) => setTimeout(() => reject(new Error('渲染超时')), 10_000)),
      ])
      rendered  = true
      svgHtml.value = DOMPurify.sanitize(svg, { USE_PROFILES: { svg: true, svgFilters: true } })
      status.value  = 'done'
    } finally {
      stage.remove()
      document.getElementById(id)?.remove()
    }
  } catch {
    status.value = 'error'
  }
}

onMounted(tryRender)
watch(() => props.streaming, s => { if (!s) tryRender() })
</script>

<style scoped>
.mermaid-block {
  margin: 10px 0;
  border-radius: 10px;
  background: var(--bg-secondary);
  padding: 14px;
  overflow-x: auto;
}
.mermaid-loading {
  color: var(--text-tertiary);
  font-size: 12px;
  padding: 6px 0;
  font-style: italic;
}
.mermaid-svg-wrap {
  position: relative;
  display: inline-block;
  width: 100%;
}
.mermaid-inner :deep(svg) {
  max-width: 100%;
  height: auto;
  display: block;
}
.mermaid-copy-btn {
  position: absolute;
  bottom: 8px;
  right: 8px;
  opacity: 0;
  width: 28px;
  height: 28px;
  border-radius: 7px;
  border: none;
  background: rgba(0, 0, 0, 0.50);
  color: white;
  cursor: pointer;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: opacity 0.15s, background 0.12s;
}
.mermaid-svg-wrap:hover .mermaid-copy-btn { opacity: 1; }
.mermaid-copy-btn:hover { background: rgba(0, 0, 0, 0.72); }
.mermaid-error {
  white-space: pre-wrap;
  word-break: break-all;
  font-size: 12px;
  color: #c0392b;
  background: rgba(192, 57, 43, 0.06);
  border-radius: 6px;
  padding: 10px;
  margin: 0;
}
.mermaid-toast {
  position: fixed;
  bottom: 32px;
  left: 50%;
  transform: translateX(-50%);
  background: rgba(30, 30, 30, 0.88);
  color: #fff;
  padding: 8px 20px;
  border-radius: 20px;
  font-size: 13px;
  pointer-events: none;
  z-index: 99999;
}
</style>
