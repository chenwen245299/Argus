<script setup lang="ts">
/**
 * The two pieces of read-aloud UI that live outside any one viewer, mounted once
 * in MainView:
 *
 * 1. A small floating player, bottom-centre, shown only while something is being
 *    synthesised, played, paused or has failed. It stays out of the way: the dock
 *    ignores the pointer except on the pill itself, and it sits below the PDF
 *    selection popup so it can never cover it.
 * 2. The "not configured" prompt. Clicking 朗读 without a speech model must not
 *    fail silently or spend anything — `speech.read()` opens this instead, and its
 *    button goes straight to the setting that fixes it.
 *
 * All behaviour is in `stores/speech.ts`; this only renders it.
 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { Icon } from '@iconify/vue'
import { useI18n } from 'vue-i18n'
import { SPEECH_RATES, useSpeechStore } from '../stores/speech'

const { t } = useI18n()
const speech = useSpeechStore()

onMounted(() => speech.init())
// The window is going away (library closed, app quitting): do not leave a voice running.
onUnmounted(() => {
  speech.stop()
  window.removeEventListener('keydown', onPromptKeydown, true)
  closePopover()
})

// ── Mini-player ───────────────────────────────────────────────────────────────

const playerVisible = computed(() => speech.state !== 'idle')
const isError = computed(() => speech.state === 'error')
const isLoading = computed(() => speech.state === 'loading')
const isPaused = computed(() => speech.state === 'paused')

const stateLabel = computed(() => {
  if (isLoading.value) return t('speech.player.loading')
  if (isPaused.value) return t('speech.player.paused')
  return t('speech.player.playing')
})

/** The bar: how much has been heard, as a percentage. Moves every frame while a clip plays. */
const heardPercent = computed(() => Math.round(speech.heard * 1000) / 10)

// ── Seeking ──
// The bar follows the pointer while it is held and the read jumps on release: jumping on
// every move would start (and, for a part not synthesised yet, pay for) a clip per pixel.

const barEl = ref<HTMLElement | null>(null)
/** While the bar is held: where, 0..1. */
const dragFraction = ref<number | null>(null)
const shownPercent = computed(() => (dragFraction.value === null ? heardPercent.value : dragFraction.value * 100))

function fractionAt(clientX: number): number {
  const r = barEl.value?.getBoundingClientRect()
  if (!r || r.width <= 0) return 0
  return Math.min(1, Math.max(0, (clientX - r.left) / r.width))
}
function onBarDown(e: PointerEvent) {
  if (e.button !== 0) return
  e.preventDefault()
  barEl.value?.setPointerCapture(e.pointerId)
  dragFraction.value = fractionAt(e.clientX)
}
function onBarMove(e: PointerEvent) {
  if (dragFraction.value !== null) dragFraction.value = fractionAt(e.clientX)
}
function onBarUp(e: PointerEvent) {
  if (dragFraction.value === null) return
  const f = fractionAt(e.clientX)
  dragFraction.value = null
  speech.seekTo(f)
}
function onBarCancel() { dragFraction.value = null }
function onBarKey(e: KeyboardEvent) {
  const step = e.key === 'ArrowRight' ? 0.05 : e.key === 'ArrowLeft' ? -0.05 : 0
  if (!step) return
  e.preventDefault()
  speech.seekTo(Math.min(1, Math.max(0, speech.heard + step)))
}

// ── Popovers (speed, volume) ──
// Small panels above the pill, one open at a time; a press outside or Escape closes it.

type Popover = 'rate' | 'volume'
const popover = ref<Popover | null>(null)
const rateEl = ref<HTMLElement | null>(null)
const volEl = ref<HTMLElement | null>(null)

function onPopoverOutside(e: Event) {
  const root = popover.value === 'rate' ? rateEl.value : volEl.value
  if (root && !root.contains(e.target as Node)) closePopover()
}
function onPopoverKeydown(e: KeyboardEvent) {
  if (e.key !== 'Escape') return
  e.preventDefault()
  e.stopPropagation()
  closePopover()
}
function togglePopover(p: Popover) {
  if (popover.value === p) { closePopover(); return }
  if (!popover.value) {
    document.addEventListener('pointerdown', onPopoverOutside, true)
    window.addEventListener('keydown', onPopoverKeydown, true)
  }
  popover.value = p
}
function closePopover() {
  popover.value = null
  document.removeEventListener('pointerdown', onPopoverOutside, true)
  window.removeEventListener('keydown', onPopoverKeydown, true)
}
watch(playerVisible, (v) => { if (!v) closePopover() })

// ── Speed ──
// A menu rather than a button that cycles: six speeds, and going from 1.25x back to
// 1x should not take five clicks.

function rateLabel(r: number): string { return `${r}×` }

function pickRate(r: number) {
  speech.setRate(r)
  closePopover()
}

// ── Volume ──
// The speaker at the left of the pill opens a vertical slider; the wheel over either
// changes it too. It changes as the slider moves — unlike seeking, that costs nothing.

const volumePercent = computed(() => Math.round(speech.volume * 100))
const volumeIcon = computed(() => {
  const v = speech.volume
  if (v === 0) return 'fluent:speaker-mute-24-regular'
  if (v < 0.34) return 'fluent:speaker-0-24-regular'
  if (v < 0.67) return 'fluent:speaker-1-24-regular'
  return 'fluent:speaker-2-24-regular'
})

const volBar = ref<HTMLElement | null>(null)
let volDragging = false
/** What muting took away, given back by unmuting. */
let volumeBeforeMute = 1

function volumeAt(clientY: number): number {
  const r = volBar.value?.getBoundingClientRect()
  if (!r || r.height <= 0) return speech.volume
  return Math.min(1, Math.max(0, (r.bottom - clientY) / r.height))
}
function onVolDown(e: PointerEvent) {
  if (e.button !== 0) return
  e.preventDefault()
  volBar.value?.setPointerCapture(e.pointerId)
  volDragging = true
  speech.setVolume(volumeAt(e.clientY))
}
function onVolMove(e: PointerEvent) {
  if (volDragging) speech.setVolume(volumeAt(e.clientY))
}
function onVolUp() { volDragging = false }
function onVolKey(e: KeyboardEvent) {
  const step = e.key === 'ArrowUp' || e.key === 'ArrowRight' ? 0.05
    : e.key === 'ArrowDown' || e.key === 'ArrowLeft' ? -0.05 : 0
  if (!step) return
  e.preventDefault()
  speech.setVolume(speech.volume + step)
}
/** A mouse wheel notch is 5 %; a trackpad's small steps add up smoothly. */
function onVolWheel(e: WheelEvent) {
  speech.setVolume(speech.volume + Math.max(-0.05, Math.min(0.05, -e.deltaY / 1000)))
}
function toggleMute() {
  if (speech.volume > 0) {
    volumeBeforeMute = speech.volume
    speech.setVolume(0)
  } else {
    speech.setVolume(volumeBeforeMute || 1)
  }
}

/** "2/5" — only worth showing when the passage was split. */
const progressText = computed(() => {
  const { index, total } = speech.progress
  return total > 1 ? `${Math.min(index + 1, total)}/${total}` : ''
})

// ── "Not configured" prompt ───────────────────────────────────────────────────

// The live reason wins over the one captured at click time: the capability list
// can finish refreshing a moment after the click (a provider was just removed in
// settings), and the prompt should name what is true now.
const promptCopy = computed(() => {
  switch (speech.notConfiguredReason ?? speech.setupPrompt?.reason) {
    case 'no-providers':
      return {
        title: t('speech.prompt.titleNone'),
        body: t('speech.prompt.bodyNone'),
        action: t('speech.prompt.goProviders'),
        section: 'ai' as const,
      }
    case 'provider-missing':
      return {
        title: t('speech.prompt.titleMissing'),
        body: t('speech.prompt.bodyMissing'),
        action: t('speech.prompt.goSpeech'),
        section: 'speech' as const,
      }
    default:
      return {
        title: t('speech.prompt.titleUnset'),
        body: t('speech.prompt.bodyUnset'),
        action: t('speech.prompt.goSpeech'),
        section: 'speech' as const,
      }
  }
})

const primaryBtn = ref<HTMLButtonElement | null>(null)

function onPromptKeydown(e: KeyboardEvent) {
  if (e.key !== 'Escape') return
  // Captured and stopped: Escape is also "clear the selection" in the viewers, and
  // closing a dialog should not do that as well.
  e.preventDefault()
  e.stopPropagation()
  speech.dismissSetup()
}

watch(
  () => speech.setupPrompt,
  (prompt) => {
    if (prompt) {
      window.addEventListener('keydown', onPromptKeydown, true)
      void nextTick(() => primaryBtn.value?.focus())
    } else {
      window.removeEventListener('keydown', onPromptKeydown, true)
    }
  },
)
</script>

<template>
  <Teleport to="body">
    <Transition name="sp-pill">
      <div v-if="playerVisible" class="sp-dock">
        <div class="sp-pill" :class="{ error: isError }" role="status" aria-live="polite">
          <template v-if="!isError">
            <span ref="volEl" class="sp-vol">
              <button
                class="sp-lead"
                :class="{ playing: speech.state === 'playing', open: popover === 'volume' }"
                :title="t('speech.player.volume')"
                :aria-label="t('speech.player.volume')"
                aria-haspopup="dialog"
                :aria-expanded="popover === 'volume'"
                @click="togglePopover('volume')"
                @wheel.prevent="onVolWheel"
              >
                <Icon v-if="isLoading" icon="fluent:spinner-ios-20-regular" class="sp-spin" width="16" height="16" />
                <Icon v-else :icon="volumeIcon" width="16" height="16" />
              </button>
              <span v-if="popover === 'volume'" class="sp-vol-pop" @wheel.prevent="onVolWheel">
                <span class="sp-vol-value">{{ volumePercent }}</span>
                <span
                  ref="volBar"
                  class="sp-vol-bar"
                  role="slider"
                  tabindex="0"
                  aria-orientation="vertical"
                  aria-valuemin="0"
                  aria-valuemax="100"
                  :aria-valuenow="volumePercent"
                  :aria-label="t('speech.player.volume')"
                  @pointerdown="onVolDown"
                  @pointermove="onVolMove"
                  @pointerup="onVolUp"
                  @pointercancel="onVolUp"
                  @keydown="onVolKey"
                >
                  <span class="sp-vol-track">
                    <span class="sp-vol-fill" :style="{ height: `${volumePercent}%` }" />
                  </span>
                  <span class="sp-vol-thumb" :style="{ bottom: `${volumePercent}%` }" />
                </span>
                <button
                  class="sp-vol-mute"
                  :title="speech.volume === 0 ? t('speech.player.unmute') : t('speech.player.mute')"
                  :aria-label="speech.volume === 0 ? t('speech.player.unmute') : t('speech.player.mute')"
                  @click="toggleMute"
                >
                  <Icon :icon="speech.volume === 0 ? 'fluent:speaker-mute-24-regular' : 'fluent:speaker-2-24-regular'" width="14" height="14" />
                </button>
              </span>
            </span>
            <span class="sp-label">{{ stateLabel }}</span>
            <span
              ref="barEl"
              class="sp-bar"
              :class="{ dragging: dragFraction !== null }"
              role="slider"
              tabindex="0"
              aria-valuemin="0"
              aria-valuemax="100"
              :aria-valuenow="Math.round(shownPercent)"
              :aria-label="t('speech.player.seek')"
              @pointerdown="onBarDown"
              @pointermove="onBarMove"
              @pointerup="onBarUp"
              @pointercancel="onBarCancel"
              @keydown="onBarKey"
            >
              <span class="sp-bar-track">
                <span class="sp-bar-fill" :style="{ width: `${shownPercent}%` }" />
              </span>
              <span class="sp-bar-thumb" :style="{ left: `${shownPercent}%` }" />
            </span>
            <span v-if="progressText" class="sp-progress">{{ progressText }}</span>
            <span ref="rateEl" class="sp-rate">
              <button
                class="sp-rate-btn"
                :class="{ open: popover === 'rate' }"
                :title="t('speech.player.rate')"
                :aria-label="t('speech.player.rate')"
                aria-haspopup="menu"
                :aria-expanded="popover === 'rate'"
                @click="togglePopover('rate')"
              >{{ rateLabel(speech.rate) }}</button>
              <span v-if="popover === 'rate'" class="sp-rate-menu" role="menu">
                <button
                  v-for="r in [...SPEECH_RATES].reverse()"
                  :key="r"
                  class="sp-rate-item"
                  :class="{ active: r === speech.rate }"
                  role="menuitemradio"
                  :aria-checked="r === speech.rate"
                  @click="pickRate(r)"
                >{{ rateLabel(r) }}</button>
              </span>
            </span>
            <button
              v-if="!isLoading"
              class="sp-btn"
              :title="isPaused ? t('speech.player.resume') : t('speech.player.pause')"
              :aria-label="isPaused ? t('speech.player.resume') : t('speech.player.pause')"
              @click="speech.togglePause()"
            >
              <Icon :icon="isPaused ? 'fluent:play-24-filled' : 'fluent:pause-24-filled'" width="14" height="14" />
            </button>
            <button
              class="sp-btn"
              :title="t('speech.player.stop')"
              :aria-label="t('speech.player.stop')"
              @click="speech.stop()"
            >
              <Icon icon="fluent:stop-24-filled" width="14" height="14" />
            </button>
          </template>

          <template v-else>
            <Icon icon="fluent:warning-24-regular" class="sp-warn" width="16" height="16" />
            <span class="sp-error-text">{{ speech.errorMessage }}</span>
            <button class="sp-link" @click="speech.openSettings('speech')">{{ t('speech.player.settings') }}</button>
            <button
              class="sp-btn"
              :title="t('speech.player.dismiss')"
              :aria-label="t('speech.player.dismiss')"
              @click="speech.stop()"
            >
              <Icon icon="fluent:dismiss-24-regular" width="14" height="14" />
            </button>
          </template>
        </div>
      </div>
    </Transition>

    <Transition name="sp-fade">
      <div v-if="speech.setupPrompt" class="sp-overlay" @click.self="speech.dismissSetup()">
        <div class="sp-modal" role="dialog" aria-modal="true" aria-labelledby="sp-modal-title">
          <div class="sp-modal-head">
            <span class="sp-modal-icon"><Icon icon="fluent:speaker-2-24-regular" width="18" height="18" /></span>
            <h2 id="sp-modal-title" class="sp-modal-title">{{ promptCopy.title }}</h2>
          </div>
          <p class="sp-modal-body">{{ promptCopy.body }}</p>
          <div class="sp-modal-actions">
            <button class="sp-ghost" @click="speech.dismissSetup()">{{ t('speech.prompt.dismiss') }}</button>
            <button ref="primaryBtn" class="sp-primary" @click="speech.openSettings(promptCopy.section)">
              {{ promptCopy.action }}
            </button>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style>
/* Not scoped: the markup is teleported to <body>. Every selector is `sp-` prefixed. */

/* ── Mini-player ── */
.sp-dock {
  position: fixed;
  left: 0;
  right: 0;
  bottom: 18px;
  display: flex;
  justify-content: center;
  /* Below the selection popup (1000) so it can never cover it, above the panes. */
  z-index: 900;
  /* The dock spans the window; only the pill takes clicks, so nothing behind is blocked. */
  pointer-events: none;
  padding: 0 16px;
}
.sp-pill {
  pointer-events: auto;
  display: flex;
  align-items: center;
  gap: 8px;
  max-width: min(560px, 100%);
  padding: 5px 6px 5px 7px;
  font-size: var(--font-size-sm);
  color: var(--text-primary);
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-pill);
  box-shadow: var(--shadow-md);
}
.sp-pill.error {
  border-radius: var(--radius-lg);
  border-color: color-mix(in srgb, #ef4444 38%, var(--border-default));
  padding: 7px 7px 7px 12px;
}
/* The speaker doubles as the volume button. */
.sp-vol { position: relative; display: inline-flex; flex-shrink: 0; }
.sp-lead {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 50%;
  color: var(--accent);
  flex-shrink: 0;
}
.sp-lead:hover, .sp-lead.open { background: var(--bg-hover); }
.sp-vol-pop {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 6px;
  width: 42px;
  padding: 9px 0 5px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
}
.sp-vol-value {
  font-size: var(--font-size-xs);
  font-variant-numeric: tabular-nums;
  color: var(--text-secondary);
}
.sp-vol-bar {
  position: relative;
  display: flex;
  justify-content: center;
  width: 24px;
  height: 96px;
  cursor: pointer;
  touch-action: none;
  outline: none;
}
.sp-vol-track {
  position: relative;
  width: 4px;
  height: 100%;
  overflow: hidden;
  border-radius: 2px;
  background: color-mix(in srgb, var(--text-tertiary) 24%, transparent);
}
.sp-vol-fill { position: absolute; left: 0; right: 0; bottom: 0; background: var(--accent); }
.sp-vol-thumb {
  position: absolute;
  left: 50%;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 2px var(--bg-primary);
  transform: translate(-50%, 50%);
  pointer-events: none;
}
.sp-vol-mute {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 50%;
  color: var(--text-secondary);
}
.sp-vol-mute:hover { background: var(--bg-hover); color: var(--text-primary); }
.sp-lead.playing svg { animation: sp-pulse 1.4s ease-in-out infinite; }
.sp-label { font-weight: 500; white-space: nowrap; }
.sp-bar {
  /* The hit area is taller than the 4px track, so the bar is easy to grab. */
  position: relative;
  display: flex;
  align-items: center;
  width: 140px;
  height: 16px;
  flex-shrink: 0;
  cursor: pointer;
  touch-action: none;
  outline: none;
}
.sp-bar-track {
  position: relative;
  width: 100%;
  height: 4px;
  overflow: hidden;
  border-radius: 2px;
  background: color-mix(in srgb, var(--text-tertiary) 24%, transparent);
}
.sp-bar-fill {
  position: absolute;
  inset: 0 auto 0 0;
  border-radius: inherit;
  background: var(--accent);
}
.sp-bar-thumb {
  position: absolute;
  top: 50%;
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--accent);
  box-shadow: 0 0 0 2px var(--bg-primary);
  transform: translate(-50%, -50%) scale(0);
  transition: transform 0.12s ease;
  pointer-events: none;
}
.sp-bar:hover .sp-bar-thumb,
.sp-bar.dragging .sp-bar-thumb,
.sp-bar:focus-visible .sp-bar-thumb { transform: translate(-50%, -50%) scale(1); }
.sp-progress {
  font-size: var(--font-size-xs);
  font-variant-numeric: tabular-nums;
  color: var(--text-tertiary);
  white-space: nowrap;
}
.sp-rate { position: relative; display: inline-flex; flex-shrink: 0; }
.sp-rate-btn {
  min-width: 38px;
  height: 22px;
  padding: 0 7px;
  font-size: var(--font-size-xs);
  font-weight: 600;
  font-variant-numeric: tabular-nums;
  color: var(--text-secondary);
  border-radius: var(--radius-pill);
  background: var(--bg-hover);
}
.sp-rate-btn:hover, .sp-rate-btn.open { color: var(--text-primary); background: color-mix(in srgb, var(--text-tertiary) 22%, transparent); }
/* Above the pill: the dock sits at the bottom of the window. Fastest first, top down. */
.sp-rate-menu {
  position: absolute;
  bottom: calc(100% + 12px);
  left: 50%;
  transform: translateX(-50%);
  display: flex;
  flex-direction: column;
  gap: 1px;
  min-width: 64px;
  padding: 4px;
  background: var(--bg-primary);
  border: 1px solid var(--border-default);
  border-radius: var(--radius-md);
  box-shadow: var(--shadow-md);
}
.sp-rate-item {
  padding: 5px 10px;
  font-size: var(--font-size-sm);
  font-variant-numeric: tabular-nums;
  text-align: center;
  color: var(--text-secondary);
  border-radius: var(--radius-sm);
}
.sp-rate-item:hover { background: var(--bg-hover); color: var(--text-primary); }
.sp-rate-item.active { color: var(--accent); background: var(--accent-light); font-weight: 600; }
.sp-btn {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 26px;
  height: 26px;
  border-radius: 50%;
  color: var(--text-secondary);
  flex-shrink: 0;
}
.sp-btn:hover { background: var(--bg-hover); color: var(--text-primary); }
.sp-warn { color: #dc2626; flex-shrink: 0; }
.sp-error-text {
  min-width: 0;
  line-height: 1.45;
  overflow: hidden;
  display: -webkit-box;
  -webkit-line-clamp: 3;
  -webkit-box-orient: vertical;
  word-break: break-word;
}
.sp-link {
  flex-shrink: 0;
  padding: 3px 8px;
  font-size: var(--font-size-xs);
  font-weight: 500;
  color: var(--accent);
  border-radius: var(--radius-sm);
  white-space: nowrap;
}
.sp-link:hover { background: var(--accent-light); }
.sp-spin { animation: sp-spin 1s linear infinite; }
@keyframes sp-spin { to { transform: rotate(360deg); } }
@keyframes sp-pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0.45; } }

.sp-pill-enter-active, .sp-pill-leave-active { transition: opacity 0.16s ease, transform 0.16s ease; }
.sp-pill-enter-from, .sp-pill-leave-to { opacity: 0; transform: translateY(8px); }

/* ── "Not configured" prompt ── */
.sp-overlay {
  position: fixed;
  inset: 0;
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 24px;
  background: rgba(0, 0, 0, 0.45);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}
.sp-modal {
  width: 100%;
  max-width: 400px;
  padding: 20px;
  display: flex;
  flex-direction: column;
  gap: 12px;
  background: var(--bg-primary);
  border: 1px solid var(--border-subtle);
  border-radius: var(--radius-lg);
  box-shadow: var(--shadow-lg);
}
.sp-modal-head { display: flex; align-items: center; gap: 10px; }
.sp-modal-icon {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 30px;
  height: 30px;
  border-radius: 50%;
  color: var(--accent);
  background: var(--accent-light);
  flex-shrink: 0;
}
.sp-modal-title { margin: 0; font-size: var(--font-size-md); font-weight: 600; color: var(--text-primary); }
.sp-modal-body { margin: 0; font-size: var(--font-size-base); line-height: 1.6; color: var(--text-secondary); }
.sp-modal-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 4px; }
.sp-ghost, .sp-primary {
  padding: 6px 14px;
  font-size: var(--font-size-sm);
  font-weight: 500;
  border-radius: var(--radius-sm);
}
.sp-ghost { color: var(--text-secondary); background: var(--bg-secondary); border: 1px solid var(--border-subtle); }
.sp-ghost:hover { background: var(--bg-hover); color: var(--text-primary); }
.sp-primary { color: #fff; background: var(--accent); }
.sp-primary:hover { background: var(--accent-hover); }
.sp-primary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }

.sp-fade-enter-active, .sp-fade-leave-active { transition: opacity 0.14s ease; }
.sp-fade-enter-from, .sp-fade-leave-to { opacity: 0; }
</style>
