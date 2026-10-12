import type { Attachment } from '../utils/attachments'
import type { ServerToolTrace } from './index'
import type { AgentStepImage } from './chatImages'

export type PaperAiChatNode =
  | { id: string; role: 'user'; content: string; attachments?: Attachment[]; createdAt: string }
  | { id: string; role: 'assistantGroup'; promptId: string; answers: PaperAiAnswer[]; createdAt: string }

export interface PaperAiAnswer {
  id: string
  providerId: string
  providerName: string
  modelId: string
  modelName: string
  content: string
  /** Throttled render copies; never saved or sent to another window. */
  displayContent?: string
  reasoningContent?: string
  displayReasoning?: string
  withReasoning?: boolean
  createdAt: string
  startedAt?: number
  endedAt?: number
  streaming?: boolean
  error?: boolean
  errorText?: string
  tokenEstimate?: number
  inputTokens?: number
  outputTokens?: number
  totalTokens?: number
  cacheHitTokens?: number
  costUsd?: number | null
  source?: 'chat' | 'metadataExtraction'
  /** Live speech data; omitted from saved/synchronized records. */
  audioUrl?: string
  serverTools?: ServerToolTrace
  steps?: PaperAiAgentStep[]
  serverErrors?: string[]
  limitHit?: { rounds: number; max: number }
}

export interface PaperAiAgentStep {
  tool: string
  server?: string
  args: string
  argsJson?: string
  ok?: boolean
  chars?: number
  preview?: string
  truncated?: boolean
  /** Immutable page images; synchronized by filename, without dataUrl blobs. */
  images?: AgentStepImage[]
}

export interface PaperAiConversation {
  id: string
  slug: string
  title: string
  createdAt: string
  updatedAt: string
  nodes: PaperAiChatNode[]
  source?: 'chat' | 'metadataExtraction'
}
