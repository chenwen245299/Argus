// What the composer can hang off a message, and how it becomes API content.
//
// Lifted out of AiTab / LibraryChat, which had two byte-identical copies of this
// logic — the sibling of `modelLogo.ts` and `providerLogo.ts`. Video arrived with
// MiniMax and is the reason the copies had to be reconciled rather than edited
// twice.

import type { ChatContentPart, ImageDetail } from '../types'

export interface Attachment {
  id: string
  type: 'image' | 'pdf' | 'video'
  name: string
  dataUrl: string
  /**
   * DeepSeek image fidelity. Unset means full resolution; `low` rescales to
   * 512x512, which costs roughly a third of the tokens. Other providers ignore
   * it, so the field is only ever sent when the user picked it.
   */
  detail?: ImageDetail
}

/**
 * Video containers MiniMax documents for `video_url`. Listed explicitly rather
 * than as `video/*` so the picker does not invite a format that will be refused
 * after the file has already been read into memory.
 */
const VIDEO_MIME = /^video\/(mp4|quicktime|x-matroska|x-msvideo|avi)$/i
const VIDEO_EXT = /\.(mp4|mov|mkv|avi)$/i

export const ATTACHMENT_ACCEPT = 'image/*,.pdf,.mp4,.mov,.mkv,.avi'

/**
 * Ceiling on an inline video, well under MiniMax's own 50 MB.
 *
 * The binding constraint is not the API but this app: an attachment is base64'd
 * into the conversation, saved to the paper's conversations file on every edit,
 * and replayed in full on every round of an agent turn. A 50 MB clip becomes a
 * ~67 MB string doing all three. 15 MB is comfortably a short clip and keeps
 * those three costs survivable. Larger videos want MiniMax's Files API, which
 * this app does not use.
 */
export const MAX_INLINE_VIDEO_BYTES = 15 * 1024 * 1024
export const MAX_INLINE_VIDEO_MB = Math.round(MAX_INLINE_VIDEO_BYTES / (1024 * 1024))

/** The attachment kind for a picked file, or null when it is not one we take. */
export function attachmentTypeFor(file: File): Attachment['type'] | null {
  if (file.type.startsWith('image/')) return 'image'
  if (file.type === 'application/pdf') return 'pdf'
  // A file dragged in from some file managers arrives with an empty `type`, so
  // the extension is the fallback rather than the primary test.
  if (VIDEO_MIME.test(file.type) || (!file.type && VIDEO_EXT.test(file.name))) return 'video'
  return null
}

export type AttachmentRead =
  | { status: 'ok'; attachment: Attachment }
  | { status: 'too-large'; name: string; limitMb: number }

/**
 * Read one picked file into an attachment.
 *
 * Returns null *synchronously* for a file type the composer does not take, so a
 * paste handler can tell straight away whether it consumed the event; anything
 * we do take comes back as a promise.
 */
export function readAttachmentFile(file: File): Promise<AttachmentRead> | null {
  const type = attachmentTypeFor(file)
  if (!type) return null

  if (type === 'video' && file.size > MAX_INLINE_VIDEO_BYTES) {
    return Promise.resolve({
      status: 'too-large',
      name: file.name || 'video',
      limitMb: MAX_INLINE_VIDEO_MB,
    })
  }

  return new Promise<AttachmentRead>((resolve) => {
    const reader = new FileReader()
    reader.onload = () => {
      const fallback = type === 'image' ? 'pasted-image.png' : type === 'video' ? 'clip.mp4' : 'pasted-file.pdf'
      resolve({
        status: 'ok',
        attachment: {
          id: crypto.randomUUID(),
          type,
          name: file.name || fallback,
          dataUrl: reader.result as string,
        },
      })
    }
    reader.onerror = () => resolve({ status: 'too-large', name: file.name || 'file', limitMb: MAX_INLINE_VIDEO_MB })
    reader.readAsDataURL(file)
  })
}

/** One user turn's text plus its attachments, as API content parts. */
export function buildContentParts(text: string, atts?: Attachment[]): ChatContentPart[] {
  const parts: ChatContentPart[] = [{ type: 'text', text }]
  for (const att of atts ?? []) {
    if (att.type === 'image') {
      parts.push({
        type: 'image_url',
        image_url: att.detail ? { url: att.dataUrl, detail: att.detail } : { url: att.dataUrl },
      })
    } else if (att.type === 'video') {
      parts.push({ type: 'video_url', video_url: { url: att.dataUrl } })
    } else {
      parts.push({ type: 'file', file: { filename: att.name, file_data: att.dataUrl } })
    }
  }
  return parts
}
