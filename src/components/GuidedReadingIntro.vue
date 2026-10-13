<script setup lang="ts">
import { nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import GuidedReadingIcon from './GuidedReadingIcon.vue'
import {
  GUIDED_READING_CHANGED_EVENT, GUIDED_READING_DISMISSED_KEY, GUIDED_READING_EXPERIENCED_KEY,
  dismissGuidedReadingAnnouncement, guidedReadingAnnouncementNeeded,
} from '../utils/guidedReading'

const props = withDefaults(defineProps<{ ready?: boolean }>(), { ready: true })
const { t } = useI18n()
const visible = ref(false)
const dontShowAgain = ref(false)
const dialog = ref<HTMLElement | null>(null)
const acknowledgeButton = ref<HTMLButtonElement | null>(null)
let considered = false
let previousFocus: HTMLElement | null = null

watch(() => props.ready, ready => {
  if (!ready || considered) return
  considered = true
  visible.value = guidedReadingAnnouncementNeeded()
}, { immediate: true })

watch(() => visible.value && props.ready, async showing => {
  if (showing) {
    previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null
    await nextTick()
    acknowledgeButton.value?.focus()
  } else {
    previousFocus?.focus()
    previousFocus = null
  }
}, { immediate: true, flush: 'post' })

function acknowledge() {
  if (dontShowAgain.value) dismissGuidedReadingAnnouncement()
  visible.value = false
}

function onKeydown(event: KeyboardEvent) {
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    visible.value = false
  } else if (event.key === 'Tab') {
    const elements = [...(dialog.value?.querySelectorAll<HTMLElement>('button, input') ?? [])]
    const first = elements[0], last = elements[elements.length - 1]
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault()
      last?.focus()
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault()
      first?.focus()
    }
  }
}

function refreshVisibility(event: Event) {
  if (event instanceof StorageEvent && event.key !== null
    && event.key !== GUIDED_READING_EXPERIENCED_KEY && event.key !== GUIDED_READING_DISMISSED_KEY) return
  if (!guidedReadingAnnouncementNeeded()) visible.value = false
}

onMounted(() => {
  window.addEventListener(GUIDED_READING_CHANGED_EVENT, refreshVisibility)
  window.addEventListener('storage', refreshVisibility)
  refreshVisibility(new Event(GUIDED_READING_CHANGED_EVENT))
})
onUnmounted(() => {
  window.removeEventListener(GUIDED_READING_CHANGED_EVENT, refreshVisibility)
  window.removeEventListener('storage', refreshVisibility)
  previousFocus?.focus()
})
</script>

<template>
  <Teleport to="body">
    <div v-if="visible && ready" class="guided-reading-overlay" @keydown="onKeydown">
      <section ref="dialog" class="guided-reading-intro" role="dialog" aria-modal="true" aria-labelledby="guided-reading-intro-title">
        <button class="intro-close" :aria-label="t('guidedReading.close')" @click="visible = false">
          <svg viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="m6 6 12 12M18 6 6 18" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" /></svg>
        </button>
        <div class="intro-hero">
          <div class="hero-heading">
            <span class="intro-badge">{{ t('guidedReading.newFeature') }}</span>
            <h2 id="guided-reading-intro-title"><GuidedReadingIcon />{{ t('guidedReading.title') }}</h2>
            <p>{{ t('guidedReading.tagline') }}</p>
          </div>
          <div class="reading-scene" aria-hidden="true">
            <div class="scene-paper">
              <div class="paper-label">PAPER</div>
              <div class="paper-line title-line" /><div class="paper-line" /><div class="paper-line short-line" />
              <svg class="paper-chart" viewBox="0 0 160 65" fill="none">
                <path d="M12 6v48h137M12 38h137M12 22h137" stroke="currentColor" opacity=".18" />
                <path d="m15 45 27-8 25 2 28-22 26 7 25-18" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" />
                <circle cx="95" cy="17" r="5" fill="currentColor" />
              </svg>
              <div class="paper-line" /><div class="paper-line short-line" />
              <span class="paper-inspected"><GuidedReadingIcon /></span>
            </div>
            <div class="scene-chat">
              <div class="scene-reply"><span class="reply-dot" />{{ t('guidedReading.example') }}</div>
              <div class="scene-user">{{ t('guidedReading.exampleUser') }}<span>↗</span></div>
              <div class="scene-next">{{ t('guidedReading.exampleNext') }}<span class="next-dots">•••</span></div>
            </div>
          </div>
        </div>
        <div class="intro-content">
          <div class="intro-steps">
            <div class="intro-step"><span class="step-number">01</span><div><h3>{{ t('guidedReading.readTitle') }}</h3><p>{{ t('guidedReading.readDescription') }}</p></div></div>
            <div class="intro-step"><span class="step-number">02</span><div><h3>{{ t('guidedReading.explainTitle') }}</h3><p>{{ t('guidedReading.explainDescription') }}</p></div></div>
            <div class="intro-step"><span class="step-number">03</span><div><h3>{{ t('guidedReading.paceTitle') }}</h3><p>{{ t('guidedReading.paceDescription') }}</p></div></div>
          </div>
          <p class="intro-location"><GuidedReadingIcon />{{ t('guidedReading.location') }}</p>
          <footer class="intro-footer">
            <label><input v-model="dontShowAgain" type="checkbox" />{{ t('guidedReading.dontShowAgain') }}</label>
            <button ref="acknowledgeButton" class="intro-acknowledge" @click="acknowledge">{{ t('guidedReading.acknowledge') }}</button>
          </footer>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.guided-reading-overlay {
  position: fixed; inset: 0; z-index: 950; display: flex; align-items: center; justify-content: center;
  padding: 24px; background: rgba(0, 0, 0, .35); backdrop-filter: blur(5px);
}
.guided-reading-intro {
  position: relative; width: min(660px, 100%); max-height: 100%; overflow-y: auto;
  border: 1px solid var(--border-subtle); border-radius: 20px;
  background: var(--bg-primary); color: var(--text-primary); box-shadow: var(--shadow-lg);
}
.intro-close {
  position: absolute; top: 16px; right: 16px; z-index: 1; display: grid; place-items: center;
  width: 30px; height: 30px; border: none; border-radius: 8px; background: transparent;
  color: var(--text-secondary); cursor: pointer;
}
.intro-close svg { width: 21px; height: 21px; }
.intro-close:hover { background: color-mix(in srgb, var(--accent) 12%, transparent); }
.intro-hero { padding: 30px 32px 28px; background: color-mix(in srgb, var(--accent) 8%, var(--bg-primary)); }
.intro-badge { font-size: 11px; font-weight: 700; letter-spacing: .08em; color: var(--accent); }
.hero-heading h2 { display: flex; align-items: center; gap: 10px; margin: 9px 0; font-size: 28px; font-weight: 700; }
.hero-heading h2 :deep(svg) { color: var(--accent); font-size: 34px; }
.hero-heading p { margin: 0; color: var(--text-secondary); font-size: 14px; }
.reading-scene { display: flex; align-items: center; gap: 24px; margin: 27px auto 0; max-width: 520px; }
.scene-paper {
  position: relative; flex: 0 0 170px; padding: 16px; border: 1px solid var(--border-subtle);
  border-radius: 12px; background: var(--bg-primary); box-shadow: var(--shadow-sm); transform: rotate(-5deg);
}
.paper-label { font-size: 9px; letter-spacing: .15em; font-weight: 700; color: var(--text-tertiary); margin-bottom: 9px; }
.paper-line { height: 4px; margin-top: 7px; border-radius: 2px; background: var(--border-subtle); }
.title-line { height: 6px; width: 84%; background: color-mix(in srgb, var(--text-primary) 40%, transparent); }
.short-line { width: 70%; }
.paper-chart { display: block; width: 100%; margin-top: 12px; color: var(--accent); }
.paper-inspected {
  position: absolute; right: -11px; bottom: -10px; padding: 8px; border-radius: 12px;
  background: var(--accent); color: var(--text-on-accent, white); font-size: 28px;
}
.scene-chat { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 10px; font-size: 12px; line-height: 1.6; }
.scene-reply, .scene-next { padding: 12px 14px; border: 1px solid var(--border-subtle); border-radius: 12px 12px 12px 3px; background: var(--bg-primary); }
.reply-dot { display: inline-block; width: 6px; height: 6px; margin-right: 7px; border-radius: 50%; background: var(--accent); vertical-align: middle; }
.scene-user { align-self: flex-end; padding: 7px 12px; border-radius: 10px 10px 3px 10px; background: var(--accent); color: var(--text-on-accent, white); }
.scene-user span { margin-left: 15px; opacity: .75; }
.scene-next { color: var(--text-secondary); }
.next-dots { margin-left: 8px; color: var(--accent); letter-spacing: 2px; }
.intro-content { padding: 28px 32px 24px; }
.intro-steps { display: flex; flex-direction: column; gap: 18px; }
.intro-step { display: flex; gap: 14px; }
.step-number { padding-top: 2px; font-size: 12px; font-weight: 650; color: var(--accent); font-variant-numeric: tabular-nums; }
.intro-step h3 { margin: 0 0 4px; font-size: 14px; font-weight: 650; }
.intro-step p { margin: 0; font-size: 13px; line-height: 1.65; color: var(--text-secondary); }
.intro-location { display: flex; align-items: center; gap: 8px; margin: 23px 0 0; padding: 12px; border-radius: 9px; font-size: 12px; line-height: 1.6; color: var(--text-secondary); background: var(--bg-secondary); }
.intro-location :deep(svg) { font-size: 22px; color: var(--accent); }
.intro-footer { display: flex; align-items: center; justify-content: space-between; gap: 20px; margin-top: 24px; }
.intro-footer label { display: flex; align-items: center; gap: 7px; color: var(--text-secondary); font-size: 12px; cursor: pointer; }
.intro-footer input { accent-color: var(--accent); width: 14px; height: 14px; margin: 0; }
.intro-acknowledge { padding: 9px 22px; border: none; border-radius: 9px; color: var(--text-on-accent, white); background: var(--accent); font-size: 13px; font-weight: 600; cursor: pointer; }
.intro-acknowledge:hover { background: var(--accent-hover); }
button:focus-visible, input:focus-visible { outline: 2px solid var(--accent); outline-offset: 3px; }
@media (max-width: 480px) {
  .guided-reading-overlay { padding: 14px; }
  .intro-hero { padding: 26px 22px 24px; }
  .intro-content { padding: 24px 22px; }
  .reading-scene { gap: 17px; }
  .scene-paper { flex-basis: 108px; padding: 12px; }
  .scene-chat { font-size: 11px; }
  .scene-reply, .scene-next { padding: 9px 10px; }
}
</style>
