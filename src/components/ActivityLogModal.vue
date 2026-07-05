<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useActivityStore, type ActivityRange, type ActivitySession } from '../stores/activity'

const { t } = useI18n()
const activity = useActivityStore()

const emit = defineEmits<{ close: [] }>()

const range = ref<ActivityRange>('week')

const RANGES: { key: ActivityRange }[] = [
  { key: 'today' },
  { key: 'week' },
  { key: 'month' },
  { key: 'year' },
]

function dayKey(ts = Date.now()) {
  const d = new Date(ts)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function rangeStart(r: ActivityRange): Date {
  const now = new Date()
  if (r === 'today') return new Date(now.getFullYear(), now.getMonth(), now.getDate())
  if (r === 'week') {
    const d = new Date(now)
    d.setDate(d.getDate() - 6)
    return new Date(d.getFullYear(), d.getMonth(), d.getDate())
  }
  if (r === 'month') return new Date(now.getFullYear(), now.getMonth(), 1)
  return new Date(now.getFullYear(), 0, 1)
}

const days = computed(() => activity.daysInRange(range.value))

const activePendingMs = computed(() => {
  const active = activity.activeSession
  if (!active) return 0
  return Math.max(0, activity.liveReadingMs - active.accumulatedMs)
})

const openedUnique = computed(() => {
  const set = new Set<string>()
  days.value.forEach(day => Object.keys(day.opened).forEach(slug => set.add(slug)))
  return set.size
})

const openedEvents = computed(() =>
  days.value.reduce((sum, day) => sum + Object.values(day.opened).reduce((s, p) => s + p.count, 0), 0)
)

const completedUnique = computed(() => {
  const set = new Set<string>()
  days.value.forEach(day => Object.keys(day.completed).forEach(slug => set.add(slug)))
  return set.size
})

const aiAnalyzedUnique = computed(() => {
  const set = new Set<string>()
  days.value.forEach(day => Object.keys(day.ai_analyzed).forEach(slug => set.add(slug)))
  return set.size
})

const totalReadingMs = computed(() => {
  const persisted = days.value.reduce((sum, day) => sum + day.reading_ms, 0)
  return persisted + (range.value === 'today' || days.value.some(d => d.date === dayKey()) ? activePendingMs.value : 0)
})

interface PaperReadingRow {
  slug: string
  title: string
  fileType?: string
  duration_ms: number
  last_at: string
  color?: string
  widthPct?: number
}

const paperRows = computed<PaperReadingRow[]>(() => {
  const map = new Map<string, PaperReadingRow>()
  for (const day of days.value) {
    for (const row of Object.values(day.paper_reading_ms)) {
      const existing = map.get(row.slug)
      if (existing) {
        existing.duration_ms += row.duration_ms
        if (row.last_at > existing.last_at) {
          existing.last_at = row.last_at
          existing.title = row.title
          existing.fileType = row.fileType
        }
      } else {
        map.set(row.slug, { ...row })
      }
    }
  }
  const active = activity.activeSession
  if (active && (range.value === 'today' || days.value.some(d => d.date === dayKey()))) {
    const row = map.get(active.slug)
    if (row) {
      row.duration_ms += activePendingMs.value
      row.last_at = new Date().toISOString()
      row.title = active.title
      row.fileType = active.fileType
    } else if (activePendingMs.value > 0) {
      map.set(active.slug, {
        slug: active.slug,
        title: active.title,
        fileType: active.fileType,
        duration_ms: activePendingMs.value,
        last_at: new Date().toISOString(),
      })
    }
  }
  const rows = [...map.values()].sort((a, b) => b.duration_ms - a.duration_ms)
  const max = Math.max(...rows.map(row => row.duration_ms), 1)
  return rows.map(row => ({
    ...row,
    color: colorForDuration(row.duration_ms),
    widthPct: Math.max(4, Math.round((row.duration_ms / max) * 100)),
  }))
})

interface SessionRow extends ActivitySession {
  active?: boolean
}

const SESSION_MERGE_GAP_MS = 10 * 60 * 1000

function sessionDayKey(value: string) {
  const d = new Date(value)
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function canMergeSession(prev: SessionRow, next: SessionRow) {
  if (prev.slug !== next.slug) return false
  if (sessionDayKey(prev.start_at) !== sessionDayKey(next.start_at)) return false
  const prevEnd = new Date(prev.end_at).getTime()
  const nextStart = new Date(next.start_at).getTime()
  return nextStart - prevEnd <= SESSION_MERGE_GAP_MS
}

function mergeSessions(rows: SessionRow[]) {
  const sorted = [...rows].sort((a, b) => new Date(a.start_at).getTime() - new Date(b.start_at).getTime())
  const merged: SessionRow[] = []

  for (const row of sorted) {
    const last = merged[merged.length - 1]
    if (last && canMergeSession(last, row)) {
      const lastEnd = new Date(last.end_at).getTime()
      const rowEnd = new Date(row.end_at).getTime()
      last.end_at = new Date(Math.max(lastEnd, rowEnd)).toISOString()
      last.duration_ms += row.duration_ms
      last.active = last.active || row.active
      last.title = row.title || last.title
      last.fileType = row.fileType ?? last.fileType
      last.id = `${last.id}+${row.id}`
    } else {
      merged.push({ ...row })
    }
  }

  return merged
}

const sessionRows = computed<SessionRow[]>(() => {
  const rows: SessionRow[] = []
  const active = activity.activeSession
  const start = rangeStart(range.value).getTime()
  if (active && active.startAt >= start) {
    rows.push({
      id: active.id,
      slug: active.slug,
      title: active.title,
      fileType: active.fileType,
      start_at: new Date(active.startAt).toISOString(),
      end_at: new Date().toISOString(),
      duration_ms: activity.liveReadingMs,
      active: true,
    })
  }
  for (const day of days.value) rows.push(...day.sessions)
  return mergeSessions(rows.filter(row => new Date(row.start_at).getTime() >= start))
    .sort((a, b) => new Date(b.end_at).getTime() - new Date(a.end_at).getTime())
    .slice(0, 80)
})

interface ActivityBar {
  key: string
  label: string
  readingMs: number
  opened: number
  ai: number
}

function makeDateKey(d: Date) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

const chartBars = computed<ActivityBar[]>(() => {
  const now = new Date()
  if (range.value === 'today') {
    const buckets = Array.from({ length: 24 }, (_, h): ActivityBar => ({
      key: String(h),
      label: `${h}`,
      readingMs: 0,
      opened: 0,
      ai: 0,
    }))
    for (const row of sessionRows.value) {
      const h = new Date(row.start_at).getHours()
      buckets[h].readingMs += row.duration_ms
    }
    const today = days.value.find(d => d.date === dayKey())
    if (today) {
      for (const item of Object.values(today.opened)) buckets[new Date(item.last_at).getHours()].opened += 1
      for (const item of Object.values(today.ai_analyzed)) buckets[new Date(item.last_at).getHours()].ai += 1
    }
    return buckets
  }

  const start = rangeStart(range.value)
  const bars: ActivityBar[] = []
  for (let d = new Date(start); d <= now; d.setDate(d.getDate() + 1)) {
    const key = makeDateKey(d)
    bars.push({ key, label: String(d.getDate()), readingMs: 0, opened: 0, ai: 0 })
  }
  const map = new Map(bars.map(b => [b.key, b]))
  for (const day of days.value) {
    const b = map.get(day.date)
    if (!b) continue
    b.readingMs += day.reading_ms
    b.opened += Object.keys(day.opened).length
    b.ai += Object.keys(day.ai_analyzed).length
  }
  const today = map.get(dayKey())
  if (today) today.readingMs += activePendingMs.value
  return bars
})

const chartMax = computed(() => Math.max(...chartBars.value.map(b => b.readingMs), 1))

function colorForDuration(ms: number) {
  const minutes = ms / 60_000
  if (minutes < 1) return '#8fc5ff'
  if (minutes < 3) return '#4f8df7'
  if (minutes < 8) return '#48b884'
  if (minutes < 20) return '#f2b44b'
  return '#ef7a7a'
}

interface TimeOfDaySegment {
  id: string
  title: string
  startPct: number
  widthPct: number
  color: string
  label: string
}

function minuteOfDay(ts: number) {
  const d = new Date(ts)
  return d.getHours() * 60 + d.getMinutes() + d.getSeconds() / 60
}

function splitSessionIntoDaySegments(row: SessionRow): TimeOfDaySegment[] {
  const start = new Date(row.start_at).getTime()
  const end = Math.max(start, new Date(row.end_at).getTime())
  const segments: TimeOfDaySegment[] = []
  let cursor = start
  let idx = 0

  while (cursor < end) {
    const d = new Date(cursor)
    const nextDay = new Date(d.getFullYear(), d.getMonth(), d.getDate() + 1).getTime()
    const segEnd = Math.min(end, nextDay)
    const startMin = minuteOfDay(cursor)
    const durationMin = Math.max(0.2, (segEnd - cursor) / 60_000)
    const startPct = (startMin / 1440) * 100
    const rawWidthPct = (durationMin / 1440) * 100
    const widthPct = Math.min(100 - startPct, Math.max(rawWidthPct, 0.42))
    segments.push({
      id: `${row.id}:${idx}`,
      title: row.title,
      startPct,
      widthPct,
      color: colorForDuration(segEnd - cursor),
      label: `${row.title} · ${formatSessionClock(row)} · ${fmtDuration(row.duration_ms)}`,
    })
    cursor = segEnd
    idx += 1
  }
  return segments
}

const timeOfDaySegments = computed(() =>
  sessionRows.value
    .flatMap(splitSessionIntoDaySegments)
    .sort((a, b) => a.startPct - b.startPct)
)

const paperShareSegments = computed(() => {
  const total = paperRows.value.reduce((sum, row) => sum + row.duration_ms, 0)
  if (!total) return []
  let left = 0
  return paperRows.value.slice(0, 8).map(row => {
    const width = Math.max(2, (row.duration_ms / total) * 100)
    const seg = {
      slug: row.slug,
      title: row.title,
      leftPct: left,
      widthPct: width,
      color: row.color ?? colorForDuration(row.duration_ms),
      label: `${row.title} · ${fmtDuration(row.duration_ms)}`,
    }
    left += width
    return seg
  })
})

function fmtDuration(ms: number) {
  const totalMinutes = Math.max(0, Math.round(ms / 60_000))
  const h = Math.floor(totalMinutes / 60)
  const m = totalMinutes % 60
  if (h > 0) return m > 0 ? `${h}h ${m}m` : `${h}h`
  if (m > 0) return `${m}m`
  return ms > 0 ? '<1m' : '0m'
}

function fmtDateTime(value: string) {
  const d = new Date(value)
  return `${String(d.getMonth() + 1).padStart(2, '0')}/${String(d.getDate()).padStart(2, '0')} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

function fmtTime(value: string) {
  const d = new Date(value)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

function fmtMonthDay(value: string) {
  const d = new Date(value)
  return `${String(d.getMonth() + 1).padStart(2, '0')}/${String(d.getDate()).padStart(2, '0')}`
}

function formatSessionClock(row: SessionRow) {
  return `${fmtTime(row.start_at)} - ${fmtTime(row.end_at)}`
}

function fileTypeLabel(fileType?: string) {
  return fileType && fileType !== 'pdf' ? fileType.toUpperCase() : 'PDF'
}
</script>

<template>
  <div class="activity-backdrop" @click.self="emit('close')">
    <div class="activity-modal">
      <div class="activity-header">
        <div class="title-wrap">
          <span class="title-icon">
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
              <path d="M3 3v18h18"/>
              <path d="M7 14l3-3 3 2 5-7"/>
              <path d="M18 6h-4"/>
              <path d="M18 6v4"/>
            </svg>
          </span>
          <span class="activity-title">{{ t('activityLog.title') }}</span>
        </div>
        <div class="header-right">
          <div class="range-tabs">
            <button
              v-for="r in RANGES"
              :key="r.key"
              class="range-tab"
              :class="{ active: range === r.key }"
              @click="range = r.key"
            >
              {{ t('activityLog.' + r.key) }}
            </button>
          </div>
          <button class="btn-close" @click="emit('close')">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
              <line x1="18" y1="6" x2="6" y2="18"/>
              <line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>
      </div>

      <div class="activity-body">
        <div class="summary-cards">
          <div class="summary-card card-reading">
            <span class="card-label">{{ t('activityLog.readingTime') }}</span>
            <span class="card-value">{{ fmtDuration(totalReadingMs) }}</span>
          </div>
          <div class="summary-card card-opened">
            <span class="card-label">{{ t('activityLog.openedPapers') }}</span>
            <span class="card-value card-value-inline">
              {{ openedUnique }}
              <span class="card-sub">{{ t('activityLog.openedTimes', { count: openedEvents }) }}</span>
            </span>
          </div>
          <div class="summary-card card-completed">
            <span class="card-label">{{ t('activityLog.completedPapers') }}</span>
            <span class="card-value">{{ completedUnique }}</span>
          </div>
          <div class="summary-card card-ai">
            <span class="card-label">{{ t('activityLog.aiAnalyzedPapers') }}</span>
            <span class="card-value">{{ aiAnalyzedUnique }}</span>
          </div>
        </div>

        <div class="chart-card">
          <div class="chart-head">
            <div>
              <div class="chart-title">{{ t('activityLog.readingTrend') }}</div>
              <div class="chart-subtitle">{{ t('activityLog.readingTrendSub') }}</div>
            </div>
            <span v-if="activity.activeSession" class="live-chip">
              <span class="live-dot" />
              {{ t('activityLog.liveReading') }}
            </span>
          </div>
          <div v-if="chartBars.every(b => b.readingMs === 0 && b.opened === 0 && b.ai === 0)" class="empty-state">
            {{ t('activityLog.noData') }}
          </div>
          <div v-else class="bars-area">
            <div v-for="bar in chartBars" :key="bar.key" class="bar-col">
              <span v-if="bar.readingMs > 0" class="bar-value">{{ fmtDuration(bar.readingMs) }}</span>
              <div class="bar-track">
                <div
                  class="bar-fill"
                  :style="{ height: `${Math.max(3, Math.round((bar.readingMs / chartMax) * 112))}px` }"
                />
              </div>
              <div class="bar-foot">
                <span class="bar-label">{{ bar.label }}</span>
                <span v-if="bar.opened || bar.ai" class="bar-dot-row">
                  <span v-if="bar.opened" class="mini-dot opened-dot" />
                  <span v-if="bar.ai" class="mini-dot ai-dot" />
                </span>
              </div>
            </div>
          </div>
        </div>

        <div class="content-grid">
          <div class="panel-card">
            <div class="panel-head">
              <span class="panel-title">{{ t('activityLog.sessions') }}</span>
              <span class="panel-meta">{{ sessionRows.length }}</span>
            </div>
            <div v-if="sessionRows.length === 0" class="panel-empty">{{ t('activityLog.noSessions') }}</div>
            <div v-else class="session-timeline">
              <div v-for="row in sessionRows" :key="`${row.id}:${row.active ? 'active' : 'done'}`" class="timeline-row">
                <div class="timeline-axis">
                  <span class="timeline-dot" :class="{ live: row.active }" />
                  <span class="timeline-line" />
                </div>
                <div class="timeline-time">
                  <span class="timeline-clock">{{ formatSessionClock(row) }}</span>
                  <span class="timeline-date">{{ fmtMonthDay(row.start_at) }}</span>
                </div>
                <div class="timeline-card">
                  <div class="timeline-main">
                    <span class="paper-title">{{ row.title }}</span>
                    <span class="session-time">{{ fmtDateTime(row.start_at) }} - {{ fmtTime(row.end_at) }}</span>
                  </div>
                  <div class="session-side">
                    <span class="type-pill">{{ fileTypeLabel(row.fileType) }}</span>
                    <span class="duration-pill" :class="{ live: row.active }">{{ fmtDuration(row.duration_ms) }}</span>
                  </div>
                </div>
              </div>
            </div>
            <div v-if="timeOfDaySegments.length" class="day-map">
              <div class="day-map-track">
                <span class="day-tick tick-0" />
                <span class="day-tick tick-6" />
                <span class="day-tick tick-12" />
                <span class="day-tick tick-18" />
                <span class="day-tick tick-24" />
                <span
                  v-for="seg in timeOfDaySegments"
                  :key="seg.id"
                  class="day-segment"
                  :title="seg.label"
                  :style="{ left: `${seg.startPct}%`, width: `${seg.widthPct}%`, background: seg.color }"
                />
              </div>
              <div class="day-map-labels">
                <span>0</span><span>6</span><span>12</span><span>18</span><span>24</span>
              </div>
              <div class="duration-legend">
                <span><i style="background:#8fc5ff" />&lt;1m</span>
                <span><i style="background:#4f8df7" />1-3m</span>
                <span><i style="background:#48b884" />3-8m</span>
                <span><i style="background:#f2b44b" />8-20m</span>
                <span><i style="background:#ef7a7a" />20m+</span>
              </div>
            </div>
          </div>

          <div class="panel-card paper-reading-card">
            <div class="panel-head">
              <span class="panel-title">{{ t('activityLog.paperReading') }}</span>
              <span class="panel-meta">{{ paperRows.length }}</span>
            </div>
            <div v-if="paperRows.length === 0" class="panel-empty">{{ t('activityLog.noPaperTime') }}</div>
            <template v-else>
              <div class="paper-time-list">
                <div v-for="row in paperRows.slice(0, 80)" :key="row.slug" class="paper-time-row">
                  <div class="paper-text">
                    <span class="paper-title">{{ row.title }}</span>
                    <span class="paper-sub">{{ fileTypeLabel(row.fileType) }} · {{ fmtDateTime(row.last_at) }}</span>
                    <span class="paper-mini-bar">
                      <span
                        class="paper-mini-fill"
                        :style="{ width: `${row.widthPct ?? 0}%`, background: row.color }"
                      />
                    </span>
                  </div>
                  <span class="paper-duration">{{ fmtDuration(row.duration_ms) }}</span>
                </div>
              </div>
              <div class="paper-compare">
                <div class="paper-compare-head">
                  <span>{{ t('activityLog.paperCompare') }}</span>
                  <span>{{ fmtDuration(totalReadingMs) }}</span>
                </div>
                <div class="paper-share-track">
                  <span
                    v-for="seg in paperShareSegments"
                    :key="seg.slug"
                    class="paper-share-segment"
                    :title="seg.label"
                    :style="{ width: `${seg.widthPct}%`, background: seg.color }"
                  />
                </div>
              </div>
            </template>
          </div>
        </div>

      </div>
    </div>
  </div>
</template>

<style scoped>
.activity-backdrop {
  position: fixed;
  inset: 0;
  z-index: 9000;
  display: flex;
  align-items: center;
  justify-content: center;
  background: rgba(30, 41, 59, 0.32);
  backdrop-filter: blur(10px) saturate(1.05);
  -webkit-backdrop-filter: blur(10px) saturate(1.05);
}

.activity-modal {
  --activity-bg: #f7faff;
  --activity-panel: #ffffff;
  --activity-soft: #f2f6fc;
  --activity-border: #e1e8f2;
  --activity-border-strong: #d3deeb;
  --activity-text: #1f2937;
  --activity-muted: #64748b;
  --activity-faint: #94a3b8;
  --activity-blue: #4f8df7;
  --activity-green: #48b884;
  --activity-violet: #8b7cf6;
  --activity-amber: #f2b44b;
  width: 860px;
  max-width: 96vw;
  max-height: 86vh;
  display: flex;
  flex-direction: column;
  color: var(--activity-text);
  background: var(--activity-bg);
  border: 1px solid rgba(226, 232, 240, 0.92);
  border-radius: 16px;
  box-shadow: 0 28px 72px rgba(15, 23, 42, 0.18);
  overflow: hidden;
}

.activity-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 16px 18px 16px 22px;
  background: #fff;
  border-bottom: 1px solid var(--activity-border);
  flex-shrink: 0;
}

.title-wrap,
.header-right {
  display: flex;
  align-items: center;
  gap: 9px;
}

.title-icon {
  width: 30px;
  height: 30px;
  border-radius: 10px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--activity-blue);
  background: #edf4ff;
  border: 1px solid #dce9ff;
  flex-shrink: 0;
}

.activity-title {
  font-size: 16px;
  font-weight: 700;
}

.range-tabs {
  display: flex;
  align-items: center;
  gap: 1px;
  padding: 3px;
  border-radius: 999px;
  border: 1px solid #e4ebf5;
  background: #f0f4fa;
}

.range-tab {
  padding: 4px 11px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 550;
  color: var(--activity-muted);
}

.range-tab:hover {
  color: var(--activity-text);
}

.range-tab.active {
  color: #2563eb;
  background: #fff;
  box-shadow: 0 1px 5px rgba(30, 64, 175, 0.12);
}

.btn-close {
  width: 26px;
  height: 26px;
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--activity-faint);
  border-radius: 6px;
}

.btn-close:hover {
  color: var(--activity-text);
  background: #eef3fa;
}

.activity-body {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  gap: 16px;
  padding: 18px 22px 24px;
}

.summary-cards {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
}

.summary-card {
  min-width: 0;
  padding: 14px 16px;
  border-radius: 12px;
  border: 1px solid var(--activity-border);
  background: var(--activity-panel);
  box-shadow: 0 8px 24px rgba(15, 23, 42, 0.04);
  display: flex;
  flex-direction: column;
  gap: 5px;
}

.card-reading { background: #f2f7ff; }
.card-opened { background: #f3fbf7; }
.card-completed { background: #fff8eb; }
.card-ai { background: #f6f3ff; }

.card-label {
  font-size: 12px;
  color: var(--activity-muted);
}

.card-value {
  font-size: 25px;
  line-height: 1.1;
  font-weight: 760;
  color: var(--activity-text);
}

.card-value-inline {
  display: inline-flex;
  align-items: baseline;
  gap: 7px;
  min-width: 0;
}

.card-sub {
  font-size: 11px;
  font-weight: 600;
  color: var(--activity-faint);
  white-space: nowrap;
}

.chart-card,
.panel-card {
  border: 1px solid var(--activity-border);
  border-radius: 14px;
  background: rgba(255, 255, 255, 0.84);
  box-shadow: 0 10px 28px rgba(15, 23, 42, 0.045);
}

.chart-card {
  padding: 16px 18px 14px;
}

.chart-head,
.panel-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

.chart-title,
.panel-title {
  font-size: 13px;
  font-weight: 680;
}

.chart-subtitle {
  margin-top: 2px;
  font-size: 11px;
  color: var(--activity-faint);
}

.live-chip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 24px;
  padding: 0 10px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 650;
  color: #2876d8;
  background: #edf6ff;
  border: 1px solid #d8ebff;
  white-space: nowrap;
}

.live-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--activity-blue);
  animation: live-pulse 1.6s ease-in-out infinite;
}

@keyframes live-pulse {
  0%, 100% { opacity: 0.45; transform: scale(0.85); }
  50% { opacity: 1; transform: scale(1.2); }
}

.empty-state,
.panel-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  min-height: 128px;
  color: var(--activity-faint);
  font-size: 12px;
}

.bars-area {
  height: 176px;
  margin-top: 12px;
  padding: 18px 2px 6px;
  display: flex;
  align-items: flex-end;
  gap: 7px;
  overflow-x: auto;
  border-radius: 10px;
  background: #fff;
}

.bars-area::-webkit-scrollbar { height: 3px; }
.bars-area::-webkit-scrollbar-thumb { background: var(--activity-border-strong); border-radius: 2px; }

.bar-col {
  min-width: 32px;
  flex: 1;
  height: 100%;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: flex-end;
  gap: 5px;
}

.bar-value {
  min-height: 14px;
  padding: 2px 6px;
  border-radius: 999px;
  font-size: 10px;
  font-weight: 650;
  color: var(--activity-muted);
  background: rgba(255, 255, 255, 0.78);
  box-shadow: 0 1px 4px rgba(15, 23, 42, 0.06);
  white-space: nowrap;
}

.bar-track {
  width: 58%;
  max-width: 30px;
  min-width: 12px;
  height: 112px;
  display: flex;
  align-items: flex-end;
  border-radius: 7px;
  background: #edf2f9;
  overflow: hidden;
}

.bar-fill {
  width: 100%;
  min-height: 0;
  border-radius: 7px 7px 0 0;
  background: var(--activity-blue);
}

.bar-foot {
  min-height: 20px;
  display: flex;
  align-items: center;
  flex-direction: column;
  gap: 3px;
}

.bar-label {
  font-size: 10px;
  color: var(--activity-faint);
  line-height: 1;
}

.bar-dot-row {
  display: flex;
  align-items: center;
  gap: 3px;
}

.mini-dot {
  width: 5px;
  height: 5px;
  border-radius: 50%;
}

.opened-dot { background: var(--activity-green); }
.ai-dot { background: var(--activity-violet); }

.content-grid {
  display: grid;
  grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr);
  gap: 16px;
  align-items: stretch;
}

.panel-card {
  min-height: 270px;
  padding: 15px 16px;
  min-width: 0;
}

.paper-reading-card {
  display: flex;
  flex-direction: column;
  align-self: stretch;
}

.panel-meta {
  font-size: 12px;
  font-weight: 650;
  color: var(--activity-faint);
}

.session-timeline,
.paper-time-list {
  margin-top: 12px;
  display: flex;
  flex-direction: column;
  max-height: 320px;
  overflow-y: auto;
  padding-right: 4px;
}

.session-timeline::-webkit-scrollbar,
.paper-time-list::-webkit-scrollbar { width: 4px; }
.session-timeline::-webkit-scrollbar-thumb,
.paper-time-list::-webkit-scrollbar-thumb { background: var(--activity-border-strong); border-radius: 4px; }

.timeline-row {
  position: relative;
  display: grid;
  grid-template-columns: 18px 76px minmax(0, 1fr);
  gap: 8px;
  min-width: 0;
}

.timeline-axis {
  position: relative;
  display: flex;
  justify-content: center;
}

.timeline-dot {
  position: relative;
  z-index: 1;
  width: 9px;
  height: 9px;
  margin-top: 16px;
  border-radius: 50%;
  background: var(--activity-blue);
  border: 2px solid #fff;
  box-shadow: 0 0 0 2px #d9e8ff;
}

.timeline-dot.live {
  background: var(--activity-green);
  box-shadow: 0 0 0 2px #d8f2e4;
}

.timeline-line {
  position: absolute;
  top: 25px;
  bottom: -8px;
  width: 2px;
  border-radius: 999px;
  background: #dbe6f3;
}

.timeline-row:last-child .timeline-line {
  display: none;
}

.timeline-time {
  padding-top: 9px;
  display: flex;
  flex-direction: column;
  gap: 3px;
  align-items: flex-end;
  min-width: 0;
}

.timeline-clock {
  font-size: 11px;
  font-weight: 720;
  color: var(--activity-text);
  white-space: nowrap;
}

.timeline-date {
  font-size: 10px;
  font-weight: 650;
  color: var(--activity-faint);
  white-space: nowrap;
}

.timeline-card,
.paper-time-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 9px 10px;
  border-radius: 10px;
  background: #fff;
  border: 1px solid #edf2f7;
}

.timeline-card {
  margin-bottom: 8px;
  min-width: 0;
}

.timeline-main,
.paper-text {
  min-width: 0;
  display: flex;
  flex-direction: column;
  gap: 3px;
}

.paper-title {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 12px;
  font-weight: 640;
  color: var(--activity-text);
}

.session-time,
.paper-sub {
  font-size: 11px;
  color: var(--activity-faint);
  white-space: nowrap;
}

.day-map {
  margin-top: 8px;
  padding: 10px 10px 8px;
  border-radius: 12px;
  border: 1px solid #e8eef7;
  background: linear-gradient(180deg, #ffffff 0%, #f8fbff 100%);
}

.paper-compare-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 8px;
  font-size: 11px;
  color: var(--activity-faint);
}

.paper-compare-head span:first-child {
  font-weight: 700;
  color: var(--activity-muted);
}

.day-map-track {
  position: relative;
  height: 28px;
  border-radius: 999px;
  border: 1px solid #dfe8f4;
  background:
    linear-gradient(90deg, rgba(148, 163, 184, 0.11) 1px, transparent 1px) 0 0 / 25% 100%,
    #f1f5fb;
  overflow: hidden;
}

.day-segment {
  position: absolute;
  top: 0;
  bottom: 0;
  min-width: 4px;
  border-radius: 999px;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.26), 0 1px 5px rgba(15, 23, 42, 0.13);
}

.day-tick {
  position: absolute;
  top: 0;
  bottom: 0;
  width: 1px;
  background: rgba(148, 163, 184, 0.18);
}
.tick-0 { left: 0; }
.tick-6 { left: 25%; }
.tick-12 { left: 50%; }
.tick-18 { left: 75%; }
.tick-24 { right: 0; }

.day-map-labels {
  display: grid;
  grid-template-columns: repeat(5, 1fr);
  margin-top: 5px;
  font-size: 10px;
  color: var(--activity-faint);
}
.day-map-labels span:nth-child(2),
.day-map-labels span:nth-child(3),
.day-map-labels span:nth-child(4) { text-align: center; }
.day-map-labels span:last-child { text-align: right; }

.duration-legend {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 6px;
  margin-top: 7px;
  color: var(--activity-faint);
  font-size: 9.5px;
  white-space: nowrap;
}

.duration-legend span {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.duration-legend i {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  box-shadow: 0 0 0 1px rgba(255, 255, 255, 0.8);
}

.paper-compare {
  margin-top: 12px;
  padding: 16px 12px 17px;
  border-radius: 12px;
  border: 1px solid #e8eef7;
  background: #fff;
}

.paper-reading-card .paper-time-list {
  flex: 1 1 auto;
  min-height: 0;
  margin-bottom: 12px;
}

.paper-reading-card .paper-compare {
  margin-top: auto;
}

.paper-share-track {
  display: flex;
  height: 28px;
  padding: 2px;
  gap: 2px;
  border-radius: 999px;
  background: #eef3fa;
  overflow: hidden;
}

.paper-share-segment {
  min-width: 5px;
  border-radius: 999px;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.32);
}

.paper-mini-bar {
  width: 100%;
  height: 5px;
  border-radius: 999px;
  background: #eef3fa;
  overflow: hidden;
  margin-top: 4px;
}

.paper-mini-fill {
  display: block;
  height: 100%;
  min-width: 4px;
  border-radius: inherit;
}

.session-side {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
}

.type-pill,
.duration-pill,
.paper-duration {
  display: inline-flex;
  align-items: center;
  height: 22px;
  padding: 0 8px;
  border-radius: 999px;
  font-size: 11px;
  font-weight: 650;
  white-space: nowrap;
}

.type-pill {
  color: var(--activity-muted);
  background: #f0f4fa;
}

.duration-pill,
.paper-duration {
  color: #2876d8;
  background: #edf6ff;
}

.duration-pill.live {
  color: #2c8b65;
  background: #ecf9f2;
}

.paper-duration {
  flex-shrink: 0;
}

@media (max-width: 760px) {
  .summary-cards,
  .content-grid {
    grid-template-columns: 1fr;
  }
}
</style>
