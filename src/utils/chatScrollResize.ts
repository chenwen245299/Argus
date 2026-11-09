import { nextTick, watch, type Ref } from 'vue'

/** Keep the latest answer in view as the composer changes the scroll viewport.
 * Use the previous viewport height to distinguish a pinned reader from someone
 * reading earlier messages before the resize reduces their visible area. */
export function useChatScrollResize(messages: Ref<HTMLElement | null>, followBottom?: () => boolean) {
  watch(messages, (scroller, _previous, onCleanup) => {
    if (!scroller) return
    let disposed = false
    let previousHeight = scroller.clientHeight
    const measure = () => {
      const follow = followBottom?.() ?? scroller.scrollHeight - scroller.scrollTop - previousHeight < 48
      previousHeight = scroller.clientHeight
      if (follow) void nextTick(() => {
        // User input can pause following after measurement but before Vue's
        // queued callback. Recheck intent rather than applying a stale decision.
        if (!disposed && scroller === messages.value && (followBottom?.() ?? true)) scroller.scrollTop = scroller.scrollHeight
      })
    }
    const observer = new ResizeObserver(measure)
    observer.observe(scroller)
    onCleanup(() => { disposed = true; observer.disconnect() })
  }, { flush: 'post' })
}
