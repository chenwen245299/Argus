<script setup lang="ts">
import { ref, watch } from 'vue'
import { currentTranslation, translationHistory, clearTranslationHistory, deleteTranslationEntry } from '../../stores/translationHistory'

type View = 'current' | 'history'
const view = ref<View>('current')
const sourceExpanded = ref(false)

// When a new translation starts, switch back to current view and reset expand state.
watch(() => currentTranslation.sourceText, () => {
  view.value = 'current'
  sourceExpanded.value = false
})

function fmtTime(iso: string) {
  const d = new Date(iso)
  return d.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
}
</script>

<template>
  <div class="trans-tab">
    <!-- Header -->
    <div class="list-toolbar">
      <span class="list-heading">{{ view === 'history' ? '翻译记录' : '翻译' }}</span>
      <button
        v-if="view === 'history' && translationHistory.entries.length > 0"
        class="icon-btn"
        title="清空记录"
        @click="clearTranslationHistory"
      >
        <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <polyline points="3 6 5 6 21 6"/>
          <path d="M19 6l-1 14H6L5 6"/>
          <path d="M10 11v6M14 11v6"/>
          <path d="M9 6V4h6v2"/>
        </svg>
      </button>
      <button
        v-if="view === 'history'"
        class="icon-btn"
        title="返回当前翻译"
        @click="view = 'current'"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M18 6 6 18M6 6l12 12"/>
        </svg>
      </button>
      <button
        v-else
        class="icon-btn"
        title="翻译记录"
        @click="view = 'history'"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M12 8v4l3 3"/>
          <circle cx="12" cy="12" r="9"/>
        </svg>
      </button>
    </div>

    <!-- Current translation view -->
    <div v-if="view === 'current'" class="current-view">
      <div v-if="!currentTranslation.sourceText" class="empty">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="m5 8 6 6"/><path d="m4 14 6-6 2-3"/>
          <path d="M2 5h12"/><path d="M7 2h1"/>
          <path d="m22 22-5-10-5 10"/><path d="M14 18h6"/>
        </svg>
        <p>暂无翻译</p>
        <span>在 PDF 中选中文字后点击翻译</span>
      </div>

      <template v-else>
        <!-- Source text -->
        <div class="section-label source-label">
          <span>原文</span>
          <button
            type="button"
            class="source-toggle-btn"
            @click="sourceExpanded = !sourceExpanded"
          >
            <svg v-if="!sourceExpanded" width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="6 9 12 15 18 9"/></svg>
            <svg v-else width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5"><polyline points="18 15 12 9 6 15"/></svg>
            {{ sourceExpanded ? '折叠原文' : '展开原文' }}
          </button>
        </div>
        <div
          class="source-text selectable-text"
          :class="{ 'source-collapsed': !sourceExpanded }"
        >{{ currentTranslation.sourceText }}</div>

        <div class="divider" />

        <!-- Translation result -->
        <div class="section-label">
          译文
          <span v-if="currentTranslation.loading" class="loading-dot" />
        </div>
        <div class="result-text selectable-text">
          <template v-if="currentTranslation.error">
            <span class="error-text">{{ currentTranslation.error }}</span>
          </template>
          <template v-else>
            {{ currentTranslation.result }}
            <span v-if="currentTranslation.loading" class="cursor">▋</span>
          </template>
        </div>
      </template>
    </div>

    <!-- History view -->
    <template v-else>
      <div v-if="translationHistory.entries.length === 0" class="empty">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M12 8v4l3 3"/><circle cx="12" cy="12" r="9"/>
        </svg>
        <p>暂无翻译记录</p>
      </div>

      <ul v-else class="entry-list">
        <li v-for="entry in translationHistory.entries" :key="entry.id" class="entry-item">
          <div class="entry-header">
            <span class="entry-time">{{ fmtTime(entry.createdAt) }}</span>
            <button class="entry-delete-btn" title="删除" @click="deleteTranslationEntry(entry.id)">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5">
                <line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/>
              </svg>
            </button>
          </div>
          <div class="entry-source selectable-text">{{ entry.sourceText }}</div>
          <div class="entry-divider" />
          <div class="entry-result selectable-text">{{ entry.result }}</div>
        </li>
      </ul>
    </template>
  </div>
</template>

<style scoped>
.trans-tab {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

.list-toolbar {
  display: flex;
  align-items: center;
  height: 40px;
  padding: 0 10px 0 12px;
  background: var(--bg-secondary);
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  gap: 4px;
}

.list-heading {
  flex: 1;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-primary);
}

.icon-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  background: transparent;
  transition: background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.icon-btn:hover { background: var(--bg-tertiary); color: var(--text-secondary); }

/* ── Current translation view ── */
.current-view {
  flex: 1;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
  padding: 14px;
  gap: 8px;
}

.empty {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  color: var(--text-tertiary);
  padding: 24px;
}
.empty p { font-size: var(--font-size-sm); font-weight: 500; color: var(--text-secondary); margin: 0; }
.empty span { font-size: var(--font-size-xs); text-align: center; }

.section-label {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: var(--font-size-sm);
  font-weight: 600;
  color: var(--text-tertiary);
  text-transform: uppercase;
  letter-spacing: 0.04em;
}
.source-label {
  justify-content: space-between;
}
.source-toggle-btn {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 2px 6px;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-tertiary);
  font-size: var(--font-size-sm);
  font-weight: 600;
  letter-spacing: 0;
  text-transform: none;
  transition: background 0.1s, color 0.1s;
}
.source-toggle-btn:hover {
  background: var(--bg-secondary);
  color: var(--text-secondary);
}

.loading-dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent);
  animation: pulse 1s ease-in-out infinite;
}

.source-text {
  font-size: var(--font-size-md);
  color: var(--text-secondary);
  line-height: 1.6;
  white-space: pre-wrap;
  background: var(--bg-secondary);
  border-radius: var(--radius-md);
  padding: 10px 12px;
  border: 1px solid var(--border-subtle);
  overflow-y: auto;
}
.source-text.source-collapsed {
  /* ~10 lines: 14px * 1.6 * 10 + 20px padding */
  max-height: 244px;
}

.divider {
  height: 1px;
  background: var(--border-subtle);
  flex-shrink: 0;
}

.result-text {
  font-size: var(--font-size-md);
  color: var(--text-primary);
  line-height: 1.7;
  white-space: pre-wrap;
  flex: 1;
  min-height: 60px;
}

.cursor {
  display: inline-block;
  animation: blink 0.8s step-end infinite;
  color: var(--accent);
  font-size: 0.9em;
  line-height: 1;
}

.error-text { color: #e53e3e; }

/* ── History list ── */
.entry-list {
  list-style: none;
  overflow-y: auto;
  flex: 1;
  padding: 6px 0;
}

.entry-item {
  padding: 10px 14px;
  border-bottom: 1px solid var(--border-subtle);
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.entry-item:last-child { border-bottom: none; }

.entry-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.entry-time {
  font-size: 11px;
  color: var(--text-tertiary);
}

.entry-delete-btn {
  display: flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  border-radius: var(--radius-sm);
  color: var(--text-tertiary);
  opacity: 0;
  transition: opacity 0.1s, background 0.1s, color 0.1s;
  flex-shrink: 0;
}
.entry-item:hover .entry-delete-btn { opacity: 1; }
.entry-delete-btn:hover { background: var(--bg-hover); color: #e53e3e; }

.entry-source {
  font-size: var(--font-size-md);
  color: var(--text-secondary);
  line-height: 1.5;
  white-space: pre-wrap;
}

.entry-divider {
  height: 1px;
  background: var(--border-subtle);
}

.entry-result {
  font-size: var(--font-size-md);
  color: var(--text-primary);
  line-height: 1.6;
  white-space: pre-wrap;
}

@keyframes pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.3; }
}

@keyframes blink {
  0%, 100% { opacity: 1; }
  50% { opacity: 0; }
}
</style>
