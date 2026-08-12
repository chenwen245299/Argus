import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'

// Sending papers from the relation graph to the library chat ("智能问答").
//
// The two live in different webviews — the chat is its own window — so this goes
// over Tauri's event bus. It's a request/response pair rather than a fire-and-
// forget broadcast because the sender has to tell three outcomes apart:
//
//   • the chat is open and on 文献库论文  → it answers, papers are added
//   • the chat is open on another source  → it answers with `declined`
//   • the chat isn't open at all          → nobody answers, and the wait times out
//
// The last two both need to prompt the user, but with different wording, and a
// plain broadcast could not distinguish them from success.

const REQUEST_EVENT = 'argus-chat-add-papers'
const RESULT_EVENT = 'argus-chat-add-papers-result'

interface AddPapersRequest {
  requestId: string
  slugs: string[]
}

export interface AddPapersResult {
  requestId: string
  /** Set when the chat is open but not on the 文献库论文 knowledge source. */
  declined?: boolean
  added: number
  alreadyPresent: number
}

/** What the caller should tell the user. */
export type AddPapersOutcome =
  | { status: 'added'; added: number; alreadyPresent: number }
  | { status: 'declined' }     // chat open, wrong knowledge source
  | { status: 'unavailable' }  // chat not open

/**
 * Ask an open chat window to add these papers. Resolves once it answers, or
 * after `timeoutMs` if nothing does.
 */
export async function requestAddPapersToChat(
  slugs: string[],
  timeoutMs = 700,
): Promise<AddPapersOutcome> {
  if (!slugs.length) return { status: 'added', added: 0, alreadyPresent: 0 }
  const requestId = `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`

  return new Promise<AddPapersOutcome>((resolve) => {
    let settled = false
    let unlisten: UnlistenFn | null = null
    let timer: ReturnType<typeof setTimeout> | null = null

    const finish = (outcome: AddPapersOutcome) => {
      if (settled) return
      settled = true
      if (timer) clearTimeout(timer)
      unlisten?.()
      resolve(outcome)
    }

    listen<AddPapersResult>(RESULT_EVENT, (event) => {
      const payload = event.payload
      // Another send could be in flight; only our own reply counts.
      if (!payload || payload.requestId !== requestId) return
      finish(payload.declined
        ? { status: 'declined' }
        : { status: 'added', added: payload.added, alreadyPresent: payload.alreadyPresent })
    }).then((off) => {
      // The listener may resolve after a fast reply already settled us.
      if (settled) off()
      else unlisten = off
    })

    timer = setTimeout(() => finish({ status: 'unavailable' }), timeoutMs)
    void emit(REQUEST_EVENT, { requestId, slugs } satisfies AddPapersRequest)
      .catch(() => finish({ status: 'unavailable' }))
  })
}

/**
 * Handle those requests in the chat window. `handler` returns the counts it
 * applied, or null to decline (it isn't on the 文献库论文 source).
 */
export async function serveAddPapersToChat(
  handler: (slugs: string[]) => { added: number; alreadyPresent: number } | null,
): Promise<UnlistenFn> {
  return listen<AddPapersRequest>(REQUEST_EVENT, (event) => {
    const payload = event.payload
    if (!payload?.requestId || !Array.isArray(payload.slugs)) return
    const applied = handler(payload.slugs)
    void emit(RESULT_EVENT, {
      requestId: payload.requestId,
      declined: applied === null ? true : undefined,
      added: applied?.added ?? 0,
      alreadyPresent: applied?.alreadyPresent ?? 0,
    } satisfies AddPapersResult)
  })
}
