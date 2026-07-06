<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useData } from 'vitepress'

const OWNER = 'chenwen245299'
const NAME = 'Argus'
const API = `https://api.github.com/repos/${OWNER}/${NAME}/releases/latest`
const RELEASES_PAGE = `https://github.com/${OWNER}/${NAME}/releases`
// Redirects to the newest *published* release (ignores drafts/prereleases).
const RELEASES_LATEST = `https://github.com/${OWNER}/${NAME}/releases/latest`

const { lang } = useData()
const zh = computed(() => lang.value.startsWith('zh'))

type Asset = { name: string; url: string; label: string; size: number }

const status = ref<'loading' | 'ready' | 'empty' | 'error'>('loading')
const version = ref('')
const pubDate = ref('')
const releaseUrl = ref('')
const macAssets = ref<Asset[]>([])
const winAssets = ref<Asset[]>([])
const os = ref<'mac' | 'win' | ''>('')

const t = computed(() =>
  zh.value
    ? {
        title: '下载 Argus',
        sub: '轻量、AI 原生的文献阅读软件。选择你的平台，一键下载最新版本。',
        latest: '最新版本',
        released: '发布于',
        download: '下载',
        recommended: '推荐给你的设备',
        loading: '正在获取最新版本…',
        empty: '还没有已发布的版本。',
        error: '无法获取版本信息，请前往 Releases 页面手动下载。',
        latestNotes: '查看最新版本更新日志 →',
        releasesPage: '前往 Releases 页面 →',
        macNoteTitle: 'macOS 首次打开',
        macNote: '由于我没钱买苹果的开发者账号，所以没有证书，导致macOS 首次打开会被拦截。针对该问题，只需要在终端里运行下面的代码清除隔离标记即可',
        macDetail: '详见安装教程',
        macDetailLink: '/zh/guide/installation',
        noAsset: '此平台暂无安装包',
      }
    : {
        title: 'Download Argus',
        sub: 'A lightweight, AI-native literature reader. Pick your platform and grab the latest build.',
        latest: 'Latest version',
        released: 'Released',
        download: 'Download',
        recommended: 'Recommended for your device',
        loading: 'Fetching the latest release…',
        empty: 'No release has been published yet.',
        error: 'Could not load release info. Please download from the Releases page.',
        latestNotes: 'See the latest release notes →',
        releasesPage: 'Open the Releases page →',
        macNoteTitle: 'Opening on macOS',
        macNote: "I can't afford an Apple Developer account, so the app isn't code-signed and macOS blocks it on first open. To fix it, just clear the quarantine flag by running the following in Terminal:",
        macDetail: 'See the installation guide',
        macDetailLink: '/guide/installation',
        noAsset: 'No installer for this platform yet',
      },
)

function fmtSize(b: number): string {
  if (!b) return ''
  const mb = b / 1048576
  return mb >= 1 ? `${mb.toFixed(1)} MB` : `${(b / 1024).toFixed(0)} KB`
}

function macLabel(name: string): string {
  if (/aarch64|arm64/i.test(name)) return zh.value ? 'Apple 芯片' : 'Apple Silicon'
  if (/x64|x86_64|intel/i.test(name)) return 'Intel'
  if (/universal/i.test(name)) return zh.value ? '通用版' : 'Universal'
  return 'macOS'
}

function winLabel(name: string): string {
  if (/\.exe$/i.test(name)) return zh.value ? '安装程序 (.exe)' : 'Installer (.exe)'
  if (/\.msi$/i.test(name)) return 'MSI (.msi)'
  return name
}

onMounted(async () => {
  const ua = navigator.userAgent
  os.value = /Mac/i.test(ua) ? 'mac' : /Win/i.test(ua) ? 'win' : ''

  try {
    const res = await fetch(API, { headers: { Accept: 'application/vnd.github+json' } })
    if (res.status === 404) {
      status.value = 'empty'
      return
    }
    if (!res.ok) throw new Error(String(res.status))
    const data = await res.json()
    version.value = data.tag_name || data.name || ''
    pubDate.value = (data.published_at || '').slice(0, 10)
    releaseUrl.value = data.html_url || RELEASES_LATEST

    const assets: any[] = data.assets || []
    macAssets.value = assets
      .filter((a) => /\.dmg$/i.test(a.name))
      .map((a) => ({ name: a.name, url: a.browser_download_url, label: macLabel(a.name), size: a.size }))
    winAssets.value = assets
      .filter((a) => /\.(exe|msi)$/i.test(a.name))
      .map((a) => ({ name: a.name, url: a.browser_download_url, label: winLabel(a.name), size: a.size }))

    status.value = macAssets.value.length || winAssets.value.length ? 'ready' : 'empty'
  } catch {
    status.value = 'error'
  }
})
</script>

<template>
  <div class="dl-page">
    <div class="dl-container">
      <header class="dl-head">
        <h1 class="dl-title">{{ t.title }}</h1>
        <p class="dl-sub">{{ t.sub }}</p>
        <p v-if="status === 'ready' && version" class="dl-ver">
          {{ t.latest }} <strong>{{ version }}</strong>
          <span v-if="pubDate"> · {{ t.released }} {{ pubDate }}</span>
        </p>
      </header>

      <p v-if="status === 'loading'" class="dl-msg">{{ t.loading }}</p>
      <p v-else-if="status === 'empty'" class="dl-msg">
        {{ t.empty }} <a :href="RELEASES_PAGE" target="_blank" rel="noreferrer">{{ t.releasesPage }}</a>
      </p>
      <p v-else-if="status === 'error'" class="dl-msg">
        {{ t.error }} <a :href="RELEASES_LATEST" target="_blank" rel="noreferrer">{{ t.releasesPage }}</a>
      </p>

      <div v-else class="dl-cards">
        <!-- macOS -->
        <section class="dl-card" :class="{ 'dl-card--rec': os === 'mac' }">
          <span v-if="os === 'mac'" class="dl-rec">{{ t.recommended }}</span>
          <svg class="dl-card__icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M16.365 1.43c0 1.14-.493 2.27-1.177 3.08-.744.9-1.99 1.57-2.987 1.57-.12 0-.23-.02-.3-.03-.01-.06-.04-.22-.04-.39 0-1.15.572-2.27 1.206-2.98.804-.94 2.142-1.64 3.248-1.68.03.13.05.28.05.43zm4.565 15.71c-.03.07-.463 1.58-1.518 3.12-.945 1.34-1.94 2.71-3.43 2.71-1.517 0-1.9-.88-3.63-.88-1.698 0-2.302.91-3.67.91-1.377 0-2.332-1.26-3.428-2.8-1.287-1.82-2.323-4.63-2.323-7.28 0-4.28 2.797-6.55 5.552-6.55 1.448 0 2.675.95 3.6.95.865 0 2.222-1.01 3.902-1.01.613 0 2.886.06 4.374 2.19-.13.09-2.383 1.37-2.383 4.19 0 3.26 2.854 4.42 2.955 4.45z" />
          </svg>
          <h2 class="dl-card__name">macOS</h2>
          <div v-if="macAssets.length" class="dl-btns">
            <a
              v-for="a in macAssets"
              :key="a.url"
              class="al-btn al-btn--primary dl-btn"
              :href="a.url"
            >
              {{ t.download }}<span v-if="a.label"> · {{ a.label }}</span>
              <em v-if="a.size">{{ fmtSize(a.size) }}</em>
            </a>
          </div>
          <p v-else class="dl-none">{{ t.noAsset }}</p>
        </section>

        <!-- Windows -->
        <section class="dl-card" :class="{ 'dl-card--rec': os === 'win' }">
          <span v-if="os === 'win'" class="dl-rec">{{ t.recommended }}</span>
          <svg class="dl-card__icon" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
            <path d="M3 3h8v8H3zM13 3h8v8h-8zM3 13h8v8H3zM13 13h8v8h-8z" />
          </svg>
          <h2 class="dl-card__name">Windows</h2>
          <div v-if="winAssets.length" class="dl-btns">
            <a
              v-for="a in winAssets"
              :key="a.url"
              class="al-btn al-btn--primary dl-btn"
              :href="a.url"
            >
              {{ t.download }}<span v-if="a.label"> · {{ a.label }}</span>
              <em v-if="a.size">{{ fmtSize(a.size) }}</em>
            </a>
          </div>
          <p v-else class="dl-none">{{ t.noAsset }}</p>
        </section>
      </div>

      <!-- macOS quarantine note -->
      <aside v-if="status === 'ready' && macAssets.length" class="dl-macnote">
        <strong>{{ t.macNoteTitle }}</strong>
        <p>{{ t.macNote }}</p>
        <pre><code>xattr -cr /Applications/Argus.app</code></pre>
        <a :href="t.macDetailLink" class="dl-link">{{ t.macDetail }} →</a>
      </aside>

      <p v-if="status === 'ready'" class="dl-all">
        <a :href="releaseUrl || RELEASES_LATEST" target="_blank" rel="noreferrer">{{ t.latestNotes }}</a>
      </p>
    </div>
  </div>
</template>

<style scoped>
.dl-container {
  max-width: 880px;
  margin: 0 auto;
  padding: 64px 24px 96px;
}
.dl-head {
  text-align: center;
}
.dl-title {
  font-size: clamp(32px, 5vw, 46px);
  font-weight: 900;
  letter-spacing: -0.03em;
  line-height: 1.15;
  margin: 0;
  background: var(--argus-grad);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
  padding-bottom: 0.1em;
}
.dl-sub {
  max-width: 56ch;
  margin: 14px auto 0;
  color: var(--vp-c-text-2);
  font-size: 16px;
  line-height: 1.6;
}
.dl-ver {
  margin: 18px 0 0;
  font-size: 14px;
  color: var(--vp-c-text-2);
}
.dl-ver strong {
  color: var(--vp-c-brand-1);
}
.dl-msg {
  text-align: center;
  margin: 48px 0;
  color: var(--vp-c-text-2);
}

.dl-cards {
  margin-top: 44px;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 24px;
}
.dl-card {
  position: relative;
  text-align: center;
  padding: 40px 28px;
  border-radius: 18px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
  transition: border-color 0.18s ease, box-shadow 0.18s ease;
}
.dl-card--rec {
  border-color: var(--vp-c-brand-1);
  box-shadow: 0 18px 44px -26px var(--vp-c-brand-3);
}
.dl-rec {
  position: absolute;
  top: -12px;
  left: 50%;
  transform: translateX(-50%);
  white-space: nowrap;
  padding: 4px 12px;
  border-radius: 999px;
  font-size: 12px;
  font-weight: 600;
  color: #fff;
  background: var(--argus-grad);
}
.dl-card__icon {
  display: block;
  width: 42px;
  height: 42px;
  margin: 0 auto;
  color: var(--vp-c-text-1);
}
.dl-card__name {
  margin: 12px 0 20px;
  font-size: 22px;
  font-weight: 800;
  border: 0;
  padding: 0;
}
.dl-btns {
  display: flex;
  flex-direction: column;
  gap: 12px;
}
.dl-btn {
  height: auto;
  min-height: 46px;
  padding: 8px 20px;
  flex-wrap: wrap;
  gap: 4px 8px;
}
.dl-btn em {
  font-style: normal;
  font-weight: 400;
  font-size: 12px;
  opacity: 0.85;
}
.dl-none {
  color: var(--vp-c-text-3);
  font-size: 14px;
}

.dl-macnote {
  margin-top: 36px;
  padding: 20px 22px;
  border-radius: 14px;
  border: 1px solid var(--vp-c-divider);
  background: var(--vp-c-bg-soft);
}
.dl-macnote strong {
  display: block;
  margin-bottom: 6px;
}
.dl-macnote p {
  margin: 0 0 10px;
  color: var(--vp-c-text-2);
  font-size: 14px;
  line-height: 1.6;
}
.dl-macnote pre {
  margin: 0 0 10px;
  padding: 12px 14px;
  border-radius: 8px;
  background: var(--vp-c-bg);
  border: 1px solid var(--vp-c-divider);
  overflow-x: auto;
  font-size: 13px;
}
.dl-link {
  font-weight: 600;
  color: var(--vp-c-brand-1);
}
.dl-all {
  text-align: center;
  margin-top: 32px;
}

@media (max-width: 640px) {
  .dl-cards {
    grid-template-columns: 1fr;
  }
}
</style>
