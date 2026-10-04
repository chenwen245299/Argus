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
import { useSpeechStore } from '../stores/speech'

const { t } = useI18n()
const speech = useSpeechStore()

onMounted(() => speech.init())
// The window is going away (library closed, app quitting): do not leave a voice running.
onUnmounted(() => {
  speech.stop()
  window.removeEventListener('keydown', onPromptKeydown, true)
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
            <span class="sp-lead" :class="{ playing: speech.state === 'playing' }">
              <Icon v-if="isLoading" icon="fluent:spinner-ios-20-regular" class="sp-spin" width="16" height="16" />
              <Icon v-else icon="fluent:speaker-2-24-regular" width="16" height="16" />
            </span>
            <span class="sp-label">{{ stateLabel }}</span>
            <span v-if="progressText" class="sp-progress">{{ progressText }}</span>
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
  padding: 5px 6px 5px 12px;
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
.sp-lead { display: inline-flex; color: var(--accent); flex-shrink: 0; }
.sp-lead.playing svg { animation: sp-pulse 1.4s ease-in-out infinite; }
.sp-label { font-weight: 500; white-space: nowrap; }
.sp-progress {
  font-size: var(--font-size-xs);
  font-variant-numeric: tabular-nums;
  color: var(--text-tertiary);
  white-space: nowrap;
}
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
