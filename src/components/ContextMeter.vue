<script setup lang="ts">
import { computed, ref, nextTick, useId, watch, onMounted, onUnmounted } from 'vue'
import { useI18n } from 'vue-i18n'
import { contextPercent, type ContextUsage } from '../utils/chatContext'
const props = defineProps<{ usage: ContextUsage | null; busy: boolean; compacting: boolean; error?: string; notice?: string; label?: string }>()
const emit = defineEmits<{ compact: []; inspect: [] }>()
const { t } = useI18n()
const root = ref<HTMLElement | null>(null), panel = ref<HTMLElement | null>(null)
const hovered = ref(false)
const detailId = useId()
const position = ref({ left: '0px', top: '0px', visibility: 'hidden' as 'hidden' | 'visible' })
const open = computed(() => !!props.usage && hovered.value)
const canCompact = computed(() => !!props.usage?.canCompact && !props.busy)
const percent = computed(() => props.usage ? contextPercent(props.usage) : 0)
const circumference = 2 * Math.PI * 5.5
const rows = computed(() => props.usage ? [
  { key: 'system', tokens: props.usage.systemTokens },
  { key: 'tools', tokens: props.usage.toolsTokens },
  { key: 'messages', tokens: props.usage.messageTokens },
] : [])
const total = computed(() => rows.value.reduce((n, row) => n + row.tokens, 0))
const fmt = (n: number) => Math.round(n).toLocaleString()
let closeTimer: ReturnType<typeof setTimeout> | null = null
async function enter() {
  if (closeTimer) clearTimeout(closeTimer)
  hovered.value = true
  emit('inspect')
  await nextTick(); place()
}
function leave() { closeTimer = setTimeout(() => { hovered.value = false }, 180) }
function place() {
  if (!root.value || !panel.value) return
  const r = root.value.getBoundingClientRect(), p = panel.value.getBoundingClientRect()
  position.value = { left: `${Math.max(12, Math.min(r.right - p.width, innerWidth - p.width - 12))}px`,
    top: `${Math.max(12, r.top - p.height - 8)}px`, visibility: 'visible' }
}
function compact() { if (canCompact.value) emit('compact') }
function outside(e: MouseEvent) {
  if (!root.value?.contains(e.target as Node) && !panel.value?.contains(e.target as Node)) {
    hovered.value = false
  }
}
function key(e: KeyboardEvent) { if (e.key === 'Escape') hovered.value = false }
watch(() => `${props.usage?.providerId ?? ''}:${props.usage?.modelId ?? ''}`, () => { hovered.value = false })
watch(() => [open.value, props.error, props.notice, props.busy], async () => { if (open.value) { await nextTick(); place() } })
onMounted(() => { document.addEventListener('mousedown', outside); document.addEventListener('keydown', key); window.addEventListener('resize', place) })
onUnmounted(() => {
  if (closeTimer) clearTimeout(closeTimer)
  document.removeEventListener('mousedown', outside); document.removeEventListener('keydown', key); window.removeEventListener('resize', place)
})
</script>

<template>
  <span v-if="usage" ref="root" class="context-meter" @mouseenter="enter" @mouseleave="leave" @focusin="enter" @focusout="leave">
    <button type="button" class="context-trigger" :class="{ warning: percent >= 80, compacting }"
      :aria-label="`${label ? label + '，' : ''}${t('chatContext.usedPercent', { percent })}，${t(compacting ? 'chatContext.compacting' : 'chatContext.compact')}`"
      :aria-disabled="!canCompact" :aria-describedby="open ? detailId : undefined" @click="compact">
      <svg width="14" height="14" viewBox="0 0 14 14" aria-hidden="true">
        <circle class="context-track" cx="7" cy="7" r="5.5" />
        <circle class="context-fill" cx="7" cy="7" r="5.5" transform="rotate(-90 7 7)"
          :stroke-dasharray="`${circumference * percent / 100} ${circumference}`" />
      </svg>
      <span v-if="label" class="context-model">{{ label }}</span>
      <span>{{ percent }}%</span>
    </button>
    <Teleport to="body">
      <div v-if="open" :id="detailId" ref="panel" class="context-panel" :style="position" role="tooltip"
        :aria-label="t('chatContext.title')" @mouseenter="enter" @mouseleave="leave" @focusin="enter" @focusout="leave">
        <div class="context-heading"><strong>{{ label || t('chatContext.title') }}</strong><span>{{ t('chatContext.usedPercent', { percent }) }}</span></div>
        <div class="context-total" :title="[usage.capacityEstimated ? t('chatContext.assumedCapacity') : '', usage.mediaEstimated ? t('chatContext.mediaEstimate') : ''].filter(Boolean).join('\n')">{{ usage.estimated ? '~' : '' }}{{ fmt(usage.usedTokens) }} / {{ usage.capacityEstimated ? '~' : '' }}{{ fmt(usage.contextWindow) }} tokens</div>
        <div class="context-bar"><span v-for="row in rows" :key="row.key" :class="row.key"
          :style="{ width: `${total ? percent * row.tokens / total : 0}%` }" /></div>
        <dl class="context-rows">
          <div><dt>{{ t('chatContext.remaining') }}</dt><dd>~{{ fmt(usage.remainingTokens) }}</dd></div>
          <div v-for="row in rows" :key="row.key"><dt><i :class="row.key" />{{ t(`chatContext.${row.key}`) }}</dt><dd>~{{ fmt(row.tokens) }}</dd></div>
        </dl>
        <p v-if="error" class="context-error" role="alert">{{ error }}</p>
        <p v-else-if="notice" class="context-status" role="status">{{ notice }}</p>
      </div>
    </Teleport>
  </span>
</template>

<style scoped>
.context-meter { display: inline-flex; flex-shrink: 0; }
.context-model { max-width: 90px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.context-trigger { display: inline-flex; align-items: center; gap: 5px; padding: 4px 6px; border: 0; border-radius: 6px; background: transparent; color: var(--text-tertiary); font: inherit; font-size: 11px; font-variant-numeric: tabular-nums; cursor: pointer; }
.context-trigger:hover { color: var(--text-secondary); background: var(--bg-hover); }
.context-trigger:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.context-trigger[aria-disabled="true"] { cursor: default; }
.context-trigger.warning { color: var(--warning, var(--accent)); }
.context-track, .context-fill { fill: none; stroke-width: 2; }
.context-track { stroke: var(--border-subtle); }
.context-fill { stroke: currentColor; stroke-linecap: round; }
.context-panel { position: fixed; z-index: 1100; width: min(290px, calc(100vw - 24px)); box-sizing: border-box; padding: 14px; border: 1px solid var(--border-subtle); border-radius: 12px; background: var(--bg-primary); box-shadow: var(--shadow-lg); color: var(--text-secondary); font-size: 12px; line-height: 1.5; }
.context-heading, .context-rows > div { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.context-heading strong { color: var(--text-primary); font-weight: 600; }
.context-total { margin-top: 5px; color: var(--text-primary); font-variant-numeric: tabular-nums; }
.context-bar { display: flex; height: 4px; margin: 10px 0; overflow: hidden; border-radius: 4px; background: var(--bg-hover); }
.context-bar span { flex-shrink: 0; }
.system { background: var(--context-system); }
.tools { background: var(--context-tools); }
.messages { background: var(--context-messages); }
.context-rows { margin: 0; }
.context-rows > div { padding: 3px 0; }
.context-rows dd { margin: 0; color: var(--text-primary); font-variant-numeric: tabular-nums; }
.context-rows i { display: inline-block; width: 7px; height: 7px; margin-right: 6px; border-radius: 2px; }
.context-status { margin: 10px 0 0; color: var(--text-tertiary); font-size: 11px; }
.context-error { margin: 10px 0 0; color: var(--danger, var(--accent)); overflow-wrap: anywhere; }
</style>
