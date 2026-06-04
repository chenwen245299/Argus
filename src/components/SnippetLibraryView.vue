<script setup lang="ts">
import { ref, computed } from 'vue'
import { useI18n } from 'vue-i18n'
import {
  libraries,
  snippets,
  deleteSnippet,
  type Snippet,
} from '../stores/snippetLibrary'

const props = defineProps<{ libraryId: string }>()
const emit = defineEmits<{ 'open-paper': [slug: string, page: number, title: string] }>()
const { t } = useI18n()

const searchQuery = ref('')

const library = computed(() => libraries.value.find(l => l.id === props.libraryId))

const items = computed(() => {
  const q = searchQuery.value.toLowerCase().trim()
  const list = snippets.value
    .filter(s => s.libraryId === props.libraryId)
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
  if (!q) return list
  return list.filter(s =>
    s.text.toLowerCase().includes(q) ||
    s.paperTitle.toLowerCase().includes(q) ||
    s.note.toLowerCase().includes(q) ||
    s.tags.some(tag => tag.toLowerCase().includes(q))
  )
})

function openPaper(s: Snippet) {
  emit('open-paper', s.paperId, s.page, s.paperTitle)
}

function handleDelete(s: Snippet) {
  if (!window.confirm(`删除这条素材？`)) return
  deleteSnippet(s.id)
}

function truncate(text: string, max = 120) {
  return text.length > max ? text.slice(0, max) + '…' : text
}
</script>

<template>
  <div class="snippet-view">
    <!-- Header -->
    <div class="snippet-header">
      <div class="header-left">
        <span v-if="library?.emoji" class="lib-emoji">{{ library.emoji }}</span>
        <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
        </svg>
        <span class="lib-name">{{ library?.name ?? t('snippets.title') }}</span>
        <span class="item-count">{{ items.length }}</span>
      </div>
      <div class="search-wrap">
        <svg class="search-icon" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2">
          <circle cx="11" cy="11" r="8"/><line x1="21" y1="21" x2="16.65" y2="16.65"/>
        </svg>
        <input v-model="searchQuery" class="search-input" :placeholder="t('snippets.search')" />
      </div>
    </div>

    <!-- Empty state -->
    <div v-if="items.length === 0" class="empty-state">
      <svg width="40" height="40" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4" opacity="0.35">
        <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/>
        <line x1="12" y1="11" x2="12" y2="17"/>
        <line x1="9" y1="14" x2="15" y2="14"/>
      </svg>
      <p class="empty-title">{{ t('snippets.noSnippets') }}</p>
      <p class="empty-hint">{{ t('snippets.addHint') }}</p>
    </div>

    <!-- Table -->
    <div v-else class="table-wrap">
      <table class="snippet-table">
        <thead>
          <tr>
            <th class="col-tags">{{ t('snippets.tags') }}</th>
            <th class="col-content">{{ t('snippets.sourceText') }}</th>
            <th class="col-paper">{{ t('snippets.sourcePaper') }}</th>
            <th class="col-note">{{ t('snippets.note') }}</th>
            <th class="col-action"></th>
          </tr>
        </thead>
        <tbody>
          <template v-for="s in items" :key="s.id">
            <tr class="snippet-row" style="cursor:pointer" @dblclick="openPaper(s)">
              <td class="col-tags">
                <div class="tag-list">
                  <span v-for="tag in s.tags" :key="tag" class="tag-chip">{{ tag }}</span>
                  <span v-if="s.tags.length === 0" class="no-tag">—</span>
                </div>
              </td>
              <td class="col-content">
                <span
                  class="content-preview"
                  :style="s.color ? { borderLeft: `3px solid ${s.color}`, paddingLeft: '7px', background: s.color + '22' } : {}"
                >{{ truncate(s.text) }}</span>
              </td>
              <td class="col-paper">
                <span class="paper-title">{{ s.paperTitle }}</span>
                <span class="page-num">P{{ s.page }}</span>
              </td>
              <td class="col-note">
                <span class="note-preview">{{ s.note || '—' }}</span>
              </td>
              <td class="col-action" @click.stop>
                <button class="del-btn" :title="t('hl.delete')" @click="handleDelete(s)">
                  <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                    <polyline points="3 6 5 6 21 6"/>
                    <path d="M19 6l-1 14H6L5 6"/>
                    <path d="M10 11v6"/><path d="M14 11v6"/>
                    <path d="M9 6V4h6v2"/>
                  </svg>
                </button>
              </td>
            </tr>
          </template>
        </tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.snippet-view {
  display: flex;
  flex-direction: column;
  height: 100%;
  background: var(--bg-primary);
}

.snippet-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 14px 20px 12px;
  border-bottom: 1px solid var(--border-subtle);
  flex-shrink: 0;
  gap: 12px;
}

.header-left {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-secondary);
}

.lib-emoji { font-size: 18px; }

.lib-name {
  font-size: 16px;
  font-weight: 600;
  color: var(--text-primary);
}

.item-count {
  font-size: 12px;
  color: var(--text-tertiary);
  background: var(--bg-secondary);
  border-radius: 10px;
  padding: 1px 7px;
}

.search-wrap {
  display: flex;
  align-items: center;
  gap: 6px;
  background: var(--bg-secondary);
  border: 1px solid var(--border-subtle);
  border-radius: 6px;
  padding: 5px 10px;
}

.search-icon { color: var(--text-tertiary); flex-shrink: 0; }

.search-input {
  border: none;
  background: transparent;
  outline: none;
  font-size: 13px;
  color: var(--text-primary);
  width: 160px;
}
.search-input::placeholder { color: var(--text-tertiary); }

/* Empty state */
.empty-state {
  flex: 1;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  padding: 40px;
  color: var(--text-secondary);
}

.empty-title {
  font-size: 15px;
  font-weight: 500;
  margin: 4px 0 0;
}

.empty-hint {
  font-size: 13px;
  color: var(--text-tertiary);
  text-align: center;
}

/* Table */
.table-wrap {
  flex: 1;
  overflow-y: auto;
  padding: 0 16px 16px;
}

.snippet-table {
  width: 100%;
  border-collapse: collapse;
  font-size: 13px;
}

.snippet-table th {
  text-align: left;
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-tertiary);
  padding: 10px 8px 8px;
  border-bottom: 1px solid var(--border-subtle);
  position: sticky;
  top: 0;
  background: var(--bg-primary);
  z-index: 1;
}

.snippet-row {
  transition: background 0.1s;
}
.snippet-row:hover { background: var(--bg-hover); }

.snippet-row td {
  padding: 9px 8px;
  border-bottom: 1px solid var(--border-subtle);
  vertical-align: middle;
}

.col-tags { width: 120px; }
.col-content { }
.col-paper { width: 180px; }
.col-note { width: 140px; }
.col-action { width: 36px; text-align: center; }

.tag-list {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.tag-chip {
  display: inline-block;
  padding: 1px 7px;
  border-radius: 10px;
  background: color-mix(in srgb, var(--accent) 12%, transparent);
  color: var(--accent);
  font-size: 11px;
  white-space: nowrap;
}

.no-tag { color: var(--text-tertiary); }

.content-preview {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  line-height: 1.5;
  color: var(--text-primary);
}

.paper-title {
  display: block;
  color: var(--text-primary);
  overflow: hidden;
  white-space: nowrap;
  text-overflow: ellipsis;
  max-width: 170px;
}

.page-num {
  font-size: 11px;
  color: var(--text-tertiary);
}

.note-preview {
  display: -webkit-box;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
  overflow: hidden;
  color: var(--text-secondary);
}

.del-btn {
  color: var(--text-tertiary);
  padding: 4px;
  border-radius: 4px;
  opacity: 0;
  transition: opacity 0.15s;
}
.snippet-row:hover .del-btn { opacity: 1; }
.del-btn:hover { color: #e53935; background: var(--bg-hover); }

</style>
