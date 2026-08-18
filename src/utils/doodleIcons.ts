// Hand-drawn icons for the in-app tutorials and tips. The Fluent set (see
// fluentIcons.ts) stays the app's UI chrome — these are deliberately sketchy
// and only decorate explanatory content: onboarding, setup walkthroughs, and
// the margin notes rendered by DoodleNote.vue.
//
// The bodies come from koboyo.com and are baked into doodles.json by
// `node scripts/fetch-doodles.mjs`, so nothing is fetched at runtime. Usage is
// the same as any Iconify icon:
//
//   <Icon icon="doodle:terminal" width="28" height="28" />
//
// The JSON is ~65 KB, so it is dynamic-imported at startup like the other
// collections; <Icon> re-renders reactively once it registers, which means call
// sites need no readiness guard.

import { ref } from 'vue'
import { addCollection } from '@iconify/vue'

export const doodleIconsReady = ref(false)
let loadPromise: Promise<void> | null = null

export function ensureDoodleIcons(): Promise<void> {
  if (doodleIconsReady.value) return Promise.resolve()
  if (loadPromise) return loadPromise
  loadPromise = import('../assets/doodles/doodles.json')
    .then(mod => {
      addCollection((mod as { default: Parameters<typeof addCollection>[0] }).default)
      doodleIconsReady.value = true
    })
    .catch(e => {
      console.error('Failed to load doodle icons:', e)
      loadPromise = null
    })
  return loadPromise
}
