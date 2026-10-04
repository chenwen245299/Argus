import type { AiModel } from '../types'
import { beijingParts, holidayNameOn, isCnMakeupWorkday } from './cnHolidays'

// One place for "what did this call cost". The paper chat, the library chat and
// the usage dashboard each used to carry their own copy of this arithmetic, and
// they had already drifted: only the paper chat split cache hits from misses, so
// the dashboard billed every input token at the full price.

export const DEFAULT_USD_TO_CNY_RATE = 7.2

/**
 * DeepSeek's peak window, the ONE implementation of it (the toolbar chip and
 * every cost estimate go through here). The official wording, from the pricing
 * page https://api-docs.deepseek.com/zh-cn/quick_start/pricing:
 *
 *   北京时间周一至周五（不含中国法定节假日）9:00 - 12:00、14:00 - 18:00 为高峰时段；
 *   其余时段，包括周末及中国法定节假日全天均为空闲时段。
 *
 * So peak = a Beijing-time Monday–Friday that is not an official day off, and
 * then 09:00 <= t < 12:00 or 14:00 <= t < 18:00. A weekend is off-peak even when
 * it is an official make-up WORKING day (调休上班): that is the literal reading
 * (it is not Monday–Friday), and DeepSeek's 2026-09-19 "API 峰谷时间说明", as the
 * press quotes it (IT之家, 凤凰网), says the same: 调休上班的周末、中国法定节假日
 * 全天均按空闲时段计费. A weekday that is an official day off — including the
 * bridge days of a long holiday — is a holiday. Everything is read off the
 * BEIJING wall clock and calendar date, so it does not depend on the user's own
 * timezone. Holiday data: ./cnHolidays.
 */
export type PeakReason = 'peak-hours' | 'weekday-offpeak-hours' | 'weekend' | 'holiday'

export interface PeakPeriod {
  peak: boolean
  reason: PeakReason
  /** Set when `reason` is `holiday`, e.g. `国庆节`. */
  holidayName?: string
  /** Set on a weekend that is an official make-up working day (调休上班); still off-peak. */
  makeupWorkday?: boolean
}

/** Peak or off-peak at `date`, and why. An invalid Date is reported as plain off-peak. */
export function describePeakPeriod(date: Date): PeakPeriod {
  const b = beijingParts(date)
  if (!b) return { peak: false, reason: 'weekday-offpeak-hours' }
  // A holiday wins over a weekend, so a Saturday that is also National Day says
  // "国庆节" rather than just "周末".
  const holidayName = holidayNameOn(b.year, b.month, b.day)
  if (holidayName !== null) return { peak: false, reason: 'holiday', holidayName }
  if (b.weekday === 0 || b.weekday === 6) {
    return isCnMakeupWorkday(b.year, b.month, b.day)
      ? { peak: false, reason: 'weekend', makeupWorkday: true }
      : { peak: false, reason: 'weekend' }
  }
  const m = b.minutes
  return (m >= 9 * 60 && m < 12 * 60) || (m >= 14 * 60 && m < 18 * 60)
    ? { peak: true, reason: 'peak-hours' }
    : { peak: false, reason: 'weekday-offpeak-hours' }
}

export function isPeakHour(date: Date): boolean {
  return describePeakPeriod(date).peak
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
