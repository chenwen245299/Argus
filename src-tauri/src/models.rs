use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaperMeta {
    pub id: String,
    pub title: String,
    // `serde(default)` so a meta.json that predates these fields (or was
    // hand-edited / partially synced) still loads instead of failing wholesale,
    // which would make the paper unreadable in the UI.
    #[serde(default)]
    pub authors: Vec<String>,
    pub year: Option<u32>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub venue: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub added_at: String,
    pub original_filename: Option<String>,
    /// User-controlled reading status: "unread" | "reading" | "read"
    #[serde(default = "default_reading_status")]
    pub reading_status: String,
    /// AI-generated short abstract for this paper.
    #[serde(rename = "abstract", default, skip_serializing_if = "Option::is_none")]
    pub paper_abstract: Option<String>,
    /// User-provided BibTeX citation entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bibtex: Option<String>,
    /// Note IDs pinned to canvas hover for this paper (empty = show most recent).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canvas_notes: Vec<String>,
    /// How the paper was imported: "file" | "arxiv" | "url". None for legacy entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<String>,
    /// User-provided citation count (e.g. from Google Scholar).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cite_count: Option<u32>,
}

pub fn normalize_import_source(import_source: Option<&str>, arxiv_id: Option<&str>) -> String {
    match import_source
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
        .as_deref()
    {
        Some("file") | Some("arxiv") | Some("biorxiv") | Some("url") => import_source
            .unwrap_or("file")
            .trim()
            .to_ascii_lowercase(),
        _ if arxiv_id.is_some_and(|id| !id.trim().is_empty()) => "arxiv".to_string(),
        _ => "file".to_string(),
    }
}

fn default_reading_status() -> String {
    "unread".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaperStatus {
    pub text_extracted: bool,
    pub ai_summary_done: bool,
    pub vectorized: bool,
    pub metadata_fetched: bool,
    pub last_updated: String,
}

impl Default for PaperStatus {
    fn default() -> Self {
        PaperStatus {
            text_extracted: false,
            ai_summary_done: false,
            vectorized: false,
            metadata_fetched: false,
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReadingState {
    pub page: u32,
    pub scroll_ratio: f32,
    pub updated_at: String,
}

impl Default for ReadingState {
    fn default() -> Self {
        ReadingState {
            page: 1,
            scroll_ratio: 0.0,
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Highlight {
    pub id: String,
    pub page: u32,
    pub rects: Vec<Rect>,
    pub text: String,
    pub color: String,
    pub note: Option<String>,
    pub created_at: String,
    #[serde(default = "default_highlight_style")]
    pub style: String,
}

fn default_highlight_style() -> String {
    "highlight".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Cached index entry — index.json is a rebuildable cache, so extra fields like `tags` are fine.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaperIndexEntry {
    pub slug: String,
    pub id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u32>,
    #[serde(default)]
    pub venue: Option<String>,
    pub tags: Vec<String>,
    pub status: PaperStatus,
    pub added_at: String,
    #[serde(default = "default_reading_status_entry")]
    pub reading_status: String,
    /// Unix seconds of meta.json when last indexed — used by incremental scan to skip unchanged files.
    /// Old index.json files deserialize this as 0, causing a one-time re-read to populate it.
    #[serde(default)]
    pub meta_mtime: u64,
    /// Import source: "file" | "arxiv" | "url". None for legacy entries (inferred on frontend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_source: Option<String>,
    /// User-provided citation count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cite_count: Option<u32>,
}

fn default_reading_status_entry() -> String {
    "unread".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LibraryConfig {
    pub version: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct IndexFile {
    pub papers: Vec<PaperIndexEntry>,
}

// ── Multi-Notes ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

// ── M4: Collections ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Collection {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    pub parent_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Assignment {
    pub paper_id: String,
    pub collection_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CollectionsFile {
    pub collections: Vec<Collection>,
    pub assignments: Vec<Assignment>,
}

impl Default for CollectionsFile {
    fn default() -> Self {
        CollectionsFile {
            collections: vec![],
            assignments: vec![],
        }
    }
}

// ── M4: App Settings ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppSettings {
    pub appearance: String,
    pub extraction_default: String,
    /// USD to CNY exchange rate used for displaying provider-reported AI costs.
    #[serde(default = "default_usd_to_cny_rate")]
    pub usd_to_cny_rate: f64,
    /// AI provider ID to use for metadata extraction (None = default)
    #[serde(default)]
    pub metadata_ai_provider_id: Option<String>,
    /// AI model ID to use for metadata extraction (None = default)
    #[serde(default)]
    pub metadata_ai_model_id: Option<String>,
    /// User-editable prompt template for AI metadata extraction.
    #[serde(default = "default_metadata_ai_prompt")]
    pub metadata_ai_prompt: String,
    /// AI provider ID to use for paper summaries (None = default)
    #[serde(default)]
    pub ai_summary_provider_id: Option<String>,
    /// AI model ID to use for paper summaries (None = default)
    #[serde(default)]
    pub ai_summary_model_id: Option<String>,
    /// User-editable prompt template for AI paper summaries.
    #[serde(default = "default_ai_summary_prompt")]
    pub ai_summary_prompt: String,
    /// AI provider ID to use for abstract extraction (None = default)
    #[serde(default)]
    pub abstract_ai_provider_id: Option<String>,
    /// AI model ID to use for abstract extraction (None = default)
    #[serde(default)]
    pub abstract_ai_model_id: Option<String>,
    /// User-editable prompt for AI abstract extraction.
    #[serde(default = "default_abstract_ai_prompt")]
    pub abstract_ai_prompt: String,
    /// AI provider ID to use for inline translation (None = default)
    #[serde(default)]
    pub translate_ai_provider_id: Option<String>,
    /// AI model ID to use for inline translation (None = default)
    #[serde(default)]
    pub translate_ai_model_id: Option<String>,
    /// User-editable prompt for inline text translation.
    #[serde(default = "default_translate_ai_prompt")]
    pub translate_ai_prompt: String,
    /// AI provider ID for chat conversation title generation (None = default)
    #[serde(default)]
    pub title_ai_provider_id: Option<String>,
    /// AI model ID for chat conversation title generation (None = default)
    #[serde(default)]
    pub title_ai_model_id: Option<String>,
    /// User-editable prompt for chat title generation.
    #[serde(default = "default_title_ai_prompt")]
    pub title_ai_prompt: String,
}

pub fn default_usd_to_cny_rate() -> f64 {
    7.20
}

pub fn default_metadata_ai_prompt() -> String {
    r#"Extract academic paper metadata from the text below.
Do NOT guess or infer missing fields — use null for anything not explicitly found in the text.
Reply with a JSON code block in exactly this format:

```json
{"title": "...", "authors": ["First Last", "First Last"], "year": 2024, "venue": "...", "doi": "...", "arxiv_id": "..."}
```

Text:
{text}"#
        .to_string()
}

pub fn default_ai_summary_prompt() -> String {
    "帮我用中文讲一下这篇论文，讲的越详细越好，我有这个领域的通用基础，但是没有这个小方向的基础。输出的时候只包含关于论文的讲解，不要包含寒暄的内容。开始时先用一段话总结这篇论文的核心内容。请用markdown形式输出你的讲解内容。"
        .to_string()
}

pub fn legacy_ai_summary_prompt_plain_detail() -> String {
    "帮我用中文讲一下这篇论文，讲的越详细越好，我有这个领域的通用基础，但是没有这个小方向的基础。输出的时候只包含关于论文的讲解，不要包含寒暄的内容。开始时先用一段话总结这篇论文的核心内容。"
        .to_string()
}

pub fn legacy_ai_summary_prompt_fulltext_detail() -> String {
    r#"你是一名严谨的研究助理。请只根据下面的论文全文，用中文写一份 Markdown 格式的 AI 总结。

目标读者是没有读过这篇论文的人；读完后应能完整理解这篇论文做了什么、为什么重要、怎么做、实验结果如何、有什么局限和可借鉴点。

要求：
- 不要编造全文中没有的信息；如果某项信息在论文全文中没有找到，请明确写“论文中未找到明确说明”
- 保留关键术语、方法名、数据集名和指标名
- 结论要具体，避免泛泛而谈
- 如果全文被截断，请基于可见内容总结，并说明可能不完整

请使用以下结构：
# AI 总结
## 一句话概括
## 研究背景与问题
## 核心创新点
- 列出 3-5 点，并解释每一点为什么重要
## 方法详解
- 详细说明模型、算法、系统设计、数据流程或理论推导
- 解释关键公式、模块或实验设计背后的直觉
## 实验设置
- 数据集、基线方法、评价指标、训练/实现细节
## 主要结果
- 总结最关键的定量和定性结果
- 对比基线，解释结果说明了什么
## 局限与注意点
## 适合引用的结论
## 延伸阅读或实践启发

论文全文：
{fulltext}"#
        .to_string()
}

pub fn legacy_ai_summary_prompt_with_metadata() -> String {
    r#"你是一名严谨的研究助理。请根据下面的论文全文和元数据，用中文写一份 Markdown 格式的 AI 总结。

目标读者是没有读过这篇论文的人；读完后应能完整理解这篇论文做了什么、为什么重要、怎么做、实验结果如何、有什么局限和可借鉴点。不要编造文本中没有的信息；如果某项信息在论文中没有找到，请明确写“论文中未找到明确说明”。

请使用以下结构：
# AI 总结
## 一句话概括
## 研究背景与问题
## 核心创新点
- 列出 3-5 点，并解释每一点为什么重要
## 方法详解
- 详细说明模型、算法、系统设计、数据流程或理论推导
- 解释关键公式、模块或实验设计背后的直觉
## 实验设置
- 数据集、基线方法、评价指标、训练/实现细节
## 主要结果
- 总结最关键的定量和定性结果
- 对比基线，解释结果说明了什么
## 局限与注意点
## 适合引用的结论
## 延伸阅读或实践启发

论文元数据：
标题：{title}
作者：{authors}
年份：{year}
期刊/会议：{venue}
DOI：{doi}
arXiv：{arxiv_id}
摘要：{abstract}

论文全文：
{fulltext}"#
        .to_string()
}

pub fn default_abstract_ai_prompt() -> String {
    "请只从下面给定的论文原文片段中抽取作者原文的 Abstract/摘要段落。\n\
要求：\n\
- 只输出从原文中找到的摘要正文，不要生成、改写、翻译或概括\n\
- 去掉“Abstract”“摘要”等标题词，只保留摘要正文\n\
- 如果给定文本中没有明确的摘要段落，请只输出 NOT_FOUND\n\
- 不要输出解释、Markdown、代码块或额外格式\n\n\
论文原文片段：\n{fulltext}"
        .to_string()
}

pub fn legacy_abstract_ai_prompt_generated() -> String {
    "请根据以下论文全文，用中文写一段简洁的摘要（3-5句话），概括论文的研究问题、核心方法和主要发现。只输出摘要正文，不要包含标题或额外格式。\n\n论文全文：\n{fulltext}"
        .to_string()
}

pub fn is_legacy_abstract_ai_prompt(prompt: &str) -> bool {
    prompt.trim() == legacy_abstract_ai_prompt_generated().trim()
}

pub fn default_title_ai_prompt() -> String {
    "请根据以下对话内容生成一个简洁的标题（不超过20字，直接输出标题文字，不要引号和多余说明）：\n\n用户：{user_msg}\n\nAI：{ai_msg}"
        .to_string()
}

pub fn default_translate_ai_prompt() -> String {
    "请将以下英文文本翻译成中文，保持学术风格，直接输出翻译结果，不需要任何额外说明：\n\n{text}"
        .to_string()
}

pub fn is_legacy_ai_summary_prompt(prompt: &str) -> bool {
    let prompt = prompt.trim();
    prompt == legacy_ai_summary_prompt_with_metadata().trim()
        || prompt == legacy_ai_summary_prompt_fulltext_detail().trim()
        || prompt == legacy_ai_summary_prompt_plain_detail().trim()
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            appearance: "system".to_string(),
            extraction_default: "lopdf".to_string(),
            usd_to_cny_rate: default_usd_to_cny_rate(),
            metadata_ai_provider_id: None,
            metadata_ai_model_id: None,
            metadata_ai_prompt: default_metadata_ai_prompt(),
            ai_summary_provider_id: None,
            ai_summary_model_id: None,
            ai_summary_prompt: default_ai_summary_prompt(),
            abstract_ai_provider_id: None,
            abstract_ai_model_id: None,
            abstract_ai_prompt: default_abstract_ai_prompt(),
            translate_ai_provider_id: None,
            translate_ai_model_id: None,
            translate_ai_prompt: default_translate_ai_prompt(),
            title_ai_provider_id: None,
            title_ai_model_id: None,
            title_ai_prompt: default_title_ai_prompt(),
        }
    }
}

// ── M4: Search ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchHit {
    pub paper_id: String,
    pub slug: String,
    pub title: String,
    pub authors: Vec<String>,
    pub snippet: String,
}

// ── M5: AI Service Center ─────────────────────────────────────────────────────

fn bool_true() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiModel {
    pub id: String,
    pub display_name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub context_length: Option<u64>,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    /// CNY price per 1 million input tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price_per_million: Option<f64>,
    /// CNY price per 1 million output tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price_per_million: Option<f64>,
    /// Time-based peak/off-peak pricing (e.g. DeepSeek). When true, the prices
    /// above are the off-peak (standard) prices and the peak_* fields hold the
    /// higher peak-hour prices.
    #[serde(default)]
    pub peak_pricing: bool,
    /// CNY peak-hour price per 1 million input tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_input_price_per_million: Option<f64>,
    /// CNY peak-hour price per 1 million output tokens
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_output_price_per_million: Option<f64>,
    /// CNY price per 1 million cached (prompt-cache-hit) input tokens, e.g.
    /// DeepSeek's much cheaper cache-hit rate. Used to estimate per-message cost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_input_price_per_million: Option<f64>,
    /// USD price per 1 million input tokens, as returned by providers like OpenRouter
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price_usd_per_million: Option<f64>,
    /// USD price per 1 million output tokens, as returned by providers like OpenRouter
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price_usd_per_million: Option<f64>,
    /// OpenRouter provider preference order (slugs like "Anthropic", "Together", etc.)
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provider_order: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiProvider {
    pub id: String,
    pub name: String,
    pub kind: String, // "openai_compatible" | "anthropic"
    pub base_url: String,
    #[serde(default = "bool_true")]
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<AiModel>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct AiSettings {
    #[serde(default)]
    pub providers: Vec<AiProvider>,
    pub default_provider_id: Option<String>,
    pub default_model_id: Option<String>,
}

/// Returned to frontend — never includes the raw API key.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiProviderInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub enabled: bool,
    pub has_key: bool,
    pub models: Vec<AiModel>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiSettingsInfo {
    pub providers: Vec<AiProviderInfo>,
    pub default_provider_id: Option<String>,
    pub default_model_id: Option<String>,
}

/// Input struct for add/update (id = None means new provider).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AiProviderInput {
    pub id: Option<String>,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub enabled: bool,
    pub models: Vec<AiModel>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: ChatContent,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub enum ChatContent {
    Text(String),
    Parts(Vec<ChatContentPart>),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum ChatContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlData },
    #[serde(rename = "file")]
    File { file: FileData },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ImageUrlData {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileData {
    pub filename: String,
    pub file_data: String,
}

impl From<String> for ChatContent {
    fn from(s: String) -> Self {
        ChatContent::Text(s)
    }
}

impl From<&str> for ChatContent {
    fn from(s: &str) -> Self {
        ChatContent::Text(s.to_string())
    }
}

// ── M7: RAG / Vectorization ──────────────────────────────────────────────────

/// RAG configuration. Both `provider_id` and `embedding_model` must be `Some` and non-empty
/// before any vectorization or retrieval can proceed — the app never substitutes a default.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RagSettings {
    pub provider_id: Option<String>,
    pub embedding_model: Option<String>,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "bool_true")]
    pub enabled: bool,
}

fn default_chunk_size() -> usize {
    1024
}
fn default_chunk_overlap() -> usize {
    200
}
fn default_top_k() -> usize {
    5
}

impl RagSettings {
    pub fn is_configured(&self) -> bool {
        self.enabled
            && self
                .provider_id
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false)
            && self
                .embedding_model
                .as_deref()
                .map(|s| !s.is_empty())
                .unwrap_or(false)
    }
}

impl Default for RagSettings {
    fn default() -> Self {
        RagSettings {
            provider_id: None,
            embedding_model: None,
            chunk_size: 1024,
            chunk_overlap: 200,
            top_k: 5,
            enabled: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VectorsMeta {
    pub provider_id: String,
    pub embedding_model: String,
    pub dimension: usize,
}

/// Per-embedding-model statistics for the multi-model vector store.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingModelStat {
    pub embedding_model: String,
    pub dimension: usize,
    pub total_chunks: usize,
    pub unique_papers: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct VectorStoreInfo {
    /// Chunk count for the *currently selected* embedding model.
    pub total_chunks: usize,
    /// Vectorized-paper count for the currently selected model.
    pub unique_papers: usize,
    pub dimension: Option<usize>,
    pub provider_id: Option<String>,
    pub embedding_model: Option<String>,
    /// Every embedding model that currently has vectors stored.
    pub models: Vec<EmbeddingModelStat>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: String,
    pub paper_id: String,
    pub slug: String,
    pub chunk_index: u32,
    pub text: String,
    pub score: f32,
    pub paper_title: String,
    /// "text" | "metadata" | "highlight" | "note"
    pub source_type: String,
    pub source_id: Option<String>,
    /// Human-readable label, e.g. "第3页批注" or "笔记: 我的想法"
    pub source_label: Option<String>,
}

// ── Embedding map (vector space visualization) ────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingMapPaper {
    pub paper_id: String,
    pub slug: String,
    pub title: String,
    pub chunk_count: usize,
    /// 2D PCA projection of the paper centroid (z-scored per axis)
    pub x: f32,
    pub y: f32,
    /// "unread" | "reading" | "read"
    pub reading_status: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingMapChunk {
    /// Index into `EmbeddingMapData::papers`
    pub paper: usize,
    pub x: f32,
    pub y: f32,
    /// "text" | "metadata" | "highlight" | "note"
    pub source_type: String,
    pub source_label: Option<String>,
    pub preview: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingMapEdge {
    /// Indices into `EmbeddingMapData::papers`
    pub a: usize,
    pub b: usize,
    /// Cosine similarity of the two paper centroids
    pub sim: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct EmbeddingMapData {
    pub papers: Vec<EmbeddingMapPaper>,
    pub chunks: Vec<EmbeddingMapChunk>,
    pub edges: Vec<EmbeddingMapEdge>,
    pub dimension: usize,
    /// The model whose vectors this map is showing.
    pub embedding_model: Option<String>,
    /// Every embedding model that has vectors stored, for the model picker.
    pub available_models: Vec<EmbeddingModelStat>,
}

// ── Snippet retrieval result ──────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RetrievedSnippet {
    pub snippet_id: String,
    pub library_id: String,
    pub text: String,
    pub score: f32,
    pub paper_id: String,
    pub paper_title: String,
    pub page: u32,
    pub note: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SnippetStoreInfo {
    pub embedded_count: usize,
}

// ── Vectorize pipeline (frontend-orchestrated) ───────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct HighlightInput {
    pub id: String,
    pub page: u32,
    pub text: String,
    pub note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NoteInput {
    pub id: String,
    pub title: String,
    pub content: String,
}

/// Raw content returned to frontend so it can chunk via LlamaIndex SentenceSplitter.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PaperVectorizeInput {
    pub paper_id: String,
    pub paper_title: String,
    /// Pre-formatted metadata string ready to embed as a single chunk.
    pub meta_text: String,
    /// Full extracted text (may be empty if not extracted yet).
    pub fulltext: String,
    pub highlights: Vec<HighlightInput>,
    pub notes: Vec<NoteInput>,
}

/// A single pre-chunked piece of text sent from frontend back to Rust for embedding.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ChunkInput {
    pub text: String,
    pub source_type: String,
    pub source_id: Option<String>,
    pub source_label: Option<String>,
}

// ── M8: arXiv Auto-Tracking ──────────────────────────────────────────────────

pub const DEFAULT_ARXIV_ANALYSIS_PROMPT: &str = r#"你是一名研究助理。根据以下论文元数据，评估其与这些主题的相关性：{topics}。

论文标题：{title}
作者：{authors}
摘要：{abstract}

提供（所有文字字段必须使用中文）：
1. relevance_score：整数 0-10（10 = 高度相关），有一个话题符合就算是相关了，也就是至少要6分以上
2. relevance_reason：一句话解释评分原因
3. key_contributions：2-3 个主要贡献的要点列表
4. summary：2-3 句通俗易懂的总结
5. matched_topics：从上方主题列表中选出与本文最匹配的主题，返回中文列表（无匹配则返回空列表）

仅回复符合此模式的有效 JSON：
{"relevance_score": 0, "relevance_reason": "", "key_contributions": [], "summary": "", "matched_topics": []}"#;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArxivConfig {
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub auto_fetch_enabled: bool,
    #[serde(default = "default_interval_days")]
    pub interval_days: u32,
    #[serde(default = "default_fetch_time")]
    pub fetch_time: String,
    #[serde(default = "default_days_back")]
    pub days_back: u32,
    #[serde(default = "default_max_fetch")]
    pub max_fetch: u32,
    #[serde(default)]
    pub ai_analysis_enabled: bool,
    #[serde(default = "default_arxiv_analysis_prompt")]
    pub ai_analysis_prompt: String,
    #[serde(default = "default_ai_filter_enabled")]
    pub ai_filter_enabled: bool,
    #[serde(default = "default_ai_filter_threshold")]
    pub ai_filter_threshold: f32,
    pub ai_provider_id: Option<String>,
    pub ai_model_id: Option<String>,
    pub last_fetch_date: Option<String>,
    #[serde(default = "default_ai_analysis_concurrency")]
    pub ai_analysis_concurrency: u32,
    #[serde(default)]
    pub fetch_biorxiv: bool,
    #[serde(default = "default_fetch_arxiv")]
    pub fetch_arxiv: bool,
}

fn default_fetch_arxiv() -> bool {
    true
}

fn default_ai_analysis_concurrency() -> u32 {
    5
}

fn default_interval_days() -> u32 {
    1
}
fn default_fetch_time() -> String {
    "09:00".to_string()
}
fn default_days_back() -> u32 {
    5
}
fn default_max_fetch() -> u32 {
    100
}
fn default_arxiv_analysis_prompt() -> String {
    DEFAULT_ARXIV_ANALYSIS_PROMPT.to_string()
}
fn default_ai_filter_enabled() -> bool {
    true
}
fn default_ai_filter_threshold() -> f32 {
    6.0
}

impl Default for ArxivConfig {
    fn default() -> Self {
        ArxivConfig {
            categories: vec![],
            keywords: vec![],
            auto_fetch_enabled: false,
            interval_days: 1,
            fetch_time: "09:00".to_string(),
            days_back: 5,
            max_fetch: 100,
            ai_analysis_enabled: false,
            ai_analysis_prompt: DEFAULT_ARXIV_ANALYSIS_PROMPT.to_string(),
            ai_filter_enabled: true,
            ai_filter_threshold: 6.0,
            ai_provider_id: None,
            ai_model_id: None,
            last_fetch_date: None,
            ai_analysis_concurrency: 5,
            fetch_biorxiv: false,
            fetch_arxiv: true,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArxivPaper {
    pub arxiv_id: String,
    pub title: String,
    pub authors: Vec<String>,
    pub summary: String,
    pub categories: Vec<String>,
    pub published: String,
    pub updated: String,
    pub pdf_url: String,
    pub abs_url: String,
    pub relevance_score: Option<f32>,
    pub relevance_reason: Option<String>,
    #[serde(default)]
    pub key_contributions: Vec<String>,
    #[serde(default)]
    pub analysis_summary: Option<String>,
    #[serde(default)]
    pub matched_topics: Vec<String>,
    #[serde(default = "default_analysis_status")]
    pub analysis_status: String,
    #[serde(default)]
    pub in_library: bool,
    pub fetched_at: String,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub rating: u8,
    /// "biorxiv" for bioRxiv papers; None / missing = arXiv.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

fn default_analysis_status() -> String {
    "pending".to_string()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ArxivScheduleStatus {
    pub auto_fetch_enabled: bool,
    pub last_fetch_date: Option<String>,
    pub next_scheduled: Option<String>,
    pub fetching: bool,
    pub analyzing: bool,
    pub analyzed_count: u32,
    pub total_pending: u32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ArxivInbox {
    #[serde(default)]
    pub papers: Vec<ArxivPaper>,
    pub last_updated: String,
}

// ── M9: Canvas ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Viewport {
    pub offset_x: f64,
    pub offset_y: f64,
    pub zoom: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Viewport {
            offset_x: 0.0,
            offset_y: 0.0,
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasNode {
    pub node_id: String,
    pub paper_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Node-level override for hover content source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hover_source: Option<String>,
    /// "paper" (default/None), "text", or "shape"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_type: Option<String>,
    /// Text content for text/shape nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Font size in px for text nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// Font bold for text nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_bold: Option<bool>,
    /// Font italic for text nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_italic: Option<bool>,
    /// Width for shape nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    /// Height for shape nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// Shape kind for shape nodes: "rect" (default) | "ellipse" | "diamond".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape_kind: Option<String>,
    /// Fill color for shape nodes (separate from `color`, which is the stroke).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<String>,
    /// Stroke/border width in px for shape nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    /// Rotation in degrees (text/shape nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    /// Opacity 0..1 (text/shape nodes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// Corner radius in px for shape nodes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corner_radius: Option<f64>,
    /// Font family for text nodes (CSS font-family value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    /// Text alignment for text nodes: "left" | "center" | "right".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_align: Option<String>,
    /// Line node kind: "line" | "arrow".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_kind: Option<String>,
    /// Line node endpoints as offsets within the node's bounding box.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub line_points: Vec<CanvasEdgePoint>,
    /// Stacking order (z-index) for layering (置顶/置底).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_index: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasEdgePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasEdge {
    pub edge_id: String,
    /// Upstream node (arrow origin).
    pub from_node_id: String,
    /// Downstream node (arrow tip).
    pub to_node_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke_width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control_y: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_points: Vec<CanvasEdgePoint>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Canvas {
    pub id: String,
    pub name: String,
    pub nodes: Vec<CanvasNode>,
    pub edges: Vec<CanvasEdge>,
    #[serde(default)]
    pub viewport: Viewport,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasIndexEntry {
    pub id: String,
    pub name: String,
    pub node_count: u32,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CanvasSettings {
    /// Global default hover content source:
    /// "notes" | "summary" | "abstract" | "none"
    #[serde(default = "default_hover_content_source")]
    pub hover_content_source: String,
}

fn default_hover_content_source() -> String {
    "notes".to_string()
}

impl Default for CanvasSettings {
    fn default() -> Self {
        CanvasSettings {
            hover_content_source: default_hover_content_source(),
        }
    }
}

// ── Snippet Library ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SnippetLibrary {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Snippet {
    pub id: String,
    pub library_id: String,
    pub text: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub note: String,
    pub paper_id: String,
    pub paper_title: String,
    pub page: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub created_at: String,
}

// ── M10: Canvas Enhance ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SuggestedEdge {
    pub from_paper_id: String,
    pub to_paper_id: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct NodePosition {
    pub node_id: String,
    pub x: f64,
    pub y: f64,
}
