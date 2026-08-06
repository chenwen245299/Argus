import type { AiModel } from '../types'

// One place for "what did this call cost". The paper chat, the library chat and
// the usage dashboard each used to carry their own copy of this arithmetic, and
// they had already drifted: only the paper chat split cache hits from misses, so
// the dashboard billed every input token at the full price.

export const DEFAULT_USD_TO_CNY_RATE = 7.2

/**
 * DeepSeek-style peak hours in Beijing time (UTC+8): 09:00–12:00 & 14:00–18:00.
 * Computed from UTC so it is correct regardless of the user's own timezone.
 */
export function isPeakHour(date: Date): boolean {
  const minutes = ((date.getUTCHours() + 8) % 24) * 60 + date.getUTCMinutes()
  const h = minutes / 60
  return (h >= 9 && h < 12) || (h >= 14 && h < 18)
}

export interface UsageForPricing {
  inputTokens: number
  outputTokens: number
  /**
   * Input tokens served from the provider's context cache. DeepSeek reports it
   * exactly — `prompt_cache_hit_tokens` on /chat/completions,
   * `usage.input_tokens_details.cached_tokens` on the Responses API — so it is a
   * measured number, never an estimate. Absent/0 means "no cache hit".
   */
  cacheHitTokens?: number
  /** When the call happened; decides peak vs off-peak. Defaults to now. */
  at?: Date
}

/** True when the model has enough configured prices to derive a cost at all. */
export function hasConfiguredPrice(model: AiModel | undefined | null): boolean {
  if (!model) return false
  return (
    model.input_price_usd_per_million != null ||
    model.output_price_usd_per_million != null ||
    model.input_price_per_million != null ||
    model.output_price_per_million != null
  )
}

/**
 * Cost of one call in CNY, or null when the model has no prices configured.
 *
 * Input is billed in two halves. Providers with context caching charge far less
 * for a cache hit — DeepSeek's hit price is roughly a tenth of its miss price —
 * so a workload that deliberately reuses a long prefix (which is exactly what
 * the full-text paper tasks here do) is dramatically cheaper than input-token
 * count alone suggests. Charging every input token at the miss price overstates
 * such a bill several-fold.
 */
export function estimateCostCny(
  model: AiModel | undefined | null,
  usage: UsageForPricing,
  usdToCnyRate: number = DEFAULT_USD_TO_CNY_RATE,
): number | null {
  if (!hasConfiguredPrice(model) || !model) return null
  const rate = Number.isFinite(usdToCnyRate) && usdToCnyRate > 0 ? usdToCnyRate : DEFAULT_USD_TO_CNY_RATE
  const input = Math.max(0, usage.inputTokens || 0)
  const output = Math.max(0, usage.outputTokens || 0)
  // A provider can only cache what it was sent, so a bogus count can't inflate
  // the discount beyond the input itself.
  const cacheHit = Math.min(Math.max(0, usage.cacheHitTokens ?? 0), input)
  const cacheMiss = input - cacheHit

  // USD prices win when set, matching how the dashboard has always priced them.
  // There is no USD cache-hit price field, so those models bill all input at the
  // one rate — as before.
  if (model.input_price_usd_per_million != null || model.output_price_usd_per_million != null) {
    let cost = 0
    if (model.input_price_usd_per_million != null) {
      cost += (input / 1e6) * model.input_price_usd_per_million * rate
    }
    if (model.output_price_usd_per_million != null) {
      cost += (output / 1e6) * model.output_price_usd_per_million * rate
    }
    return Number.isFinite(cost) ? cost : null
  }

  const peak = !!model.peak_pricing && isPeakHour(usage.at ?? new Date())
  const inPrice =
    (peak && model.peak_input_price_per_million != null
      ? model.peak_input_price_per_million
      : model.input_price_per_million) ?? 0
  const outPrice =
    (peak && model.peak_output_price_per_million != null
      ? model.peak_output_price_per_million
      : model.output_price_per_million) ?? 0
  // Without a configured cache-hit price, a hit costs the same as a miss — the
  // safe assumption, since guessing a discount would understate the bill.
  const cacheHitPrice =
    model.cache_hit_input_price_per_million != null
      ? model.cache_hit_input_price_per_million
      : inPrice

  const cost =
    (cacheMiss / 1e6) * inPrice + (cacheHit / 1e6) * cacheHitPrice + (output / 1e6) * outPrice
  return Number.isFinite(cost) ? cost : null
}
