import type { Theme } from 'vitepress'
import DefaultTheme from 'vitepress/theme'
import Media from './components/Media.vue'
import Landing from './components/Landing.vue'
import Download from './components/Download.vue'
import './custom.css'

export default {
  extends: DefaultTheme,
  enhanceApp({ app, router }) {
    // Available in any markdown page.
    app.component('Media', Media)
    app.component('Landing', Landing)
    app.component('Download', Download)

    // Toggle a `.argus-scrolled` class on <html> so the landing nav can stay
    // transparent at the top of the hero but gain a background once scrolled.
    if (typeof window !== 'undefined') {
      const sync = () => {
        document.documentElement.classList.toggle('argus-scrolled', window.scrollY > 24)
      }
      window.addEventListener('scroll', sync, { passive: true })
      const prev = router.onAfterRouteChanged
      router.onAfterRouteChanged = (to) => {
        prev?.(to)
        requestAnimationFrame(sync) // scroll resets to top on navigation
      }
      requestAnimationFrame(sync)
    }
  },
} satisfies Theme
