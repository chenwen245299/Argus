import { ref, watch, onScopeDispose, type Ref } from 'vue'
import { useChatScrollResize } from './chatScrollResize'

/** Follow new output until the reader scrolls up; resume on a return to bottom.
 * Scroll intent is recorded before native movement so a queued stream frame
 * cannot win a race with a wheel, touch, keyboard or scrollbar interaction. */
export function useChatScroll(messages: Ref<HTMLElement | null>) {
  const stickToBottom = ref(true)
  let frame: number | null = null
  let previousTop = 0
  const atBottom = (el: HTMLElement) => el.scrollHeight - el.scrollTop - el.clientHeight <= 2
  const cancelScroll = () => {
    if (frame !== null) cancelAnimationFrame(frame)
    frame = null
  }
  function pauseFollowing() {
    stickToBottom.value = false
    cancelScroll()
  }
  function scrollToBottom(force = false) {
    if (force) stickToBottom.value = true
    if (!stickToBottom.value || frame !== null) return
    const scroller = messages.value
    frame = requestAnimationFrame(() => {
      frame = null
      if (!stickToBottom.value || !scroller || messages.value !== scroller) return
      scroller.scrollTop = scroller.scrollHeight
      previousTop = scroller.scrollTop
    })
  }

  useChatScrollResize(messages, () => stickToBottom.value)
  watch(messages, (el, _old, onCleanup) => {
    cancelScroll()
    if (!el) return
    previousTop = el.scrollTop
    let touchY: number | null = null
    const onScroll = () => {
      const top = el.scrollTop
      // A shrinking answer/expanding viewport can clamp scrollTop upward at
      // the bottom. That is layout, not an instruction to stop following.
      if (top < previousTop - 0.5 && !atBottom(el)) pauseFollowing()
      else if (top > previousTop + 0.5 && atBottom(el)) stickToBottom.value = true
      previousTop = top
    }
    const onWheel = (event: WheelEvent) => {
      if (event.deltaY < 0) pauseFollowing()
      else if (event.deltaY > 0 && atBottom(el)) stickToBottom.value = true
    }
    const onKey = (event: KeyboardEvent) => {
      const target = event.target instanceof Element ? event.target : null
      if (target?.closest('input,textarea,[contenteditable="true"]')) return
      if (['ArrowUp', 'PageUp', 'Home'].includes(event.key) || (event.key === ' ' && event.shiftKey)) pauseFollowing()
      else if (['ArrowDown', 'PageDown', 'End', ' '].includes(event.key) && atBottom(el)) stickToBottom.value = true
    }
    const onPointer = (event: PointerEvent) => {
      if (event.target === el && event.clientX >= el.getBoundingClientRect().right - 16) pauseFollowing()
    }
    const onTouchStart = (event: TouchEvent) => { touchY = event.touches[0]?.clientY ?? null }
    const onTouchMove = (event: TouchEvent) => {
      const y = event.touches[0]?.clientY
      if (touchY != null && y != null) {
        if (y > touchY) pauseFollowing()
        else if (y < touchY && atBottom(el)) stickToBottom.value = true
      }
      touchY = y ?? null
    }
    el.addEventListener('scroll', onScroll, { passive: true })
    el.addEventListener('wheel', onWheel, { passive: true })
    el.addEventListener('keydown', onKey)
    el.addEventListener('pointerdown', onPointer, { passive: true })
    el.addEventListener('touchstart', onTouchStart, { passive: true })
    el.addEventListener('touchmove', onTouchMove, { passive: true })
    scrollToBottom()
    onCleanup(() => {
      cancelScroll()
      el.removeEventListener('scroll', onScroll)
      el.removeEventListener('wheel', onWheel)
      el.removeEventListener('keydown', onKey)
      el.removeEventListener('pointerdown', onPointer)
      el.removeEventListener('touchstart', onTouchStart)
      el.removeEventListener('touchmove', onTouchMove)
    })
  }, { flush: 'post' })
  onScopeDispose(cancelScroll)
  return { stickToBottom, pauseFollowing, scrollToBottom }
}
