import type { ServerToolTrace } from '../types'

interface AnswerDisplay {
  content: string
  streaming?: boolean
  reasoningContent?: string
  error?: boolean
  errorText?: string
  audioUrl?: string
  steps?: unknown[]
  agentSteps?: unknown[]
  serverTools?: ServerToolTrace
  serverErrors?: unknown[]
  agentServerErrors?: unknown[]
  limitHit?: unknown
  agentLimit?: unknown
  agentFallback?: unknown
  agentEvicted?: number
}

/** Keep pending replies and meaningful results, but hide an empty reply left
 * behind by Stop/Steer. This is a display decision: records and billing stay. */
export function hasVisibleChatAnswer(answer: AnswerDisplay): boolean {
  return !!(answer.streaming || answer.content.trim() || answer.reasoningContent?.trim()
    || answer.error || answer.errorText?.trim() || answer.audioUrl
    || answer.steps?.length || answer.agentSteps?.length
    || answer.serverErrors?.length || answer.agentServerErrors?.length
    || answer.limitHit || answer.agentLimit || answer.agentFallback || answer.agentEvicted
    || answer.serverTools?.citations?.length || answer.serverTools?.images?.length
    || Object.values(answer.serverTools?.calls ?? {}).some(count => count > 0))
}
