// Mainland-China public-holiday calendar, keyed by BEIJING calendar date.
//
// What it is for: DeepSeek prices (and the toolbar chip shows) the peak window
// only on Monday–Friday that are NOT a Chinese statutory holiday — see
// `describePeakPeriod` in modelPricing.ts. Which weekdays are holidays is not a
// computation: the State Council (国务院办公厅) publishes it each November for
// the next year, with 调休 bridge days that no formula can predict (2026-10-05..07
// are Mon–Wed, taken off by working Sun 09-20 and Sat 10-10). So the data has
// three layers, strongest first:
//
//   1. RUNTIME  — the Rust side (`holidays.rs`) refreshes the community dataset
//                 NateScarlet/holiday-cn in the background and caches it in the
//                 app-data dir. `get_cn_holidays` hands us that cache; a year it
//                 covers replaces the embedded one. This is what keeps the app
//                 right in January without anyone remembering to ship a table.
//   2. EMBEDDED — the 2025 and 2026 arrangements below, each date confirmed
//                 against the gov.cn notice AND holiday-cn. Always there, even
//                 with no network and no Tauri (browser dev server, tests).
//   3. FALLBACK — a year with neither: only the STATUTORY days (13 of them),
//                 from a lunar-date table plus fixed dates. Bridge days are
//                 unknowable here, so they come out as ordinary workdays; a
//                 console.warn says so once.
//
// Pure module: no Vue. It only touches Tauri (lazily, in try/catch) to fetch the
// runtime layer, and does nothing at all when invoke is unavailable.

import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

/** One day of a published arrangement; the shape holiday-cn uses. */
export interface HolidayDay {
  /** `YYYY-MM-DD`, a Beijing calendar date. */
  date: string
  /** The holiday it belongs to, e.g. `国庆节`. */
  name: string
  /** `true`: a day off. `false`: a make-up WORKING day (调休上班), usually a weekend. */
  isOffDay: boolean
}

export interface HolidayYear {
  year: number
  days: HolidayDay[]
}

type Source = 'runtime' | 'embedded' | 'fallback'

interface YearTable {
  source: Source
  /** Days off -> holiday name. */
  off: Map<string, string>
  /** Make-up working days (调休上班). */
  work: Set<string>
}

// ── Layer 2: the embedded arrangements ──────────────────────────────────────
// Source: 国务院办公厅关于2025年部分节假日安排的通知 (国办发明电〔2024〕12号,
// https://www.gov.cn/zhengce/zhengceku/202411/content_6986383.htm) and
// 国务院办公厅关于2026年部分节假日安排的通知 (国办发明电〔2025〕7号,
// https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm), each date
// cross-checked against holiday-cn. Third column: 1 = day off, 0 = make-up
// working day. To add next year, see "Updating the embedded table" in AGENTS.md.
type Row = readonly [date: string, name: string, off: 0 | 1]

const EMBEDDED_ROWS: ReadonlyArray<{ year: number; days: ReadonlyArray<Row> }> = [
  {
    year: 2025,
    days: [
      ['2025-01-01', '元旦', 1],
      ['2025-01-26', '春节', 0],
      ['2025-01-28', '春节', 1],
      ['2025-01-29', '春节', 1],
      ['2025-01-30', '春节', 1],
      ['2025-01-31', '春节', 1],
      ['2025-02-01', '春节', 1],
      ['2025-02-02', '春节', 1],
      ['2025-02-03', '春节', 1],
      ['2025-02-04', '春节', 1],
      ['2025-02-08', '春节', 0],
      ['2025-04-04', '清明节', 1],
      ['2025-04-05', '清明节', 1],
      ['2025-04-06', '清明节', 1],
      ['2025-04-27', '劳动节', 0],
      ['2025-05-01', '劳动节', 1],
      ['2025-05-02', '劳动节', 1],
      ['2025-05-03', '劳动节', 1],
      ['2025-05-04', '劳动节', 1],
      ['2025-05-05', '劳动节', 1],
      ['2025-05-31', '端午节', 1],
      ['2025-06-01', '端午节', 1],
      ['2025-06-02', '端午节', 1],
      ['2025-09-28', '国庆节、中秋节', 0],
      ['2025-10-01', '国庆节、中秋节', 1],
      ['2025-10-02', '国庆节、中秋节', 1],
      ['2025-10-03', '国庆节、中秋节', 1],
      ['2025-10-04', '国庆节、中秋节', 1],
      ['2025-10-05', '国庆节、中秋节', 1],
      ['2025-10-06', '国庆节、中秋节', 1],
      ['2025-10-07', '国庆节、中秋节', 1],
      ['2025-10-08', '国庆节、中秋节', 1],
      ['2025-10-11', '国庆节、中秋节', 0],
    ],
  },
  {
    year: 2026,
    days: [
      ['2026-01-01', '元旦', 1],
      ['2026-01-02', '元旦', 1],
      ['2026-01-03', '元旦', 1],
      ['2026-01-04', '元旦', 0],
      ['2026-02-14', '春节', 0],
      ['2026-02-15', '春节', 1],
      ['2026-02-16', '春节', 1],
      ['2026-02-17', '春节', 1],
      ['2026-02-18', '春节', 1],
      ['2026-02-19', '春节', 1],
      ['2026-02-20', '春节', 1],
      ['2026-02-21', '春节', 1],
      ['2026-02-22', '春节', 1],
      ['2026-02-23', '春节', 1],
      ['2026-02-28', '春节', 0],
      ['2026-04-04', '清明节', 1],
      ['2026-04-05', '清明节', 1],
      ['2026-04-06', '清明节', 1],
      ['2026-05-01', '劳动节', 1],
      ['2026-05-02', '劳动节', 1],
      ['2026-05-03', '劳动节', 1],
      ['2026-05-04', '劳动节', 1],
      ['2026-05-05', '劳动节', 1],
      ['2026-05-09', '劳动节', 0],
      ['2026-06-19', '端午节', 1],
      ['2026-06-20', '端午节', 1],
      ['2026-06-21', '端午节', 1],
      ['2026-09-20', '国庆节', 0],
      ['2026-09-25', '中秋节', 1],
      ['2026-09-26', '中秋节', 1],
      ['2026-09-27', '中秋节', 1],
      ['2026-10-01', '国庆节', 1],
      ['2026-10-02', '国庆节', 1],
      ['2026-10-03', '国庆节', 1],
      ['2026-10-04', '国庆节', 1],
      ['2026-10-05', '国庆节', 1],
      ['2026-10-06', '国庆节', 1],
      ['2026-10-07', '国庆节', 1],
      ['2026-10-10', '国庆节', 0],
    ],
  },
]

/** The embedded arrangements in the holiday-cn shape (for tests and tooling). */
export const EMBEDDED_CN_HOLIDAYS: ReadonlyArray<HolidayYear> = EMBEDDED_ROWS.map(y => ({
  year: y.year,
  days: y.days.map(([date, name, off]) => ({ date, name, isOffDay: off === 1 })),
}))

// ── Beijing wall clock ──────────────────────────────────────────────────────

export interface BeijingParts {
  year: number
  /** 1–12 */
  month: number
  /** 1–31 */
  day: number
  /** 0 = Sunday … 6 = Saturday */
  weekday: number
  /** Minutes since 00:00 Beijing time. */
  minutes: number
}

/**
 * The Beijing (UTC+8) wall-clock reading of an instant, independent of the
 * user's own timezone. China has no DST, so a fixed offset is exact. `null` for
 * an invalid Date.
 */
export function beijingParts(date: Date): BeijingParts | null {
  const t = date.getTime()
  if (!Number.isFinite(t)) return null
  const b = new Date(t + 8 * 3_600_000)
  const year = b.getUTCFullYear()
  if (!Number.isFinite(year)) return null
  return {
    year,
    month: b.getUTCMonth() + 1,
    day: b.getUTCDate(),
    weekday: b.getUTCDay(),
    minutes: b.getUTCHours() * 60 + b.getUTCMinutes(),
  }
}

const pad2 = (n: number) => (n < 10 ? '0' + n : String(n))
const dateKey = (y: number, m: number, d: number) => `${y}-${pad2(m)}-${pad2(d)}`

/** `YYYY-MM-DD` that is a real calendar date. */
function parseDateKey(s: unknown): { y: number; m: number; d: number } | null {
  if (typeof s !== 'string') return null
  const mt = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s)
  if (!mt) return null
  const y = +mt[1], m = +mt[2], d = +mt[3]
  const t = new Date(Date.UTC(y, m - 1, d))
  if (t.getUTCFullYear() !== y || t.getUTCMonth() !== m - 1 || t.getUTCDate() !== d) return null
  return { y, m, d }
}

// ── Layer 3: the statutory days, derived ────────────────────────────────────

/**
 * 春节 (正月初一), 端午 (五月初五) and 中秋 (八月十五) as `MM-DD`, in that order.
 *
 * A table, not `Intl.DateTimeFormat('en-u-ca-chinese')`, because Intl turned out
 * not to be reliable across engines. Compared with the Hong Kong Observatory's
 * Gregorian–Lunar conversion tables (https://www.hko.gov.hk/tc/gts/time/calendar/,
 * one file per year, 2016–2100): JavaScriptCore (WKWebView on macOS) agrees with
 * all 85 years, but Node 24's bundled ICU 78.3 (V8) is one day off for 春节 2027
 * (it says 02-07, the truth is 02-06) and for 春节 2030 (it says 02-02, the truth
 * is 02-03); WebView2 on Windows ships Chromium's own ICU, which was not tested
 * and may or may not share that. Those are the two new moons that
 * fall within minutes of midnight Beijing time (about 23:56 on 2027-02-06 and
 * 00:07 on 2030-02-03), which is where implementations part ways; a holiday a day
 * out is a wrong chip.
 * These dates are the HKO's, and all 2016–2026 of them lie inside the official
 * arrangements of those years. 2025–2060 is plenty: this layer only ever serves
 * a year that has no real data, and a newer build will carry its own table.
 */
const LUNAR_HOLIDAYS: Readonly<Record<number, string>> = {
  2025: '01-29 05-31 10-06',
  2026: '02-17 06-19 09-25',
  2027: '02-06 06-09 09-15',
  2028: '01-26 05-28 10-03',
  2029: '02-13 06-16 09-22',
  2030: '02-03 06-05 09-12',
  2031: '01-23 06-24 10-01',
  2032: '02-11 06-12 09-19',
  2033: '01-31 06-01 09-08',
  2034: '02-19 06-20 09-27',
  2035: '02-08 06-10 09-16',
  2036: '01-28 05-30 10-04',
  2037: '02-15 06-18 09-24',
  2038: '02-04 06-07 09-13',
  2039: '01-24 05-27 10-02',
  2040: '02-12 06-14 09-20',
  2041: '02-01 06-03 09-10',
  2042: '01-22 06-22 09-28',
  2043: '02-10 06-11 09-17',
  2044: '01-30 05-31 10-05',
  2045: '02-17 06-19 09-25',
  2046: '02-06 06-08 09-15',
  2047: '01-26 05-29 10-04',
  2048: '02-14 06-15 09-22',
  2049: '02-02 06-04 09-11',
  2050: '01-23 06-23 09-30',
  2051: '02-11 06-13 09-19',
  2052: '02-01 06-01 09-07',
  2053: '02-19 06-20 09-26',
  2054: '02-08 06-10 09-16',
  2055: '01-28 05-30 10-05',
  2056: '02-15 06-17 09-24',
  2057: '02-04 06-06 09-13',
  2058: '01-24 06-25 10-02',
  2059: '02-12 06-14 09-21',
  2060: '02-02 06-03 09-09',
}

/**
 * The day in April of the 清明 solar term (the statutory 清明节), by the standard
 * empirical formula `⌊Y·0.2422 + 4.81⌋ − ⌊Y/4⌋` (Y = year mod 100, valid 2001–2099).
 * Identical to the HKO's 節氣 column for every year 2016–2099 and to a
 * solar-longitude computation for 2000–2099. Intl exposes no solar terms.
 */
function qingmingDay(year: number): number {
  const y = year % 100
  return Math.floor(y * 0.2422 + 4.81) - Math.floor(y / 4)
}

/**
 * The statutory days off of `year` under 全国年节及纪念日放假办法 (revised
 * 2024-11): 元旦 1/1; 春节 除夕 + 正月初一..初三; 清明; 劳动节 5/1–5/2; 端午; 中秋;
 * 国庆节 10/1–10/3 — 13 days. No bridge days, no make-up days: those exist only in
 * a published arrangement. A year outside the lunar table (2025–2060) gets the
 * fixed-date days only, rather than a guess.
 */
export function statutoryHolidays(year: number): Map<string, string> {
  const out = new Map<string, string>()
  const add = (t: number, name: string) => {
    const c = new Date(t)
    const k = dateKey(c.getUTCFullYear(), c.getUTCMonth() + 1, c.getUTCDate())
    // 中秋 can fall inside 10/1–10/3 (2028, 2039, 2058); one day, both names,
    // as in the official 2025 arrangement ("国庆节、中秋节").
    const prev = out.get(k)
    out.set(k, prev && prev !== name ? `${prev}、${name}` : name)
  }
  const day = (m: number, d: number) => Date.UTC(year, m - 1, d)
  const lunar = LUNAR_HOLIDAYS[year]?.split(' ').map(md => {
    const [m, d] = md.split('-')
    return day(+m, +d)
  })

  add(day(1, 1), '元旦')
  if (lunar) {
    const [cny, dragon] = lunar
    add(cny - 86_400_000, '春节')                       // 除夕
    for (let i = 0; i < 3; i++) add(cny + i * 86_400_000, '春节') // 初一..初三
    add(dragon, '端午节')
  }
  add(day(4, qingmingDay(year)), '清明节')
  add(day(5, 1), '劳动节'); add(day(5, 2), '劳动节')
  for (const d of [1, 2, 3]) add(day(10, d), '国庆节')
  if (lunar) add(lunar[2], '中秋节') // after 国庆节, so a merged day reads 国庆节、中秋节
  return out
}

// ── The three layers ────────────────────────────────────────────────────────

function tableOf(source: Source, rows: Iterable<{ date: string; name: string; isOffDay: boolean }>): YearTable {
  const t: YearTable = { source, off: new Map(), work: new Set() }
  for (const r of rows) {
    if (r.isOffDay) t.off.set(r.date, r.name)
    else t.work.add(r.date)
  }
  return t
}

const embedded = new Map<number, YearTable>(
  EMBEDDED_CN_HOLIDAYS.map(y => [y.year, tableOf('embedded', y.days)]),
)
const runtime = new Map<number, YearTable>()
const runtimeSignature = new Map<number, string>()
const fallback = new Map<number, YearTable>()
let warnedNoData = false

function tableFor(year: number): YearTable {
  const real = runtime.get(year) ?? embedded.get(year)
  if (real) return real
  let t = fallback.get(year)
  if (!t) {
    t = { source: 'fallback', off: statutoryHolidays(year), work: new Set() }
    fallback.set(year, t)
    // One warning, and only for the year that matters now: a past year falling
    // back is just old history, a current one means the app is out of date AND
    // the background refresh has not (yet) delivered.
    if (!warnedNoData && beijingParts(new Date())?.year === year) {
      warnedNoData = true
      console.warn(
        `[cnHolidays] No official holiday arrangement for ${year}; using the statutory days only ` +
        `(bridge days / 调休 are unknown, so they count as ordinary workdays). ` +
        `Update Argus or wait for the background holiday refresh.`,
      )
    }
  }
  return t
}

/** Name of the public holiday on a Beijing calendar date, or `null` if it is not a day off. */
export function holidayNameOn(year: number, month: number, day: number): string | null {
  return tableFor(year).off.get(dateKey(year, month, day)) ?? null
}

/** Whether a Beijing calendar date is an official day off (any weekday, weekends included). */
export function isCnPublicHoliday(year: number, month: number, day: number): boolean {
  return holidayNameOn(year, month, day) !== null
}

/** Whether a Beijing calendar date is an official make-up WORKING day (调休上班). */
export function isCnMakeupWorkday(year: number, month: number, day: number): boolean {
  return tableFor(year).work.has(dateKey(year, month, day))
}

/** Where a year's data comes from — for diagnostics and tests. */
export function holidayDataSource(year: number): Source {
  return tableFor(year).source
}

// ── Layer 1: the runtime calendar ───────────────────────────────────────────

const listeners = new Set<() => void>()

/** Be told when the runtime calendar changed what a date means. Returns the unsubscribe function. */
export function subscribeCnHolidays(listener: () => void): () => void {
  listeners.add(listener)
  return () => { listeners.delete(listener) }
}

/**
 * How far a runtime year may differ from the embedded one for the same year, as
 * the symmetric difference of their days-off sets. The embedded tables were
 * checked against the State Council notices, and a real amendment moves a day or
 * two (a bridge day added, a make-up day swapped); a mirror that disagrees on more
 * than this is a bad or partial publish, not news.
 */
export const MAX_RUNTIME_DRIFT_DAYS = 3

let warnedRuntimeDrift = false

/**
 * Take a runtime-fetched calendar (the holiday-cn shape). Each year it carries
 * replaces the embedded or fallback data for that year; years it does not carry
 * are untouched. Malformed years or entries are ignored, never half-applied, and
 * a year with no valid day off is skipped so a bad payload cannot blank out a
 * good embedded year. A year that also has embedded data is accepted only when
 * its days off differ from the embedded ones by at most `MAX_RUNTIME_DRIFT_DAYS`
 * (the backend can check a payload's shape, but not what the state council
 * decided); a year with no embedded data is taken as the backend validated it.
 * Returns whether anything changed.
 */
export function mergeRuntimeCalendar(years: unknown): boolean {
  if (!Array.isArray(years)) return false
  let changed = false
  for (const raw of years) {
    const year = (raw as HolidayYear | null)?.year
    const days = (raw as HolidayYear | null)?.days
    if (typeof year !== 'number' || !Number.isInteger(year) || !Array.isArray(days)) continue
    const rows: HolidayDay[] = []
    const seen = new Set<string>()
    for (const d of days) {
      const p = parseDateKey(d?.date)
      if (!p || p.y !== year || seen.has(d.date) || typeof d.isOffDay !== 'boolean') continue
      seen.add(d.date)
      rows.push({
        date: d.date,
        name: typeof d.name === 'string' && d.name.trim() ? d.name.trim() : '节假日',
        isOffDay: d.isOffDay,
      })
    }
    if (!rows.some(r => r.isOffDay)) continue
    const known = embedded.get(year)
    if (known) {
      const runtimeOff = new Set(rows.filter(r => r.isOffDay).map(r => r.date))
      let drift = 0
      for (const d of runtimeOff) if (!known.off.has(d)) drift++
      for (const d of known.off.keys()) if (!runtimeOff.has(d)) drift++
      if (drift > MAX_RUNTIME_DRIFT_DAYS) {
        if (!warnedRuntimeDrift) {
          warnedRuntimeDrift = true
          console.warn(
            `[cnHolidays] Ignoring the fetched ${year} holiday calendar: its days off differ from the ` +
            `built-in table by ${drift} days (more than ${MAX_RUNTIME_DRIFT_DAYS}), which looks like a bad or partial publish.`,
          )
        }
        continue
      }
    }
    rows.sort((a, b) => (a.date < b.date ? -1 : 1))
    const signature = JSON.stringify(rows)
    if (runtimeSignature.get(year) === signature) continue
    runtime.set(year, tableOf('runtime', rows))
    runtimeSignature.set(year, signature)
    fallback.delete(year)
    changed = true
  }
  if (changed) for (const l of [...listeners]) { try { l() } catch { /* a listener must not break the others */ } }
  return changed
}

/** Read the backend's cached calendar (no network, instant) and merge it. Silent on any failure. */
export async function loadRuntimeCalendar(): Promise<void> {
  try {
    const snap = await invoke<{ years?: unknown }>('get_cn_holidays')
    mergeRuntimeCalendar(snap?.years)
  } catch {
    // Not running inside Tauri, or the backend has no cache yet: the embedded
    // table and the fallback cover it.
  }
}

/** Every window that prices usage imports this module, so each one syncs by itself. */
function startRuntimeSync(): void {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) return
  void loadRuntimeCalendar()
  // The backend emits this when a background refresh actually changed the data.
  listen('cn-holidays-updated', () => { void loadRuntimeCalendar() }).catch(() => {})
}

startRuntimeSync()
