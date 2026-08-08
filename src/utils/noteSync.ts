import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'

// A note can be open in two places at once — the right-sidebar Notes tab and a
// standalone note window — and each keeps its own editor buffer, autosaving on a
// debounce. Without a channel between them the second writer silently overwrites
// the first: the sidebar would flush its stale buffer on tab switch or unmount
// and wipe everything typed in the window.
//
// So every successful save is broadcast, and the other side adopts it. Tauri's
// `emit` reaches every webview, the sender included, hence `origin`.

const NOTE_SAVED_EVENT = 'argus-note-saved'

export interface NoteSavedPayload {
  slug: string
  noteId: string
  content: string
  /** Who wrote it, so a sender can skip the echo of its own save. */
  origin: string
}

/** Stable per-webview id for `origin`. */
export const NOTE_SYNC_ORIGIN = `note-sync-${Math.random().toString(36).slice(2, 10)}`

export function broadcastNoteSaved(slug: string, noteId: string, content: string) {
  void emit(NOTE_SAVED_EVENT, {
    slug,
    noteId,
    content,
    origin: NOTE_SYNC_ORIGIN,
  } satisfies NoteSavedPayload).catch(() => {
    // Not running under Tauri (or the event bridge is down) — local editing is
    // unaffected, the two views just won't mirror each other.
  })
}

/** Subscribe to saves made elsewhere. The caller's own saves are filtered out. */
export async function onNoteSavedElsewhere(
  handler: (payload: NoteSavedPayload) => void,
): Promise<UnlistenFn> {
  return listen<NoteSavedPayload>(NOTE_SAVED_EVENT, (event) => {
    const payload = event.payload
    if (!payload || payload.origin === NOTE_SYNC_ORIGIN) return
    handler(payload)
  })
}
