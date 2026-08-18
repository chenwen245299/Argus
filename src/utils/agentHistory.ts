import type { ChatMessage } from '../types'

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
  /** The result the model got back (already bounded by the backend budget). */
  preview?: string
}

// Per-result and per-answer caps for the replayed results. What the model
// already read was itself capped to the context budget, so these are generous —
// full enough to reuse verbatim, bounded enough not to blow the window on a
// long, tool-heavy conversation.
const REPLAY_STEP_CHARS = 12_000
const REPLAY_ANSWER_CHARS = 48_000

/**
 * Rebuild one assistant turn's tool activity as an OpenAI-style tool exchange:
 * a single assistant message carrying every `tool_call` the turn made, followed
 * by one `tool` message per call with its result. The caller splices this in
 * *before* that turn's final assistant answer.
 *
 * The default history a follow-up turn sends carries only the user text and the
 * final assistant answer; the tool calls and their results are dropped, so the
 * model re-fetches what it already had. Replaying the real calls and results —
 * as native tool messages the model is trained to read — lets it reuse them
 * instead of calling the same tool with the same arguments again.
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
): ChatMessage[] {
  if (!steps?.length) return []

  const calls: { id: string; name: string; args: string }[] = []
  const results: ChatMessage[] = []
  let budget = REPLAY_ANSWER_CHARS

  steps.forEach((step, i) => {
    if (!step.tool) return
    const id = `call_${answerId}_${i}`
    // `arguments` must be a JSON string; argsJson already is one.
    const args = (step.argsJson ?? '').trim() || '{}'
    calls.push({ id, name: step.tool, args })

    let content: string
    if (step.ok === false) {
      content = (step.preview ?? '').trim() || '{"error":"tool call failed"}'
    } else {
      const result = (step.preview ?? '').trim()
      const room = Math.min(REPLAY_STEP_CHARS, budget)
      if (!result) {
        content = ''
      } else if (room <= 0) {
        content = '（结果从略：复用上下文已达上限）'
      } else if (result.length > room) {
        content = `${result.slice(0, room)}\n…（结果过长，已截断）`
        budget -= room
      } else {
        content = result
        budget -= result.length
      }
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
  return [assistant, ...results]
}
