// What the composer can hang off a message, and how it becomes API content.
//
// Lifted out of AiTab / LibraryChat, which had two byte-identical copies of this
// logic — the sibling of `modelLogo.ts` and `providerLogo.ts`. Video arrived with
// MiniMax and is the reason the copies had to be reconciled rather than edited
// twice; audio arrived with StepFun's end-to-end speech models.

import type { ChatContentPart, ImageDetail } from '../types'

export interface Attachment {
  id: string
  type: 'image' | 'pdf' | 'video' | 'audio'
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
 * Video containers the providers that read video document for `video_url`.
 * Listed explicitly rather than as `video/*` so the picker does not invite a
 * format that will be refused after the file has already been read into memory.
 *
 * The intersection is narrower than it looks: MiniMax takes AVI, StepFun does
 * not (it documents MP4, QuickTime and Matroska). The list is the union, because
 * the picker cannot know which provider the message will go to — but MP4 is the
 * only container both accept, so it is what the hint recommends.
 */
const VIDEO_MIME = /^video\/(mp4|quicktime|x-matroska|x-msvideo|avi)$/i
const VIDEO_EXT = /\.(mp4|mov|mkv|avi)$/i

/**
 * Audio containers StepFun's `input_audio` block accepts — mp3 and wav, and
 * nothing else. The media type is the only signal the API gets (there is no
 * `format` field beside `data`), so a clip whose data URI says something else
 * would be refused server-side.
 */
const AUDIO_MIME = /^audio\/(mpeg|mp3|wav|x-wav|wave|vnd\.wave)$/i
const AUDIO_EXT = /\.(mp3|wav)$/i

export const ATTACHMENT_ACCEPT = 'image/*,.pdf,.mp4,.mov,.mkv,.avi,.mp3,.wav'

/**
 * Ceiling on an inline video, well under the 50 MB MiniMax allows and the 128 MB
 * StepFun allows.
 *
 * The binding constraint is not the API but this app: an attachment is base64'd
 * into the conversation, saved to the paper's conversations file on every edit,
 * and replayed in full on every round of an agent turn. A 50 MB clip becomes a
 * ~67 MB string doing all three. 15 MB is comfortably a short clip and keeps
 * those three costs survivable. Larger videos want a hosted URL or a provider's
 * Files API, neither of which this app uses.
 */
export const MAX_INLINE_VIDEO_BYTES = 15 * 1024 * 1024
export const MAX_INLINE_VIDEO_MB = Math.round(MAX_INLINE_VIDEO_BYTES / (1024 * 1024))

/**
 * Ceiling on an inline sound clip.
 *
 * Here the API is the binding constraint for once: StepFun caps a whole request
 * at 10 MB *after* base64, which is about 7.5 MB of wav. 6 MB of source leaves
 * room for the conversation around it — roughly a minute of 16-bit 44.1 kHz wav,
 * or the better part of an hour as mp3.
 */
export const MAX_INLINE_AUDIO_BYTES = 6 * 1024 * 1024
export const MAX_INLINE_AUDIO_MB = Math.round(MAX_INLINE_AUDIO_BYTES / (1024 * 1024))

/** The attachment kind for a picked file, or null when it is not one we take. */
export function attachmentTypeFor(file: File): Attachment['type'] | null {
  if (file.type.startsWith('image/')) return 'image'
  if (file.type === 'application/pdf') return 'pdf'
  // A file dragged in from some file managers arrives with an empty `type`, so
  // the extension is the fallback rather than the primary test.
  if (VIDEO_MIME.test(file.type) || (!file.type && VIDEO_EXT.test(file.name))) return 'video'
  if (AUDIO_MIME.test(file.type) || (!file.type && AUDIO_EXT.test(file.name))) return 'audio'
  return null
}

/** The size ceiling for one attachment kind, or null when it is not capped here. */
function limitFor(type: Attachment['type']): number | null {
  if (type === 'video') return MAX_INLINE_VIDEO_BYTES
  if (type === 'audio') return MAX_INLINE_AUDIO_BYTES
  return null
}

export type AttachmentRead =
  | { status: 'ok'; attachment: Attachment }
  | { status: 'too-large'; name: string; limitMb: number; type: Attachment['type'] }
  /** The file could not be read at all — moved, unreadable, or a browser error. */
  | { status: 'unreadable'; name: string }

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

  const limit = limitFor(type)
  if (limit !== null && file.size > limit) {
    return Promise.resolve({
      status: 'too-large',
      name: file.name || type,
      limitMb: Math.round(limit / (1024 * 1024)),
      type,
    })
  }

  return new Promise<AttachmentRead>((resolve) => {
    const reader = new FileReader()
    reader.onload = () => {
      const fallback =
        type === 'image' ? 'pasted-image.png'
        : type === 'video' ? 'clip.mp4'
        : type === 'audio' ? 'clip.wav'
        : 'pasted-file.pdf'
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
    // A read failure is not a size problem, and reporting it as one sends the
    // user off trimming a file that was never too big — an image or a PDF has no
    // size limit here at all, so the "limit" would have been invented.
    reader.onerror = () => resolve({ status: 'unreadable', name: file.name || 'file' })
    reader.readAsDataURL(file)
  })
}

/** One user turn's text plus its attachments, as API content parts. */
export function buildContentParts(text: string, atts?: Attachment[]): ChatContentPart[] {
  const parts: ChatContentPart[] = [{ type: 'text', text }]
  for (const att of atts ?? []) {
    // An exhaustive switch rather than a trailing `else`: the old shape sent an
    // unrecognised kind as a PDF `file` block, which is a well-formed request the
    // model simply cannot read — no error anywhere, just a clip the model never
    // heard. A kind added later now falls through to `default` and is dropped
    // loudly instead.
    switch (att.type) {
      case 'image':
        parts.push({
          type: 'image_url',
          image_url: att.detail ? { url: att.dataUrl, detail: att.detail } : { url: att.dataUrl },
        })
        break
      case 'video':
        parts.push({ type: 'video_url', video_url: { url: att.dataUrl } })
        break
      case 'audio':
        parts.push({ type: 'input_audio', input_audio: { data: att.dataUrl } })
        break
      case 'pdf':
        parts.push({ type: 'file', file: { filename: att.name, file_data: att.dataUrl } })
        break
      default: {
        // Exhaustiveness check: adding a kind to `Attachment['type']` without a
        // case here is a compile error rather than a silently dropped file.
        const unreachable: never = att.type
        console.warn('Unhandled attachment kind', unreachable)
      }
    }
  }
  return parts
}
