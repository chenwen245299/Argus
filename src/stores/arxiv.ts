import { defineStore } from 'pinia'
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { ArxivConfig, ArxivPaper, ArxivScheduleStatus } from '../types'
import { fetchArxivCategories } from '../utils/arxivFetch'
import { fetchBiorxivAsArxivPapers } from '../utils/biorxivFetch'

export type SortMode = 'score' | 'date' | 'status' | 'rating'
export type SortOrder = 'desc' | 'asc'
export type FilterMode = 'all' | 'unread' | 'pending_analysis'

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
      list = list.filter(p => p.analysis_status === 'pending' || p.analysis_status === 'failed')
    }
    if (sortMode.value === 'score') {
      list.sort((a, b) => (a.relevance_score ?? -1) - (b.relevance_score ?? -1))
    } else if (sortMode.value === 'date') {
      list.sort((a, b) => a.published.localeCompare(b.published))
    } else if (sortMode.value === 'rating') {
      list.sort((a, b) => (a.rating ?? 0) - (b.rating ?? 0))
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
      papers.value = inbox.papers.map(p => ({
        ...p,
        read: p.read || knownRead.has(p.arxiv_id),
      }))
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
      analyzing.value = false
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
      papers.value = inbox.papers
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
      const today = new Date().toISOString().slice(0, 10)
      const from = new Date(Date.now() - config.value.days_back * 86400000).toISOString().slice(0, 10)
      const arxivPapers = config.value.fetch_arxiv && config.value.categories.length > 0
        ? await fetchArxivCategories(config.value, from, today)
        : []
      const biorxivPapers = config.value.fetch_biorxiv
        ? await fetchBiorxivAsArxivPapers(from, today)
        : []
      const fetched = [...arxivPapers, ...biorxivPapers]
      const result = await invoke<ArxivPaper[]>('store_arxiv_papers', { papers: fetched, updateLastFetch: true })
      const knownRead = new Set(papers.value.filter(p => p.read).map(p => p.arxiv_id))
      papers.value = result.map(p => ({ ...p, read: p.read || knownRead.has(p.arxiv_id) }))
      await loadScheduleStatus()
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
      let dateFrom: string
      if (!config.value.last_fetch_date) {
        dateFrom = new Date(Date.now() - config.value.days_back * 86400000).toISOString().slice(0, 10)
      } else {
        const next = new Date(new Date(config.value.last_fetch_date).getTime() + 86400000).toISOString().slice(0, 10)
        if (next > today) return
        dateFrom = next
      }
      const arxivPapers = config.value.fetch_arxiv && config.value.categories.length > 0
        ? await fetchArxivCategories(config.value, dateFrom, today)
        : []
      const biorxivPapers = config.value.fetch_biorxiv
        ? await fetchBiorxivAsArxivPapers(dateFrom, today)
        : []
      const fetched = [...arxivPapers, ...biorxivPapers]
      const result = await invoke<ArxivPaper[]>('store_arxiv_papers', { papers: fetched, updateLastFetch: true })
      const knownRead2 = new Set(papers.value.filter(p => p.read).map(p => p.arxiv_id))
      papers.value = result.map(p => ({ ...p, read: p.read || knownRead2.has(p.arxiv_id) }))
      await loadScheduleStatus()
    } catch (e) {
      fetchMessage.value = String(e)
    } finally {
      fetching.value = false
    }
  }

  async function startAnalysis() {
    analyzing.value = true
    analyzeProgress.value = { done: 0, total: 0 }
    try {
      await invoke('start_arxiv_analysis')
    } catch (e) {
      analyzing.value = false
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

  async function addToLibrary(arxivId: string, collectionId?: string) {
    const slug = await invoke<string>('add_arxiv_to_library', {
      arxivId,
      collectionId: collectionId ?? null,
    })
    const p = papers.value.find(p => p.arxiv_id === arxivId)
    if (p) papers.value = papers.value.filter(p => p.arxiv_id !== arxivId)
    return slug
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
        loadInbox()
      }
      scheduleStatus.value = scheduleStatus.value
        ? { ...scheduleStatus.value, fetching: status === 'fetching' }
        : null
    })

    unlistenAnalysis = await listen<{
      done: number; total: number; arxiv_id: string; status: string;
      bulk?: boolean;
      score?: number; reason?: string; message?: string; removed?: boolean;
      key_contributions?: string[]; analysis_summary?: string | null; matched_topics?: string[]
    }>('arxiv-analysis', (e) => {
      const {
        done, total, arxiv_id, status, bulk, score, reason, removed,
        key_contributions, analysis_summary, matched_topics,
      } = e.payload
      const isBulk = total > 1 || bulk === true

      if (isBulk) {
        if (total > 0 || status === 'started' || status === 'finished') {
          analyzeProgress.value = { done, total }
        }

        if (status === 'finished' || status === 'error') {
          analyzing.value = false
          loadInbox()
          loadScheduleStatus()
        } else {
          analyzing.value = true
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
          p.analysis_status = status === 'done' ? 'done'
            : status === 'analyzing' ? 'analyzing'
            : status === 'failed' ? 'failed'
            : p.analysis_status
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
    sortMode, sortOrder, filterMode, newCount,
    sortedPapers,
    load, loadConfig, loadInbox, loadScheduleStatus,
    saveConfig, refreshInbox, fetchManual, fetchCatchUp,
    startAnalysis, cancelAnalysis, setAutoFetch,
    markRead, ratePaper, addToLibrary,
    subscribeEvents, unsubscribeEvents,
  }
})
