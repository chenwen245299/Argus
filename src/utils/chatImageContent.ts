import { SEND_PAPER_IMAGE_TOOL, type AgentStepImage } from '../types/chatImages'

export interface ImageStep { tool: string; ok?: boolean; images?: AgentStepImage[] }
export type ImageContentPart = { type: 'text'; content: string } | { type: 'image'; image: AgentStepImage }

/** Resolve only images actually sent into this answer. Never turn a model's
 *  arbitrary file name or URL into a filesystem read. References stand alone
 *  between paragraphs so Markdown lists, maths and tables remain intact. */
export function chatImageContent(content: string, steps: ImageStep[] = []) {
  const images = steps.flatMap(step =>
    step.tool === SEND_PAPER_IMAGE_TOOL && step.ok === true ? step.images ?? [] : [],
  )
  const byFile = new Map(images.filter(image => image.file).map(image => [image.file!, image]))
  const used = new Set<string>()
  const parts: ImageContentPart[] = []
  let text = ''
  let fence: { char: string; length: number } | null = null
  for (const line of content.match(/[^\n]*\n|[^\n]+$/g) ?? []) {
    const trimmed = line.trimEnd()
    const boundary = /^ {0,3}(`{3,}|~{3,})(.*)$/.exec(trimmed)
    if (boundary) {
      if (!fence) fence = { char: boundary[1]![0]!, length: boundary[1]!.length }
      else if (boundary[1]![0] === fence.char && boundary[1]!.length >= fence.length && !boundary[2]!.trim()) fence = null
      text += line
      continue
    }
    const ref = !fence && /^ {0,3}!\[[^\]\r\n]*\]\(argus-image:\/\/([A-Za-z0-9._-]+\.png)\)[ \t]*$/.exec(trimmed)
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
