import { defineConfig } from 'vitepress'

// Shared repo/site constants
const REPO = 'https://github.com/chenwen245299/Argus'

export default defineConfig({
  // Project page lives at https://chenwen245299.github.io/Argus/ — base is
  // case-sensitive and needs both leading and trailing slashes.
  base: '/Argus/',

  title: 'Argus',
  lastUpdated: true,
  cleanUrls: true,
  ignoreDeadLinks: true,

  head: [
    ['link', { rel: 'icon', href: '/Argus/icon.png' }],
  ],

  themeConfig: {
    logo: '/icon.png',
    search: {
      provider: 'local',
      options: {
        locales: {
          zh: {
            translations: {
              button: { buttonText: '搜索文档', buttonAriaLabel: '搜索文档' },
              modal: {
                displayDetails: '显示详细列表',
                resetButtonTitle: '清除查询条件',
                backButtonTitle: '关闭搜索',
                noResultsText: '无法找到相关结果',
                footer: {
                  selectText: '选择',
                  navigateText: '切换',
                  closeText: '关闭',
                },
              },
            },
          },
        },
      },
    },
    socialLinks: [{ icon: 'github', link: REPO }],
  },

  locales: {
    // English — default (root) locale.
    root: {
      label: 'English',
      lang: 'en',
      themeConfig: {
        nav: [
          { text: 'Docs', link: '/guide/introduction' },
          { text: 'Download', link: '/download' },
        ],
        sidebar: {
          '/guide/': [
            {
              text: 'Getting Started',
              items: [
                { text: 'Introduction', link: '/guide/introduction' },
                { text: 'Installation', link: '/guide/installation' },
                { text: 'Quick Start', link: '/guide/quick-start' },
              ],
            },
            {
              text: 'Core Features',
              items: [
                { text: 'Literature Library', link: '/guide/library' },
                { text: 'Reading & Notes', link: '/guide/reading' },
                { text: 'AI Workflows', link: '/guide/ai' },
              ],
            },
            {
              text: 'Highlight Features',
              items: [
                { text: 'Canvas', link: '/guide/canvas' },
                { text: 'Snippet Library', link: '/guide/snippets' },
                { text: 'Embedding Map', link: '/guide/embedding-map' },
                { text: 'arXiv & bioRxiv Tracking', link: '/guide/arxiv' },
                { text: 'RAG & Library Q&A', link: '/guide/rag' },
              ],
            },
          ],
        },
        editLink: {
          pattern: `${REPO}/edit/main/docs/:path`,
          text: 'Edit this page on GitHub',
        },
        footer: {
          message: 'Released under the license in the repository.',
          copyright: `Copyright © 2025-present Argus`,
        },
        docFooter: { prev: 'Previous', next: 'Next' },
      },
    },

    // Chinese — served at /Argus/zh/.
    zh: {
      label: '简体中文',
      lang: 'zh-CN',
      link: '/zh/',
      themeConfig: {
        nav: [
          { text: '文档', link: '/zh/guide/introduction' },
          { text: '下载', link: '/zh/download' },
        ],
        sidebar: {
          '/zh/guide/': [
            {
              text: '开始使用',
              items: [
                { text: '项目简介', link: '/zh/guide/introduction' },
                { text: '安装教程', link: '/zh/guide/installation' },
                { text: '快速上手', link: '/zh/guide/quick-start' },
              ],
            },
            {
              text: '基础功能',
              items: [
                { text: '文献库管理', link: '/zh/guide/library' },
                { text: '阅读与笔记', link: '/zh/guide/reading' },
                { text: 'AI 工作流', link: '/zh/guide/ai' },
              ],
            },
            {
              text: '特色功能',
              items: [
                { text: '画布', link: '/zh/guide/canvas' },
                { text: '素材库', link: '/zh/guide/snippets' },
                { text: '向量图谱', link: '/zh/guide/embedding-map' },
                { text: 'arXiv / bioRxiv 追踪', link: '/zh/guide/arxiv' },
                { text: 'RAG问答', link: '/zh/guide/rag' },
              ],
            },
          ],
        },
        editLink: {
          pattern: `${REPO}/edit/main/docs/:path`,
          text: '在 GitHub 上编辑此页',
        },
        footer: {
          message: '基于仓库中的许可协议发布。',
          copyright: `版权所有 © 2025至今 Argus`,
        },
        docFooter: { prev: '上一页', next: '下一页' },
        outline: { label: '本页目录' },
        lastUpdatedText: '最后更新',
        returnToTopLabel: '返回顶部',
        sidebarMenuLabel: '菜单',
        darkModeSwitchLabel: '外观',
        lightModeSwitchTitle: '切换到浅色模式',
        darkModeSwitchTitle: '切换到深色模式',
      },
    },
  },
})
