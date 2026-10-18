import type { ChatMessage } from '../types'
import type { Attachment } from './attachments'

export interface ContextUsage {
  usedTokens: number
  contextWindow: number
  remainingTokens: number
  systemTokens: number
  toolsTokens: number
  messageTokens: number
  reservedTokens: number
  thresholdTokens: number
  estimated: boolean
  capacityEstimated: boolean
  mediaEstimated: boolean
  canCompact: boolean
  providerId: string
  modelId: string
  systemFingerprint?: string
}

export interface ContextCheckpoint {
  throughMessageId: string
  fingerprint: string
  messages: ChatMessage[]
  createdAt: string
}
export interface PreparedContext {
  messages: ChatMessage[]
  usage: ContextUsage
  compacted: boolean
  summary?: string | null
}

/** Checkpoints are invalidated by editing/deleting history or selecting another
 * answer variant. Hash all model-visible fields, including attachments/results. */
export function historyFingerprint(messages: ChatMessage[]): string {
  const text = JSON.stringify(messages)
  let a = 2166136261, b = 5381
  for (let i = 0; i < text.length; i++) {
    a = Math.imul(a ^ text.charCodeAt(i), 16777619)
    b = Math.imul(b, 33) ^ text.charCodeAt(i)
  }
  return `${text.length}:${a >>> 0}:${b >>> 0}`
}

export function estimateContextText(text: string): number {
  let ascii = 0, wide = 0
  for (const char of text) char.charCodeAt(0) < 128 ? ascii++ : wide++
  return wide + Math.ceil(ascii / 4)
}

export function contextPercent(usage: ContextUsage): number {
  if (!Number.isFinite(usage.contextWindow) || usage.contextWindow <= 0) return 0
  return Math.max(0, Math.min(100, Math.round(usage.usedTokens / usage.contextWindow * 100)))
}

/** Match backend media estimates without scanning inline base64 blobs. */
export function withContextDraft(usage: ContextUsage, text: string, attachments: Attachment[] = []): ContextUsage {
  const media = attachments.reduce((n, attachment) => n + (attachment.type === 'image'
    ? attachment.detail === 'low' ? 85 : 2048 : attachment.type === 'pdf' ? 4096 : 8192), 0)
  const draft = text || attachments.length ? estimateContextText(text) + 4 + (attachments.length ? 4 : 0) + media : 0
  const usedTokens = usage.usedTokens + draft
  return { ...usage, usedTokens, remainingTokens: Math.max(0, usage.contextWindow - usedTokens),
    messageTokens: usage.messageTokens + draft, estimated: usage.estimated || draft > 0,
    mediaEstimated: usage.mediaEstimated || attachments.length > 0 }
}

export function applyContextCheckpoint<T extends { id: string }>(
  records: T[], checkpoint: ContextCheckpoint | undefined,
  history: (records: T[]) => ChatMessage[],
  fingerprintHistory: (records: T[]) => ChatMessage[] = history,
): ChatMessage[] {
  if (checkpoint) {
    const index = records.findIndex(m => m.id === checkpoint.throughMessageId)
    if (index >= 0 && historyFingerprint(fingerprintHistory(records.slice(0, index + 1))) === checkpoint.fingerprint) {
      return [...checkpoint.messages, ...history(records.slice(index + 1))]
    }
  }
  return history(records)
}
