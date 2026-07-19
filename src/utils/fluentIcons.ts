// Bundles Microsoft's Fluent UI System Icons (MIT-licensed) into the app so any
// `fluent:<name>` icon can be rendered anywhere through Iconify's <Icon> — no
// per-icon imports needed. Mirrors the Fluent Emoji setup (see fluentEmoji.ts):
// the ~12 MB icon bodies are dynamic-imported in the background at startup and
// registered with Iconify, after which every <Icon icon="fluent:…"> resolves
// offline. Iconify's <Icon> re-renders reactively once the collection lands, so
// individual call sites don't need a readiness guard.
//
// Naming convention: `fluent:<name>-<size>-<variant>`, e.g.
//   fluent:dismiss-24-regular   ← outline, the default for UI chrome
//   fluent:star-24-filled       ← solid, for active / selected states
// Sizes shipped: 16, 20, 24, 28, 32, 48. We standardise on 24 for chrome and
// let the render size be controlled by the <Icon> width/height (or CSS).

import { ref } from 'vue'
import { addCollection } from '@iconify/vue'

// Flips true once the SVG bodies have registered. Rendering doesn't depend on
// it (Iconify updates reactively), but pickers/lists can await it.
export const fluentIconsReady = ref(false)
let loadPromise: Promise<void> | null = null

export function ensureFluentUiIcons(): Promise<void> {
  if (fluentIconsReady.value) return Promise.resolve()
  if (loadPromise) return loadPromise
  loadPromise = import('@iconify-json/fluent/icons.json')
    .then(mod => {
      addCollection((mod as { default: Parameters<typeof addCollection>[0] }).default)
      fluentIconsReady.value = true
    })
    .catch(e => {
      console.error('Failed to load Fluent UI icons:', e)
      loadPromise = null
    })
  return loadPromise
}
