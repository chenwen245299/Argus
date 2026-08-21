<script setup lang="ts">
/**
 * The account balance for one provider, shown beside its name in a model picker
 * and in its settings panel, with a button to go and re-read it.
 *
 * Renders nothing at all for the providers that publish no balance (everyone
 * but DeepSeek and OpenRouter), so it can be dropped into every picker's group
 * header unconditionally.
 *
 * The store owns the fetching and the caching; this is display plus one action.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { Icon } from '@iconify/vue'
import { providerSupportsBalance, useAiStore } from '../stores/ai'

const props = defineProps<{ providerId: string }>()

const { t } = useI18n()
const ai = useAiStore()

/**
 * Only DeepSeek and OpenRouter have anything to show. Without this the
 * "checking…" state would flash on every provider's header, including the ones
 * that will never report a number.
 */
const supported = computed(() =>
  providerSupportsBalance(ai.settings.providers.find(p => p.id === props.providerId))
)

const balance = computed(() => ai.balances[props.providerId] ?? null)
const error = computed(() => ai.balanceErrors[props.providerId] ?? '')

/**
 * Nothing to show and nothing to retry — a supported provider with no key on
 * file, most often. A lone refresh button there would promise a fix it cannot
 * deliver, so the whole slot stays empty until there is a figure or a failure.
 */
const visible = computed(
  () => supported.value && (!!balance.value || !!error.value || ai.balancesLoading)
)

const CURRENCY_SYMBOL: Record<string, string> = { CNY: '¥', USD: '$', EUR: '€' }

/**
 * Two decimals up to a thousand, then thinned out — a picker header has room
 * for "¥110.00" but not for "¥12,345.67".
 */
function amount(value: number, currency: string) {
  const symbol = CURRENCY_SYMBOL[currency] ?? `${currency} `
  const magnitude = Math.abs(value)
  if (magnitude >= 10000) return `${symbol}${(value / 1000).toFixed(1)}k`
  if (magnitude >= 1000) return `${symbol}${value.toFixed(0)}`
  return `${symbol}${value.toFixed(2)}`
}

const label = computed(() => {
  const b = balance.value
  if (!b) return ''
  return amount(b.remaining, b.currency)
})

/** The full breakdown, as a tooltip — whatever the provider chose to report. */
const title = computed(() => {
  const b = balance.value
  if (!b) return error.value
  const lines = [t('balance.remaining', { amount: amount(b.remaining, b.currency) })]
  if (b.granted !== undefined && b.toppedUp !== undefined) {
    lines.push(t('balance.granted', { amount: amount(b.granted, b.currency) }))
    lines.push(t('balance.toppedUp', { amount: amount(b.toppedUp, b.currency) }))
  }
  if (b.totalUsage !== undefined) {
    lines.push(t('balance.used', { amount: amount(b.totalUsage, b.currency) }))
  }
  for (const other of b.otherCurrencies ?? []) {
    lines.push(amount(other.remaining, other.currency))
  }
  if (!b.isAvailable) lines.push(t('balance.exhausted'))
  return lines.join('\n')
})

/** Warn before the account actually stops working. */
const low = computed(() => {
  const b = balance.value
  if (!b) return false
  if (!b.isAvailable) return true
  return b.currency === 'CNY' ? b.remaining < 5 : b.remaining < 1
})

/**
 * Forced, so it ignores the freshness window — the whole point of pressing it is
 * that you believe the cached figure is out of date (you just topped up). One
 * call refreshes every provider, since that is what the backend command does;
 * `.stop` keeps the click off whatever the tag happens to be sitting inside.
 */
function refresh() {
  void ai.loadBalances(true)
}
</script>

<template>
  <span v-if="visible" class="balance-slot">
    <button
      class="balance-refresh"
      :class="{ spinning: ai.balancesLoading }"
      :disabled="ai.balancesLoading"
      :title="t('balance.refresh')"
      @click.stop="refresh"
    >
      <Icon icon="fluent:arrow-clockwise-24-regular" width="12" height="12" />
    </button>
    <span v-if="balance" class="balance-tag" :class="{ low }" :title="title">{{ label }}</span>
    <span v-else-if="error" class="balance-tag failed" :title="error">
      {{ t('balance.unavailable') }}
    </span>
    <span v-else class="balance-tag loading" :title="t('balance.loading')">···</span>
  </span>
</template>

<style scoped>
.balance-slot {
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  gap: 3px;
}

.balance-tag {
  padding: 0 5px;
  border-radius: var(--radius-sm, 4px);
  background: color-mix(in srgb, var(--text-tertiary) 12%, transparent);
  color: var(--text-secondary);
  font-size: 9.5px;
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  line-height: 15px;
  letter-spacing: 0;
  text-transform: none;
  white-space: nowrap;
}
.balance-tag.low {
  background: color-mix(in srgb, #f59e0b 18%, transparent);
  color: #b45309;
}
/* The amber that reads on a light ground is too dark on a dark one. The theme
   list is the same one the syntax highlighter uses to pick its dark palette. */
:is(
  [data-theme='dark'], [data-theme='midnight'], [data-theme='aurora'],
  [data-theme='twilight'], [data-theme='ocean'], [data-theme='mocha'],
  [data-theme='pine']
) .balance-tag.low {
  background: color-mix(in srgb, #f59e0b 22%, transparent);
  color: #fbbf24;
}
.balance-tag.failed,
.balance-tag.loading {
  color: var(--text-tertiary);
  background: transparent;
}

.balance-refresh {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm, 4px);
  background: transparent;
  color: var(--text-tertiary);
  cursor: pointer;
}
.balance-refresh:hover:not(:disabled) {
  background: var(--bg-hover);
  color: var(--text-secondary);
}
.balance-refresh:disabled {
  cursor: default;
}
.balance-refresh.spinning {
  color: var(--text-secondary);
}
.balance-refresh.spinning :deep(svg) {
  animation: balance-spin 0.9s linear infinite;
}
@keyframes balance-spin {
  to { transform: rotate(360deg); }
}
@media (prefers-reduced-motion: reduce) {
  .balance-refresh.spinning :deep(svg) { animation: none; }
}
</style>
