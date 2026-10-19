import type { ChatMessage, ChatContentPart } from '../types'
import type { AgentStepImage } from '../types/chatImages'

// One tool call from a chat answer's agent trail — only the fields needed to
// replay it into a later turn's context. Both AiTab's `AgentStep` and
// LibraryChat's `AgentStep` are assignable to this.
export interface ReplayStep {
  tool: string
  /** Pretty-printed JSON arguments the model called the tool with. */
  argsJson?: string
  /** Compact human summary of the arguments; fallback when `argsJson` is absent. */
  args?: string
  /** undefined while running, then whether the call succeeded. */
  ok?: boolean
  /** The result the model got back (as persisted; not further truncated here). */
  preview?: string
  chars?: number
  previewClipped?: boolean
  images?: AgentStepImage[]
}

/**
 * Rebuild one assistant turn's tool activity as an OpenAI-style tool exchange:
 * a single assistant message carrying every `tool_call` the turn made, followed
 * by one `tool` message per call with its result. The caller splices this in
 * *before* that turn's final assistant answer.
 *
 * The default history a follow-up turn sends carries only the user text and the
 * final assistant answer; the tool calls and their results are dropped, so the
 * model re-fetches what it already had. Replaying the real calls and their
 * **full** results — as native tool messages the model is trained to read —
 * lets it reuse them instead of calling the same tool with the same arguments
 * again. Results are bounded only by the live model context budget. Persisted
 * text stays complete; saved images travel as references resolved before HTTP.
 *
 * Tool-call ids are synthesised as `call_<answerId>_<i>` and paired between the
 * assistant turn and its results; providers only require the pairing to line up
 * within the request, which it does, and `answerId` is unique per answer so the
 * ids stay unique across turns too.
 *
 * Returns [] when the turn made no tool calls.
 */
export function buildToolExchangeMessages(
  steps: ReplayStep[] | undefined,
  answerId: string,
  conversationId?: string,
): ChatMessage[] {
  if (!steps?.length) return []

  const calls: { id: string; name: string; args: string }[] = []
  const results: ChatMessage[] = []
  const images: ChatContentPart[] = []

  steps.forEach((step, i) => {
    if (!step.tool) return
    // A steer can interrupt a tool before it produced a result. Only completed
    // exchanges may enter the continuation; an empty success would mislead it.
    if (step.ok === undefined && !step.preview?.trim()) return
    const id = `call_${answerId}_${i}`
    // `arguments` must be a JSON string; argsJson already is one.
    const args = (step.argsJson ?? '').trim() || '{}'
    calls.push({ id, name: step.tool, args })

    // The full result, verbatim — a follow-up turn should see exactly what the
    // model already fetched so it can reuse it rather than re-calling the tool.
    let content = step.preview ?? ''
    if (!content && step.ok === false) content = '{"error":"tool call failed"}'
    if (step.previewClipped || (typeof step.chars === 'number' && content.length < step.chars)) {
      content += '\n[This legacy saved result is incomplete; read the missing source again rather than assuming it is complete.]'
    }
    if (step.ok !== false) for (const image of step.images ?? []) {
      let url = image.dataUrl
      if (image.file && conversationId) {
        const bytes = new TextEncoder().encode(JSON.stringify({ conversationId, file: image.file }))
        const encoded = btoa(Array.from(bytes, byte => String.fromCharCode(byte)).join(''))
          .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
        url = 'argus-chat-image:' + encoded
      }
      if (!url) continue
      images.push({ type: 'text', text: `Previously inspected paper '${image.slug}', PDF page index ${image.page}; ${image.crop ? 'CROP from full-page fractions ' + JSON.stringify(image.crop) : 'FULL PAGE'}. ${image.caption ?? ''} Inspect the actual figure, axes, legend and caption.` })
      images.push({ type: 'image_url', image_url: { url } })
    }
    results.push({ role: 'tool', tool_call_id: id, content })
  })

  if (!calls.length) return []

  const assistant: ChatMessage = {
    role: 'assistant',
    content: '',
    tool_calls: calls.map(c => ({
      id: c.id,
      type: 'function',
      function: { name: c.name, arguments: c.args },
    })),
  }
  return [assistant, ...results, ...(images.length ? [{ role: 'user' as const, content: images }] : [])]
}
