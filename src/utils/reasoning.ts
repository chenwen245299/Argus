import type { AiProviderInfo } from '../types'
import { isSoclaas } from './soclaas'

const EFFORTS = ['none', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']
function rank(effort: string): number {
  const index = EFFORTS.indexOf(effort)
  return index < 0 ? EFFORTS.length : index
}

/** null leaves other providers' controls alone; [] has no discovered control. */
export function soclaasReasoningEfforts(provider: AiProviderInfo | undefined, modelId?: string): string[] | null {
  if (!provider || !isSoclaas(provider.name, provider.base_url, provider.kind)) return null
  return [...new Set(provider.models.find(m => m.id === modelId)?.reasoning_efforts ?? [])]
    .filter(Boolean).sort((a, b) => rank(a) - rank(b))
}

/** Mirrors soclaas::resolve_effort, including the off/lowest-level behavior. */
export function resolveReasoningEffort(supported: string[], enabled: boolean, requested: string): string | undefined {
  if (!enabled) return [...supported].sort((a, b) => rank(a) - rank(b))[0]
  const thinking = supported.filter(s => s !== 'none')
  if (thinking.includes(requested)) return requested
  return thinking.sort((a, b) => Math.abs(rank(a) - rank(requested)) - Math.abs(rank(b) - rank(requested))
    || rank(b) - rank(a))[0]
}

/** A SoCLaaS-specific preference must stay valid when switching providers. */
export function standardReasoningLevel(level: string): 'low' | 'medium' | 'high' {
  return level === 'low' || level === 'minimal' ? 'low' : level === 'medium' ? 'medium' : 'high'
}
