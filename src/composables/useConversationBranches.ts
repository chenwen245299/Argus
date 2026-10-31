import { computed, ref, watch, type Ref } from 'vue'
import { conversationBranchRows, type BranchConversation } from '../utils/chatBranches'

/** Local display preference; folding never changes the saved conversations. */
export function useConversationBranches<T extends BranchConversation>(
  conversations: Ref<T[]>, scope: () => string | null,
) {
  const collapsedBranches = ref(new Set<string>())
  const allRows = computed(() => conversationBranchRows(conversations.value))
  const key = () => {
    const current = scope()
    return current ? `argus:chat:collapsed-branches:${current}` : null
  }
  watch(scope, () => {
    let ids: unknown
    try { const name = key(); ids = name ? JSON.parse(localStorage.getItem(name) || '[]') : [] } catch { ids = [] }
    collapsedBranches.value = new Set(Array.isArray(ids) ? ids.filter((id): id is string => typeof id === 'string') : [])
  }, { immediate: true, flush: 'sync' })

  const conversationRows = computed(() => {
    let hiddenDepth: number | null = null
    return allRows.value.filter(row => {
      if (hiddenDepth !== null && row.depth > hiddenDepth) return false
      hiddenDepth = collapsedBranches.value.has(row.conversation.id) ? row.depth : null
      return true
    })
  })
  function save() {
    try { const name = key(); if (name) localStorage.setItem(name, JSON.stringify([...collapsedBranches.value])) } catch { /* storage unavailable */ }
  }
  function toggleBranch(id: string) {
    if (collapsedBranches.value.has(id)) collapsedBranches.value.delete(id)
    else collapsedBranches.value.add(id)
    save()
  }
  function revealBranch(id: string) {
    const rows = allRows.value, index = rows.findIndex(row => row.conversation.id === id)
    if (index < 0) return
    let depth = rows[index].depth, changed = false
    for (let i = index - 1; i >= 0 && depth > 0; i--) {
      if (rows[i].depth >= depth) continue
      depth = rows[i].depth
      changed = collapsedBranches.value.delete(rows[i].conversation.id) || changed
    }
    if (changed) save()
  }
  return { conversationRows, collapsedBranches, toggleBranch, revealBranch }
}
