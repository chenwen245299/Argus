import { SentenceSplitter } from 'llamaindex'
import type { PaperVectorizeInput, ChunkInput } from '../types'

let _splitter: SentenceSplitter | null = null

function getSplitter(chunkSize: number, chunkOverlap: number): SentenceSplitter {
  if (!_splitter || (_splitter as any).chunkSize !== chunkSize || (_splitter as any).chunkOverlap !== chunkOverlap) {
    _splitter = new SentenceSplitter({ chunkSize, chunkOverlap })
  }
  return _splitter
}

/**
 * Converts raw paper content (from get_paper_vectorize_input) into a flat
 * array of ChunkInput ready to send to embed_and_store_chunks.
 *
 * Chunking strategy:
 *  - metadata  → always one chunk (short, never split)
 *  - fulltext   → SentenceSplitter (respects sentence/paragraph boundaries)
 *  - highlights → one chunk per highlight (naturally short)
 *  - notes      → SentenceSplitter per note file
 */
export function buildChunks(
  input: PaperVectorizeInput,
  chunkSize: number,
  chunkOverlap: number,
): ChunkInput[] {
  const splitter = getSplitter(chunkSize, chunkOverlap)
  const chunks: ChunkInput[] = []

  // 1. Metadata – single chunk, no splitting
  if (input.meta_text.trim()) {
    chunks.push({
      text: input.meta_text,
      source_type: 'metadata',
      source_id: null,
      source_label: '论文基本信息',
    })
  }

  // 2. Fulltext – SentenceSplitter
  if (input.fulltext.trim()) {
    const texts = splitter.splitText(input.fulltext)
    for (const text of texts) {
      if (text.trim()) {
        chunks.push({ text, source_type: 'text', source_id: null, source_label: null })
      }
    }
  }

  // 3. Highlights – one chunk per highlight (with optional user note)
  for (const h of input.highlights) {
    let text = `高亮文本 (第${h.page}页): ${h.text}`
    if (h.note) text += `\n用户批注: ${h.note}`
    chunks.push({
      text,
      source_type: 'highlight',
      source_id: h.id,
      source_label: `第${h.page}页批注`,
    })
  }

  // 4. Notes – SentenceSplitter per note
  for (const note of input.notes) {
    const texts = splitter.splitText(note.content)
    texts.forEach((text, i) => {
      if (!text.trim()) return
      chunks.push({
        text,
        source_type: 'note',
        source_id: note.id,
        source_label: i === 0 ? `笔记: ${note.title}` : `笔记: ${note.title} (续${i + 1})`,
      })
    })
  }

  return chunks
}
