<script setup lang="ts">
import { onMounted, onUnmounted, ref } from 'vue'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import type { UnlistenFn } from '@tauri-apps/api/event'

// Our standalone windows run with native decorations OFF on Windows (see the
// `decorations(false)` calls in the Rust window builders), so — like the main
// window's titlebar — each window has to draw its own minimize / maximize /
// close controls. macOS keeps its native traffic lights, so this renders nothing
// there. Drop <WindowControls /> at the end of a custom titlebar.
const isWindows = navigator.userAgent.toLowerCase().includes('windows')
const appWindow = getCurrentWebviewWindow()
const isMaximized = ref(false)
let unlistenResize: UnlistenFn | null = null

async function refreshMaximized() {
  try {
    isMaximized.value = await appWindow.isMaximized()
  } catch {
    isMaximized.value = false
  }
}

async function minimize() {
  await appWindow.minimize().catch(() => {})
}
async function toggleMaximize() {
  await appWindow.toggleMaximize().catch(() => {})
  void refreshMaximized()
}
async function closeWindow() {
  await appWindow.close().catch(() => {})
}

onMounted(async () => {
  if (!isWindows) return
  await refreshMaximized()
  // Native maximize (double-click / snap) also flips the icon.
  unlistenResize = await appWindow.onResized(() => { void refreshMaximized() })
})

onUnmounted(() => {
  unlistenResize?.()
})
</script>

<template>
  <div v-if="isWindows" class="window-controls" @mousedown.stop>
    <button class="window-control-btn" title="最小化" @click="minimize">
      <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
        <path d="M1.5 5.5h8" />
      </svg>
    </button>
    <button class="window-control-btn" :title="isMaximized ? '还原' : '最大化'" @click="toggleMaximize">
      <svg v-if="isMaximized" width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
        <path d="M3.5 1.5h6v6h-6z" />
        <path d="M1.5 3.5v6h6" />
      </svg>
      <svg v-else width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
        <path d="M1.5 1.5h8v8h-8z" />
      </svg>
    </button>
    <button class="window-control-btn close" title="关闭" @click="closeWindow">
      <svg width="11" height="11" viewBox="0 0 11 11" aria-hidden="true">
        <path d="M2 2l7 7M9 2L2 9" />
      </svg>
    </button>
  </div>
</template>

<style scoped>
.window-controls {
  align-self: stretch;
  display: flex;
  align-items: stretch;
  flex-shrink: 0;
  -webkit-app-region: no-drag;
}
.window-control-btn {
  width: 46px;
  align-self: stretch;
  display: flex;
  align-items: center;
  justify-content: center;
  border: none;
  background: transparent;
  color: var(--text-secondary);
  cursor: pointer;
  transition: background 0.12s, color 0.12s;
}
.window-control-btn svg {
  fill: none;
  stroke: currentColor;
  stroke-width: 1.2;
  vector-effect: non-scaling-stroke;
}
.window-control-btn:hover {
  background: var(--bg-hover);
  color: var(--text-primary);
}
.window-control-btn.close:hover {
  background: #e81123;
  color: #fff;
}
</style>
