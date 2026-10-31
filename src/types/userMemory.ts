export type MemoryCategory = 'knowledge' | 'background' | 'research' | 'preferences' | 'goals'
export type KnowledgeLevel = 'familiar' | 'learning' | 'new' | 'unspecified'
export interface UserMemoryEntry {
  id: string
  key: string
  category: MemoryCategory
  content: string
  knowledgeLevel: KnowledgeLevel
  evidence: string
  source: string
  createdAt: string
  updatedAt: string
  locked: boolean
}
export interface UserMemory {
  enabled: boolean
  autoLearn: boolean
  entries: UserMemoryEntry[]
  revision: number
  lastCheckedAt: string | null
  lastError: string | null
  updating: boolean
  pendingTurns?: number
  backgroundRuns?: number
  liveWrites?: number
  syncRoot?: string | null
  syncError?: string | null
}
