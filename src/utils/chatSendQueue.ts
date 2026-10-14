import { computed, onScopeDispose, shallowReactive, shallowRef, watch } from 'vue'
import type { Attachment } from './attachments'

export interface QueuedChatMessage {
  id: string
  scope: string
  text: string
  files: Attachment[]
}

/** One FIFO per conversation. Keep captured payloads out of deep reactivity:
 * they include attachments and the live conversation that owns the request. */
export function useChatSendQueue<T extends QueuedChatMessage>(options: {
  busy: (scope: string) => boolean
  valid: (item: T) => boolean
  send: (item: T) => Promise<void>
  interrupt: (item: T) => Promise<void>
}) {
  const items = shallowRef<T[]>([])
  const active = shallowReactive(new Set<string>())
  const paused = shallowReactive(new Set<string>())
  const held = new Set<string>()
  const urgent = new Set<string>()
  const steering = new Map<string, Promise<void>>()
  const running = new Map<string, Promise<void>>()
  let disposed = false
  let revision = 0

  function remove(id: string) {
    items.value = items.value.filter(item => item.id !== id)
    held.delete(id)
    urgent.delete(id)
    pump()
  }

  function pump() {
    if (disposed) return
    for (const scope of new Set(items.value.map(item => item.scope))) {
      if (active.has(scope) || paused.has(scope) || steering.has(scope) || options.busy(scope)) continue
      const item = items.value.find(item => item.scope === scope && urgent.has(item.id))
        ?? items.value.find(item => item.scope === scope)
      if (!item || held.has(item.id)) continue
      if (!options.valid(item)) { remove(item.id); continue }
      active.add(scope)
      items.value = items.value.filter(entry => entry.id !== item.id)
      urgent.delete(item.id)
      const epoch = revision
      // Defer send until the lock is installed, including synchronous callbacks.
      const task = Promise.resolve().then(() => {
        if (!disposed && epoch === revision && options.valid(item)) return options.send(item)
      }).catch(error => console.error('Chat send failed', error)).finally(() => {
        if (running.get(scope) !== task) return
        running.delete(scope)
        active.delete(scope)
        pump()
      })
      running.set(scope, task)
    }
  }

  function enqueue(item: T) {
    if (disposed) return
    paused.delete(item.scope)
    items.value = [...items.value, item]
    pump()
  }

  function steer(item: T) {
    if (disposed) return
    if (!items.value.some(entry => entry.id === item.id)) items.value = [...items.value, item]
    urgent.add(item.id)
    held.delete(item.id)
    paused.delete(item.scope)
    if (steering.has(item.scope)) return
    const epoch = revision
    const task = Promise.resolve().then(async () => {
      await options.interrupt(item)
      await running.get(item.scope)
    }).catch(error => {
      paused.add(item.scope)
      console.error('Chat steering failed', error)
    }).finally(() => {
      if (steering.get(item.scope) !== task) return
      steering.delete(item.scope)
      if (epoch === revision) pump()
    })
    steering.set(item.scope, task)
  }

  function edit(id: string, text: string, files: Attachment[]) {
    items.value = items.value.map(item => item.id === id ? { ...item, text: text.trim(), files } : item)
  }
  function hold(id: string, on: boolean) {
    if (on) held.add(id)
    else { held.delete(id); pump() }
  }
  function pause(scope: string) { paused.add(scope) }
  function resume(scope: string) { paused.delete(scope); pump() }
  function clear(scope?: string) {
    if (scope) {
      pause(scope)
      items.value.filter(item => item.scope === scope).forEach(item => { held.delete(item.id); urgent.delete(item.id) })
      items.value = items.value.filter(item => item.scope !== scope)
    } else {
      revision++
      items.value = []
      held.clear()
      urgent.clear()
      paused.clear()
    }
  }

  // Also drain after requests started outside the composer (regenerate, guided
  // reading, or a stream mirrored from another paper-AI window) have finished.
  const busyScopes = computed(() => items.value.map(item => `${item.scope}:${options.busy(item.scope)}`).join('|'))
  watch(busyScopes, pump, { flush: 'post' })
  onScopeDispose(() => { disposed = true; clear() })
  return { items, active, paused, enqueue, steer, edit, hold, remove, pause, resume, clear }
}
