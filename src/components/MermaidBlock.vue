<template>
  <div class="mermaid-block">
    <div v-if="status === 'pending'" class="mermaid-loading">{{ t('mermaid.rendering') }}</div>
    <div v-else-if="status === 'done'" class="mermaid-svg-wrap">
      <div class="mermaid-inner" role="button" tabindex="0" :aria-label="t('mermaid.enlarge')"
        :title="t('mermaid.enlarge')" @click="openPreview" @keydown.enter.prevent="openPreview"
        @keydown.space.prevent="openPreview" v-html="svgHtml" />
      <div class="mermaid-actions">
        <button type="button" class="mermaid-enlarge-btn" :title="t('mermaid.enlarge')"
          :aria-label="t('mermaid.enlarge')" @click="openPreview">
          <Icon icon="fluent:arrow-expand-24-regular" width="16" height="16" />
        </button>
        <button type="button" class="mermaid-copy-btn" :title="t('mermaid.copyImage')"
          :aria-label="t('mermaid.copyImage')" @click="copyAsImage">
          <Icon icon="fluent:copy-24-regular" width="16" height="16" />
        </button>
      </div>
    </div>
    <pre v-else class="mermaid-error">{{ src }}</pre>
    <Teleport to="body">
      <div v-if="previewSrc" class="mermaid-preview-overlay" @click.self="closePreview" @keydown="onPreviewKeydown">
        <section ref="previewDialog" class="mermaid-preview" role="dialog" aria-modal="true" :aria-label="t('mermaid.preview')">
          <div class="mermaid-preview-toolbar">
            <span class="mermaid-preview-title">{{ t('mermaid.preview') }}</span>
            <button type="button" :title="t('pdf.zoomOut')" :aria-label="t('pdf.zoomOut')" :disabled="previewScale <= minScale" @click="zoomPreview(1 / 1.25)">
              <Icon icon="fluent:subtract-24-regular" width="18" height="18" />
            </button>
            <span class="mermaid-preview-scale">{{ Math.round(previewScale * 100) }}%</span>
            <button type="button" :title="t('pdf.zoomIn')" :aria-label="t('pdf.zoomIn')" :disabled="previewScale >= 8" @click="zoomPreview(1.25)">
              <Icon icon="fluent:add-24-regular" width="18" height="18" />
            </button>
            <button type="button" :title="t('mermaid.fit')" :aria-label="t('mermaid.fit')" @click="fitPreview">
              <Icon icon="fluent:scale-fit-24-regular" width="18" height="18" />
            </button>
            <button ref="previewClose" type="button" class="mermaid-preview-close" :title="t('mermaid.close')"
              :aria-label="t('mermaid.close')" @click="closePreview">
              <Icon icon="fluent:dismiss-24-regular" width="20" height="20" />
            </button>
          </div>
          <div ref="previewViewport" class="mermaid-preview-viewport" :class="{ dragging }"
            @wheel.prevent="onPreviewWheel" @pointerdown="startDrag" @pointermove="moveDrag"
            @pointerup="endDrag" @pointercancel="endDrag" @lostpointercapture="endDrag">
            <div class="mermaid-preview-stage">
              <img ref="previewImage" :src="previewSrc" :alt="t('mermaid.preview')" draggable="false"
                :style="{ width: `${previewSize.width * previewScale}px`, height: `${previewSize.height * previewScale}px` }" />
            </div>
          </div>
          <div class="mermaid-preview-hint">{{ t('mermaid.hint') }}</div>
        </section>
      </div>
      <div v-if="toastText" class="mermaid-toast">{{ toastText }}</div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, nextTick, watch, onMounted, onBeforeUnmount } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import DOMPurify from 'dompurify'
import { svgStringToPngBlob } from '../utils/svgToPng'
import { copyPngBlobToClipboard } from '../utils/clipboard'

const { t } = useI18n()

const props = defineProps<{ src: string; streaming?: boolean }>()

// Upper bound on diagram source length. Real mermaid diagrams are a few KB at
// most; a pathologically large source can hang the renderer, so past this we
// fall back to showing the raw text instead of rendering.
const MAX_MERMAID_SRC = 50_000

const status  = ref<'pending' | 'done' | 'error'>('pending')
const svgHtml = ref('')
let   rendered = false

const previewSrc = ref('')
const previewDialog = ref<HTMLElement | null>(null)
const previewClose = ref<HTMLButtonElement | null>(null)
const previewViewport = ref<HTMLElement | null>(null)
const previewImage = ref<HTMLImageElement | null>(null)
const previewSize = ref({ width: 1, height: 1 })
const previewScale = ref(1)
const fitScale = ref(1)
const minScale = computed(() => Math.min(0.1, fitScale.value))
const dragging = ref(false)
let previousFocus: HTMLElement | null = null
let drag: { id: number; x: number; y: number; left: number; top: number } | null = null

async function openPreview(event: Event) {
  if (!svgHtml.value || previewSrc.value) return
  // Isolate SVG IDs and styles from the inline diagram, retaining vector detail
  // at every zoom level. Only the already sanitized SVG enters this image.
  const doc = new DOMParser().parseFromString(svgHtml.value, 'image/svg+xml')
  const svg = doc.documentElement as unknown as SVGSVGElement
  const box = svg.viewBox.baseVal
  const width = box?.width || parseFloat(svg.getAttribute('width') || '') || 800
  const height = box?.height || parseFloat(svg.getAttribute('height') || '') || 600
  svg.setAttribute('xmlns', 'http://www.w3.org/2000/svg')
  svg.setAttribute('width', String(width))
  svg.setAttribute('height', String(height))
  svg.style.removeProperty('max-width')
  previewSize.value = { width, height }
  previousFocus = event.currentTarget as HTMLElement
  previewSrc.value = `data:image/svg+xml;charset=utf-8,${encodeURIComponent(new XMLSerializer().serializeToString(svg))}`
  await nextTick()
  fitPreview()
  previewClose.value?.focus()
}

function closePreview() {
  endDrag()
  previewSrc.value = ''
  previousFocus?.focus({ preventScroll: true })
  previousFocus = null
}

function fitPreview() {
  const viewport = previewViewport.value
  if (!viewport) return
  fitScale.value = Math.max(0.001, Math.min(1,
    (viewport.clientWidth - 48) / previewSize.value.width,
    (viewport.clientHeight - 48) / previewSize.value.height))
  previewScale.value = fitScale.value
  viewport.scrollTo(0, 0)
}

async function zoomPreview(factor: number, x?: number, y?: number) {
  const viewport = previewViewport.value
  const image = previewImage.value
  if (!viewport || !image) return
  const bounds = viewport.getBoundingClientRect()
  const anchorX = x ?? bounds.left + viewport.clientWidth / 2
  const anchorY = y ?? bounds.top + viewport.clientHeight / 2
  const before = image.getBoundingClientRect()
  const fractionX = (anchorX - before.left) / before.width
  const fractionY = (anchorY - before.top) / before.height
  previewScale.value = Math.max(minScale.value, Math.min(8, previewScale.value * factor))
  await nextTick()
  const after = image.getBoundingClientRect()
  viewport.scrollLeft += after.left + fractionX * after.width - anchorX
  viewport.scrollTop += after.top + fractionY * after.height - anchorY
}

function onPreviewWheel(event: WheelEvent) {
  if (event.deltaY) zoomPreview(event.deltaY < 0 ? 1.1 : 1 / 1.1, event.clientX, event.clientY)
}

function startDrag(event: PointerEvent) {
  const viewport = previewViewport.value
  if (!viewport || event.button !== 0 || !event.isPrimary) return
  drag = { id: event.pointerId, x: event.clientX, y: event.clientY, left: viewport.scrollLeft, top: viewport.scrollTop }
  dragging.value = true
  viewport.setPointerCapture(event.pointerId)
}

function moveDrag(event: PointerEvent) {
  const viewport = previewViewport.value
  if (!viewport || !drag || drag.id !== event.pointerId) return
  viewport.scrollLeft = drag.left + drag.x - event.clientX
  viewport.scrollTop = drag.top + drag.y - event.clientY
}

function endDrag() {
  const id = drag?.id
  drag = null
  dragging.value = false
  if (id !== undefined && previewViewport.value?.hasPointerCapture(id)) previewViewport.value.releasePointerCapture(id)
}

function onPreviewKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    closePreview()
  } else if (event.key === 'Tab') {
    const buttons = [...(previewDialog.value?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? [])]
    const first = buttons[0], last = buttons[buttons.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last?.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first?.focus()
    }
  }
}

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
    _mermaid.initialize({
      startOnLoad: false,
      theme: 'neutral',
      fontFamily: '"Helvetica Neue", Arial, sans-serif',
      // Our SVG-only sanitizer removes foreignObject HTML labels. Render text
      // as SVG instead so labels survive sanitizing and standalone PNG export.
      htmlLabels: false,
      // Keep Mermaid's default secure keys, and prevent per-diagram directives
      // from switching the labels back to HTML before they reach the sanitizer.
      secure: ['secure', 'securityLevel', 'startOnLoad', 'maxTextSize', 'suppressErrorRendering', 'maxEdges', 'htmlLabels'],
    })
  }
  return _mermaid
}

async function tryRender() {
  if (props.streaming || rendered) return
  if (props.src.length > MAX_MERMAID_SRC) {
    status.value = 'error'
    return
  }
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
onBeforeUnmount(() => {
  if (toastTimer) clearTimeout(toastTimer)
  if (previewSrc.value) closePreview()
})
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
.mermaid-inner { cursor: zoom-in; }
.mermaid-inner:focus-visible { outline: 2px solid var(--accent); outline-offset: 4px; }
.mermaid-actions {
  position: absolute;
  top: 8px;
  right: 8px;
  display: flex;
  gap: 4px;
  opacity: 0;
  transition: opacity 0.15s;
}
.mermaid-actions button {
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
  padding: 0;
  transition: background 0.12s;
}
.mermaid-svg-wrap:hover .mermaid-actions,
.mermaid-svg-wrap:focus-within .mermaid-actions { opacity: 1; }
.mermaid-actions button:hover { background: rgba(0, 0, 0, 0.72); }
.mermaid-actions button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
@media (hover: none) { .mermaid-actions { opacity: 1; } }
.mermaid-preview-overlay {
  position: fixed;
  inset: 0;
  z-index: 10000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
  background: rgba(0, 0, 0, 0.65);
}
.mermaid-preview {
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border: 1px solid var(--border-default);
  border-radius: var(--radius-lg, 12px);
  background: var(--bg-primary);
  color: var(--text-primary);
  box-shadow: var(--shadow-lg);
}
.mermaid-preview-toolbar {
  display: flex;
  flex-shrink: 0;
  align-items: center;
  gap: 6px;
  padding: 10px 12px;
  border-bottom: 1px solid var(--divider);
}
.mermaid-preview-title { flex: 1; font-size: 13px; font-weight: 600; }
.mermaid-preview-toolbar button {
  display: flex;
  align-items: center;
  justify-content: center;
  flex-shrink: 0;
  width: 30px;
  height: 30px;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm, 5px);
  background: transparent;
  color: var(--text-primary);
  cursor: pointer;
}
.mermaid-preview-toolbar button:hover { background: var(--bg-hover); }
.mermaid-preview-toolbar button:disabled { opacity: 0.35; cursor: default; }
.mermaid-preview-toolbar button:focus-visible { outline: 2px solid var(--accent); }
.mermaid-preview-scale { min-width: 42px; text-align: center; font-size: 12px; font-variant-numeric: tabular-nums; }
.mermaid-preview-viewport {
  flex: 1;
  min-height: 0;
  overflow: auto;
  overscroll-behavior: contain;
  touch-action: none;
  user-select: none;
  cursor: grab;
  /* Mermaid's neutral palette has dark text, including in dark app themes. */
  background: #fff;
  color-scheme: light;
}
.mermaid-preview-viewport.dragging { cursor: grabbing; }
.mermaid-preview-stage {
  display: flex;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  width: max-content;
  min-width: 100%;
  min-height: 100%;
  padding: 24px;
}
.mermaid-preview-stage img { display: block; flex-shrink: 0; max-width: none; }
.mermaid-preview-hint {
  padding: 8px 12px;
  border-top: 1px solid var(--divider);
  color: var(--text-secondary);
  font-size: 11px;
  text-align: center;
}
@media (max-width: 480px) {
  .mermaid-preview-overlay { padding: 8px; }
  .mermaid-preview-toolbar { gap: 2px; padding: 8px; }
}
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
