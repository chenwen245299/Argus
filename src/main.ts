// Promise.withResolvers is not available in WebKit < Safari 17.4 (macOS Ventura and below)
if (typeof (Promise as any).withResolvers === 'undefined') {
  (Promise as any).withResolvers = function <T>() {
    let resolve!: (value: T | PromiseLike<T>) => void
    let reject!:  (reason?: unknown) => void
    const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej })
    return { promise, resolve, reject }
  }
}

import { createApp } from 'vue'
import { createPinia } from 'pinia'
import VirtualScroller from 'vue-virtual-scroller'
import 'vue-virtual-scroller/dist/vue-virtual-scroller.css'
import 'pdfjs-dist/web/pdf_viewer.css'
import 'katex/dist/katex.min.css'
import 'highlight.js/styles/github.css'
import { addIcon } from '@iconify/vue'
import { i18n } from './i18n'
import App from './App.vue'
import './assets/main.css'
// Theme palettes must load after main.css so [data-theme] rules win at equal specificity.
import './assets/themes.css'

// Suppress browser's native right-click menu everywhere except text inputs.
document.addEventListener('contextmenu', (e) => {
  const tag = (e.target as HTMLElement).tagName
  if (tag !== 'INPUT' && tag !== 'TEXTAREA') e.preventDefault()
})


// Custom app icon: the original hand-drawn translate glyph (language-neutral —
// no letters, no kana). Registered eagerly so every translate button/tab can
// use `argus:translate` instead of Fluent's translate (which shows Japanese あ).
addIcon('argus:translate', {
  body: '<g fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="m5 8 6 6"/><path d="m4 14 6-6 2-3"/><path d="M2 5h12"/><path d="M7 2h1"/><path d="m22 22-5-10-5 10"/><path d="M14 18h6"/></g>',
  width: 24,
  height: 24,
})

const app = createApp(App)
app.use(createPinia())
app.use(VirtualScroller)
app.use(i18n)
app.mount('#app')

// Register the Fluent UI System Icons (app chrome) first, then warm the Fluent
// Emoji SVGs — both in the background so the large JSON parses never block first
// paint. Iconify's <Icon> picks the icons up reactively once each set lands.
import('./utils/fluentIcons').then(m => m.ensureFluentUiIcons())
import('./utils/fluentEmoji').then(m => m.ensureFluentIcons())
