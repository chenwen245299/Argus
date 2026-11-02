import { SEND_PAPER_IMAGE_TOOL, type AgentStepImage } from '../types/chatImages'

export interface ImageStep { tool: string; ok?: boolean; images?: AgentStepImage[] }
export type ImageContentPart = { type: 'text'; content: string } | { type: 'image'; image: AgentStepImage }
  | { type: 'imageGrid'; columns: number; rows: number; cells: (AgentStepImage | null)[] }

const IMAGE_REFERENCE = /^!\[[^\]\r\n]*\]\(argus-image:\/\/([A-Za-z0-9._-]+\.png)\)$/

/** Recognize image-only Markdown tables, resolving only this answer's files. */
function imageTable(lines: string[], start: number, byFile: Map<string, AgentStepImage>) {
  const row = (line = '') => {
    const match = /^ {0,3}\|(.*)\|[ \t]*$/.exec(line.trimEnd())
    return match ? match[1]!.split(/(?<!\\)\|/).map(cell => cell.trim()) : null
  }
  const header = row(lines[start]), separator = row(lines[start + 1])
  if (!header?.length || header.length > 4 || separator?.length !== header.length
    || !separator.every(cell => /^:?-{3,}:?$/.test(cell))) return null
  const rows = [header]
  let end = start + 2
  while (end < lines.length && /^ {0,3}\|/.test(lines[end]!)) {
    const cells = row(lines[end])
    if (!cells || cells.length !== header.length) return null
    rows.push(cells)
    end++
  }
  if (rows.length * header.length > 8) return null
  const cells: (AgentStepImage | null)[] = []
  for (const value of rows.flat()) {
    if (!value) { cells.push(null); continue }
    const ref = IMAGE_REFERENCE.exec(value)
    const image = ref ? byFile.get(ref[1]!) : undefined
    if (!image) return null
    cells.push(image)
  }
  if (!cells.some(Boolean)) return null
  return { end, columns: header.length, rows: rows.length, cells }
}

/** Resolve only images actually sent into this answer. Never turn arbitrary
 * file names into reads. Standalone references and image-only tables preserve
 * the surrounding Markdown. */
export function chatImageContent(content: string, steps: ImageStep[] = []) {
  const images = steps.flatMap(step =>
    step.tool === SEND_PAPER_IMAGE_TOOL && step.ok === true ? step.images ?? [] : [],
  )
  const byFile = new Map(images.filter(image => image.file).map(image => [image.file!, image]))
  const used = new Set<string>()
  const parts: ImageContentPart[] = []
  let text = ''
  let fence: { char: string; length: number } | null = null
  const lines = content.match(/[^\n]*\n|[^\n]+$/g) ?? []
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index]!
    const trimmed = line.trimEnd()
    const boundary = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(trimmed)
    if (boundary) {
      if (!fence) fence = { char: boundary[1]![0]!, length: boundary[1]!.length }
      else if (boundary[1]![0] === fence.char && boundary[1]!.length >= fence.length && !boundary[2]!.trim()) fence = null
      text += line
      continue
    }
    const grid = !fence ? imageTable(lines, index, byFile) : null
    if (grid) {
      if (text) parts.push({ type: 'text', content: text })
      text = ''
      parts.push({ type: 'imageGrid', columns: grid.columns, rows: grid.rows, cells: grid.cells })
      grid.cells.forEach(image => { if (image?.file) used.add(image.file) })
      index = grid.end - 1
      continue
    }
    const ref = !fence && /^ {0,3}\S/.test(trimmed) && IMAGE_REFERENCE.exec(trimmed.trim())
    const image = ref ? byFile.get(ref[1]!) : undefined
    if (!image) {
      text += line
      continue
    }
    if (text) parts.push({ type: 'text', content: text })
    text = ''
    parts.push({ type: 'image', image })
    used.add(image.file!)
  }
  if (text) parts.push({ type: 'text', content: text })
  return { parts, remaining: images.filter(image => !image.file || !used.has(image.file)) }
}
