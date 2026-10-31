/** Durable provenance; the child owns its own copy of the inherited messages. */
export interface ConversationBranch {
  parentId: string
  parentTitle: string
  messageId: string
  turn: number
}

export interface BranchConversation {
  id: string
  updatedAt: string
  branch?: ConversationBranch
}

/** Keep each family together, ordered by its most recently active descendant.
 * Missing parents and malformed cycles remain visible and independently usable. */
export function conversationBranchRows<T extends BranchConversation>(conversations: readonly T[]) {
  const byId = new Map(conversations.map(conv => [conv.id, conv]))
  const children = new Map<string, T[]>()
  const roots: T[] = []
  for (const conv of conversations) {
    const parent = conv.branch?.parentId
    if (parent && parent !== conv.id && byId.has(parent)) {
      const siblings = children.get(parent) ?? []
      siblings.push(conv)
      children.set(parent, siblings)
    } else roots.push(conv)
  }
  const activity = new Map<string, string>()
  function latest(conv: T, path = new Set<string>()): string {
    if (path.has(conv.id)) return conv.updatedAt
    if (activity.has(conv.id)) return activity.get(conv.id)!
    const next = new Set(path).add(conv.id)
    const stamp = (children.get(conv.id) ?? []).reduce((stamp, child) => {
      const date = latest(child, next)
      return date > stamp ? date : stamp
    }, conv.updatedAt)
    activity.set(conv.id, stamp)
    return stamp
  }
  const order = (a: T, b: T) => latest(b).localeCompare(latest(a)) || a.id.localeCompare(b.id)
  const rows: { conversation: T; depth: number; branchCount: number }[] = []
  const seen = new Set<string>()
  function visit(conv: T, depth: number) {
    if (seen.has(conv.id)) return
    seen.add(conv.id)
    const start = rows.length
    const row = { conversation: conv, depth, branchCount: 0 }
    rows.push(row)
    for (const child of [...(children.get(conv.id) ?? [])].sort(order)) visit(child, depth + 1)
    row.branchCount = rows.length - start - 1
  }
  for (const root of roots.sort(order)) visit(root, 0)
  for (const conv of [...conversations].sort(order)) visit(conv, 0)
  return rows
}

/** JSON also unwraps Vue proxies. A branch must never inherit a live request,
 * render buffer, compaction checkpoint, or the source conversation's bills. */
export function cloneBranchRecords<T>(records: T): T {
  return JSON.parse(JSON.stringify(records, (key, value) => {
    if (['displayContent', 'displayReasoning', 'audioUrl', 'contextContent'].includes(key)) return undefined
    if (key === 'streaming' || key === 'contextCompacting') return false
    return value
  })) as T
}

/** Only files referenced by the retained tool evidence belong to the child. */
export function branchImageFiles(records: unknown): string[] {
  const files = new Set<string>()
  function walk(value: unknown) {
    if (!value || typeof value !== 'object') return
    if (Array.isArray(value)) { value.forEach(walk); return }
    const record = value as Record<string, unknown>
    if (typeof record.file === 'string' && record.file) files.add(record.file)
    Object.values(record).forEach(walk)
  }
  walk(records)
  return [...files]
}
