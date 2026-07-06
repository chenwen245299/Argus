<script setup lang="ts">
import { computed } from 'vue'
import { useData, withBase } from 'vitepress'
import Media from './Media.vue'

const REPO = 'https://github.com/chenwen245299/Argus'
const { lang } = useData()
const zh = computed(() => lang.value.startsWith('zh'))

const t = computed(() =>
  zh.value
    ? {
        eyebrow: '轻量 · AI 原生 · 本地优先',
        title: 'Argus',
        tagline: '轻量、AI 原生的文献阅读软件',
        sub: '阅读、分析、检索，与你的整个文献库对话 —— 本地优先，一站集成的桌面应用。',
        download: '下载',
        start: '快速上手',
        github: 'GitHub',
        downloadLink: withBase('/zh/download'),
        startLink: withBase('/zh/guide/introduction'),
        platforms: '支持 macOS 与 Windows · 免费开源',
        featuresTitle: '一个应用，覆盖常见科研和阅读流程',
        featuresSub: '把阅读、笔记、AI、检索、arXiv 追踪和图谱整合在一处 —— 数据以可见形式保留在文件夹中，方便备份和迁移',
        features: [
          { icon: '📚', title: '文献库管理', desc: '导入 PDF 与电子书，用文件夹和标签整理，记录阅读状态，集中维护元数据。' },
          { icon: '📖', title: '阅读与笔记', desc: '标签页阅读 PDF、高亮批注、写 Markdown 笔记，带 OCR 兜底的全文提取。' },
          { icon: '🤖', title: 'AI 工作流', desc: '提取元数据与摘要、生成 AI 总结、并与任意论文进行对话，以及自动分析和筛选论文。' },
          { icon: '🔍', title: 'RAG 全库问答', desc: '本地向量索引，带来源地对整个文献库提问，还能可视化 embedding 空间。' },
          { icon: '📡', title: 'arXiv 追踪', desc: '定时抓取 arXiv / bioRxiv 新预印本，AI 相关性过滤，每日收件箱集中处理。' },
          { icon: '🕸️', title: '研究画布', desc: '把论文作为节点排布、连接相关工作、获取连线建议，并导出关系图谱。' },
        ],
        showcasesTitle: '看它怎么工作',
        showcases: [
          { title: '沉浸式阅读，笔记随手记', desc: '多标签 PDF 阅读、逐篇记住阅读进度、高亮批注，并在独立窗口里并排写笔记。公式、图表、代码一应俱全。', media: '/media/reading.mp4', link: withBase('/zh/guide/reading') },
          { title: '与论文对话，AI 原生', desc: '把论文文本作为上下文，和单篇论文对话；或使用你喜欢的 AI 供应商（OpenAI、Anthropic、OpenRouter、DeepSeek、Ollama 等）提取论文元数据、生成论文摘要和总结分析。每次调用都记录 token 与费用。', media: '/media/ai-chat.mp4', link: withBase('/zh/guide/ai') },
          { title: '问遍整个文献库', desc: '本地构建语义索引，带来源地回答跨论文的问题；比较不同论文之间的关系，快速检索论文；用二维 embedding 图看见相关工作的聚类。', media: '/media/library-chat.mp4', link: withBase('/zh/guide/rag') },
          { title: '画出研究的全貌', desc: '在画布上把论文连成关系图，构建论文 / 技术的发展路线，获得连线建议与自动布局，并导出 PNG / PDF / SVG。', media: '/media/canvas.mp4', link: withBase('/zh/guide/canvas') },
        ],
        ctaTitle: '让你的文献库，聪明起来',
        ctaSub: '轻量、AI 原生、开源、本地优先。下载即用！',
      }
    : {
        eyebrow: 'Lightweight · AI-native · Local-first',
        title: 'Argus',
        tagline: 'A lightweight, AI-native literature reader',
        sub: 'Read, analyze, search, and chat with your entire paper library — all local-first, all in one desktop app.',
        download: 'Download',
        start: 'Get Started',
        github: 'GitHub',
        downloadLink: withBase('/download'),
        startLink: withBase('/guide/introduction'),
        platforms: 'macOS & Windows · Free and open source',
        featuresTitle: 'One app for your whole reading workflow',
        featuresSub: 'Reading, notes, AI, search, tracking, and maps in one place — with all your data staying on your own machine.',
        features: [
          { icon: '📚', title: 'Literature Library', desc: 'Import PDFs and ebooks, organize with folders and tags, track reading status, keep metadata in one place.' },
          { icon: '📖', title: 'Reading & Notes', desc: 'Tabbed PDF reading, highlights and annotations, rich notes — with an OCR-backed full-text pipeline.' },
          { icon: '🤖', title: 'AI Workflows', desc: 'Extract metadata and abstracts, generate configurable analysis, and chat with any paper. Bring your own keys.' },
          { icon: '🔍', title: 'RAG Library Q&A', desc: 'A local vector index to ask questions across your whole library — with sources and an embedding-space map.' },
          { icon: '📡', title: 'arXiv Tracking', desc: 'Auto-fetch arXiv / bioRxiv preprints on a schedule, filter by AI relevance, triage from a daily inbox.' },
          { icon: '🕸️', title: 'Research Canvas', desc: 'Arrange papers as nodes, connect related work, get edge suggestions, and export relationship maps.' },
        ],
        showcasesTitle: 'See it in action',
        showcases: [
          { title: 'Immersive reading, notes at hand', desc: 'Tabbed PDF reading with per-paper reading state, highlights and annotations, and notes side by side in their own windows — math, diagrams, and code included.', media: '/media/reading.mp4', link: withBase('/guide/reading') },
          { title: 'Chat with your papers, AI-native', desc: 'Chat with a single paper using its text as context, or generate configurable analysis with OpenAI, Claude, OpenRouter, Ollama, and more. Every call tracks tokens and cost.', media: '/media/ai-chat.mp4', link: withBase('/guide/ai') },
          { title: 'Ask across the whole library', desc: 'Build a local semantic index and get sourced answers spanning papers — then see clusters of related work on a 2-D embedding map.', media: '/media/library-chat.mp4', link: withBase('/guide/rag') },
          { title: 'Map the shape of your research', desc: 'Connect papers into relationship maps on a canvas, get edge suggestions and auto-layout, and export to PNG / PDF / SVG.', media: '/media/canvas.mp4', link: withBase('/guide/canvas') },
        ],
        ctaTitle: 'Make your library smarter',
        ctaSub: 'Free, open source, local-first. Download and go — your data always stays yours.',
      },
)

const heroImg = withBase('/main.png')
</script>

<template>
  <div class="argus-landing">
    <!-- Hero -->
    <section class="al-hero">
      <div class="al-container al-hero__inner">
        <span class="al-eyebrow">{{ t.eyebrow }}</span>
        <h1 class="al-title">{{ t.title }}</h1>
        <p class="al-tagline">{{ t.tagline }}</p>
        <p class="al-sub">{{ t.sub }}</p>
        <div class="al-actions">
          <a class="al-btn al-btn--primary" :href="t.downloadLink">{{ t.download }}</a>
          <a class="al-btn al-btn--secondary" :href="t.startLink">{{ t.start }}</a>
          <a class="al-btn al-btn--ghost" :href="REPO" target="_blank" rel="noreferrer">{{ t.github }} ↗</a>
        </div>
        <p class="al-platforms">{{ t.platforms }}</p>
        <div class="al-hero__shot">
          <img :src="heroImg" alt="Argus" loading="eager" />
        </div>
      </div>
    </section>

    <!-- Feature grid -->
    <section class="al-features">
      <div class="al-container">
        <h2 class="al-h2">{{ t.featuresTitle }}</h2>
        <p class="al-lead">{{ t.featuresSub }}</p>
        <div class="al-grid">
          <div v-for="f in t.features" :key="f.title" class="al-fcard">
            <div class="al-fcard__icon">{{ f.icon }}</div>
            <h3 class="al-fcard__title">{{ f.title }}</h3>
            <p class="al-fcard__desc">{{ f.desc }}</p>
          </div>
        </div>
      </div>
    </section>

    <!-- Showcase rows (alternating) -->
    <section class="al-showcases">
      <div class="al-container">
        <h2 class="al-h2">{{ t.showcasesTitle }}</h2>
        <div
          v-for="(s, i) in t.showcases"
          :key="s.title"
          class="al-show"
          :class="{ 'al-show--reverse': i % 2 === 1 }"
        >
          <div class="al-show__media">
            <Media :src="s.media" :caption="s.title" />
          </div>
          <div class="al-show__body">
            <h3 class="al-show__title">{{ s.title }}</h3>
            <p class="al-show__desc">{{ s.desc }}</p>
            <a class="al-link" :href="s.link">{{ t.start }} →</a>
          </div>
        </div>
      </div>
    </section>

    <!-- Download CTA -->
    <section class="al-ctawrap">
      <div class="al-container">
        <div class="al-cta">
          <h2 class="al-cta__title">{{ t.ctaTitle }}</h2>
          <p class="al-cta__sub">{{ t.ctaSub }}</p>
          <div class="al-actions al-actions--center">
            <a class="al-btn al-btn--primary" :href="t.downloadLink">{{ t.download }}</a>
            <a class="al-btn al-btn--ghost" :href="REPO" target="_blank" rel="noreferrer">{{ t.github }} ↗</a>
          </div>
        </div>
      </div>
    </section>
  </div>
</template>
