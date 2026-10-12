import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'
import type { PaperAiAnswer, PaperAiConversation } from '../types/paperAi'

export const PAPER_AI_SYNC_EVENT = 'argus-paper-ai-sync'
export const PAPER_AI_SAVED_EVENT = 'paper-ai-conversations-saved'
export const PAPER_AI_SYNC_INTERVAL_MS = 250

export interface PaperAiStreamInfo {
  answerId: string
}

interface Scope {
  root: string
  slug: string
  target?: string
}

type Content =
  | { kind: 'request'; conversationId?: string; select?: boolean }
  | { kind: 'snapshot'; conversation: PaperAiConversation; streams: PaperAiStreamInfo[]; select?: boolean }
  | { kind: 'update'; conversationId: string; title: string; updatedAt: string; answers: PaperAiAnswer[]; streams: PaperAiStreamInfo[] }
  | { kind: 'select'; conversationId: string }
  | { kind: 'stop'; conversationId: string }
  | { kind: 'closed' }

export type PaperAiSyncOutgoing = Scope & Content
export type PaperAiSyncMessage = PaperAiSyncOutgoing & { origin: string; revision: number; clock: number }
export interface PaperAiSavedMessage {
  root: string
  slug: string
  origin?: string
}

interface Transport {
  emit: (name: string, payload: PaperAiSyncMessage) => Promise<void>
  listen: (name: string, handler: (payload: PaperAiSyncMessage) => void) => Promise<UnlistenFn>
}

/** The two windows share events, while only the owner writes a live answer.
 *  Updates carry current answers, not the entire conversation on each token.
 *  A request/snapshot handshake lets a newly opened window join mid-answer. */
export class PaperAiSync {
  readonly origin: string
  private disposed = false
  private revision = 0
  private off: UnlistenFn | null = null
  private starting: Promise<void> | null = null
  private queued = new Map<string, { timer: ReturnType<typeof setTimeout>; make: () => PaperAiSyncOutgoing }>()
  private received = new Map<string, number>()

  constructor(
    private handler: (payload: PaperAiSyncMessage) => void,
    private transport: Transport = {
      emit,
      listen: (name, callback) => listen<PaperAiSyncMessage>(name, event => callback(event.payload)),
    },
    origin = crypto.randomUUID(),
  ) {
    this.origin = origin
  }

  start(): Promise<void> {
    if (!this.starting) this.starting = this.transport.listen(PAPER_AI_SYNC_EVENT, payload => {
      if (this.disposed || !payload || payload.origin === this.origin || (payload.target && payload.target !== this.origin)) return
      if (!payload.root || !payload.slug) return
      if (payload.kind === 'snapshot' || payload.kind === 'update') {
        const id = payload.kind === 'snapshot' ? payload.conversation.id : payload.conversationId
        const key = JSON.stringify([payload.origin, payload.root, payload.slug, id])
        if (payload.revision <= (this.received.get(key) ?? -1)) return
        this.received.set(key, payload.revision)
      }
      this.handler(payload)
    }).then(off => {
      if (this.disposed) off()
      else this.off = off
    }).catch(error => { this.starting = null; throw error })
    return this.starting
  }

  async send(payload: PaperAiSyncOutgoing) {
    if (this.disposed) return
    await this.transport.emit(PAPER_AI_SYNC_EVENT, { ...payload, origin: this.origin, revision: ++this.revision, clock: performance.now() }).catch(() => {})
  }

  queue(key: string, make: () => PaperAiSyncOutgoing) {
    if (this.disposed) return
    const pending = this.queued.get(key)
    if (pending) { pending.make = make; return }
    const timer = setTimeout(() => {
      const latest = this.queued.get(key)
      this.queued.delete(key)
      if (latest) void this.send(latest.make())
    }, PAPER_AI_SYNC_INTERVAL_MS)
    this.queued.set(key, { timer, make })
  }

  cancel(key: string) {
    const pending = this.queued.get(key)
    if (pending) clearTimeout(pending.timer)
    this.queued.delete(key)
  }

  dispose(scope?: Scope) {
    if (scope) void this.send({ ...scope, kind: 'closed' })
    this.disposed = true
    this.off?.()
    for (const pending of this.queued.values()) clearTimeout(pending.timer)
    this.queued.clear()
  }
}

/** performance.now() has a separate origin in each window. Translate both
 *  endpoints, preserving historical durations and live throughput estimates. */
export function translatePaperAiClocks(answers: PaperAiAnswer[], senderClock: number) {
  if (!Number.isFinite(senderClock)) return
  const shift = performance.now() - senderClock
  for (const answer of answers) {
    if (typeof answer.startedAt === 'number') answer.startedAt += shift
    if (typeof answer.endedAt === 'number') answer.endedAt += shift
  }
}

/** Reconcile disk state without replacing live objects or losing unsaved writes.
 *  An authoritative disk list also removes deleted, unprotected conversations. */
export function reconcilePaperAiConversations(
  current: readonly PaperAiConversation[],
  incoming: readonly PaperAiConversation[],
  protectedIds: ReadonlySet<string> = new Set(),
  authoritative = true,
): PaperAiConversation[] {
  const old = new Map(current.map(c => [c.id, c]))
  const next = new Map(incoming.map(c => {
    const existing = old.get(c.id)
    return [c.id, existing && (protectedIds.has(c.id) || existing.updatedAt > c.updatedAt) ? existing : c] as const
  }))
  for (const conv of current) {
    if (!next.has(conv.id) && (!authoritative || protectedIds.has(conv.id))) next.set(conv.id, conv)
  }
  return [...next.values()].sort((a, b) => b.updatedAt.localeCompare(a.updatedAt))
}
