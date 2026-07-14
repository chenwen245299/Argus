<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import { invoke } from '@tauri-apps/api/core'
import { renderMarkdown } from '../utils/renderMarkdown'
import { updateStore, startUpdate } from '../stores/update'

const { t } = useI18n()

function openGitHub() {
  invoke('open_url', { url: 'https://github.com/chenwen245299/Argus' }).catch(console.error)
}

// Route release notes (fetched from the remote GitHub release) through the
// shared renderer, which sanitizes with DOMPurify and forces external links to
// open with rel="noopener" — no bespoke, laxer sanitize config here.
const renderedNotes = computed(() => {
  if (!updateStore.releaseNotes) return ''
  return renderMarkdown(updateStore.releaseNotes)
})
</script>

<template>
  <Teleport to="body">
    <div v-if="updateStore.showReleaseNotes" class="rn-overlay" @click.self="updateStore.showReleaseNotes = false">
      <div class="rn-modal">
        <div class="rn-header">
          <div class="rn-title-wrap">
            <span class="rn-badge">v{{ updateStore.newVersion }}</span>
            <h2 class="rn-title">{{ t('about.releaseNotesTitle') }}</h2>
          </div>
          <button class="rn-close" @click="updateStore.showReleaseNotes = false">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
              <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
            </svg>
          </button>
        </div>
        <div v-if="renderedNotes" class="rn-body markdown-body" v-html="renderedNotes" />
        <div v-else class="rn-body rn-body-empty">{{ t('about.updateAvailableDesc', { v: updateStore.newVersion }) }}</div>
        <div class="rn-star">
          <p class="rn-star-text">{{ t('about.starText') }}</p>
          <button class="rn-star-btn" @click="openGitHub">
            <svg class="rn-github-icon" viewBox="0 0 24 24" fill="currentColor" width="15" height="15">
              <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0 0 24 12c0-6.63-5.37-12-12-12z"/>
            </svg>
            <span>Star</span>
          </button>
        </div>
        <div class="rn-footer">
          <button class="rn-dismiss" @click="updateStore.showReleaseNotes = false">{{ t('about.later') }}</button>
          <button class="rn-install" @click="startUpdate(); updateStore.showReleaseNotes = false">{{ t('about.updateNow') }}</button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<!-- Not scoped — targets v-html markdown content -->
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

.rn-body-empty { flex: none; }

.rn-star {
  display: flex;
  align-items: center;
  gap: 12px;
  padding: 12px 18px;
  border-top: 1px solid var(--border-subtle);
  background: color-mix(in srgb, var(--accent) 6%, var(--bg-primary));
  flex-shrink: 0;
}

.rn-star-text {
  flex: 1;
  margin: 0;
  font-size: var(--font-size-sm);
  color: var(--text-secondary);
  line-height: 1.5;
}

.rn-star-btn {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-shrink: 0;
  padding: 5px 13px;
  border-radius: var(--radius-md);
  background: var(--bg-tertiary);
  color: var(--text-primary);
  font-size: var(--font-size-sm);
  font-weight: 500;
  cursor: pointer;
  border: 1px solid var(--border-subtle);
  transition: background 0.15s, color 0.15s;
}
.rn-star-btn:hover {
  background: var(--bg-hover);
  color: var(--accent);
}

.rn-github-icon { flex-shrink: 0; }

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
