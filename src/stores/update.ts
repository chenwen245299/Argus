import { reactive, shallowRef } from 'vue'
import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import type { Update } from '@tauri-apps/plugin-updater'
import { getVersion } from '@tauri-apps/api/app'

export const updateStore = reactive({
  state: 'idle' as 'idle' | 'checking' | 'available' | 'downloading' | 'error',
  hasUpdate: false,
  version: '',
  newVersion: '',
  releaseNotes: '',
  downloadProgress: 0,
  errorMsg: '',
  showLatestFeedback: false,
  showReleaseNotes: false,
})

const updateManifest = shallowRef<Update | null>(null)

export const initUpdateStore = async () => {
  try {
    updateStore.version = await getVersion()
  } catch (e) {
    console.error(e)
  }
}

export const checkForUpdates = async (manual: boolean) => {
  if (updateStore.state === 'downloading') return

  if (manual) {
    updateStore.state = 'checking'
    updateStore.showLatestFeedback = false
  }
  updateStore.errorMsg = ''

  try {
    const update = await check()

    if (update) {
      updateStore.hasUpdate = true
      updateStore.newVersion = update.version
      updateStore.releaseNotes = update.body ?? ''
      updateManifest.value = update
      updateStore.state = 'available'
      if (manual && updateStore.releaseNotes) {
        updateStore.showReleaseNotes = true
      }
    } else {
      updateStore.hasUpdate = false
      updateStore.state = 'idle'
      if (manual) {
        updateStore.showLatestFeedback = true
        setTimeout(() => { updateStore.showLatestFeedback = false }, 3000)
      }
    }
  } catch (err: any) {
    console.error(err)
    updateStore.state = 'idle'
    if (manual) {
      updateStore.errorMsg = '检查失败：' + (err.message || err)
    }
  }
}

export const startUpdate = async () => {
  if (!updateManifest.value) return

  updateStore.state = 'downloading'
  updateStore.downloadProgress = 0
  updateStore.errorMsg = ''

  try {
    let downloadedBytes = 0
    let totalBytes = 0
    let lastPercentage = 0

    await updateManifest.value.downloadAndInstall((event: any) => {
      if (event.event === 'Started') {
        totalBytes = event.data.contentLength || 0
      } else if (event.event === 'Progress') {
        downloadedBytes += event.data.chunkLength
        if (totalBytes > 0) {
          const pct = Math.floor((downloadedBytes / totalBytes) * 100)
          if (pct > lastPercentage) {
            updateStore.downloadProgress = pct
            lastPercentage = pct
          }
        } else {
          if (updateStore.downloadProgress < 90 && Math.random() > 0.8) {
            updateStore.downloadProgress += 1
          }
        }
      } else if (event.event === 'Finished') {
        updateStore.downloadProgress = 100
      }
    })

    updateStore.downloadProgress = 100
    await relaunch()
  } catch (err: any) {
    console.error(err)
    updateStore.state = 'error'
    updateStore.errorMsg = '更新失败：' + (err.message || err)
  }
}
