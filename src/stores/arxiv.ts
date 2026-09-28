import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type {
  ArxivAnalysisEvent, ArxivAnalysisRun, ArxivConfig, ArxivPaper, ArxivScheduleStatus,
} from '../types'
import { fetchArxivCategories } from '../utils/arxivFetch'
import { fetchBiorxivAsArxivPapers } from '../utils/biorxivFetch'
import { i18n } from '../i18n'

/** Result of a duplicate-aware import command (add_arxiv_to_library). */
interface ImportOutcome { status: 'imported' | 'duplicate'; slug?: string; existingSlug?: string; title?: string }

export type SortMode = 'score' | 'date' | 'status' | 'rating'
export type SortOrder = 'desc' | 'asc'
export type FilterMode = 'all' | 'unread' | 'pending_analysis'

/** The provider is throttling: the whole bulk run is paused until `until` (ms epoch). */
export interface AnalysisWaiting {
  message: string
  retryIn: number
  concurrency: number
  until: number
}

/** How the last bulk run ended, or why it could not start. Stays until dismissed or a new run starts. */
export type AnalysisNotice =
  | {
      kind: 'finished'
      total: number
      succeeded: number
      failed: number
      filtered: number
      reverted: number
      /** Set when the run stopped early on a provider-wide error (quota, bad key, persistent overload). */
      stoppedReason: string | null
      cancelled: boolean
      /** False when the backend predates the per-outcome counters. */
      hasCounts: boolean
    }
  | { kind: 'error'; message: string }

/** A single-paper analysis refused before anything was sent (disk untouched). */
export interface SingleAnalysisError {
  arxiv_id: string
  message: string
}

const num = (v: unknown): number => (typeof v === 'number' && Number.isFinite(v) ? v : 0)

export const DEFAULT_ARXIV_ANALYSIS_PROMPT = `你是一名研究助理。根据以下论文元数据，评估其与这些主题的相关性：{topics}。

论文标题：{title}
作者：{authors}
摘要：{abstract}

提供（所有文字字段必须使用中文）：
1. relevance_score：整数 0-10（10 = 高度相关），有一个话题符合就算是相关了，也就是至少要6分以上
2. relevance_reason：一句话解释评分原因
3. key_contributions：2-3 个主要贡献的要点列表
4. summary：2-3 句通俗易懂的总结
5. matched_topics：从上方主题列表中选出与本文最匹配的主题，返回中文列表（无匹配则返回空列表）

仅回复符合此模式的有效 JSON：
{"relevance_score": 0, "relevance_reason": "", "key_contributions": [], "summary": "", "matched_topics": []}`

const DEFAULT_CONFIG: ArxivConfig = {
  categories: [],
  keywords: [],
  auto_fetch_enabled: false,
  interval_days: 1,
  fetch_time: '09:00',
  days_back: 5,
  max_fetch: 100,
  ai_analysis_enabled: false,
  ai_analysis_prompt: DEFAULT_ARXIV_ANALYSIS_PROMPT,
  ai_analysis_focus: '',
  ai_filter_enabled: true,
  ai_filter_threshold: 6,
  ai_provider_id: null,
  ai_model_id: null,
  last_fetch_date: null,
  ai_analysis_concurrency: 5,
  fetch_biorxiv: false,
  fetch_arxiv: true,
}

export const useArxivStore = defineStore('arxiv', () => {
  const papers = ref<ArxivPaper[]>([])
  const config = ref<ArxivConfig>({ ...DEFAULT_CONFIG })
  const scheduleStatus = ref<ArxivScheduleStatus | null>(null)
  const loaded = ref(false)

  // Progress state
  const fetching = ref(false)
  const refreshing = ref(false)
  const fetchMessage = ref('')
  const analyzing = ref(false)
  const analyzeProgress = ref({ done: 0, total: 0 })
  // How many previously-failed papers the current bulk run is retrying.
  const analyzeRetryingFailed = ref(0)
  const analyzeWaiting = ref<AnalysisWaiting | null>(null)
  const analysisNotice = ref<AnalysisNotice | null>(null)
  const lastSingleError = ref<SingleAnalysisError | null>(null)
  // The run whose outcome has already been shown (or dismissed) here, so the
  // status poll does not bring it back.
  let lastRunSeenAt: number | null = null

  // Results of the running batch, by paper; `null` = filtered out. The backend
  // writes them to disk every couple of seconds, so an inbox read mid-run can
  // lag the events already shown — this is laid over every such read.
  const runOutcomes = new Map<string, Partial<ArxivPaper> | null>()

  function withRunOutcomes(list: ArxivPaper[]): ArxivPaper[] {
    if (runOutcomes.size === 0) return list
    const out: ArxivPaper[] = []
    for (const p of list) {
      const o = runOutcomes.get(p.arxiv_id)
      if (o === undefined) out.push(p)
      else if (o !== null) out.push({ ...p, ...o })
    }
    return out
  }

  function noticeFromRun(r: ArxivAnalysisRun): AnalysisNotice {
    return {
      kind: 'finished',
      total: num(r.total),
      succeeded: num(r.succeeded),
      failed: num(r.failed),
      filtered: num(r.filtered),
      reverted: num(r.reverted),
      stoppedReason: r.stopped_reason ? String(r.stopped_reason) : null,
      cancelled: r.cancelled === true,
      hasCounts: true,
    }
  }

  // UI state
  const sortMode = ref<SortMode>('score')
  const sortOrder = ref<SortOrder>('desc')
  const filterMode = ref<FilterMode>('all')
  const newCount = ref(0)  // badge count for main window

  // Event listeners
  let unlistenFetch: UnlistenFn | null = null
  let unlistenAnalysis: UnlistenFn | null = null
  let unlistenRecommend: UnlistenFn | null = null
  let statusPollTimer: ReturnType<typeof setInterval> | null = null

  const sortedPapers = computed(() => {
    let list = [...papers.value]
    if (filterMode.value === 'unread') {
      list = list.filter(p => !p.read)
    } else if (filterMode.value === 'pending_analysis') {
      // What "AI 分析全部" would pick up, plus papers in flight so a running batch
      // does not make rows blink out of this view and back in on failure.
      list = list.filter(p =>
        p.analysis_status === 'pending' || p.analysis_status === 'failed' || p.analysis_status === 'analyzing')
    }
    if (sortMode.value === 'score') {
      list.sort((a, b) => (a.relevance_score ?? -1) - (b.relevance_score ?? -1))
    } else if (sortMode.value === 'date') {
      list.sort((a, b) => a.published.localeCompare(b.published))
    } else if (sortMode.value === 'rating') {
      list.sort((a, b) => (a.rating ?? 0) - (b.rating ?? 0))
    } else if (sortMode.value === 'status') {
      // Order by analysis lifecycle: done → analyzing → pending → failed.
      const rank: Record<string, number> = { done: 3, analyzing: 2, pending: 1, failed: 0 }
      list.sort((a, b) => (rank[a.analysis_status] ?? -1) - (rank[b.analysis_status] ?? -1))
    }
    if (sortOrder.value === 'desc') {
      list.reverse()
    }
    return list
  })

  async function loadConfig() {
    try {
      const loaded = await invoke<ArxivConfig>('get_arxiv_config')
      config.value = { ...DEFAULT_CONFIG, ...loaded }
    } catch { /* no library open */ }
  }

  async function saveConfig(c: ArxivConfig) {
    const next = { ...DEFAULT_CONFIG, ...c }
    await invoke('save_arxiv_config', { config: next })
    config.value = next
  }

  async function loadInbox() {
    try {
      const inbox = await invoke<{ papers: ArxivPaper[]; last_updated: string }>('get_arxiv_inbox')
      // Preserve read=true from current frontend state to guard against in-flight
      // mark_paper_read calls being overtaken by a concurrent loadInbox.
      const knownRead = new Set(papers.value.filter(p => p.read).map(p => p.arxiv_id))
      papers.value = withRunOutcomes(inbox.papers.map(p => ({
        ...p,
        read: p.read || knownRead.has(p.arxiv_id),
      })))
    } catch { papers.value = [] }
  }

  function applyScheduleStatus(status: ArxivScheduleStatus) {
    scheduleStatus.value = status
    if (status.analyzing) {
      analyzing.value = true
      analyzeProgress.value = {
        done: status.analyzed_count,
        total: status.analyzed_count + status.total_pending,
      }
    } else if (analyzing.value) {
      // Missed the 'finished' event — the run is over either way.
      analyzing.value = false
      analyzeWaiting.value = null
      runOutcomes.clear()
    }
    // A window opened mid-pause, or after a run ended while it was closed,
    // missed the events: take both from the backend.
    const w = status.analyzing ? status.waiting : null
    if (w && !analyzeWaiting.value && w.until_ms - Date.now() > 1000) {
      analyzeWaiting.value = {
        message: w.message ?? '',
        retryIn: Math.ceil((w.until_ms - Date.now()) / 1000),
        concurrency: num(w.concurrency),
        until: w.until_ms,
      }
    }
    const run = status.analyzing ? null : status.last_run
    if (run && run.finished_at_ms !== lastRunSeenAt) {
      lastRunSeenAt = run.finished_at_ms
      if (!analysisNotice.value) analysisNotice.value = noticeFromRun(run)
    }
  }

  async function loadScheduleStatus() {
    try {
      applyScheduleStatus(await invoke<ArxivScheduleStatus>('get_arxiv_schedule_status'))
    } catch { scheduleStatus.value = null }
  }

  async function load() {
    await Promise.all([loadConfig(), loadInbox(), loadScheduleStatus()])
    loaded.value = true
  }

  async function refreshInbox() {
    if (refreshing.value) return
    refreshing.value = true
    fetchMessage.value = ''
    try {
      const inbox = await invoke<{ papers: ArxivPaper[]; last_updated: string }>('refresh_arxiv_inbox')
      papers.value = withRunOutcomes(inbox.papers)
      await loadScheduleStatus()
    } catch (e) {
      fetchMessage.value = String(e)
    } finally {
      refreshing.value = false
    }
  }

  async function fetchManual() {
    if (fetching.value) { fetchMessage.value = '抓取已在进行中'; return }
    fetching.value = true
    fetchMessage.value = ''
    try {
      await loadConfig()
      if (!config.value.fetch_arxiv && !config.value.fetch_biorxiv)
        throw new Error('请至少开启一种爬取来源（arXiv 或 bioRxiv）')
      const arxivReady = config.value.fetch_arxiv && config.value.categories.length > 0
      // arXiv 需要至少一个分类才能抓取；若只开了 arXiv 却没选分类，直接报错而非静默无结果。
      if (config.value.fetch_arxiv && !arxivReady && !config.value.fetch_biorxiv)
        throw new Error('已开启 arXiv 抓取但未选择任何分类，请先在设置中至少选择一个 arXiv 分类')
      const today = new Date().toISOString().slice(0, 10)
      const from = new Date(Date.now() - config.value.days_back * 86400000).toISOString().slice(0, 10)
      const arxivPapers = arxivReady
        ? await fetchArxivCategories(config.value, from, today)
        : []
      const biorxivPapers = config.value.fetch_biorxiv
        ? await fetchBiorxivAsArxivPapers(from, today)
        : []
      const fetched = [...arxivPapers, ...biorxivPapers]
      const result = await invoke<ArxivPaper[]>('store_arxiv_papers', { papers: fetched, updateLastFetch: true })
      const knownRead = new Set(papers.value.filter(p => p.read).map(p => p.arxiv_id))
      papers.value = withRunOutcomes(result.map(p => ({ ...p, read: p.read || knownRead.has(p.arxiv_id) })))
      await loadScheduleStatus()
      // arXiv 开着但缺分类：bioRxiv 已正常抓取，仍要提示 arXiv 被跳过，避免用户误以为 arXiv 生效了。
      if (config.value.fetch_arxiv && !arxivReady)
        fetchMessage.value = '⚠ 已跳过 arXiv：未选择任何分类。请在设置中至少选择一个 arXiv 分类。'
    } catch (e) {
      fetchMessage.value = String(e)
    } finally {
      fetching.value = false
    }
  }

  async function fetchCatchUp() {
    if (fetching.value) return
    fetching.value = true
    fetchMessage.value = ''
    try {
      await loadConfig()
      if (!config.value.auto_fetch_enabled) return
      if (!config.value.fetch_arxiv && !config.value.fetch_biorxiv) return
      const today = new Date().toISOString().slice(0, 10)
      // arXiv announces papers with a lag: a paper submitted on day D only becomes
      // queryable via submittedDate:[D..] after that day's announcement cycle (next
      // day, longer over weekends). So a window that starts at last_fetch_date+1
      // collapses to "today", where arXiv has nothing yet — the fetch advances
      // last_fetch_date to today anyway (bioRxiv/store side-effect), and the lagged
      // papers fall permanently between windows. bioRxiv has no such lag, which is
      // why only bioRxiv appeared to auto-fetch. Always overlap back by days_back so
      // freshly-announced arXiv papers are caught; merge_into_inbox dedups by id so
      // re-scanning recent days is harmless.
      const lookbackStart = new Date(Date.now() - config.value.days_back * 86400000).toISOString().slice(0, 10)
      let dateFrom: string
      if (!config.value.last_fetch_date) {
        dateFrom = lookbackStart
      } else {
        const next = new Date(new Date(config.value.last_fetch_date).getTime() + 86400000).toISOString().slice(0, 10)
        if (next > today) return
        // Widen to whichever start is earlier: the day after last fetch (to fill a
        // long offline gap) or the days_back lookback (to absorb the announce lag).
        dateFrom = next < lookbackStart ? next : lookbackStart
      }
      const arxivReady = config.value.fetch_arxiv && config.value.categories.length > 0
      const arxivPapers = arxivReady
        ? await fetchArxivCategories(config.value, dateFrom, today)
        : []
      const biorxivPapers = config.value.fetch_biorxiv
        ? await fetchBiorxivAsArxivPapers(dateFrom, today)
        : []
      const fetched = [...arxivPapers, ...biorxivPapers]
      const result = await invoke<ArxivPaper[]>('store_arxiv_papers', { papers: fetched, updateLastFetch: true })
      const knownRead2 = new Set(papers.value.filter(p => p.read).map(p => p.arxiv_id))
      papers.value = withRunOutcomes(result.map(p => ({ ...p, read: p.read || knownRead2.has(p.arxiv_id) })))
      await loadScheduleStatus()
      // 自动抓取同样不静默跳过：arXiv 开着但没选分类时给出可见提示。
      if (config.value.fetch_arxiv && !arxivReady)
        fetchMessage.value = '⚠ 已跳过 arXiv 自动抓取：未选择任何分类。请在设置中至少选择一个 arXiv 分类。'
    } catch (e) {
      fetchMessage.value = String(e)
    } finally {
      fetching.value = false
    }
  }

  async function startAnalysis() {
    analyzing.value = true
    analyzeProgress.value = { done: 0, total: 0 }
    analyzeRetryingFailed.value = 0
    analyzeWaiting.value = null
    analysisNotice.value = null
    try {
      await invoke('start_arxiv_analysis')
    } catch (e) {
      analyzing.value = false
      analysisNotice.value = { kind: 'error', message: String(e) }
      throw e
    }
  }

  async function cancelAnalysis() {
    await invoke('cancel_arxiv_analysis')
  }

  async function setAutoFetch(enabled: boolean) {
    await invoke('set_arxiv_auto_fetch', { enabled })
    config.value.auto_fetch_enabled = enabled
    await loadScheduleStatus()
  }

  async function markRead(arxivId: string) {
    const p = papers.value.find(p => p.arxiv_id === arxivId)
    if (!p || p.read) return
    p.read = true
    try {
      await invoke('mark_arxiv_paper_read', { arxivId })
    } catch (e) {
      p.read = false  // rollback optimistic update
      console.error('mark_arxiv_paper_read failed:', e)
    }
  }

  async function ratePaper(arxivId: string, rating: number) {
    const p = papers.value.find(p => p.arxiv_id === arxivId)
    if (!p) return
    const prevRating = p.rating
    p.rating = rating
    try {
      await invoke('rate_arxiv_paper', { arxivId, rating })
    } catch (e) {
      p.rating = prevRating  // rollback optimistic update
      console.error('rate_arxiv_paper failed:', e)
    }
  }

  // Returns the imported paper's slug, or null when the user cancels a duplicate.
  async function addToLibrary(arxivId: string, collectionId?: string): Promise<string | null> {
    let res = await invoke<ImportOutcome>('add_arxiv_to_library', {
      arxivId,
      collectionId: collectionId ?? null,
    })
    if (res.status === 'duplicate') {
      const msg = i18n.global.t('import.duplicateConfirm').replace('{title}', res.title ?? arxivId)
      if (!window.confirm(msg)) return null   // canceled — leave recommendation in place
      res = await invoke<ImportOutcome>('add_arxiv_to_library', {
        arxivId,
        collectionId: collectionId ?? null,
        force: true,
      })
    }
    papers.value = papers.value.filter(p => p.arxiv_id !== arxivId)
    return res.slug ?? null
  }

  async function subscribeEvents() {
    if (unlistenFetch) { unlistenFetch(); unlistenFetch = null }
    if (unlistenAnalysis) { unlistenAnalysis(); unlistenAnalysis = null }
    if (unlistenRecommend) { unlistenRecommend(); unlistenRecommend = null }
    if (statusPollTimer) { clearInterval(statusPollTimer); statusPollTimer = null }

    unlistenFetch = await listen<{
      status: string; done: number; total: number; message?: string
    }>('arxiv-fetch', (e) => {
      const { status, done, total, message } = e.payload
      if (status === 'fetching') {
        fetching.value = true
        fetchMessage.value = message ?? ''
      } else if (status === 'done') {
        fetching.value = false
        fetchMessage.value = ''
        loadInbox().catch(() => {})
      }
      scheduleStatus.value = scheduleStatus.value
        ? { ...scheduleStatus.value, fetching: status === 'fetching' }
        : null
    })

    unlistenAnalysis = await listen<ArxivAnalysisEvent>('arxiv-analysis', (e) => {
      const ev = e.payload
      const {
        done, total, arxiv_id, status, bulk, score, reason, removed, message,
        key_contributions, analysis_summary, matched_topics,
      } = ev
      const isBulk = total > 1 || bulk === true

      if (isBulk) {
        if (total > 0 || status === 'started' || status === 'finished') {
          analyzeProgress.value = { done, total }
        }

        if (status === 'waiting') {
          const retryIn = Math.max(0, num(ev.retry_in))
          analyzeWaiting.value = {
            message: message ?? '',
            retryIn,
            concurrency: num(ev.concurrency),
            until: Date.now() + retryIn * 1000,
          }
        } else if (analyzeWaiting.value) {
          // Requests already in flight when the pause began may still land
          // (done/failed/filtered) during it; the batch is still paused, so keep
          // the countdown up until it runs out. Anything else — a new request
          // going out, the run ending — means the pause is over.
          const inFlightResult = status === 'done' || status === 'failed' || status === 'filtered'
          if (!inFlightResult || Date.now() >= analyzeWaiting.value.until) {
            analyzeWaiting.value = null
          }
        }

        if (status === 'started') {
          analysisNotice.value = null
          analyzeRetryingFailed.value = num(ev.retrying_failed)
          runOutcomes.clear()
        } else if (status === 'finished') {
          if (typeof ev.finished_at_ms === 'number') lastRunSeenAt = ev.finished_at_ms
          analysisNotice.value = {
            kind: 'finished',
            total: num(total),
            succeeded: num(ev.succeeded),
            failed: num(ev.failed),
            filtered: num(ev.filtered),
            reverted: num(ev.reverted),
            stoppedReason: ev.stopped_reason ? String(ev.stopped_reason) : null,
            cancelled: ev.cancelled === true,
            hasCounts: typeof ev.succeeded === 'number' || typeof ev.failed === 'number'
              || typeof ev.filtered === 'number',
          }
        } else if (status === 'error') {
          analysisNotice.value = { kind: 'error', message: message || '未知错误' }
        }

        if (status === 'finished' || status === 'error') {
          analyzing.value = false
          analyzeWaiting.value = null
          analyzeRetryingFailed.value = 0
          // Everything is on disk before 'finished' is sent.
          runOutcomes.clear()
          loadInbox().catch(() => {})
          loadScheduleStatus().catch(() => {})
        } else {
          analyzing.value = true
        }
      } else if (status === 'error' && arxiv_id) {
        // Single-paper request refused before anything was sent; the view that
        // flipped the paper to 'analyzing' puts it back.
        lastSingleError.value = { arxiv_id, message: message || '未知错误' }
      }

      // Remember the batch's results until they are surely on disk (see runOutcomes).
      if (arxiv_id && isBulk) {
        if (removed) {
          runOutcomes.set(arxiv_id, null)
        } else if (status === 'done') {
          runOutcomes.set(arxiv_id, {
            analysis_status: 'done',
            analysis_error: null,
            relevance_score: score ?? null,
            relevance_reason: reason ?? null,
            key_contributions: key_contributions ?? [],
            analysis_summary: analysis_summary ?? null,
            matched_topics: matched_topics ?? [],
          })
        } else if (status === 'failed') {
          runOutcomes.set(arxiv_id, message
            ? { analysis_status: 'failed', analysis_error: message }
            : { analysis_status: 'failed' })
        }
      }

      // Update individual paper status inline
      if (arxiv_id) {
        if (removed) {
          papers.value = papers.value.filter(p => p.arxiv_id !== arxiv_id)
          return
        }
        const p = papers.value.find(p => p.arxiv_id === arxiv_id)
        if (p) {
          if (status === 'done') {
            p.analysis_status = 'done'
            p.analysis_error = null
          } else if (status === 'analyzing') {
            p.analysis_status = 'analyzing'
          } else if (status === 'pending') {
            p.analysis_status = 'pending'
          } else if (status === 'failed') {
            p.analysis_status = 'failed'
            if (message) p.analysis_error = message
          }
          if (score !== undefined) p.relevance_score = score
          if (reason !== undefined) p.relevance_reason = reason
          if (key_contributions !== undefined) p.key_contributions = key_contributions
          if (analysis_summary !== undefined) p.analysis_summary = analysis_summary
          if (matched_topics !== undefined) p.matched_topics = matched_topics
        }
      }
    })

    unlistenRecommend = await listen<{ count: number }>('arxiv-new-recommendations', (e) => {
      newCount.value = e.payload.count
    })

    await loadScheduleStatus()
    statusPollTimer = setInterval(loadScheduleStatus, 2000)
  }

  function unsubscribeEvents() {
    if (unlistenFetch) { unlistenFetch(); unlistenFetch = null }
    if (unlistenAnalysis) { unlistenAnalysis(); unlistenAnalysis = null }
    if (unlistenRecommend) { unlistenRecommend(); unlistenRecommend = null }
    if (statusPollTimer) { clearInterval(statusPollTimer); statusPollTimer = null }
  }

  return {
    papers, config, scheduleStatus, loaded,
    fetching, refreshing, fetchMessage, analyzing, analyzeProgress,
    analyzeRetryingFailed, analyzeWaiting, analysisNotice, lastSingleError,
    sortMode, sortOrder, filterMode, newCount,
    sortedPapers,
    load, loadConfig, loadInbox, loadScheduleStatus,
    saveConfig, refreshInbox, fetchManual, fetchCatchUp,
    startAnalysis, cancelAnalysis, setAutoFetch,
    markRead, ratePaper, addToLibrary,
    subscribeEvents, unsubscribeEvents,
  }
})
