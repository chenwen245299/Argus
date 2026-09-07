// Brand mark for a *provider* row.
//
// Lifted out of AiSettings so the usage dashboard shows the same mark for the
// same provider — the sibling of `modelLogo.ts`, which does the job for a model
// row. Matching is by substring over the provider's name and base URL, most
// specific brand first, because a provider added as a plain `openai_compatible`
// adapter is identified only by where it points.

const providerSvgModules = import.meta.glob<{ default: string }>(
  '/src/assets/providers/*.svg',
  { eager: true },
)
const providerIconMap: Record<string, string> = {}
for (const [path, mod] of Object.entries(providerSvgModules)) {
  providerIconMap[path.replace(/^.*\//, '')] = mod.default
}

const LOGO_MAP: [string[], string][] = [
  [['deepseek', 'api.deepseek.com'], 'deepseek.svg'],
  [['openai', 'api.openai.com'], 'openai.svg'],
  [['anthropic', 'claude', 'api.anthropic.com'], 'claude.svg'],
  [['openrouter', 'openrouter.ai'], 'openrouter.svg'],
  [['kimi', 'moonshot'], 'kimi.svg'],
  [['ollama', '11434'], 'ollama-color.svg'],
  [['gemini', 'generativelanguage.googleapis.com'], 'gemini.svg'],
  [['gemma'], 'gemma.svg'],
  [['grok'], 'grok.svg'],
  [['xai', 'x.ai', 'api.x.ai'], 'xai.svg'],
  [['huggingface', 'huggingface.co'], 'huggingface.svg'],
  [['lmstudio'], 'lmstudio.svg'],
  [['siliconflow', 'silicon'], 'siliconflow.svg'],
  // Qwen's own mark, matched before the generic Alibaba Cloud one. Keyed on the
  // Aliyun MaaS host and the Qwen brand so a plain Aliyun endpoint still falls to
  // alibaba.svg, while any Qwen-named provider gets qwenai.svg. Deliberately NOT
  // keyed on "token-plan": MiMo's Token-Plan host is token-plan-cn.xiaomimimo.com,
  // which must reach the MiMo mark below rather than being caught here.
  [['qwenai', 'maas.aliyuncs', 'qwen', '千问', '通义'], 'qwenai.svg'],
  [['alibaba', 'dashscope', 'aliyun'], 'alibaba.svg'],
  [['baidu', 'qianfan', 'baidubce'], 'baidu.svg'],
  [['zhipu', 'bigmodel', 'chatglm'], 'zhipu.svg'],
  [['tencent', 'hunyuan'], 'tencent.svg'],
  [['bytedance', 'doubao', 'volcengine', 'volces'], 'bytedance.svg'],
  [['nvidia', 'integrate.api.nvidia'], 'nvidia.svg'],
  [['microsoft', 'azure', 'openai.azure'], 'microsoft.svg'],
  [['mimo', 'xiaomimimo', 'xiaomi', 'micloud'], 'xiaomimimo.svg'],
  [['minimax', 'hailuo'], 'minimax.svg'],
  [['mole', 'moleapi'], 'MoleAPI.svg'],
]

/** Resolved asset URL for a provider's mark, or '' when no brand matches. */
export function providerLogo(name: string, baseUrl = ''): string {
  const haystack = `${name} ${baseUrl}`.toLowerCase()
  for (const [keywords, file] of LOGO_MAP) {
    if (keywords.some(k => haystack.includes(k))) {
      return providerIconMap[file] ?? ''
    }
  }
  return ''
}
