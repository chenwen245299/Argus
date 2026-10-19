import { computed, ref, onMounted, onUnmounted } from 'vue'
import { getCurrentWindow } from '@tauri-apps/api/window'
import type { UnlistenFn } from '@tauri-apps/api/event'

/** OS focus chooses one reading window; visible background windows never count. */
export function useReadingWindowFocus() {
  const focused = ref(document.hasFocus())
  const visible = ref(document.visibilityState !== 'hidden')
  let disposed = false, revision = 0, off: UnlistenFn | null = null
  const focus = () => { revision++; focused.value = true }
  const blur = () => { revision++; focused.value = false }
  const visibility = () => { visible.value = document.visibilityState !== 'hidden' }
  onMounted(async () => {
    window.addEventListener('focus', focus)
    window.addEventListener('blur', blur)
    document.addEventListener('visibilitychange', visibility)
    try {
      const current = getCurrentWindow()
      const stop = await current.onFocusChanged(event => {
        revision++; focused.value = event.payload
      })
      if (disposed) { stop(); return }
      off = stop
      const sample = revision
      const initial = await current.isFocused()
      if (!disposed && revision === sample) focused.value = initial
    } catch { /* Browser preview uses DOM focus/visibility. */ }
  })
  onUnmounted(() => {
    disposed = true; off?.()
    window.removeEventListener('focus', focus)
    window.removeEventListener('blur', blur)
    document.removeEventListener('visibilitychange', visibility)
  })
  return computed(() => focused.value && visible.value)
}
