import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

// A generic, single-slot progress tracker for background batch operations
// triggered from the paper list's multi-select context menu (vectorize, AI
// analysis, delete, …). Only one batch runs at a time; the Toolbar renders its
// status next to the AI-analysis progress chips.

export type BatchItemStatus = 'pending' | 'running' | 'done' | 'error'

export interface BatchTaskItem {
  slug: string
  title: string
  status: BatchItemStatus
  message?: string
}

export const useBatchTasksStore = defineStore('batchTasks', () => {
  const label = ref('')
  const items = ref<BatchTaskItem[]>([])
  const running = ref(false)
  let clearTimer: ReturnType<typeof setTimeout> | null = null

  const total = computed(() => items.value.length)
  const doneCount = computed(
    () => items.value.filter(i => i.status === 'done' || i.status === 'error').length
  )
  const errorCount = computed(() => items.value.filter(i => i.status === 'error').length)
  const activeCount = computed(() => items.value.filter(i => i.status === 'running').length)

  function start(opLabel: string, targets: { slug: string; title: string }[]) {
    if (clearTimer) { clearTimeout(clearTimer); clearTimer = null }
    label.value = opLabel
    items.value = targets.map(t => ({ slug: t.slug, title: t.title, status: 'pending' as const }))
    running.value = true
  }

  function setStatus(slug: string, status: BatchItemStatus, message?: string) {
    const it = items.value.find(i => i.slug === slug)
    if (it) { it.status = status; it.message = message }
  }

  function finish() {
    running.value = false
    // Keep the final tally visible briefly, then clear the chip.
    clearTimer = setTimeout(() => {
      items.value = []
      label.value = ''
    }, 3000)
  }

  return {
    label,
    items,
    running,
    total,
    doneCount,
    errorCount,
    activeCount,
    start,
    setStatus,
    finish,
  }
})
