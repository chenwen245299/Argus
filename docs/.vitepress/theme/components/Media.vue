<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { withBase } from 'vitepress'

// A reusable, theme-aware media block for GIFs, videos, and screenshots.
// - `.mp4` / `.webm` / `.mov`  → autoplaying looped muted <video>
// - `.gif` / `.png` / `.jpg` / …  → <img>
// - no `src`, or the file fails to load → a styled "coming soon" placeholder
// Click the media to open it enlarged in a lightbox (Esc / click backdrop to close).
const props = defineProps<{
  src?: string
  caption?: string
  alt?: string
}>()

const failed = ref(false)
const open = ref(false)
const inlineVideo = ref<HTMLVideoElement | null>(null)

// Resolve to a base-aware, site-root URL. Accepts "/media/x", "media/x",
// full URLs, and explicit relative paths ("./", "../").
const resolved = computed(() => {
  const s = props.src
  if (!s) return ''
  if (/^https?:\/\//.test(s)) return s
  if (s.startsWith('.')) return s
  return withBase(s.startsWith('/') ? s : `/${s}`)
})
const isVideo = computed(() => /\.(mp4|webm|mov)$/i.test(props.src ?? ''))
const showPlaceholder = computed(() => !props.src || failed.value)

function openLightbox() {
  if (showPlaceholder.value) return
  open.value = true
  if (typeof document !== 'undefined') document.body.style.overflow = 'hidden'
}
function closeLightbox() {
  open.value = false
  if (typeof document !== 'undefined') document.body.style.overflow = ''
}
function onKey(e: KeyboardEvent) {
  if (e.key === 'Escape' && open.value) closeLightbox()
}

onMounted(() => {
  window.addEventListener('keydown', onKey)
  // Vue can set `muted` as an attribute only, which some browsers ignore for the
  // autoplay gate. Force the property and kick off playback explicitly.
  const v = inlineVideo.value
  if (v) {
    v.muted = true
    v.play().catch(() => {})
  }
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKey)
  if (typeof document !== 'undefined') document.body.style.overflow = ''
})
</script>

<template>
  <figure class="argus-media">
    <div v-if="showPlaceholder" class="argus-media__ph">
      <div class="argus-media__ph-play">▶</div>
      <div class="argus-media__ph-text">{{ caption || 'Demo coming soon' }}</div>
    </div>

    <button
      v-else
      type="button"
      class="argus-media__trigger"
      :aria-label="caption || 'Enlarge media'"
      @click="openLightbox"
    >
      <video
        v-if="isVideo"
        ref="inlineVideo"
        :src="resolved"
        autoplay
        loop
        muted
        playsinline
        preload="auto"
        @error="failed = true"
      />
      <img
        v-else
        :src="resolved"
        :alt="alt || caption || ''"
        loading="lazy"
        @error="failed = true"
      />
      <span class="argus-media__zoom" aria-hidden="true">⤢</span>
    </button>

    <figcaption v-if="caption && !showPlaceholder">{{ caption }}</figcaption>
  </figure>

  <Teleport to="body">
    <div v-if="open" class="argus-lightbox" @click.self="closeLightbox">
      <button type="button" class="argus-lightbox__close" aria-label="Close" @click="closeLightbox">✕</button>
      <video
        v-if="isVideo"
        :src="resolved"
        autoplay
        loop
        controls
        playsinline
      />
      <img v-else :src="resolved" :alt="alt || caption || ''" />
    </div>
  </Teleport>
</template>

<style scoped>
.argus-media {
  margin: 24px 0;
  /* Keep demos a sensible size even when the doc column is full-width. */
  max-width: 900px;
}
.argus-media__trigger {
  display: block;
  position: relative;
  width: 100%;
  padding: 0;
  border: 0;
  background: none;
  cursor: zoom-in;
  border-radius: 14px;
}
.argus-media :is(video, img) {
  display: block;
  width: 100%;
  height: auto;
  border-radius: 14px;
  border: 1px solid var(--vp-c-divider);
  box-shadow: var(--vp-shadow-2);
  background: var(--vp-c-bg-soft);
}
.argus-media__zoom {
  position: absolute;
  top: 10px;
  right: 10px;
  display: grid;
  place-items: center;
  width: 32px;
  height: 32px;
  border-radius: 8px;
  font-size: 15px;
  color: #fff;
  background: rgba(0, 0, 0, 0.5);
  opacity: 0;
  transition: opacity 0.15s ease;
  pointer-events: none;
}
.argus-media__trigger:hover .argus-media__zoom,
.argus-media__trigger:focus-visible .argus-media__zoom {
  opacity: 1;
}
.argus-media figcaption {
  margin-top: 10px;
  text-align: center;
  font-size: 13px;
  color: var(--vp-c-text-2);
}
.argus-media__ph {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 12px;
  aspect-ratio: 16 / 9;
  border-radius: 14px;
  border: 1px dashed var(--vp-c-divider);
  background:
    radial-gradient(120% 120% at 50% 0%, var(--vp-c-bg-soft), var(--vp-c-bg));
  color: var(--vp-c-text-2);
  text-align: center;
  padding: 24px;
}
.argus-media__ph-play {
  display: grid;
  place-items: center;
  width: 46px;
  height: 46px;
  border-radius: 50%;
  font-size: 14px;
  color: #fff;
  background: var(--argus-grad);
  box-shadow: 0 6px 18px -6px var(--vp-c-brand-3);
}
.argus-media__ph-text {
  font-size: 13px;
  max-width: 42ch;
  line-height: 1.5;
}
</style>

<style>
/* Lightbox lives at <body> (teleported), so styles are global (not scoped). */
.argus-lightbox {
  position: fixed;
  inset: 0;
  z-index: 9999;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 4vmin;
  background: rgba(0, 0, 0, 0.82);
  -webkit-backdrop-filter: blur(4px);
  backdrop-filter: blur(4px);
}
.argus-lightbox :is(video, img) {
  max-width: 92vw;
  max-height: 88vh;
  border-radius: 12px;
  box-shadow: 0 30px 80px -20px rgba(0, 0, 0, 0.6);
}
.argus-lightbox__close {
  position: absolute;
  top: 18px;
  right: 22px;
  width: 40px;
  height: 40px;
  border: 0;
  border-radius: 50%;
  font-size: 18px;
  color: #fff;
  background: rgba(255, 255, 255, 0.15);
  cursor: pointer;
  transition: background 0.15s ease;
}
.argus-lightbox__close:hover {
  background: rgba(255, 255, 255, 0.28);
}
</style>
