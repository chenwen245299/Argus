<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { formatChatCost, summarizeChatCost, type ChatRun } from '../utils/chatRunStats'
import { useAiStore } from '../stores/ai'
import { useSettingsStore } from '../stores/settings'
import { DEFAULT_USD_TO_CNY_RATE, estimateCostCny } from '../utils/modelPricing'

const props = defineProps<{ runs: ChatRun[] }>()
const { t } = useI18n()
const ai = useAiStore()
const settings = useSettingsStore()
const cost = computed(() => {
  const configured = Number(settings.settings.usd_to_cny_rate)
  const rate = Number.isFinite(configured) && configured > 0 ? configured : DEFAULT_USD_TO_CNY_RATE
  return summarizeChatCost(props.runs, rate, run => {
    if (typeof run.inputTokens !== 'number' || typeof run.outputTokens !== 'number') return null
    const model = ai.settings.providers.find(p => p.id === run.providerId)?.models.find(m => m.id === run.modelId)
    return estimateCostCny(model, { inputTokens: run.inputTokens, outputTokens: run.outputTokens,
      cacheHitTokens: run.cacheHitTokens, at: run.createdAt ? new Date(run.createdAt) : undefined }, rate)
  })
})
const hint = computed(() => cost.value.missing
  ? t('chatStats.costMissing', { n: cost.value.missing, known: cost.value.knownCny.toFixed(6) })
  : t(cost.value.estimated ? 'chatStats.costEstimateHint' : 'chatStats.costHint', { amount: cost.value.knownCny.toFixed(6) }))
</script>

<template>
  <span class="run-cost" :title="hint" :aria-label="t('chatStats.cost')">
    {{ cost.cny === null ? '¥—' : `${cost.estimated ? '≈' : ''}¥${formatChatCost(cost.cny)}` }}
  </span>
</template>

<style scoped>
.run-cost { display: inline-flex; align-items: center; flex-shrink: 0; margin-right: 4px; color: var(--text-tertiary); font-size: 11px; line-height: 20px; font-variant-numeric: tabular-nums; white-space: nowrap; }
</style>
