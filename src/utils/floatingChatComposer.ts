import { nextTick, ref, watch, type Ref } from 'vue'

/** Let the conversation scroll behind the floating composer, while keeping its
 * final message above the whole dock (queue, attachments and approval included).
 * Resizing the dock must not move someone who has scrolled up to read. */
export function useFloatingChatComposer(messages: Ref<HTMLElement | null>, followBottom?: () => boolean) {
  const composerDock = ref<HTMLElement | null>(null)
  const composerClearance = ref('160px')

  watch(composerDock, (dock, _previous, onCleanup) => {
    if (!dock) return
    let disposed = false
    const measure = () => {
      const scroller = messages.value
      const follow = followBottom?.() ?? (!!scroller && scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 48)
      composerClearance.value = `${Math.ceil(dock.getBoundingClientRect().height)}px`
      if (follow && scroller) void nextTick(() => {
        if (!disposed && scroller === messages.value) scroller.scrollTop = scroller.scrollHeight
      })
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(dock)
    onCleanup(() => { disposed = true; observer.disconnect() })
  }, { flush: 'post' })

  return { composerDock, composerClearance }
}
