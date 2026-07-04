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
  // Last version we auto-popped the dialog for, so scheduled checks don't re-nag
  // for a version the user has already seen this session.
  lastPromptedVersion: '',
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
      // Manual checks always surface the dialog; auto checks pop it up once per
      // newly-discovered version so a user who keeps the app running still gets
      // notified without being nagged on every scheduled check.
      if (manual || updateStore.lastPromptedVersion !== update.version) {
        updateStore.showReleaseNotes = true
        updateStore.lastPromptedVersion = update.version
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

// ── Auto-update scheduling ──────────────────────────────────────────────────
// Many users leave the app running for days, so a one-shot check at launch
// rarely catches a release. We run a check shortly after startup and then once
// a day at 9am for as long as the window stays open.

let startupTimer: ReturnType<typeof setTimeout> | null = null
let dailyTimer: ReturnType<typeof setTimeout> | null = null

const HOUR_OF_DAY = 9

function msUntilNext9am(): number {
  const now = new Date()
  const next = new Date(now)
  next.setHours(HOUR_OF_DAY, 0, 0, 0)
  if (next.getTime() <= now.getTime()) {
    next.setDate(next.getDate() + 1)
  }
  return next.getTime() - now.getTime()
}

export const startAutoUpdateSchedule = () => {
  stopAutoUpdateSchedule()

  // Catch-up check a little after launch (endpoint/network settle time).
  startupTimer = setTimeout(() => { checkForUpdates(false) }, 60 * 1000)

  // Recurring daily check at 9am; re-arm each day since setTimeout is one-shot.
  const scheduleNextDaily = () => {
    dailyTimer = setTimeout(() => {
      checkForUpdates(false)
      scheduleNextDaily()
    }, msUntilNext9am())
  }
  scheduleNextDaily()
}

export const stopAutoUpdateSchedule = () => {
  if (startupTimer) { clearTimeout(startupTimer); startupTimer = null }
  if (dailyTimer) { clearTimeout(dailyTimer); dailyTimer = null }
}
