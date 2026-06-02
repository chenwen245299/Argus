import { defineStore } from 'pinia'
import { ref } from 'vue'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'

export type AiMetaStage = 'text' | 'ai'

export type AiSummaryStage =
  | 'queued'
  | 'fulltext'
  | 'text'
  | 'fulltext_ready'
  | 'preparing'
  | 'ai'
  | 'saving'
  | 'done'
  | 'error'

export type PaperTaskKind = 'summary' | 'extract'

export interface AiSummaryJob {
  kind: PaperTaskKind
  stage: AiSummaryStage
  generatedChars: number
  fulltextChars?: number
  contextChars?: number
  provider?: string
  model?: string
  elapsedSeconds?: number
  truncated?: boolean
  message?: string
}

export interface AiSummaryProgressEvent {
  slug: string
  stage: AiSummaryStage
  fulltext_chars?: number
  context_chars?: number
  provider?: string
  model?: string
  elapsed_seconds?: number
  truncated?: boolean
}

export const usePaperTasksStore = defineStore('paperTasks', () => {
  const aiSummaryJobs = ref<Record<string, AiSummaryJob>>({})
  const aiMetaSlug = ref<string | null>(null)
  const aiMetaStage = ref<AiMetaStage | null>(null)
  const abstractSlug = ref<string | null>(null)

  let progressUnlisten: UnlistenFn | null = null
  let progressListening = false

  function setAiSummaryJob(slug: string, patch: Partial<AiSummaryJob>) {
    const existing = aiSummaryJobs.value[slug]
    aiSummaryJobs.value = {
      ...aiSummaryJobs.value,
      [slug]: {
        ...existing,
        ...patch,
        kind: patch.kind ?? existing?.kind ?? 'summary',
        stage: patch.stage ?? existing?.stage ?? 'queued',
        generatedChars: patch.generatedChars ?? existing?.generatedChars ?? 0,
      },
    }
  }

  function removeAiSummaryJob(slug: string) {
    const { [slug]: _removed, ...rest } = aiSummaryJobs.value
    aiSummaryJobs.value = rest
  }

  function isAiSummaryActive(slug: string) {
    const stage = aiSummaryJobs.value[slug]?.stage
    return !!stage && stage !== 'done' && stage !== 'error'
  }

  function setAiMetaTask(slug: string, stage: AiMetaStage | null = null) {
    aiMetaSlug.value = slug
    aiMetaStage.value = stage
  }

  function clearAiMetaTask() {
    aiMetaSlug.value = null
    aiMetaStage.value = null
  }

  function setAbstractTask(slug: string | null) {
    abstractSlug.value = slug
  }

  async function startListening() {
    if (progressListening) return
    progressListening = true
    progressUnlisten = await listen<AiSummaryProgressEvent>('ai-summary-progress', (ev) => {
      const payload = ev.payload
      if (!payload?.slug) return
      const existing = aiSummaryJobs.value[payload.slug]
      if (!existing && (payload.stage === 'done' || payload.stage === 'error')) return
      setAiSummaryJob(payload.slug, {
        stage: payload.stage,
        fulltextChars: payload.fulltext_chars ?? existing?.fulltextChars,
        contextChars: payload.context_chars ?? existing?.contextChars,
        provider: payload.provider ?? existing?.provider,
        model: payload.model ?? existing?.model,
        elapsedSeconds: payload.elapsed_seconds ?? existing?.elapsedSeconds,
        truncated: payload.truncated ?? existing?.truncated,
      })
    })
  }

  function stopListening() {
    progressUnlisten?.()
    progressUnlisten = null
    progressListening = false
  }

  return {
    aiSummaryJobs,
    aiMetaSlug,
    aiMetaStage,
    abstractSlug,
    setAiSummaryJob,
    removeAiSummaryJob,
    isAiSummaryActive,
    setAiMetaTask,
    clearAiMetaTask,
    setAbstractTask,
    startListening,
    stopListening,
  }
})
