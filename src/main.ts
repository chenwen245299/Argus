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
import { i18n } from './i18n'
import App from './App.vue'
import './assets/main.css'

// Suppress browser's native right-click menu everywhere except text inputs.
document.addEventListener('contextmenu', (e) => {
  const tag = (e.target as HTMLElement).tagName
  if (tag !== 'INPUT' && tag !== 'TEXTAREA') e.preventDefault()
})


const app = createApp(App)
app.use(createPinia())
app.use(VirtualScroller)
app.use(i18n)
app.mount('#app')

// Warm up the Fluent Emoji SVGs in the background so collection icons upgrade
// from the native fallback shortly after startup (non-blocking).
import('./utils/fluentEmoji').then(m => m.ensureFluentIcons())
