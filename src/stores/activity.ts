import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

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
  days: Record<string, DailyActivity>
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

function dayKey(ts = Date.now()) {
  const d = new Date(ts)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function iso(ts = Date.now()) {
  return new Date(ts).toISOString()
}

function emptyDay(date: string): DailyActivity {
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

function normalizeData(input: unknown): ActivityData {
  const raw = input as Partial<ActivityData> | null
  if (!raw || typeof raw !== 'object') return { version: 1, days: {} }
  const days = raw.days && typeof raw.days === 'object' ? raw.days : {}
  return { version: 1, days: days as Record<string, DailyActivity> }
}

function clampReasonableDuration(ms: number) {
  if (!Number.isFinite(ms) || ms <= 0) return 0
  return Math.min(ms, 5 * 60 * 1000)
}

export const useActivityStore = defineStore('activity', () => {
  const libraryPath = ref<string | null>(null)
  const data = ref<ActivityData>({ version: 1, days: {} })
  const activeSession = ref<ActiveReadingSession | null>(null)
  const activeNow = ref(Date.now())
  let heartbeatTimer: ReturnType<typeof setInterval> | null = null

  const storageKey = computed(() =>
    libraryPath.value ? `argus:activity:${libraryPath.value}` : null
  )

  function save() {
    const key = storageKey.value
    if (!key) return
    try {
      const cutoff = new Date()
      cutoff.setDate(cutoff.getDate() - 370)
      const cutoffKey = dayKey(cutoff.getTime())
      const days = Object.fromEntries(
        Object.entries(data.value.days).filter(([date]) => date >= cutoffKey)
      )
      data.value = { version: 1, days }
      localStorage.setItem(key, JSON.stringify(data.value))
    } catch {}
  }

  function load(path: string | null) {
    endReading()
    libraryPath.value = path
    if (!path) {
      data.value = { version: 1, days: {} }
      return
    }
    try {
      data.value = normalizeData(JSON.parse(localStorage.getItem(`argus:activity:${path}`) || 'null'))
    } catch {
      data.value = { version: 1, days: {} }
    }
  }

  function ensureDay(date = dayKey()) {
    const existing = data.value.days[date]
    if (existing) return existing
    const next = emptyDay(date)
    data.value.days = { ...data.value.days, [date]: next }
    return next
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
    const day = ensureDay()
    upsertPaperStat(day.opened, slug, title, fileType)
    day.updated_at = iso()
    save()
  }

  function recordCompletedPaper(slug: string, title: string, fileType?: string) {
    if (!storageKey.value) return
    const day = ensureDay()
    upsertPaperStat(day.completed, slug, title, fileType, false)
    day.updated_at = iso()
    save()
  }

  function recordAiAnalysis(slug: string, title: string, fileType?: string) {
    if (!storageKey.value) return
    const day = ensureDay()
    upsertPaperStat(day.ai_analyzed, slug, title, fileType, false)
    day.updated_at = iso()
    save()
  }

  function addReadingDuration(slug: string, title: string, fileType: string | undefined, ms: number, at = Date.now()) {
    const duration = clampReasonableDuration(ms)
    if (!duration || !storageKey.value) return
    const day = ensureDay(dayKey(at))
    day.reading_ms += duration
    const existing = day.paper_reading_ms[slug]
    day.paper_reading_ms[slug] = {
      slug,
      title,
      fileType,
      duration_ms: (existing?.duration_ms ?? 0) + duration,
      last_at: iso(at),
    }
    day.updated_at = iso(at)
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
      const day = ensureDay(dayKey(active.startAt))
      day.sessions = [
        {
          id: active.id,
          slug: active.slug,
          title: active.title,
          fileType: active.fileType,
          start_at: iso(active.startAt),
          end_at: iso(endAt),
          duration_ms: duration,
        },
        ...day.sessions,
      ].slice(0, 120)
      day.updated_at = iso(endAt)
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
    Object.values(data.value.days).sort((a, b) => b.date.localeCompare(a.date))
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
