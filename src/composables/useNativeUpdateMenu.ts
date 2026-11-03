import { onMounted, onUnmounted } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { checkForUpdates } from '../stores/update'

/** Let the main window own native menu checks and their visible feedback. */
export function useNativeUpdateMenu(openAbout: () => void) {
  let disposed = false
  let unlisten: UnlistenFn | undefined

  async function handleRequest() {
    if (disposed) return
    const requested = await invoke<boolean>('take_update_check_request')
    if (!requested || disposed) return
    openAbout()
    await checkForUpdates(true)
  }

  onMounted(async () => {
    try {
      unlisten = await listen('argus-check-for-updates', () => {
        void handleRequest().catch(console.error)
      })
      if (disposed) { unlisten(); return }
      // The native menu can be clicked before this webview finishes loading.
      await handleRequest()
    } catch (error) {
      console.error('Native update menu:', error)
    }
  })
  onUnmounted(() => { disposed = true; unlisten?.() })
}
