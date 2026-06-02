import { createApp } from 'vue'
import { createPinia } from 'pinia'
import VirtualScroller from 'vue-virtual-scroller'
import 'vue-virtual-scroller/dist/vue-virtual-scroller.css'
import 'pdfjs-dist/web/pdf_viewer.css'
import 'katex/dist/katex.min.css'
import 'highlight.js/styles/github.css'
import router from './router'
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
app.use(router)
app.use(VirtualScroller)
app.use(i18n)
app.mount('#app')
