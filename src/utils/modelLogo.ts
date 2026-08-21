// Brand mark for a model row in a picker.
//
// Lifted out of LibraryChat so every picker (library chat, canvas chat) shows
// the same logo for the same model. Matching is by substring over the provider
// and model names, most specific brand first — an OpenAI-compatible adapter
// pointed at DeepSeek must show DeepSeek's mark, not OpenAI's.

import type { ModelOption } from '../stores/ai'

const modelSvgModules = import.meta.glob<{ default: string }>('/src/assets/models/*.svg', { eager: true })
const modelIconMap: Record<string, string> = {}
for (const [path, mod] of Object.entries(modelSvgModules)) {
  modelIconMap[path.replace(/^.*\//, '').replace(/\.svg$/, '')] = mod.default
}

/**
 * @param model      the row being drawn
 * @param providerKind the provider's `kind`, when the caller knows it.
 *   "openai_compatible" is passed through as empty: it is the generic adapter
 *   used for DeepSeek/Kimi/… and would otherwise claim the OpenAI brand.
 */
export function modelLogo(model?: ModelOption | null, providerKind?: string): string {
  if (!model) return ''
  const kind = providerKind === 'openai_compatible' ? '' : providerKind
  const haystack = [kind, model.providerId, model.providerName, model.modelId, model.displayName]
    .filter(Boolean)
    .join(' ')
    .toLowerCase()

  // Model-specific brands first, so generic adapter kinds don't win.
  if (haystack.includes('deepseek')) return modelIconMap.deepseek
  if (haystack.includes('kimi') || haystack.includes('moonshot')) return modelIconMap.kimi
  if (haystack.includes('claude') || haystack.includes('anthropic')) return modelIconMap.claude
  if (haystack.includes('gemma')) return modelIconMap.gemma
  if (haystack.includes('gemini') || haystack.includes('google')) return modelIconMap.gemini
  if (haystack.includes('qwen') || haystack.includes('通义') || haystack.includes('alibaba')) {
    return modelIconMap.qwen ?? modelIconMap.alibaba
  }
  if (haystack.includes('grok') || haystack.includes('xai')) return modelIconMap.grok ?? modelIconMap.xai
  if (haystack.includes('zhipu') || haystack.includes('智谱') || haystack.includes('glm')) return modelIconMap.zhipu
  if (haystack.includes('baidu') || haystack.includes('ernie')) return modelIconMap.baidu
  if (haystack.includes('doubao') || haystack.includes('bytedance')) return modelIconMap.bytedance
  if (haystack.includes('mistral') || haystack.includes('huggingface')) return modelIconMap.huggingface
  // MiMo ids ("mimo-v2.5-…") and the provider name both carry "mimo"; the icon
  // file is xiaomimimo.svg, which the haystack never spells out on its own.
  if (haystack.includes('mimo') || haystack.includes('xiaomi')) return modelIconMap.xiaomimimo
  if (haystack.includes('openai') || haystack.includes('gpt')) return modelIconMap.openai
  // Ollama is a host, not a model brand — the provider name pollutes the
  // haystack, so match its mark only after every real model brand above.
  if (haystack.includes('ollama')) return modelIconMap['ollama-color']

  // Fall back to filename-based matching for less common providers.
  const keys = [
    model.providerId,
    model.providerName,
    model.modelId.split('/')[0],
    model.displayName.split(':')[0],
  ]
  for (const raw of keys) {
    const key = raw.toLowerCase().replace(/\s+/g, '-').replace(/[^a-z0-9-]/g, '')
    if (modelIconMap[key]) return modelIconMap[key]
  }
  // Last resort: an icon filename that appears anywhere in the names. This
  // catches marks like `dots` inside "Dots Studio: Dots3-Note Preview", which
  // the split-key lookup above misses ("dots-studio" ≠ "dots"). Runs after every
  // explicit brand check so it can't steal an ambiguous match.
  for (const key of Object.keys(modelIconMap)) {
    if (haystack.includes(key)) return modelIconMap[key]
  }
  return ''
}

/** Short capability list for a picker row: 视觉 · 工具 · 推理.
 *
 *  An unknown capability is shown as-is rather than dropped — a provider that
 *  reports something new should still surface it. `embedding` is the one
 *  exception: those models are filtered out of the chat pickers entirely, and
 *  the word would only be noise on the ones that carry it alongside chat. */
export function modelCapabilityText(model: ModelOption): string {
  const map: Record<string, string> = {
    vision: '视觉',
    audio: '音频',
    reasoning: '推理',
    tool_calling: '工具',
    image_gen: '文生图',
    video: '视频',
  }
  return (model.capabilities ?? [])
    .filter(cap => cap !== 'embedding')
    .map(cap => map[cap] ?? cap)
    .join(' · ')
}
