/**
 * Free / discounted badges for the model pickers.
 *
 * The data comes from the provider's own catalogue (OpenRouter's `/models`
 * gives both), parsed in `llm.rs`. Nothing here infers an offer from a name or
 * a price threshold — a badge that says "free" has to be the provider saying it.
 */
import type { AiModel } from '../types'

export interface ModelOffer {
  kind: 'free' | 'discount'
  /** Short badge text, e.g. `FREE` or `5折`. */
  label: string
  /** Whether a scheduled discount is in effect right now. */
  activeNow: boolean
  /** Full sentence for the tooltip. */
  title: string
}

/** `[HHMM, HHMM]`, where an end below the start wraps past midnight. */
type Window = [number, number]

function hhmmNowUtc(now = new Date()): number {
  return now.getUTCHours() * 100 + now.getUTCMinutes()
}

function inWindow(hhmm: number, [start, end]: Window): boolean {
  // 22:00 → 02:00 is a window that crosses midnight, so the two halves are
  // tested separately rather than as one range.
  return start <= end ? hhmm >= start && hhmm < end : hhmm >= start || hhmm < end
}

/** Format a UTC HHMM as the viewer's local `HH:MM`. */
function localTime(hhmm: number, now = new Date()): string {
  const d = new Date(Date.UTC(
    now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate(),
    Math.floor(hhmm / 100), hhmm % 100,
  ))
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/**
 * The badge for a model, or null when its price list advertises nothing.
 *
 * `now` is injectable so the schedule can be tested without waiting for a
 * particular hour.
 */
export function modelOffer(
  model: Pick<AiModel, 'is_free' | 'discount_percent' | 'discount_windows'>,
  now = new Date(),
): ModelOffer | null {
  if (model.is_free) {
    return {
      kind: 'free',
      label: 'FREE',
      activeNow: true,
      title: '供应商对这个模型的输入和输出都不计费',
    }
  }

  const percent = model.discount_percent
  if (!percent) return null
  const windows = (model.discount_windows ?? []) as Window[]

  // No windows means a standing promotion — OpenRouter's `pricing.discount`,
  // which applies whenever you call the model. Windows mean a time-of-day
  // schedule, which only applies inside them.
  const scheduled = windows.length > 0
  const activeNow = !scheduled || windows.some(w => inWindow(hhmmNowUtc(now), w))
  // "5折" reads better than "-50%" in Chinese, and the tenths place matters:
  // a 35% discount is 6.5折, not 7折.
  const tenths = Math.round((100 - percent) / 10 * 10) / 10
  const label = `${tenths % 1 === 0 ? tenths : tenths.toFixed(1)}折`

  if (!scheduled) {
    return {
      kind: 'discount',
      label,
      activeNow: true,
      title: `供应商正在促销：当前价格为原价的 ${label}（省 ${percent}%）`,
    }
  }

  const hours = windows
    .map(([s, e]) => `${localTime(s, now)}–${localTime(e, now)}`)
    .join('、')
  return {
    kind: 'discount',
    label,
    activeNow,
    title: activeNow
      ? `正在打折：输入价 ${label}（本地时间 ${hours}）`
      : `每天 ${hours}（本地时间）打 ${label}，现在是原价`,
  }
}

/** Assumed size when nothing reveals it.
 *
 *  Most closed models never publish a parameter count, so roughly two thirds of
 *  a catalogue lands here. The number is a placeholder for "a large model", and
 *  the UI marks it with `~` so it never reads as something we looked up. */
export const ASSUMED_PARAM_BILLIONS = 100

export interface ModelSizeLabel {
  /** `27B`, `2.4T`, `~100B`. */
  text: string
  /** False when this is the assumption rather than a published figure. */
  known: boolean
  title: string
}

/** Human-readable parameter count for a model row. */
export function modelSizeLabel(paramBillions?: number): ModelSizeLabel {
  const known = typeof paramBillions === 'number' && paramBillions > 0
  const b = known ? paramBillions! : ASSUMED_PARAM_BILLIONS

  // Trillions past 1000B, and drop a trailing `.0` — "27B", not "27.0B".
  const trim = (n: number) => (Number.isInteger(n) ? String(n) : n.toFixed(1))
  const text = b >= 1000 ? `${trim(b / 1000)}T` : b >= 1 ? `${trim(b)}B` : `${Math.round(b * 1000)}M`

  return {
    text: known ? text : `~${text}`,
    known,
    title: known
      ? `约 ${text} 参数`
      : `供应商未公开参数量，按 ${ASSUMED_PARAM_BILLIONS}B 估算`,
  }
}

/**
 * Promotions already looked up this session, keyed by provider id.
 *
 * Module scope, not component scope: the settings modal is destroyed and rebuilt
 * every time it opens, and a full catalogue costs ~450 requests to enumerate.
 * The catalogue does not change often enough to pay that twice in one sitting.
 */
const discountsByProvider = new Map<string, Record<string, number>>()

export function cachedDiscounts(providerId: string): Record<string, number> | undefined {
  return discountsByProvider.get(providerId)
}

export function cacheDiscounts(providerId: string, found: Record<string, number>): void {
  discountsByProvider.set(providerId, found)
}
