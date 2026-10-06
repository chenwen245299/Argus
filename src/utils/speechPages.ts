import { chunkForSpeech, prepareSpeechText } from './speechText'
import { readFraction, seekTarget } from './speechFollow'

export interface SpeechPageOptions {
  startPage: number
  endPage: number
  getText: (page: number) => Promise<string>
  maxChars: number
  skipCitations: boolean
  cancelled: () => boolean
}

/** Local text is read a page at a time, only when playback needs another batch. */
export class SpeechPages {
  readonly chunks: string[] = []
  readonly pages: { page: number; start: number; end: number }[] = []
  private cursor: number

  constructor(readonly options: SpeechPageOptions) {
    this.cursor = options.startPage
  }

  async next(): Promise<string[] | null> {
    while (this.cursor <= this.options.endPage && !this.options.cancelled()) {
      const page = this.cursor++
      const raw = await this.options.getText(page)
      if (this.options.cancelled()) return null
      const text = prepareSpeechText(raw, { skipCitations: this.options.skipCitations })
      if (!text) continue // Blank/image-only pages never reach the speech API.
      const chunks = chunkForSpeech(text, {
        maxChars: this.options.maxChars,
        // Only the very first clip is small; page breaks do not reset the warm pipeline.
        firstChars: this.chunks.length ? 900 : 200,
      })
      const start = this.chunks.length
      this.chunks.push(...chunks)
      this.pages.push({ page, start, end: this.chunks.length })
      return chunks
    }
    return null
  }

  position(index: number, fraction: number): { page: number; fraction: number } {
    const span = this.pages.find(p => index >= p.start && index < p.end)
    return span
      ? { page: span.page, fraction: readFraction(this.chunks.slice(span.start, span.end), index - span.start, fraction) }
      : { page: this.options.startPage, fraction: 0 }
  }

  /** Seeking within an already prepared page reuses its chunks and cached audio. */
  target(page: number, fraction: number): { index: number; fraction: number } | null {
    const span = this.pages.find(p => p.page === page)
    if (!span) return null
    const target = seekTarget(this.chunks.slice(span.start, span.end), fraction)
    return target ? { index: target.index + span.start, fraction: target.fraction } : null
  }
}

/** Preserve PDF line ends and separate text runs so neighbouring words do not merge. */
export function pdfSpeechText(items: readonly unknown[]): string {
  return items.map(item => {
    if (!item || typeof item !== 'object' || !('str' in item) || typeof item.str !== 'string') return ''
    return item.str + ('hasEOL' in item && item.hasEOL ? '\n' : ' ')
  }).join('')
}
