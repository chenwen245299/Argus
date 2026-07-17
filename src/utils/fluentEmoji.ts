// Renders emojis with Microsoft's Fluent Emoji (flat) icon set instead of the
// platform's native emoji font, so they look identical on every OS.
//
// Storage stays as the native unicode character (`collection.emoji`), so nothing
// needs migrating — we translate the character to a `fluent-emoji-flat:<name>`
// icon id at render time via the set's `chars.json` (codepoints → icon name).
// The heavy 8.9 MB `icons.json` (the actual SVG bodies) is loaded lazily and
// only when something first needs to draw a fluent icon.

import { ref } from 'vue'
import { addCollection } from '@iconify/vue'
import charsRaw from '@iconify-json/fluent-emoji-flat/chars.json'

const chars = charsRaw as Record<string, string>

function toCodepointKey(s: string): string {
  return Array.from(s).map(c => c.codePointAt(0)!.toString(16)).join('-')
}

// Native emoji character → `fluent-emoji-flat:<name>` icon id, or null when the
// set has no matching glyph (caller should fall back to the native character).
export function fluentIconFor(value?: string | null): string | null {
  const v = value?.trim()
  if (!v) return null
  const key = toCodepointKey(v)
  // Try the exact sequence, then with/without the VS16 (fe0f) presentation
  // selector, which is often dropped or added inconsistently.
  const name =
    chars[key] ??
    chars[key.replace(/-fe0f\b/g, '')] ??
    chars[key.endsWith('-fe0f') ? key : `${key}-fe0f`]
  return name ? `fluent-emoji-flat:${name}` : null
}

// Whether the SVG bodies have finished loading and registering.
export const fluentReady = ref(false)
let loadPromise: Promise<void> | null = null

export function ensureFluentIcons(): Promise<void> {
  if (fluentReady.value) return Promise.resolve()
  if (loadPromise) return loadPromise
  loadPromise = import('@iconify-json/fluent-emoji-flat/icons.json')
    .then(mod => {
      addCollection((mod as { default: Parameters<typeof addCollection>[0] }).default)
      fluentReady.value = true
    })
    .catch(e => {
      console.error('Failed to load fluent-emoji-flat icons:', e)
      loadPromise = null
    })
  return loadPromise
}

// The full pickable list, built lazily from the cheap chars map. Each native
// character maps to exactly one icon name, so no de-duplication is needed.
export interface FluentEmojiEntry {
  char: string
  name: string
}

let pickerList: FluentEmojiEntry[] | null = null

export function fluentEmojiList(): FluentEmojiEntry[] {
  if (pickerList) return pickerList
  pickerList = Object.entries(chars).map(([cp, name]) => ({
    char: String.fromCodePoint(...cp.split('-').map(h => parseInt(h, 16))),
    name,
  }))
  return pickerList
}
