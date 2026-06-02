<script setup lang="ts">
import { computed, onMounted } from 'vue'
import { marked } from 'marked'
import { invoke } from '@tauri-apps/api/core'
import { updateStore, initUpdateStore, checkForUpdates, startUpdate } from '../../stores/update'

onMounted(() => {
  initUpdateStore()
})

function openGitHub() {
  invoke('open_url', { url: 'https://github.com/chenwen245299/Argus' }).catch(console.error)
}

const renderedNotes = computed(() => {
  if (!updateStore.releaseNotes) return ''
  try {
    return marked.parse(updateStore.releaseNotes) as string
  } catch {
    return updateStore.releaseNotes
  }
})
</script>

<template>
  <!-- Release Notes Modal -->
  <Teleport to="body">
    <div v-if="updateStore.showReleaseNotes" class="rn-overlay" @click.self="updateStore.showReleaseNotes = false">
      <div class="rn-modal">
        <div class="rn-header">
          <div class="rn-title-wrap">
            <span class="rn-badge">v{{ updateStore.newVersion }}</span>
            <h2 class="rn-title">更新内容</h2>
          </div>
          <button class="rn-close" @click="updateStore.showReleaseNotes = false">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>
        <div class="rn-body markdown-body" v-html="renderedNotes" />
        <div class="rn-footer">
          <button class="rn-dismiss" @click="updateStore.showReleaseNotes = false">稍后</button>
          <button class="rn-install" @click="startUpdate(); updateStore.showReleaseNotes = false">立即更新</button>
        </div>
      </div>
    </div>
  </Teleport>

  <div class="about-settings">
    <!-- App hero -->
    <div class="app-hero">
      <div class="app-logo">
        <img src="/src-tauri/icons/128x128.png" alt="Argus" class="app-logo-img" />
      </div>
      <div class="app-info">
        <h1 class="app-name">Argus</h1>
        <span class="app-version">版本 {{ updateStore.version || '—' }}</span>
      </div>
    </div>

    <p class="app-desc">面向 AI 时代的本地优先文献管理工具。</p>

    <!-- Info card -->
    <div class="section-card">
      <div class="info-row">
        <span class="row-label">框架</span>
        <span class="row-val">Tauri 2 + Vue 3</span>
      </div>
      <div class="info-row">
        <span class="row-label">存储</span>
        <span class="row-val">本地文件系统 + SQLite</span>
      </div>
      <div class="info-row">
        <span class="row-label">许可</span>
        <span class="row-val">GPL-3.0</span>
      </div>
    </div>

    <!-- Update card -->
    <div class="section-card update-card">
      <div class="update-header">
        <span class="update-title">检查更新</span>
        <span v-if="updateStore.state === 'available'" class="update-badge">有新版本</span>
      </div>

      <div class="version-row">
        <span class="row-label">当前版本</span>
        <span class="row-val">{{ updateStore.version || '—' }}</span>
      </div>
      <div v-if="updateStore.state === 'available'" class="version-row">
        <span class="row-label">新版本</span>
        <span class="row-val new-version">{{ updateStore.newVersion }}</span>
      </div>

      <!-- Download progress -->
      <div v-if="updateStore.state === 'downloading'" class="progress-wrap">
        <div class="progress-bar">
          <div class="progress-fill" :style="{ width: updateStore.downloadProgress + '%' }" />
        </div>
        <span class="progress-label">{{ updateStore.downloadProgress }}%</span>
      </div>

      <!-- Error -->
      <p v-if="updateStore.errorMsg" class="error-msg">{{ updateStore.errorMsg }}</p>

      <!-- Action buttons -->
      <div class="update-actions">
        <div v-if="updateStore.showLatestFeedback" class="latest-tip">
          <span class="latest-dot" />
          已是最新版本
        </div>

        <button v-else-if="updateStore.state === 'checking'" class="btn-check" disabled>
          <span class="spinner" />
          检查中…
        </button>

        <button v-else-if="updateStore.state === 'downloading'" class="btn-check" disabled>
          <span class="spinner" />
          下载中…
        </button>

        <template v-else-if="updateStore.state === 'available'">
          <button v-if="updateStore.releaseNotes" class="btn-notes" @click="updateStore.showReleaseNotes = true">
            查看更新内容
          </button>
          <button class="btn-update" @click="startUpdate">
            立即更新到 {{ updateStore.newVersion }}
          </button>
        </template>

        <button v-else class="btn-check" @click="checkForUpdates(true)">
          检查更新
        </button>
      </div>

      <div class="star-sep" />
      <div class="star-row">
        <p class="star-text">如果觉得好用，欢迎给我们的项目点个 Star ⭐</p>
        <button class="github-link" @click="openGitHub">
          <svg class="github-icon" viewBox="0 0 24 24" fill="currentColor" width="15" height="15">
            <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0 0 24 12c0-6.63-5.37-12-12-12z"/>
          </svg>
          <span>Argus</span>
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.about-settings {
  padding: 32px 28px;
  max-width: 520px;
  width: 100%;
  align-self: center;
  display: flex;
  flex-direction: column;
  gap: 18px;
}

.app-hero {
  display: flex;
  align-items: center;
  gap: 16px;
}

.app-logo {
  width: 60px;
  height: 60px;
  border-radius: 14px;
  overflow: hidden;
  flex-shrink: 0;
  background: var(--bg-tertiary);
  display: flex;
  align-items: center;
  justify-content: center;
}

.app-logo-img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.app-info { display: flex; flex-direction: column; gap: 3px; }

.app-name {
  font-size: 24px;
  font-weight: 700;
  color: var(--text-primary);
  margin: 0;
  letter-spacing: 0;
}

.app-version {
  font-size: 13px;
  color: var(--text-tertiary);
}

.app-desc {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.6;
  margin: 0;
}

.section-card {
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  padding: 14px 16px;
  display: flex;
  flex-direction: column;
  gap: 9px;
}

.info-row,
.version-row {
  display: flex;
  align-items: center;
  gap: 12px;
}

.row-label {
  font-size: var(--font-size-sm);
  color: var(--text-tertiary);
  width: 66px;
  flex-shrink: 0;
}

.row-val {
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
}

/* Star section inside update card */
.star-sep {
  height: 1px;
  background: var(--border-subtle);
  margin: 2px 0;
}

.star-row {
  display: flex;
  align-items: center;
  gap: 12px;
  padding-top: 2px;
}

.star-text {
  flex: 1;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  margin: 0;
  line-height: 1.5;
}

.github-link {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 5px 11px;
  border-radius: var(--radius-md);
  background: var(--bg-tertiary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  font-weight: 500;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
  border: 1px solid var(--border-subtle);
}
.github-link:hover {
  background: var(--bg-hover);
  color: var(--accent);
}

.github-icon { flex-shrink: 0; }

/* Update card */
.update-card { gap: 10px; }

.update-header {
  display: flex;
  align-items: center;
  gap: 8px;
}

.update-title {
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.update-badge {
  font-size: 10px;
  font-weight: 600;
  color: #fff;
  background: var(--accent);
  border-radius: 6px;
  padding: 2px 7px;
}

.new-version { color: var(--accent); font-weight: 600; }

.progress-wrap {
  display: flex;
  align-items: center;
  gap: 10px;
}

.progress-bar {
  flex: 1;
  height: 5px;
  background: var(--bg-tertiary);
  border-radius: 3px;
  overflow: hidden;
}

.progress-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
  transition: width 0.3s ease;
}

.progress-label {
  font-size: 11px;
  color: var(--text-tertiary);
  width: 30px;
  text-align: right;
  flex-shrink: 0;
}

.error-msg {
  font-size: 12px;
  color: #e53e3e;
  margin: 0;
}

.update-actions {
  margin-top: 2px;
  display: flex;
  gap: 8px;
  align-items: center;
  flex-wrap: wrap;
}

.btn-check,
.btn-update,
.btn-notes {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-sm);
  font-weight: 500;
  padding: 6px 14px;
  border-radius: var(--radius-md);
  cursor: pointer;
  transition: opacity 0.15s;
}

.btn-check {
  background: var(--bg-tertiary);
  color: var(--text-secondary);
}
.btn-check:disabled { opacity: 0.6; cursor: default; }

.btn-update {
  background: var(--accent);
  color: #fff;
}
.btn-update:hover { opacity: 0.85; }

.btn-notes {
  background: var(--bg-tertiary);
  color: var(--accent);
}
.btn-notes:hover { opacity: 0.75; }

.latest-tip {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-sm);
  color: #34c759;
  font-weight: 500;
}

.latest-dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: #34c759;
  animation: pulse 1.8s ease-in-out infinite;
}

.spinner {
  width: 12px;
  height: 12px;
  border: 2px solid var(--border-default);
  border-top-color: var(--text-secondary);
  border-radius: 50%;
  animation: spin 0.7s linear infinite;
  flex-shrink: 0;
}

@keyframes spin { to { transform: rotate(360deg); } }
@keyframes pulse {
  0%, 100% { opacity: 1; transform: scale(1); }
  50%       { opacity: 0.5; transform: scale(0.85); }
}
</style>

<!-- Release notes modal (not scoped — targets v-html content) -->
<style>
.rn-overlay {
  position: fixed;
  inset: 0;
  z-index: 9999;
  background: rgba(0, 0, 0, 0.45);
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

.rn-modal {
  background: var(--bg-primary);
  border-radius: var(--radius-lg);
  border: 1px solid var(--border-subtle);
  box-shadow: var(--shadow-lg);
  width: 100%;
  max-width: 520px;
  max-height: 78vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.rn-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 16px 18px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.rn-title-wrap { display: flex; align-items: center; gap: 10px; }

.rn-badge {
  font-size: 11px;
  font-weight: 700;
  color: #fff;
  background: var(--accent);
  border-radius: 6px;
  padding: 2px 7px;
}

.rn-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--text-primary);
  margin: 0;
}

.rn-close {
  width: 26px;
  height: 26px;
  border: none;
  background: var(--bg-hover);
  border-radius: var(--radius-sm);
  color: var(--text-secondary);
  display: flex;
  align-items: center;
  justify-content: center;
  cursor: pointer;
  transition: background 0.1s;
}
.rn-close:hover { background: var(--bg-tertiary); }

.rn-body {
  flex: 1;
  overflow-y: auto;
  padding: 16px 18px;
  font-size: var(--font-size-sm);
  line-height: 1.7;
  color: var(--text-secondary);
  min-height: 0;
}
.rn-body::-webkit-scrollbar { width: 4px; }
.rn-body::-webkit-scrollbar-thumb { background: var(--border-default); border-radius: 2px; }

.rn-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  padding: 12px 18px 16px;
  border-top: 1px solid var(--border-subtle);
  flex-shrink: 0;
}

.rn-dismiss {
  padding: 6px 14px;
  border-radius: var(--radius-md);
  border: none;
  background: var(--bg-tertiary);
  color: var(--text-secondary);
  font-size: var(--font-size-sm);
  font-weight: 500;
  cursor: pointer;
  transition: background 0.1s;
}
.rn-dismiss:hover { background: var(--bg-hover); }

.rn-install {
  padding: 6px 16px;
  border-radius: var(--radius-md);
  border: none;
  background: var(--accent);
  color: #fff;
  font-size: var(--font-size-sm);
  font-weight: 500;
  cursor: pointer;
  transition: opacity 0.12s;
}
.rn-install:hover { opacity: 0.85; }

/* markdown content */
.rn-body.markdown-body p { margin: 0 0 8px; }
.rn-body.markdown-body p:last-child { margin-bottom: 0; }
.rn-body.markdown-body h1,
.rn-body.markdown-body h2,
.rn-body.markdown-body h3 { font-size: 13px; font-weight: 600; color: var(--text-primary); margin: 12px 0 5px; }
.rn-body.markdown-body ul,
.rn-body.markdown-body ol { padding-left: 1.4em; margin: 5px 0; }
.rn-body.markdown-body li { margin: 3px 0; }
.rn-body.markdown-body code {
  background: var(--bg-tertiary);
  border-radius: 4px;
  padding: 1px 5px;
  font-family: 'SF Mono', 'Menlo', monospace;
  font-size: 0.87em;
}
.rn-body.markdown-body hr { border: none; border-top: 1px solid var(--border-subtle); margin: 10px 0; }
</style>
