import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'

// Sending papers from the relation graph to the library chat ("智能问答").
//
// The two live in different webviews — the chat is its own window — so this goes
// over Tauri's event bus. It's a request/response pair rather than a fire-and-
// forget broadcast because the sender has to tell the outcomes apart:
//
//   • the chat is open         → it answers, and the papers are pinned to the
//                                conversation on screen
//   • the chat isn't open      → nobody answers, and the wait times out
//   • the chat acked but did   → `timeout`: it is open and may still pin them,
//     not answer in time         so the sender must not say "open the chat"
//
// `declined` is still part of the reply. The chat used to refuse when it was not
// on the old "文献库论文" knowledge source; pins are not a mode any more, so the
// current chat never declines, but a sender must still handle an older one.
//
// The chat acknowledges a request the moment it arrives, then answers once the
// papers are applied. The short timeout only has to cover the ack — it is what
// tells "not open" apart — while applying may take longer: a slug the chat does
// not know yet makes it re-read the paper list before deciding.

const REQUEST_EVENT = 'argus-chat-add-papers'
const RESULT_EVENT = 'argus-chat-add-papers-result'
const ACK_EVENT = 'argus-chat-add-papers-ack'

/** How long to wait for the result once the chat has acknowledged the request. */
const RESULT_TIMEOUT_MS = 10_000

interface AddPapersAck {
  requestId: string
}

interface AddPapersRequest {
  requestId: string
  slugs: string[]
}

export interface AddPapersResult {
  requestId: string
  /** Set when the chat refused the papers. The current chat never does. */
  declined?: boolean
  added: number
  alreadyPresent: number
  /** New papers left out because the conversation hit its pin limit. */
  overLimit?: number
  /** Papers the chat could not find in the library, even after re-reading it. */
  unknown?: number
}

/** The counts a chat window reports back after applying a request. */
export interface AddPapersApplied {
  added: number
  alreadyPresent: number
  overLimit?: number
  unknown?: number
}

/** What the caller should tell the user. */
export type AddPapersOutcome =
  | { status: 'added'; added: number; alreadyPresent: number; overLimit: number; unknown: number }
  | { status: 'declined' }     // chat open, but refused (older chat windows only)
  | { status: 'unavailable' }  // chat not open
  | { status: 'timeout' }      // chat open (it acked), but no result in time

/**
 * Ask an open chat window to add these papers. Resolves once it answers, with
 * `unavailable` if nothing acknowledges the request within `timeoutMs`, or with
 * `timeout` if the chat acknowledged it but sent no result within
 * `RESULT_TIMEOUT_MS`.
 */
export async function requestAddPapersToChat(
  slugs: string[],
  timeoutMs = 700,
): Promise<AddPapersOutcome> {
  if (!slugs.length) return { status: 'added', added: 0, alreadyPresent: 0, overLimit: 0, unknown: 0 }
  const requestId = `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`

  return new Promise<AddPapersOutcome>((resolve) => {
    let settled = false
    const unlisteners: UnlistenFn[] = []
    let timer: ReturnType<typeof setTimeout> | null = null

    const finish = (outcome: AddPapersOutcome) => {
      if (settled) return
      settled = true
      if (timer) clearTimeout(timer)
      for (const off of unlisteners.splice(0)) off()
      resolve(outcome)
    }
    const keep = (off: UnlistenFn) => {
      // The listener may resolve after a fast reply already settled us.
      if (settled) off()
      else unlisteners.push(off)
    }

    timer = setTimeout(() => finish({ status: 'unavailable' }), timeoutMs)

    const onResult = listen<AddPapersResult>(RESULT_EVENT, (event) => {
      const payload = event.payload
      // Another send could be in flight; only our own reply counts.
      if (!payload || payload.requestId !== requestId) return
      finish(payload.declined
        ? { status: 'declined' }
        : {
            status: 'added',
            added: payload.added,
            alreadyPresent: payload.alreadyPresent,
            overLimit: payload.overLimit ?? 0,
            unknown: payload.unknown ?? 0,
          })
    }).then(keep)

    const onAck = listen<AddPapersAck>(ACK_EVENT, (event) => {
      if (settled || event.payload?.requestId !== requestId) return
      // The chat is open and working on it: stop treating silence as "not open".
      // Silence from here on means "still busy" — it may yet pin the papers.
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => finish({ status: 'timeout' }), RESULT_TIMEOUT_MS)
    }).then(keep)

    // Send only once both listeners are registered, so neither reply can
    // arrive before anything is listening for it.
    Promise.all([onResult, onAck])
      .then(() => {
        if (settled) return
        return emit(REQUEST_EVENT, { requestId, slugs } satisfies AddPapersRequest)
      })
      .catch(() => finish({ status: 'unavailable' }))
  })
}

/**
 * Handle those requests in the chat window. `handler` returns (or resolves to)
 * the counts it applied, or null to decline.
 */
export async function serveAddPapersToChat(
  handler: (slugs: string[]) => AddPapersApplied | null | Promise<AddPapersApplied | null>,
): Promise<UnlistenFn> {
  return listen<AddPapersRequest>(REQUEST_EVENT, async (event) => {
    const payload = event.payload
    if (!payload?.requestId || !Array.isArray(payload.slugs)) return
    void emit(ACK_EVENT, { requestId: payload.requestId } satisfies AddPapersAck).catch(() => {})
    let applied: AddPapersApplied | null
    try {
      applied = await handler(payload.slugs)
    } catch {
      // Reported as a refusal: the sender then says "could not pin, reopen the
      // chat", which is true, rather than waiting out the result timeout and
      // claiming the chat is not open.
      applied = null
    }
    void emit(RESULT_EVENT, {
      requestId: payload.requestId,
      declined: applied === null ? true : undefined,
      added: applied?.added ?? 0,
      alreadyPresent: applied?.alreadyPresent ?? 0,
      overLimit: applied?.overLimit || undefined,
      unknown: applied?.unknown || undefined,
    } satisfies AddPapersResult)
  })
}
