<script setup lang="ts">
import { Icon } from '@iconify/vue'
import type { ThemeMeta } from '../../utils/themes'

// A single marketplace card: a mini app-window painted with the theme's real
// palette, plus its name and a check when selected. The parent owns what
// "selected" means (the fixed palette, or a per-OS-appearance slot).
defineProps<{
  theme: ThemeMeta
  name: string
  active: boolean
  index?: number
}>()

defineEmits<{ (e: 'select'): void }>()
</script>

<template>
  <button
    class="theme-card"
    :class="{ active }"
    :style="{ '--i': index ?? 0 }"
    @click="$emit('select')"
  >
    <!-- Mini app window preview, painted with the theme's real palette -->
    <div class="theme-preview" :style="{ background: theme.preview.bg }">
      <div class="preview-chrome"><i /><i /><i /></div>
      <div class="preview-main">
        <div class="preview-sidebar" :style="{ background: theme.preview.sidebar }">
          <div class="side-pill" :style="{ background: theme.preview.accent }" />
          <div class="side-line" :style="{ background: theme.preview.text }" />
          <div class="side-line" :style="{ background: theme.preview.text }" />
        </div>
        <div class="preview-content">
          <div class="content-bar" :style="{ background: theme.preview.accent }" />
          <div class="content-line" :style="{ background: theme.preview.text }" />
          <div class="content-line short" :style="{ background: theme.preview.text }" />
          <div class="preview-chips">
            <span class="chip" :style="{ background: theme.preview.accent }" />
            <span class="chip chip-ghost" :style="{ borderColor: theme.preview.accent }" />
          </div>
        </div>
      </div>
    </div>
    <div class="theme-footer">
      <span class="theme-name">{{ name }}</span>
      <Icon v-if="active" icon="fluent:checkmark-24-filled" class="check-icon" width="14" height="14" />
    </div>
  </button>
</template>

<style scoped>
.theme-card {
  display: flex;
  flex-direction: column;
  border: 2px solid var(--border-default);
  border-radius: var(--radius-lg);
  overflow: hidden;
  cursor: pointer;
  background: var(--bg-secondary);
  text-align: left;
  transition: border-color 0.15s, box-shadow 0.15s, transform 0.15s;
  animation: theme-card-in 0.34s cubic-bezier(0.34, 1.4, 0.64, 1) both;
  animation-delay: calc(var(--i, 0) * 26ms);
}
.theme-card:hover {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light), var(--shadow-md);
  transform: translateY(-2px);
}
.theme-card.active {
  border-color: var(--accent);
  box-shadow: 0 0 0 3px var(--accent-light);
}

@keyframes theme-card-in {
  from { opacity: 0; transform: translateY(10px) scale(0.96); }
  to   { opacity: 1; transform: translateY(0) scale(1); }
}

/* ── Mini app window preview ── */
.theme-preview {
  height: 88px;
  display: flex;
  flex-direction: column;
  flex-shrink: 0;
  overflow: hidden;
}

/* Window chrome: macOS traffic lights */
.preview-chrome {
  height: 15px;
  display: flex;
  align-items: center;
  gap: 3.5px;
  padding: 0 7px;
  flex-shrink: 0;
  border-bottom: 1px solid rgba(128, 128, 128, 0.16);
}
.preview-chrome i { width: 4.5px; height: 4.5px; border-radius: 50%; }
.preview-chrome i:nth-child(1) { background: #ff5f57; }
.preview-chrome i:nth-child(2) { background: #ffbd2e; }
.preview-chrome i:nth-child(3) { background: #28c840; }

.preview-main {
  flex: 1;
  display: flex;
  min-height: 0;
}

.preview-sidebar {
  width: 30%;
  flex-shrink: 0;
  padding: 6px 5px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.side-pill {
  height: 5px;
  width: 82%;
  border-radius: 3px;
  opacity: 0.9;
  margin-bottom: 1px;
}
.side-line {
  height: 4px;
  width: 68%;
  border-radius: 2px;
  opacity: 0.22;
}

.preview-content {
  flex: 1;
  padding: 7px 8px 6px;
  display: flex;
  flex-direction: column;
  gap: 5px;
  min-width: 0;
}
.content-bar {
  height: 6px;
  width: 46%;
  border-radius: 3px;
  opacity: 0.9;
}
.content-line {
  height: 4px;
  width: 92%;
  border-radius: 2px;
  opacity: 0.18;
}
.content-line.short { width: 62%; opacity: 0.13; }

.preview-chips {
  display: flex;
  gap: 5px;
  margin-top: auto;
}
.chip {
  height: 9px;
  width: 26px;
  border-radius: var(--radius-pill);
}
.chip-ghost {
  background: transparent;
  border: 1.5px solid;
  box-sizing: border-box;
}

/* ── Card footer ── */
.theme-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 10px;
  gap: 4px;
}
.theme-name {
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--text-secondary);
}
.theme-card.active .theme-name { color: var(--accent); font-weight: 600; }

.check-icon {
  color: var(--accent);
  flex-shrink: 0;
}
</style>
