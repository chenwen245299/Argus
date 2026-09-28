import { mockIPC, mockWindows } from '@tauri-apps/api/mocks'

mockWindows('library-chat')

const now = new Date().toISOString()
const status = { text_extracted: true, metadata_fetched: true, vectorized: false, last_updated: now }
const paper = (slug: string, title: string, year: number) => ({
  slug, id: `id-${slug}`, title, authors: ['Ada Lovelace', 'Alan Turing'], year, venue: 'NeurIPS',
  tags: [], status, added_at: now, reading_status: 'read', meta_mtime: 0,
})
const papers = [
  paper('attn', 'Attention Is All You Need', 2017),
  paper('bert', 'BERT: Pre-training of Deep Bidirectional Transformers', 2019),
  paper('knighter', 'KNighter: Transforming Static Analysis with LLM-Synthesized Checkers', 2025),
]

const conv = {
  id: 'c1',
  title: '改成只用 agent 之后的样子',
  selectedPaperSlugs: ['attn', 'bert'],
  createdAt: now,
  updatedAt: now,
  messages: [
    { id: 'u1', role: 'user', content: '这两篇的注意力机制有什么区别？', createdAt: now },
    {
      id: 'a1', role: 'assistant', createdAt: now,
      content: '两篇都用了缩放点积注意力，区别在于 BERT 只用编码器、双向……',
      model: { providerId: 'p-anthropic', modelId: 'claude-x' }, modelLabel: 'Claude',
      agentFallback: { reason: 'no_tools', mode: 'papers', papers: 2 },
    },
    { id: 'u2', role: 'user', content: '帮我把这句话翻译成英文：注意力就是一切', createdAt: now },
    {
      id: 'a2', role: 'assistant', createdAt: now,
      content: 'Attention is all you need.',
      model: { providerId: 'p-anthropic', modelId: 'claude-x' }, modelLabel: 'Claude',
      agentFallback: { reason: 'no_tools', mode: 'none', papers: 0 },
    },
    { id: 'u3', role: 'user', content: '库里哪几篇讲静态分析？', createdAt: now },
    {
      id: 'a3', role: 'assistant', createdAt: now,
      content: '找到 1 篇：**KNighter** …',
      model: { providerId: 'p-ds', modelId: 'deepseek-chat' }, modelLabel: 'DeepSeek',
      agentSteps: [
        { tool: 'semantic_search', args: 'query: 静态分析', argsJson: '{"query":"静态分析"}', ok: true, chars: 812 },
        { tool: 'get_paper_fulltext', args: 'slug: knighter', argsJson: '{"slug":"knighter"}', ok: true, chars: 20000 },
      ],
      sources: [
        { chunk_id: 'k1', paper_id: 'id-knighter', slug: 'knighter', chunk_index: 0, text: 'We present KNighter…', score: 0.82, paper_title: papers[2].title, source_type: 'text', source_id: null, source_label: null },
        { chunk_id: 's1', paper_id: 'id-knighter', slug: 'knighter', chunk_index: 0, text: '把历史 bug 当成 checker 的训练信号', score: 0.77, paper_title: papers[2].title, source_type: 'snippet', source_id: 's1', source_label: null },
      ],
    },
    { id: 'u4', role: 'user', content: '用 DeepSeek 联网查一下最新进展', createdAt: now },
    {
      id: 'a4', role: 'assistant', createdAt: now,
      content: '根据网络搜索……',
      model: { providerId: 'p-ds', modelId: 'deepseek-chat' }, modelLabel: 'DeepSeek',
      agentFallback: { reason: 'web_search', mode: 'library', papers: 0 },
      sources: [],
    },
  ],
}

const model = (id: string, caps: string[] = []) => ({ id, display_name: id, capabilities: caps, context_length: 128000 })
const ai = {
  providers: [
    { id: 'p-ds', name: 'DeepSeek', kind: 'openai_compatible', base_url: 'https://api.deepseek.com', enabled: true, has_key: true,
      models: [model('deepseek-chat', ['tool_calling'])], server_tools: {}, speech: { enabled: false, voice: '' } },
    { id: 'p-anthropic', name: 'Anthropic', kind: 'anthropic', base_url: 'https://api.anthropic.com', enabled: true, has_key: true,
      models: [model('claude-x', ['vision'])], server_tools: {}, speech: { enabled: false, voice: '' } },
  ],
  default_provider_id: 'p-ds',
  default_model_id: 'deepseek-chat',
}

mockIPC((cmd) => {
  switch (cmd) {
    case 'get_current_library': return '/tmp/lib'
    case 'get_ai_settings': return ai
    case 'list_papers': return papers
    case 'get_library_conversations': return [conv]
    case 'fetch_provider_balances': return []
    case 'get_settings': return {}
    default: return null
  }
}, { shouldMockEvents: true })

await import('../src/main.ts')
