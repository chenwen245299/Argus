import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'

export type ActivityRange = 'today' | 'week' | 'month' | 'year'

export interface ActivityPaperStat {
  slug: string
  title: string
  fileType?: string
  count: number
  last_at: string
}

export interface ActivityReadingPaperStat {
  slug: string
  title: string
  fileType?: string
  duration_ms: number
  last_at: string
}

export interface ActivitySession {
  id: string
  slug: string
  title: string
  fileType?: string
  start_at: string
  end_at: string
  duration_ms: number
}

/** One machine's contribution to a day. Each device only ever writes its own. */
export interface DeviceDay {
  opened: Record<string, ActivityPaperStat>
  completed: Record<string, ActivityPaperStat>
  ai_analyzed: Record<string, ActivityPaperStat>
  paper_reading_ms: Record<string, ActivityReadingPaperStat>
  reading_ms: number
  sessions: ActivitySession[]
  updated_at: string
}

/** On-disk day: per-device slots, summed across devices for display. */
export interface DayV2 {
  date: string
  devices: Record<string, DeviceDay>
}

/**
 * The flattened, cross-device view a day presents to the UI. Same shape the
 * store exposed before per-device slots existed, so the panel is unchanged.
 */
export interface DailyActivity {
  date: string
  opened: Record<string, ActivityPaperStat>
  completed: Record<string, ActivityPaperStat>
  ai_analyzed: Record<string, ActivityPaperStat>
  paper_reading_ms: Record<string, ActivityReadingPaperStat>
  reading_ms: number
  sessions: ActivitySession[]
  updated_at: string
}

interface ActivityData {
  version: number
  days: Record<string, DayV2>
}

interface ActiveReadingSession {
  id: string
  slug: string
  title: string
  fileType?: string
  startAt: number
  lastCheckpointAt: number
  accumulatedMs: number
}

/** Reserved slot for pre-CRDT history migrated from a flat file. */
const LEGACY_SLOT = 'legacy'

function dayKey(ts = Date.now()) {
  const d = new Date(ts)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function iso(ts = Date.now()) {
  return new Date(ts).toISOString()
}

function emptyDeviceDay(): DeviceDay {
  return {
    opened: {},
    completed: {},
    ai_analyzed: {},
    paper_reading_ms: {},
    reading_ms: 0,
    sessions: [],
    updated_at: iso(),
  }
}

function emptyFlatDay(date: string): DailyActivity {
  return {
    date,
    opened: {},
    completed: {},
    ai_analyzed: {},
    paper_reading_ms: {},
    reading_ms: 0,
    sessions: [],
    updated_at: iso(),
  }
}

function rangeStart(range: ActivityRange): Date {
  const now = new Date()
  if (range === 'today') return new Date(now.getFullYear(), now.getMonth(), now.getDate())
  if (range === 'week') {
    const d = new Date(now)
    d.setDate(d.getDate() - 6)
    return new Date(d.getFullYear(), d.getMonth(), d.getDate())
  }
  if (range === 'month') return new Date(now.getFullYear(), now.getMonth(), 1)
  return new Date(now.getFullYear(), 0, 1)
}

function coerceDeviceDay(input: unknown): DeviceDay {
  const raw = (input && typeof input === 'object' ? input : {}) as Partial<DeviceDay>
  return {
    opened: (raw.opened as Record<string, ActivityPaperStat>) ?? {},
    completed: (raw.completed as Record<string, ActivityPaperStat>) ?? {},
    ai_analyzed: (raw.ai_analyzed as Record<string, ActivityPaperStat>) ?? {},
    paper_reading_ms: (raw.paper_reading_ms as Record<string, ActivityReadingPaperStat>) ?? {},
    reading_ms: typeof raw.reading_ms === 'number' ? raw.reading_ms : 0,
    sessions: Array.isArray(raw.sessions) ? (raw.sessions as ActivitySession[]) : [],
    updated_at: typeof raw.updated_at === 'string' ? raw.updated_at : iso(),
  }
}

/**
 * Accept both v2 (per-device) and v1 (flat) shapes. A flat day's fields are
 * wrapped into `v1SlotName`'s slot — `legacy` for a synced file, but this
 * machine's own device id for its local (localStorage) history, so the backend
 * merge (which only persists this device's slot) doesn't drop it on first save.
 */
function normalizeData(input: unknown, v1SlotName: string = LEGACY_SLOT): ActivityData {
  const raw = input as { days?: Record<string, unknown> } | null
  if (!raw || typeof raw !== 'object' || !raw.days || typeof raw.days !== 'object') {
    return { version: 2, days: {} }
  }
  const days: Record<string, DayV2> = {}
  for (const [date, day] of Object.entries(raw.days)) {
    if (!day || typeof day !== 'object') continue
    const devicesRaw = (day as { devices?: Record<string, unknown> }).devices
    if (devicesRaw && typeof devicesRaw === 'object') {
      const devices: Record<string, DeviceDay> = {}
      for (const [dev, slot] of Object.entries(devicesRaw)) devices[dev] = coerceDeviceDay(slot)
      days[date] = { date, devices }
    } else {
      days[date] = { date, devices: { [v1SlotName]: coerceDeviceDay(day) } }
    }
  }
  return { version: 2, days }
}

function hasActivityData(input: ActivityData) {
  return Object.keys(input.days).length > 0
}

function mergeStatBucket(
  target: Record<string, ActivityPaperStat>,
  src: Record<string, ActivityPaperStat>,
) {
  for (const [slug, s] of Object.entries(src)) {
    const e = target[slug]
    if (!e) {
      target[slug] = { ...s }
    } else {
      e.count += s.count
      if (s.last_at > e.last_at) {
        e.last_at = s.last_at
        e.title = s.title
        e.fileType = s.fileType
      }
    }
  }
}

/** Fold all device slots of a day into the flat view the UI consumes. */
function flatDay(day: DayV2): DailyActivity {
  const out = emptyFlatDay(day.date)
  out.updated_at = ''
  for (const slot of Object.values(day.devices)) {
    mergeStatBucket(out.opened, slot.opened)
    mergeStatBucket(out.completed, slot.completed)
    mergeStatBucket(out.ai_analyzed, slot.ai_analyzed)
    for (const [slug, r] of Object.entries(slot.paper_reading_ms)) {
      const e = out.paper_reading_ms[slug]
      if (!e) {
        out.paper_reading_ms[slug] = { ...r }
      } else {
        e.duration_ms += r.duration_ms
        if (r.last_at > e.last_at) {
          e.last_at = r.last_at
          e.title = r.title
          e.fileType = r.fileType
        }
      }
    }
    out.reading_ms += slot.reading_ms
    if (slot.sessions.length) out.sessions.push(...slot.sessions)
    if (slot.updated_at > out.updated_at) out.updated_at = slot.updated_at
  }
  if (!out.updated_at) out.updated_at = iso()
  return out
}

function clampReasonableDuration(ms: number) {
  if (!Number.isFinite(ms) || ms <= 0) return 0
  return Math.min(ms, 5 * 60 * 1000)
}

export const useActivityStore = defineStore('activity', () => {
  const libraryPath = ref<string | null>(null)
  const data = ref<ActivityData>({ version: 2, days: {} })
  const activeSession = ref<ActiveReadingSession | null>(null)
  const activeNow = ref(Date.now())
  const deviceId = ref<string>('')
  let deviceIdPromise: Promise<string> | null = null
  let heartbeatTimer: ReturnType<typeof setInterval> | null = null
  let saveChain: Promise<unknown> = Promise.resolve()

  const storageKey = computed(() =>
    libraryPath.value ? `argus:activity:${libraryPath.value}` : null
  )

  /** This machine's stable id (from the OS app-config dir), fetched once. */
  async function ensureDeviceId(): Promise<string> {
    if (deviceId.value) return deviceId.value
    if (!deviceIdPromise) {
      deviceIdPromise = invoke<string>('get_device_id')
        .then(id => {
          deviceId.value = id || 'unknown'
          return deviceId.value
        })
        .catch(e => {
          console.error('[activity] get_device_id failed:', e)
          deviceId.value = 'unknown'
          return deviceId.value
        })
    }
    return deviceIdPromise
  }

  function myDevice() {
    return deviceId.value || 'unknown'
  }

  function pruneOldDays(days: Record<string, DayV2>): Record<string, DayV2> {
    const cutoff = new Date()
    cutoff.setDate(cutoff.getDate() - 370)
    const cutoffKey = dayKey(cutoff.getTime())
    return Object.fromEntries(Object.entries(days).filter(([date]) => date >= cutoffKey))
  }

  function save() {
    const root = libraryPath.value
    const key = storageKey.value
    if (!root || !key) return
    try {
      data.value = { version: 2, days: pruneOldDays(data.value.days) }
      const snapshot = JSON.parse(JSON.stringify(data.value)) as ActivityData
      localStorage.setItem(key, JSON.stringify(snapshot))
      const device = myDevice()
      saveChain = saveChain
        .catch(() => undefined)
        .then(() => invoke('save_activity_log', { root, deviceId: device, data: snapshot }))
        .catch(e => console.error('[activity] save activity.json failed:', e))
    } catch {}
  }

  async function load(path: string | null) {
    endReading()
    await ensureDeviceId()
    libraryPath.value = path
    if (!path) {
      data.value = { version: 2, days: {} }
      return
    }
    let legacyData: ActivityData | null = null
    try {
      // Local (localStorage) history is this machine's own — slot it under this
      // device so a first save persists it, rather than the reserved legacy slot.
      legacyData = normalizeData(
        JSON.parse(localStorage.getItem(`argus:activity:${path}`) || 'null'),
        myDevice(),
      )
    } catch {}

    try {
      const fileData = normalizeData(await invoke('get_activity_log', { root: path }))
      if (hasActivityData(fileData) || !legacyData || !hasActivityData(legacyData)) {
        data.value = fileData
      } else {
        data.value = legacyData
        save()
      }
    } catch (e) {
      console.error('[activity] load activity.json failed:', e)
      data.value = legacyData ?? { version: 2, days: {} }
    }
  }

  /**
   * Re-read the file another machine synced in and adopt its OTHER-device slots,
   * keeping this device's own live slot (which may hold unsaved deltas newer than
   * disk). Safe because the merge is last-writer-wins per (day, device) cell.
   */
  async function reloadFromDisk() {
    const root = libraryPath.value
    if (!root) return
    const device = myDevice()
    try {
      const disk = normalizeData(await invoke('get_activity_log', { root }))
      const nextDays: Record<string, DayV2> = { ...data.value.days }
      for (const [date, day] of Object.entries(disk.days)) {
        const mine = nextDays[date]?.devices?.[device]
        const devices: Record<string, DeviceDay> = { ...day.devices }
        if (mine) devices[device] = mine
        nextDays[date] = { date, devices }
      }
      data.value = { version: 2, days: nextDays }
    } catch (e) {
      console.error('[activity] reload activity.json failed:', e)
    }
  }

  /** The current device's slot for `date`, created on demand. */
  function ensureDeviceDay(date = dayKey()): DeviceDay {
    const device = myDevice()
    let day = data.value.days[date]
    if (!day) {
      day = { date, devices: {} }
      data.value.days = { ...data.value.days, [date]: day }
    }
    if (!day.devices[device]) {
      day.devices = { ...day.devices, [device]: emptyDeviceDay() }
    }
    return day.devices[device]
  }

  function upsertPaperStat(
    bucket: Record<string, ActivityPaperStat>,
    slug: string,
    title: string,
    fileType?: string,
    increment = true,
  ) {
    const now = iso()
    const existing = bucket[slug]
    bucket[slug] = {
      slug,
      title,
      fileType,
      count: increment ? (existing?.count ?? 0) + 1 : Math.max(1, existing?.count ?? 1),
      last_at: now,
    }
  }

  function recordOpenPaper(slug: string, title: string, fileType?: string) {
    if (!storageKey.value) return
    const slot = ensureDeviceDay()
    upsertPaperStat(slot.opened, slug, title, fileType)
    slot.updated_at = iso()
    save()
  }

  function recordCompletedPaper(slug: string, title: string, fileType?: string) {
    if (!storageKey.value) return
    const slot = ensureDeviceDay()
    upsertPaperStat(slot.completed, slug, title, fileType, false)
    slot.updated_at = iso()
    save()
  }

  function recordAiAnalysis(slug: string, title: string, fileType?: string) {
    if (!storageKey.value) return
    const slot = ensureDeviceDay()
    upsertPaperStat(slot.ai_analyzed, slug, title, fileType, false)
    slot.updated_at = iso()
    save()
  }

  function addReadingDuration(slug: string, title: string, fileType: string | undefined, ms: number, at = Date.now()) {
    const duration = clampReasonableDuration(ms)
    if (!duration || !storageKey.value) return
    const slot = ensureDeviceDay(dayKey(at))
    slot.reading_ms += duration
    const existing = slot.paper_reading_ms[slug]
    slot.paper_reading_ms[slug] = {
      slug,
      title,
      fileType,
      duration_ms: (existing?.duration_ms ?? 0) + duration,
      last_at: iso(at),
    }
    slot.updated_at = iso(at)
    save()
  }

  function checkpoint() {
    const active = activeSession.value
    if (!active) return
    const now = Date.now()
    const delta = clampReasonableDuration(now - active.lastCheckpointAt)
    if (!delta) return
    addReadingDuration(active.slug, active.title, active.fileType, delta, now)
    active.accumulatedMs += delta
    active.lastCheckpointAt = now
    activeNow.value = now
  }

  function beginReading(slug: string, title: string, fileType?: string) {
    if (!storageKey.value) return
    const active = activeSession.value
    if (active?.slug === slug) {
      active.title = title
      active.fileType = fileType
      return
    }
    endReading()
    const now = Date.now()
    activeSession.value = {
      id: `${now}-${Math.random().toString(36).slice(2, 8)}`,
      slug,
      title,
      fileType,
      startAt: now,
      lastCheckpointAt: now,
      accumulatedMs: 0,
    }
    activeNow.value = now
  }

  function endReading() {
    const active = activeSession.value
    if (!active) return
    checkpoint()
    const endAt = Date.now()
    const duration = active.accumulatedMs
    if (duration >= 1000 && storageKey.value) {
      const slot = ensureDeviceDay(dayKey(active.startAt))
      slot.sessions = [
        {
          id: active.id,
          slug: active.slug,
          title: active.title,
          fileType: active.fileType,
          start_at: iso(active.startAt),
          end_at: iso(endAt),
          duration_ms: duration,
        },
        ...slot.sessions,
      ].slice(0, 120)
      slot.updated_at = iso(endAt)
      save()
    }
    activeSession.value = null
    activeNow.value = endAt
  }

  function startHeartbeat() {
    if (heartbeatTimer) return
    heartbeatTimer = setInterval(() => {
      activeNow.value = Date.now()
      checkpoint()
    }, 15_000)
  }

  function stopHeartbeat() {
    if (heartbeatTimer) {
      clearInterval(heartbeatTimer)
      heartbeatTimer = null
    }
    endReading()
  }

  const daysList = computed(() =>
    Object.values(data.value.days)
      .map(flatDay)
      .sort((a, b) => b.date.localeCompare(a.date))
  )

  function daysInRange(range: ActivityRange) {
    const start = rangeStart(range).getTime()
    return daysList.value.filter(d => new Date(`${d.date}T00:00:00`).getTime() >= start)
  }

  const liveReadingMs = computed(() => {
    const active = activeSession.value
    if (!active) return 0
    return active.accumulatedMs + Math.max(0, activeNow.value - active.lastCheckpointAt)
  })

  return {
    libraryPath,
    data,
    activeSession,
    liveReadingMs,
    load,
    reloadFromDisk,
    recordOpenPaper,
    recordCompletedPaper,
    recordAiAnalysis,
    beginReading,
    endReading,
    checkpoint,
    startHeartbeat,
    stopHeartbeat,
    daysInRange,
  }
})
