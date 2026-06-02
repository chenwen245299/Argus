<script setup lang="ts">
import { computed } from 'vue'
import { Handle, Position } from '@vue-flow/core'
import type { NodeProps } from '@vue-flow/core'

export interface PaperNodeData {
  title: string
  authors: string[]
  year?: number
  venue?: string
  paperId: string
  nodeId: string
  valid: boolean
  color?: string
  noteTitles?: string[]
}

const props = defineProps<NodeProps<PaperNodeData>>()

const authorsShort = computed(() => {
  const a = props.data.authors ?? []
  if (a.length === 0) return ''
  if (a.length === 1) return a[0]
  return `${a[0]} 等`
})

const nodeStyle = computed(() => {
  if (props.data.color) {
    return {
      borderColor: props.data.color,
      background: `${props.data.color}18`,
      boxShadow: `0 0 0 2px ${props.data.color}28`,
    }
  }
  return {}
})

const accentBarStyle = computed(() =>
  props.data.color ? { background: props.data.color } : null
)
</script>

<template>
  <div
    class="paper-node"
    :class="{ 'paper-node--selected': selected, 'paper-node--invalid': !data.valid }"
    :style="nodeStyle"
  >
    <!-- Color accent bar -->
    <div v-if="accentBarStyle" class="node-color-bar" :style="accentBarStyle" />

    <!-- Source handles: right + bottom (this node as upstream) -->
    <Handle id="src-right" type="source" :position="Position.Right" class="node-handle" />
    <Handle id="src-bottom" type="source" :position="Position.Bottom" class="node-handle" />
    <!-- Target handles: left + top (this node as downstream) -->
    <Handle id="tgt-left" type="target" :position="Position.Left" class="node-handle" />
    <Handle id="tgt-top" type="target" :position="Position.Top" class="node-handle" />

    <div v-if="!data.valid" class="node-invalid-badge">已删除</div>

    <div class="node-title" :title="data.title">{{ data.title || '未知论文' }}</div>
    <div class="node-meta">
      <span v-if="authorsShort" class="node-authors">{{ authorsShort }}</span>
      <span v-if="data.year" class="node-year">{{ data.year }}</span>
    </div>
    <div v-if="data.venue" class="node-venue" :title="data.venue">{{ data.venue }}</div>
    <div v-if="data.noteTitles && data.noteTitles.length > 0" class="node-notes">
      <span v-for="title in data.noteTitles" :key="title" class="node-note-badge" :title="title">{{ title }}</span>
    </div>
  </div>
</template>

<style scoped>
.paper-node {
  position: relative;
  width: 200px;
  background: var(--bg-primary, #fff);
  border: 1.5px solid var(--border-default, #d1d5db);
  border-radius: 10px;
  padding: 10px 12px 10px 16px;
  overflow: hidden;
  cursor: default;
  box-shadow: 0 1px 4px rgba(0,0,0,0.08);
  transition: border-color 0.15s, box-shadow 0.15s;
  user-select: none;
}

.paper-node:hover {
  border-color: var(--accent, #6366f1);
  box-shadow: 0 2px 10px rgba(0,0,0,0.12);
}

.paper-node--selected {
  border-color: var(--accent, #6366f1) !important;
  box-shadow: 0 0 0 3px var(--accent-light, #e0e7ff) !important;
}

.paper-node--invalid {
  opacity: 0.55;
  border-color: #ef4444 !important;
  background: #fef2f2;
}

.node-color-bar {
  position: absolute;
  left: 0;
  top: 0;
  bottom: 0;
  width: 4px;
  border-radius: 10px 0 0 10px;
}

.node-invalid-badge {
  position: absolute;
  top: -8px;
  right: 6px;
  background: #ef4444;
  color: #fff;
  font-size: 10px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 4px;
}

.node-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-primary, #111);
  line-height: 1.4;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  overflow: hidden;
  word-break: break-word;
  margin-bottom: 6px;
}

.node-meta {
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.node-authors {
  font-size: 10px;
  color: var(--text-secondary, #6b7280);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 130px;
}

.node-year {
  font-size: 10px;
  color: var(--accent, #6366f1);
  font-weight: 600;
  white-space: nowrap;
  flex-shrink: 0;
}

.node-venue {
  margin-top: 4px;
  font-size: 10px;
  color: var(--text-tertiary, #9ca3af);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.node-notes {
  margin-top: 6px;
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
}

.node-note-badge {
  display: inline-block;
  max-width: 160px;
  padding: 1px 6px;
  border-radius: 4px;
  background: var(--bg-hover, #f3f4f6);
  border: 1px solid var(--border-default, #e5e7eb);
  font-size: 10px;
  color: var(--text-secondary, #6b7280);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

/* Vue Flow handle overrides */
:deep(.node-handle) {
  width: 10px;
  height: 10px;
  background: var(--accent, #6366f1);
  border: 2px solid #fff;
  border-radius: 50%;
  opacity: 0;
  transition: opacity 0.15s;
}

.paper-node:hover :deep(.node-handle),
.paper-node--selected :deep(.node-handle) {
  opacity: 1;
}
</style>
