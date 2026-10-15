import { computed, onScopeDispose, ref, shallowReactive, shallowRef, watch, type Ref } from 'vue'
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

/** Borrow the main composer to edit a held queue row. Saving replaces that row
 * in place; cancelling leaves it untouched. Neither operation loses the draft
 * that occupied the composer before editing, or changes captured model choices. */
export function useChatQueueEditor<T extends QueuedChatMessage>(options: {
  queue: ReturnType<typeof useChatSendQueue<T>>
  scope: () => string | null
  input: Ref<string>
  files: Ref<Attachment[]>
  focus: () => void
}) {
  const editingId = ref<string | null>(null)
  let draft: { text: string; files: Attachment[] } | null = null

  function restore() {
    const id = editingId.value
    editingId.value = null
    if (draft) {
      options.input.value = draft.text
      options.files.value = draft.files
      draft = null
    }
    return id
  }
  function cancel(focus = true) {
    const id = restore()
    if (!id) return
    options.queue.hold(id, false)
    if (focus) options.focus()
  }
  function begin(id: string) {
    const item = options.queue.items.value.find(item => item.id === id && item.scope === options.scope())
    if (!item) return
    // Hold the next target before releasing a previous edit: the current
    // generation may already have finished while the user switches rows.
    options.queue.hold(id, true)
    if (editingId.value !== id) {
      cancel(false)
      draft = { text: options.input.value, files: options.files.value }
      editingId.value = id
      options.input.value = item.text
      options.files.value = item.files.map(file => ({ ...file }))
    }
    options.focus()
  }
  function commit(immediate = false): boolean {
    const id = editingId.value
    if (!id) return false
    const item = options.queue.items.value.find(item => item.id === id)
    if (!item || item.scope !== options.scope()) { cancel(); return true }
    if (!options.input.value.trim() && !options.files.value.length) return true
    options.queue.edit(id, options.input.value, [...options.files.value])
    const updated = options.queue.items.value.find(item => item.id === id)!
    restore()
    if (immediate) options.queue.steer(updated)
    else options.queue.hold(id, false)
    options.focus()
    return true
  }

  watch(options.scope, () => cancel(false), { flush: 'sync' })
  watch(options.queue.items, items => {
    if (editingId.value && !items.some(item => item.id === editingId.value)) cancel(false)
  }, { flush: 'sync' })
  onScopeDispose(() => cancel(false))
  return { editingId, begin, cancel, commit }
}
