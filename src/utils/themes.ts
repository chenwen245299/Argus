import type { ConcreteThemeId } from '../types'

/**
 * Theme registry — single source of truth for the theme marketplace UI
 * (src/components/settings/ThemeSettings.vue). The actual palettes are CSS
 * custom-property blocks in src/assets/themes.css keyed by [data-theme="<id>"].
 *
 * `preview` colors are sampled from each theme's CSS tokens so the mini
 * previews are honest representations. When editing a palette in themes.css,
 * update its entry here too. `system` is intentionally not listed — it is a
 * mode, not a theme, and the marketplace renders a dedicated card for it.
 */
export interface ThemeMeta {
  id: ConcreteThemeId
  kind: 'light' | 'dark'
  nameZh: string
  nameEn: string
  preview: {
    /** Window background (--bg-primary) */
    bg: string
    /** Sidebar surface (--bg-secondary) */
    sidebar: string
    /** Raised surface / toolbar (--bg-tertiary) */
    toolbar: string
    /** Accent color (--accent) */
    accent: string
    /** Primary text color (--text-primary) */
    text: string
  }
}

export const THEMES: ThemeMeta[] = [
  // ── Light ──────────────────────────────────────────────────────────
  {
    id: 'light',
    kind: 'light',
    nameZh: '默认',
    nameEn: 'Default',
    preview: { bg: '#ffffff', sidebar: '#f2f2f7', toolbar: '#e5e5ea', accent: '#007aff', text: '#1c1c1e' },
  },
  {
    id: 'sky',
    kind: 'light',
    nameZh: '晴空',
    nameEn: 'Sky',
    preview: { bg: '#f6fafc', sidebar: '#ecf3f8', toolbar: '#deeaf1', accent: '#0ea5e9', text: '#13252f' },
  },
  {
    id: 'mist',
    kind: 'light',
    nameZh: '晨雾',
    nameEn: 'Mist',
    preview: { bg: '#f7f9fa', sidebar: '#eef2f4', toolbar: '#e0e7eb', accent: '#6284a8', text: '#1e293b' },
  },
  {
    id: 'mint',
    kind: 'light',
    nameZh: '薄荷',
    nameEn: 'Mint',
    preview: { bg: '#f5faf7', sidebar: '#ebf4ee', toolbar: '#dcebe1', accent: '#10b981', text: '#12281d' },
  },
  {
    id: 'sakura',
    kind: 'light',
    nameZh: '樱花',
    nameEn: 'Sakura',
    preview: { bg: '#fdf7fa', sidebar: '#f9eff4', toolbar: '#f2e3eb', accent: '#ec4899', text: '#2b1823' },
  },
  {
    id: 'peach',
    kind: 'light',
    nameZh: '蜜桃',
    nameEn: 'Peach',
    preview: { bg: '#fdf8f5', sidebar: '#f9efe9', toolbar: '#f3e2d8', accent: '#f97066', text: '#2b1d18' },
  },
  {
    id: 'sepia',
    kind: 'light',
    nameZh: '羊皮纸',
    nameEn: 'Sepia',
    preview: { bg: '#fbf8f0', sidebar: '#f5f0e3', toolbar: '#ece4cf', accent: '#b4842e', text: '#2c2417' },
  },
  {
    id: 'warm',
    kind: 'light',
    nameZh: '暖调',
    nameEn: 'Warm',
    preview: { bg: '#faf8f5', sidebar: '#f2ede5', toolbar: '#e6ddd2', accent: '#e8991a', text: '#1c1a16' },
  },
  {
    id: 'forest',
    kind: 'light',
    nameZh: '自然',
    nameEn: 'Forest',
    preview: { bg: '#f6f9f4', sidebar: '#ecf2e8', toolbar: '#dce9d6', accent: '#3fa368', text: '#182418' },
  },
  {
    id: 'rose',
    kind: 'light',
    nameZh: '玫瑰',
    nameEn: 'Rose',
    preview: { bg: '#fdf8fc', sidebar: '#f5eef5', toolbar: '#ece0ed', accent: '#b06af7', text: '#1c1820' },
  },
  // ── Dark ───────────────────────────────────────────────────────────
  {
    id: 'dark',
    kind: 'dark',
    nameZh: '深色',
    nameEn: 'Dark',
    preview: { bg: '#1c1c1e', sidebar: '#2c2c2e', toolbar: '#3a3a3c', accent: '#72aef7', text: '#f5f5f7' },
  },
  {
    id: 'midnight',
    kind: 'dark',
    nameZh: '午夜',
    nameEn: 'Midnight',
    preview: { bg: '#10141d', sidebar: '#1a2029', toolbar: '#242c3a', accent: '#60a5fa', text: '#e9eef6' },
  },
  {
    id: 'aurora',
    kind: 'dark',
    nameZh: '极光',
    nameEn: 'Aurora',
    preview: { bg: '#23272f', sidebar: '#2e3440', toolbar: '#3b4252', accent: '#88c0d0', text: '#eceff4' },
  },
  {
    id: 'twilight',
    kind: 'dark',
    nameZh: '紫夜',
    nameEn: 'Twilight',
    preview: { bg: '#201a2f', sidebar: '#2b2340', toolbar: '#372e4f', accent: '#a78bfa', text: '#f0ebf9' },
  },
  {
    id: 'ocean',
    kind: 'dark',
    nameZh: '深海',
    nameEn: 'Ocean',
    preview: { bg: '#16272b', sidebar: '#20353a', toolbar: '#2b444a', accent: '#2dd4bf', text: '#e5f2f0' },
  },
  {
    id: 'mocha',
    kind: 'dark',
    nameZh: '摩卡',
    nameEn: 'Mocha',
    preview: { bg: '#201a17', sidebar: '#2b241f', toolbar: '#38302a', accent: '#d4a373', text: '#f0e8dd' },
  },
  {
    id: 'pine',
    kind: 'dark',
    nameZh: '松烟',
    nameEn: 'Pine',
    preview: { bg: '#131f1a', sidebar: '#1c2d25', toolbar: '#263b31', accent: '#4ade80', text: '#e4f0e8' },
  },
]
