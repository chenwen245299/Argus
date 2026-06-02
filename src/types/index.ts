export interface PaperMeta {
  id: string
  title: string
  authors: string[]
  year?: number
  doi?: string
  arxiv_id?: string
  venue?: string
  tags: string[]
  added_at: string
  original_filename?: string
  reading_status: string // "unread" | "reading" | "read"
  abstract?: string
  bibtex?: string
  canvas_notes?: string[]
}

export interface PaperStatus {
  text_extracted: boolean
  ai_summary_done: boolean
  vectorized: boolean
  metadata_fetched: boolean
  last_updated: string
  cli_analyzed?: boolean
}

export interface ReadingState {
  page: number
  scroll_ratio: number
  updated_at: string
}

export interface Rect {
  x: number
  y: number
  width: number
  height: number
}

export interface Highlight {
  id: string
  page: number
  rects: Rect[]
  text: string
  color: string
  note?: string
  created_at: string
  style?: 'highlight' | 'underline'
}

export interface Note {
  id: string
  title: string
  created_at: string
  updated_at: string
}

export interface PaperIndexEntry {
  slug: string
  id: string
  title: string
  authors: string[]
  year?: number
  venue?: string
  tags: string[]
  status: PaperStatus
  added_at: string
  reading_status: string
  meta_mtime?: number
}

export interface LibraryConfig {
  version: string
  created_at: string
}

export type ImportStatus = 'importing' | 'fetching_meta' | 'renaming' | 'downloading' | 'done' | 'error'

export interface ImportJob {
  id: string
  slug: string
  filename: string
  status: ImportStatus
  error?: string
}

export type SortField = 'title' | 'year' | 'added_at' | 'authors' | 'venue'
export type SortDir = 'asc' | 'desc'

export type NavItem = 'all' | 'inbox' | 'search' | `tag:${string}` | `collection:${string}`

// ── M4: Collections ───────────────────────────────────────────────────────────

export interface Collection {
  id: string
  name: string
  emoji?: string
  parent_id?: string
  created_at: string
}

export interface Assignment {
  paper_id: string
  collection_id: string
}

export interface CollectionsFile {
  collections: Collection[]
  assignments: Assignment[]
}

// ── M4: App Settings ──────────────────────────────────────────────────────────

export type ThemeId = 'system' | 'light' | 'dark' | 'warm' | 'forest' | 'rose'

export interface AppSettings {
  appearance: ThemeId
  extraction_default: string
  metadata_ai_provider_id?: string
  metadata_ai_model_id?: string
  metadata_ai_prompt?: string
  ai_summary_provider_id?: string
  ai_summary_model_id?: string
  ai_summary_prompt?: string
  abstract_ai_provider_id?: string
  abstract_ai_model_id?: string
  abstract_ai_prompt?: string
}

// ── M4: Search ────────────────────────────────────────────────────────────────

export interface SearchHit {
  paper_id: string
  slug: string
  title: string
  authors: string[]
  snippet: string
}

// ── M5: AI Service Center ─────────────────────────────────────────────────────

export interface AiModel {
  id: string
  display_name: string
  capabilities: string[]
  context_length?: number
  enabled: boolean
  input_price_per_million?: number   // CNY per 1M input tokens
  output_price_per_million?: number  // CNY per 1M output tokens
}

export interface AiProviderInfo {
  id: string
  name: string
  kind: 'openai_compatible' | 'anthropic' | string
  base_url: string
  enabled: boolean
  has_key: boolean
  models: AiModel[]
}

export interface AiSettingsInfo {
  providers: AiProviderInfo[]
  default_provider_id?: string
  default_model_id?: string
}

export interface AiProviderInput {
  id?: string
  name: string
  kind: string
  base_url: string
  enabled: boolean
  models: AiModel[]
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | string
  content: string
}

export interface ModelSelection {
  providerId: string
  modelId: string
}

// ── M6: CLI Tools ─────────────────────────────────────────────────────────────

export interface CliTool {
  id: string
  name: string
  command: string
  args_template: string[]
  enabled: boolean
  detected: boolean
  version?: string
}

export interface CliOutputPolish {
  enabled: boolean
  provider_id: string
  model_id: string
  prompt: string
}

export interface CliSettings {
  tools: CliTool[]
  prompt_templates: CliPromptTemplate[]
  polish: CliOutputPolish
}

export interface CliPromptTemplate {
  id: string
  name: string
  prompt_template: string
}

export interface CliAnalysisEntry {
  filename: string
  name: string
  created_at: string
  path: string
}

export interface CliAnalysisEvent {
  chunk: string
  source: 'stdout' | 'stderr' | 'error'
  done: boolean
  exit_code: number | null
  cancelled: boolean
}

// ── M7: RAG / Vectorization ───────────────────────────────────────────────────

export interface RagSettings {
  provider_id: string | null
  embedding_model: string | null
  chunk_size: number
  chunk_overlap: number
  top_k: number
  enabled: boolean
}

export interface VectorsMeta {
  provider_id: string
  embedding_model: string
  dimension: number
}

export interface VectorStoreInfo {
  total_chunks: number
  unique_papers: number
  dimension: number | null
  provider_id: string | null
  embedding_model: string | null
}

// ── Vectorize pipeline ────────────────────────────────────────────────────────

export interface HighlightInput {
  id: string
  page: number
  text: string
  note: string | null
}

export interface NoteInput {
  id: string
  title: string
  content: string
}

export interface PaperVectorizeInput {
  paper_id: string
  paper_title: string
  meta_text: string
  fulltext: string
  highlights: HighlightInput[]
  notes: NoteInput[]
}

export interface ChunkInput {
  text: string
  source_type: string
  source_id: string | null
  source_label: string | null
}

export interface RetrievedChunk {
  chunk_id: string
  paper_id: string
  slug: string
  chunk_index: number
  text: string
  score: number
  paper_title: string
  source_type: string   // "text" | "metadata" | "highlight" | "note"
  source_id: string | null
  source_label: string | null
}

// ── M8: arXiv Auto-Tracking ───────────────────────────────────────────────────

export interface ArxivConfig {
  categories: string[]
  keywords: string[]
  auto_fetch_enabled: boolean
  interval_days: number
  fetch_time: string
  days_back: number
  max_fetch: number
  ai_analysis_enabled: boolean
  ai_analysis_prompt: string
  ai_filter_enabled: boolean
  ai_filter_threshold: number
  ai_provider_id: string | null
  ai_model_id: string | null
  last_fetch_date: string | null
}

export interface ArxivPaper {
  arxiv_id: string
  title: string
  authors: string[]
  summary: string
  categories: string[]
  published: string
  updated: string
  pdf_url: string
  abs_url: string
  relevance_score: number | null
  relevance_reason: string | null
  key_contributions: string[]
  analysis_summary: string | null
  matched_topics: string[]
  analysis_status: 'pending' | 'analyzing' | 'done' | 'failed' | string
  in_library: boolean
  fetched_at: string
  read: boolean
  rating: number
}

export interface ArxivInbox {
  papers: ArxivPaper[]
  last_updated: string
}

// ── M9: Canvas ────────────────────────────────────────────────────────────────

export interface Viewport {
  offset_x: number
  offset_y: number
  zoom: number
}

export interface CanvasNode {
  node_id: string
  paper_id: string
  x: number
  y: number
  color?: string
  hover_source?: string
}

export interface CanvasEdge {
  edge_id: string
  from_node_id: string
  to_node_id: string
  label?: string
  color?: string
  stroke_width?: number
}

export interface Canvas {
  id: string
  name: string
  nodes: CanvasNode[]
  edges: CanvasEdge[]
  viewport: Viewport
  created_at: string
  updated_at: string
}

export interface CanvasIndexEntry {
  id: string
  name: string
  node_count: number
  updated_at: string
}

export interface CanvasSettings {
  hover_content_source: 'notes' | 'summary' | 'abstract' | 'none' | string
}

export interface ArxivScheduleStatus {
  auto_fetch_enabled: boolean
  last_fetch_date: string | null
  next_scheduled: string | null
  fetching: boolean
  analyzing: boolean
  analyzed_count: number
  total_pending: number
}

export interface SuggestedEdge {
  from_paper_id: string
  to_paper_id: string
  source: 'citations' | 'similarity' | 'ai'
  reason?: string
  score?: number
}

export interface NodePosition {
  node_id: string
  x: number
  y: number
}
