import { ref, watch, onMounted, onUnmounted, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { useActivityStore } from '../stores/activity'
import { useReadingWindowFocus } from './useReadingWindowFocus'
import type { PaperMeta } from '../types'

/** Standalone paper AI has its own writer cell, even when main is closed. */
export function usePaperAiReading(slug: Ref<string | null>) {
  const activity = useActivityStore()
  const focused = useReadingWindowFocus()
  const paper = ref<{ slug: string; title: string; fileType?: string } | null>(null)
  const source = getCurrentWebviewWindow().label
  let revision = 0, disposed = false, off: UnlistenFn | null = null
  async function refresh(path?: string | null) {
    const epoch = ++revision, currentSlug = slug.value
    paper.value = null
    await activity.load(null, source)
    try {
      const root = path === undefined ? await invoke<string | null>('get_current_library') : path
      if (!root || !currentSlug || disposed || epoch !== revision) return
      const meta = await invoke<PaperMeta>('get_paper_meta', { slug: currentSlug })
      if (disposed || epoch !== revision || currentSlug !== slug.value) return
      await activity.load(root, source)
      if (disposed || epoch !== revision || currentSlug !== slug.value) return
      paper.value = { slug: currentSlug, title: meta.title || currentSlug, fileType: meta.file_type }
    } catch (e) { console.error('[activity] paper AI reading setup failed:', e) }
  }
  function sync() {
    const info = paper.value
    if (focused.value && info && activity.libraryPath) activity.beginReading(info.slug, info.title, info.fileType)
    else activity.endReading()
  }
  const stopBeforeUnload = () => activity.endReading()
  watch([focused, paper], sync, { flush: 'sync' })
  watch(slug, () => { void refresh() })
  onMounted(async () => {
    activity.startHeartbeat()
    window.addEventListener('beforeunload', stopBeforeUnload)
    try {
      const stop = await listen<string | null>('library-changed', event => { void refresh(event.payload) })
      if (disposed) { stop(); return }
      off = stop
      await refresh()
    } catch (e) { console.error('[activity] paper AI reading listener failed:', e) }
  })
  onUnmounted(() => {
    disposed = true; revision++; off?.()
    window.removeEventListener('beforeunload', stopBeforeUnload)
    activity.stopHeartbeat()
    void activity.load(null, source)
  })
}
