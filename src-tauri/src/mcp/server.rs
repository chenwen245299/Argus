//! The MCP tool surface exposed to external agents.
//!
//! This layer is deliberately thin: it declares tool names, argument schemas and
//! descriptions, then defers to `super::tools` for every read. The security
//! argument for the whole feature lives in that module's docs — keep new tools
//! going through it rather than touching the filesystem here.
//!
//! Blocking file and SQLite I/O is wrapped in `spawn_blocking`, matching the
//! rule in AGENTS.md: the Tauri async runtime also carries in-flight AI
//! streaming, and parking one of its workers on a 5000-paper scan would stall
//! the UI.

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::{Implementation, JsonObject, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData, ServerHandler, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

use super::tools;

/// How the agent is told to use this server. Two things are worth spending
/// tokens on up front, because getting them wrong is expensive: full text is
/// paged (not dumped), and PDFs are opened by path (not returned as bytes).
const INSTRUCTIONS: &str = "\
Read-only access to the user's Argus literature library: papers, extracted full \
text, notes, highlights, collections, relationship canvases and snippet libraries.

Every paper is identified by its `slug`. Start with `list_papers` or \
`search_papers` to find one, then use `get_paper` for its metadata and an \
inventory of what else is available for it.

Reading a paper:
- `get_paper_fulltext` returns the extracted text in slices. It is paged on \
purpose — a whole paper will not fit comfortably in context. Call \
`get_paper_sections` first and request the section you need, or page with \
`offset`.
- `get_paper_pdf_path` returns an absolute path to the PDF. Open it with your \
own file-reading tool when you need figures, tables or layout; the extracted \
text is faster for everything else.

`list_conversations` and `get_conversation` reach the user's own AI \
conversations about this library — useful for picking up a line of thought they \
already started. Both are paged.

This server is read-only and exposes literature content only. It cannot read \
the user's AI provider settings or API keys.";

fn bad_request(e: String) -> ErrorData {
    ErrorData::invalid_params(e, None)
}

fn internal(e: String) -> ErrorData {
    ErrorData::internal_error(e, None)
}

/// Run a blocking library read on the blocking pool.
async fn blocking<T, F>(f: F) -> Result<T, ErrorData>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| internal(format!("task panicked: {e}")))?
        .map_err(bad_request)
}

// ── Argument schemas ─────────────────────────────────────────────────────────

fn default_limit() -> usize {
    50
}
fn default_search_limit() -> usize {
    20
}
fn default_fulltext_limit() -> usize {
    tools::DEFAULT_FULLTEXT_CHARS
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct ListPapersParams {
    /// Case-insensitive substring match against title, authors, tags and venue.
    /// For searching inside paper *content*, use `search_papers` instead.
    pub query: Option<String>,
    /// Exact tag name.
    pub tag: Option<String>,
    /// Collection id from `list_collections`.
    pub collection_id: Option<String>,
    /// One of "unread", "reading", "read".
    pub reading_status: Option<String>,
    pub year_from: Option<u32>,
    pub year_to: Option<u32>,
    /// Case-insensitive substring match on the journal or conference name.
    pub venue: Option<String>,
    /// Keep only papers with at least this many citations.
    pub min_citations: Option<u32>,
    /// true = only papers whose text has been extracted (searchable and
    /// readable); false = only those still missing it.
    pub has_fulltext: Option<bool>,
    /// "added" (default) | "year" | "citations" | "title".
    pub sort_by: Option<String>,
    /// "desc" (default) | "asc".
    pub order: Option<String>,
    /// How much of each abstract to include: "preview" (default, first 400
    /// characters), "full" (the whole abstract — use it when you intend to
    /// reason over the abstracts rather than just pick papers from them), or
    /// "none".
    pub abstract_detail: Option<String>,
    /// Max papers to return. Default 50.
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Papers to skip, for paging through a large result set.
    #[serde(default)]
    pub offset: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SlugParams {
    /// Paper identifier from `list_papers` or `search_papers`.
    pub slug: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SearchParams {
    /// Full-text query. Supports SQLite FTS5 syntax, so `"neural network"` is a
    /// phrase and `attention AND transformer` is a conjunction.
    pub query: String,
    /// "preview" (default) | "full" | "none". See `list_papers`.
    pub abstract_detail: Option<String>,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct FulltextParams {
    pub slug: String,
    /// Character offset to start at. Use the previous call's `offset + returned`
    /// to continue.
    #[serde(default)]
    pub offset: usize,
    /// Characters to return. Default 8000, capped at 40000.
    #[serde(default = "default_fulltext_limit")]
    pub limit: usize,
    /// Restrict to one section, by its heading from `get_paper_sections`.
    /// Offsets are then relative to that section.
    pub section: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct NoteParams {
    pub slug: String,
    /// Note id from `list_notes`. Omit for the most recently edited note.
    pub note_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct CanvasParams {
    /// Canvas id from `list_canvases`.
    pub canvas_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct SnippetParams {
    /// Restrict to one snippet library. Omit to search all of them.
    pub library_id: Option<String>,
    /// Case-insensitive substring match against the excerpt, its note, its tags
    /// and the source paper's title.
    pub query: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema, Default)]
pub struct NoParams {}

fn default_conversation_scope() -> String {
    "all".to_string()
}
fn default_conversation_limit() -> usize {
    30
}
fn default_message_limit() -> usize {
    20
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListConversationsParams {
    /// "library" for library-wide Q&A, "paper" for per-paper chats, or "all"
    /// (the default).
    #[serde(default = "default_conversation_scope")]
    pub scope: String,
    /// Restrict to one paper's conversations. Implies scope "paper".
    pub slug: Option<String>,
    /// Case-insensitive substring match against conversation titles and message
    /// text. Matching conversations come back with an excerpt.
    pub query: Option<String>,
    #[serde(default = "default_conversation_limit")]
    pub limit: usize,
}

impl Default for ListConversationsParams {
    fn default() -> Self {
        Self {
            scope: default_conversation_scope(),
            slug: None,
            query: None,
            limit: default_conversation_limit(),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetConversationParams {
    /// Id from `list_conversations`.
    pub conversation_id: String,
    /// The `slug` that `list_conversations` reported for this conversation.
    /// Omitting it for a paper conversation still works but is much slower.
    pub slug: Option<String>,
    /// Messages to skip, for paging through a long conversation.
    #[serde(default)]
    pub offset: usize,
    /// Messages to return. Default 20.
    #[serde(default = "default_message_limit")]
    pub limit: usize,
}

impl Default for GetConversationParams {
    fn default() -> Self {
        Self {
            conversation_id: String::new(),
            slug: None,
            offset: 0,
            limit: default_message_limit(),
        }
    }
}

// ── Schema flattening ────────────────────────────────────────────────────────
//
// `schemars` factors every nested struct out into `$defs` and points at it with
// `$ref`. That is valid JSON Schema, but MCP clients vary in whether they
// resolve references, and one that does not will drop the tool — or, worse, the
// whole `tools/list` — without saying why. Since every schema here is small and
// non-recursive, inlining the definitions costs little and removes the question
// entirely.
//
// rmcp builds schemas through `SchemaSettings::draft2020_12()` with no
// `inline_subschemas`, and the `#[tool]` macro gives no hook to change that, so
// the flattening happens after the router is built.

/// Depth ceiling for inlining. Nothing here is recursive; this only stops a
/// future self-referential type from expanding forever.
const MAX_INLINE_DEPTH: usize = 12;

/// Replace every `$ref` with the definition it points at and drop `$defs`,
/// yielding a self-contained schema.
fn inline_defs(schema: &JsonObject) -> JsonObject {
    let Some(defs) = schema.get("$defs").and_then(|d| d.as_object()).cloned() else {
        return schema.clone();
    };

    let mut root = schema.clone();
    root.remove("$defs");
    let mut value = Value::Object(root);
    resolve_refs(&mut value, &defs, 0);
    match value {
        Value::Object(o) => o,
        // resolve_refs only ever rewrites nested nodes, so the root stays an
        // object; fall back rather than panic if that ever stops holding.
        _ => schema.clone(),
    }
}

fn resolve_refs(value: &mut Value, defs: &serde_json::Map<String, Value>, depth: usize) {
    if depth > MAX_INLINE_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            let target = map
                .get("$ref")
                .and_then(|r| r.as_str())
                .and_then(|r| r.strip_prefix("#/$defs/"))
                .and_then(|name| defs.get(name))
                .cloned();

            if let Some(mut replacement) = target {
                resolve_refs(&mut replacement, defs, depth + 1);
                // A `$ref` can sit beside annotations such as `description`;
                // JSON Schema 2020-12 keeps those, so carry them over rather
                // than losing the field docs the agent reads.
                if let Value::Object(rep) = &mut replacement {
                    for (k, v) in map.iter() {
                        if k != "$ref" && !rep.contains_key(k) {
                            rep.insert(k.clone(), v.clone());
                        }
                    }
                }
                *value = replacement;
                return;
            }

            for child in map.values_mut() {
                resolve_refs(child, defs, depth + 1);
            }
        }
        Value::Array(items) => {
            for child in items {
                resolve_refs(child, defs, depth + 1);
            }
        }
        _ => {}
    }
}

/// The tool declarations, flattened, for callers that need them without a
/// running MCP service — currently the in-app agent (`super::agent`).
pub fn tool_declarations() -> Vec<rmcp::model::Tool> {
    flattened_tool_router().list_all()
}

/// Build the tool router, then flatten every schema it carries.
fn flattened_tool_router() -> ToolRouter<ArgusMcpServer> {
    let mut router = ArgusMcpServer::generated_tool_router();
    for route in router.map.values_mut() {
        route.attr.input_schema = Arc::new(inline_defs(&route.attr.input_schema));
        if let Some(output) = &route.attr.output_schema {
            route.attr.output_schema = Some(Arc::new(inline_defs(output)));
        }
    }
    router
}

// ── Server ───────────────────────────────────────────────────────────────────

/// One instance is constructed per incoming MCP session.
///
/// It resolves the library through a `LibrarySource` rather than holding a path,
/// so a library switch in the app while an agent is connected is picked up by
/// the next tool call instead of serving a stale root.
#[derive(Clone)]
pub struct ArgusMcpServer {
    library: std::sync::Arc<dyn super::LibrarySource>,
}

impl ArgusMcpServer {
    pub fn new(library: std::sync::Arc<dyn super::LibrarySource>) -> Self {
        // No router field: `#[tool_handler]` resolves tools through
        // `Self::tool_router()` below.
        Self { library }
    }

    /// What `#[tool_handler]` calls. The `#[tool_router]` macro emits
    /// `generated_tool_router`; this wraps it so every schema that leaves the
    /// server is self-contained.
    fn tool_router() -> ToolRouter<Self> {
        flattened_tool_router()
    }

    /// Resolve the library the app currently has open.
    ///
    /// Re-resolved per call, so both a library switch and the MCP switch being
    /// turned off take effect on the next tool call rather than the next
    /// connection.
    fn root(&self) -> Result<String, ErrorData> {
        self.library.root().map_err(bad_request)
    }
}

#[tool_router(router = generated_tool_router)]
impl ArgusMcpServer {
    /// List papers in the library, newest first. Supports filtering by title or
    /// author text, tag, collection, reading status and year range.
    #[tool(
        name = "list_papers",
        description = "List papers in the user's literature library, newest first. Filter by title/author text, tag, collection, reading status, year range, venue, minimum citations, or whether full text is available. Sort by `added` (default), `year`, `citations` or `title`, in either direction. Each row carries venue, DOI, arXiv id, citation count and CCF/SCI ranking. The response also carries `total` (matches before paging), so limit=1 turns this into a cheap counter for any filter.",
        annotations(title = "List papers", read_only_hint = true)
    )]
    async fn list_papers(
        &self,
        Parameters(p): Parameters<ListPapersParams>,
    ) -> Result<Json<tools::PaperListPage>, ErrorData> {
        let root = self.root()?;
        blocking(move || {
            tools::list_papers(
                &root,
                tools::ListPapersArgs {
                    query: p.query.as_deref(),
                    tag: p.tag.as_deref(),
                    collection_id: p.collection_id.as_deref(),
                    reading_status: p.reading_status.as_deref(),
                    year_from: p.year_from,
                    year_to: p.year_to,
                    venue: p.venue.as_deref(),
                    min_citations: p.min_citations,
                    has_fulltext: p.has_fulltext,
                    abstract_detail: p.abstract_detail.as_deref(),
                    sort_by: p.sort_by.as_deref(),
                    order: p.order.as_deref(),
                    limit: p.limit.clamp(1, 500),
                    offset: p.offset,
                },
            )
        })
        .await
        .map(|v| Json(v.into()))
    }

    /// Full metadata for one paper plus an inventory of what else is available.
    #[tool(
        name = "get_paper",
        description = "Get one paper's metadata (title, authors, venue, DOI, abstract, tags, BibTeX) plus an inventory of what else is available for it: how much full text was extracted, its notes, its highlight count, and the papers the user linked to it.",
        annotations(title = "Get paper", read_only_hint = true)
    )]
    async fn get_paper(
        &self,
        Parameters(p): Parameters<SlugParams>,
    ) -> Result<Json<tools::PaperDetail>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::get_paper(&root, &p.slug))
            .await
            .map(Json)
    }

    /// Full-text search across the library.
    #[tool(
        name = "search_papers",
        description = "Search the full text of every paper in the library and return matching excerpts. This searches paper *content*; to filter by title or author metadata use list_papers.",
        annotations(title = "Search papers", read_only_hint = true)
    )]
    async fn search_papers(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<Json<tools::ItemList<tools::SearchResult>>, ErrorData> {
        let root = self.root()?;
        blocking(move || {
            tools::search_papers(
                &root,
                &p.query,
                p.limit.clamp(1, 100),
                tools::AbstractDetail::parse(p.abstract_detail.as_deref()),
            )
        })
            .await
            .map(|v| Json(v.into()))
    }

    /// A slice of a paper's extracted text.
    #[tool(
        name = "get_paper_fulltext",
        description = "Read the extracted text of a paper, in slices. Papers are far too long to return whole, so this pages: use `section` to jump to one part (see get_paper_sections), or `offset` to continue where the last call stopped. Check `has_more` in the response.",
        annotations(title = "Read paper text", read_only_hint = true)
    )]
    async fn get_paper_fulltext(
        &self,
        Parameters(p): Parameters<FulltextParams>,
    ) -> Result<Json<tools::FulltextSlice>, ErrorData> {
        let root = self.root()?;
        blocking(move || {
            tools::get_paper_fulltext(&root, &p.slug, p.offset, p.limit, p.section.as_deref())
        })
        .await
        .map(|v| Json(v.into()))
    }

    /// The paper's section outline.
    #[tool(
        name = "get_paper_sections",
        description = "Get a paper's section outline (headings, nesting level, page number). Use this to decide which part of a long paper to read, then pass a heading to get_paper_fulltext.",
        annotations(title = "Get paper outline", read_only_hint = true)
    )]
    async fn get_paper_sections(
        &self,
        Parameters(p): Parameters<SlugParams>,
    ) -> Result<Json<tools::PaperSectionList>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::get_paper_sections(&root, &p.slug))
            .await
            .map(Json)
    }

    /// Where the paper's PDF lives on disk.
    #[tool(
        name = "get_paper_pdf_path",
        description = "Get the absolute path to a paper's PDF or ebook file, so you can open it with your own file-reading tool. Use this when you need figures, tables or page layout; for plain reading, get_paper_fulltext is cheaper.",
        annotations(title = "Locate paper file", read_only_hint = true)
    )]
    async fn get_paper_pdf_path(
        &self,
        Parameters(p): Parameters<SlugParams>,
    ) -> Result<Json<tools::DocumentLocation>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::get_document_path(&root, &p.slug))
            .await
            .map(Json)
    }

    /// The paper's notes, by title and modification time.
    #[tool(
        name = "list_notes",
        description = "List the notes the user has written on a paper, with their ids and last-modified times.",
        annotations(title = "List notes", read_only_hint = true)
    )]
    async fn list_notes(
        &self,
        Parameters(p): Parameters<SlugParams>,
    ) -> Result<Json<tools::ItemList<tools::NoteSummary>>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::list_notes(&root, &p.slug))
            .await
            .map(|v| Json(v.into()))
    }

    /// One note's markdown source.
    #[tool(
        name = "get_note",
        description = "Read the markdown content of one of the user's notes on a paper. Omit note_id to get the most recently edited note.",
        annotations(title = "Read note", read_only_hint = true)
    )]
    async fn get_note(
        &self,
        Parameters(p): Parameters<NoteParams>,
    ) -> Result<Json<tools::NoteContent>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::get_note(&root, &p.slug, p.note_id.as_deref()))
            .await
            .map(Json)
    }

    /// Passages the user marked while reading.
    #[tool(
        name = "get_highlights",
        description = "Get the passages the user highlighted in a paper, with page numbers and any annotation they attached. These are the parts of the paper the user personally cared about.",
        annotations(title = "Get highlights", read_only_hint = true)
    )]
    async fn get_highlights(
        &self,
        Parameters(p): Parameters<SlugParams>,
    ) -> Result<Json<tools::ItemList<tools::HighlightEntry>>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::get_highlights(&root, &p.slug))
            .await
            .map(|v| Json(v.into()))
    }

    /// One-call overview of the whole library.
    #[tool(
        name = "get_library_stats",
        description = "Get an overview of the library in one call: how many papers, their reading status, publication years, file types, most-used tags, how many have extracted full text, and how many collections/canvases/snippets exist. Call this first to understand the shape of a library before probing it with filtered listings.",
        annotations(title = "Library statistics", read_only_hint = true)
    )]
    async fn get_library_stats(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<Json<tools::LibraryStats>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::library_stats(&root)).await.map(Json)
    }

    /// The user's collection tree.
    #[tool(
        name = "list_collections",
        description = "List the user's collections — their folder tree for organizing papers — with each one's readable path, nesting depth, direct paper count, and total count including sub-collections. Pass a collection id to list_papers to see the papers themselves.",
        annotations(title = "List collections", read_only_hint = true)
    )]
    async fn list_collections(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<Json<tools::ItemList<tools::CollectionEntry>>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::list_collections(&root))
            .await
            .map(|v| Json(v.into()))
    }

    /// Available relationship canvases.
    #[tool(
        name = "list_canvases",
        description = "List the user's relationship canvases — hand-built maps of how papers relate to each other.",
        annotations(title = "List canvases", read_only_hint = true)
    )]
    async fn list_canvases(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<Json<tools::ItemList<tools::CanvasSummary>>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::list_canvases(&root))
            .await
            .map(|v| Json(v.into()))
    }

    /// One canvas as a graph.
    #[tool(
        name = "get_canvas",
        description = "Get one relationship canvas as a graph: nodes (papers, resolved to slugs and titles, plus the user's free-text annotation nodes) and the labelled edges between them. The edge labels are the user's own words for how the papers relate.",
        annotations(title = "Get canvas", read_only_hint = true)
    )]
    async fn get_canvas(
        &self,
        Parameters(p): Parameters<CanvasParams>,
    ) -> Result<Json<tools::CanvasDetail>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::get_canvas(&root, &p.canvas_id))
            .await
            .map(Json)
    }

    /// Snippet libraries.
    #[tool(
        name = "list_snippet_libraries",
        description = "List the user's snippet libraries — collections of excerpts they saved while reading, typically for citing later.",
        annotations(title = "List snippet libraries", read_only_hint = true)
    )]
    async fn list_snippet_libraries(
        &self,
        Parameters(_): Parameters<NoParams>,
    ) -> Result<Json<tools::ItemList<tools::SnippetLibraryEntry>>, ErrorData> {
        let root = self.root()?;
        blocking(move || tools::list_snippet_libraries(&root))
            .await
            .map(|v| Json(v.into()))
    }

    /// The user's own conversations with AI about their literature.
    #[tool(
        name = "list_conversations",
        description = "List the user's AI conversations — both library-wide Q&A and per-paper chats — newest first. Pass `query` to find conversations mentioning something; matches come back with an excerpt. Returns ids and slugs for get_conversation.",
        annotations(title = "List AI conversations", read_only_hint = true)
    )]
    async fn list_conversations(
        &self,
        Parameters(p): Parameters<ListConversationsParams>,
    ) -> Result<Json<tools::ItemList<tools::ConversationSummary>>, ErrorData> {
        let root = self.root()?;
        blocking(move || {
            tools::list_conversations(
                &root,
                &p.scope,
                p.slug.as_deref(),
                p.query.as_deref(),
                p.limit.clamp(1, 200),
            )
        })
        .await
        .map(|v| Json(v.into()))
    }

    /// One conversation's messages.
    #[tool(
        name = "get_conversation",
        description = "Read the messages of one AI conversation, paged. Shows what the user asked and what the model answered, including which model. Attachment payloads and provider/cost details are not included. Pass the `slug` from list_conversations for paper conversations — it makes the lookup much faster.",
        annotations(title = "Read AI conversation", read_only_hint = true)
    )]
    async fn get_conversation(
        &self,
        Parameters(p): Parameters<GetConversationParams>,
    ) -> Result<Json<tools::ConversationDetail>, ErrorData> {
        let root = self.root()?;
        blocking(move || {
            tools::get_conversation(
                &root,
                &p.conversation_id,
                p.slug.as_deref(),
                p.offset,
                p.limit.clamp(1, 100),
            )
        })
        .await
        .map(|v| Json(v.into()))
    }

    /// Saved excerpts, optionally filtered.
    #[tool(
        name = "search_snippets",
        description = "Search the excerpts the user saved to their snippet libraries, including the note and tags they attached and the source paper. Omit `query` to list them all.",
        annotations(title = "Search snippets", read_only_hint = true)
    )]
    async fn search_snippets(
        &self,
        Parameters(p): Parameters<SnippetParams>,
    ) -> Result<Json<tools::ItemList<tools::SnippetEntry>>, ErrorData> {
        let root = self.root()?;
        blocking(move || {
            tools::list_snippets(
                &root,
                p.library_id.as_deref(),
                p.query.as_deref(),
                p.limit.clamp(1, 500),
            )
        })
        .await
        .map(|v| Json(v.into()))
    }
}

#[tool_handler]
impl ServerHandler for ArgusMcpServer {
    fn get_info(&self) -> ServerInfo {
        // `ServerInfo` is #[non_exhaustive], so build from Default and assign.
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("argus", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(INSTRUCTIONS.to_string());
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every tool this server exposes. Kept as a literal so that adding a tool
    /// is a deliberate two-place edit rather than something that slips in.
    const EXPECTED_TOOLS: &[&str] = &[
        "get_canvas",
        "get_conversation",
        "get_highlights",
        "get_library_stats",
        "get_note",
        "get_paper",
        "get_paper_fulltext",
        "get_paper_pdf_path",
        "get_paper_sections",
        "list_canvases",
        "list_collections",
        "list_conversations",
        "list_notes",
        "list_papers",
        "list_snippet_libraries",
        "search_papers",
        "search_snippets",
    ];

    /// Exercises the `#[tool_router]` expansion without needing a Tauri runtime:
    /// the router is built by an associated function, so schema generation and
    /// name registration are all verifiable here.
    fn router() -> rmcp::handler::server::router::tool::ToolRouter<ArgusMcpServer> {
        ArgusMcpServer::tool_router()
    }

    /// MCP requires `outputSchema` to describe an object, because a tool's
    /// `structuredContent` is a JSON object. A tool returning a bare `Vec<T>`
    /// emits `"type": "array"` instead — and clients reject the **entire**
    /// `tools/list` response over it, so every tool on the server vanishes with
    /// no error shown to the user. This is the test that catches it.
    #[test]
    fn every_output_schema_describes_an_object() {
        for tool in router().list_all() {
            let Some(schema) = &tool.output_schema else { continue };
            assert_eq!(
                schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "tool '{}' has a non-object outputSchema — wrap the return value \
                 (e.g. in tools::ItemList) instead of returning a bare array",
                tool.name
            );
        }
    }

    /// Same rule for inputs: a tool's arguments are always an object.
    #[test]
    fn every_input_schema_describes_an_object() {
        for tool in router().list_all() {
            assert_eq!(
                tool.input_schema.get("type").and_then(|t| t.as_str()),
                Some("object"),
                "tool '{}' has a non-object inputSchema",
                tool.name
            );
        }
    }

    /// The reason this whole flattening step exists: a client that does not
    /// resolve `$ref` must still see complete schemas.
    #[test]
    fn no_schema_leaves_the_server_with_a_dangling_reference() {
        for tool in router().list_all() {
            for (kind, schema) in [
                ("inputSchema", Some(tool.input_schema.clone())),
                ("outputSchema", tool.output_schema.clone()),
            ] {
                let Some(schema) = schema else { continue };
                let rendered = serde_json::to_string(&schema).unwrap();
                assert!(
                    !rendered.contains("$ref"),
                    "{}.{kind} still has a $ref: {rendered}",
                    tool.name
                );
                assert!(
                    !rendered.contains("$defs"),
                    "{}.{kind} still carries $defs: {rendered}",
                    tool.name
                );
            }
        }
    }

    /// Flattening must preserve content, not just remove references. The nested
    /// `PaperSummary` fields have to survive inlining into `list_papers`.
    #[test]
    fn inlining_keeps_the_nested_fields() {
        let tool = router()
            .list_all()
            .into_iter()
            .find(|t| t.name == "list_papers")
            .expect("list_papers missing");
        let rendered = serde_json::to_string(tool.output_schema.as_ref().unwrap()).unwrap();
        for field in ["slug", "title", "authors", "reading_status", "total", "offset"] {
            assert!(
                rendered.contains(field),
                "inlining dropped '{field}' from list_papers output schema"
            );
        }
    }

    #[test]
    fn inlining_carries_over_annotations_next_to_a_ref() {
        let schema: JsonObject = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "item": { "$ref": "#/$defs/Thing", "description": "the important one" }
            },
            "$defs": { "Thing": { "type": "object", "properties": { "a": { "type": "string" } } } }
        }))
        .unwrap();

        let flat = inline_defs(&schema);
        let item = &flat["properties"]["item"];
        assert_eq!(item["type"], "object", "target schema was not inlined");
        assert_eq!(
            item["description"], "the important one",
            "the sibling description next to $ref was lost"
        );
        assert!(!serde_json::to_string(&flat).unwrap().contains("$defs"));
    }

    /// A self-referential type must not expand forever.
    #[test]
    fn a_cyclic_reference_terminates() {
        let schema: JsonObject = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": { "node": { "$ref": "#/$defs/Node" } },
            "$defs": {
                "Node": {
                    "type": "object",
                    "properties": { "child": { "$ref": "#/$defs/Node" } }
                }
            }
        }))
        .unwrap();

        // The assertion that matters is that this returns at all.
        let flat = inline_defs(&schema);
        assert_eq!(flat["type"], "object");
    }

    #[test]
    fn exposes_exactly_the_expected_tools() {
        let names: Vec<String> = router()
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(names, EXPECTED_TOOLS, "MCP tool surface changed");
    }

    /// The whole feature is read-only. A tool that forgets `read_only_hint`
    /// makes clients prompt for write consent they should never need — and
    /// signals that someone added a mutating tool without revisiting the
    /// security model in `tools.rs`.
    #[test]
    fn every_tool_declares_itself_read_only() {
        for tool in router().list_all() {
            let read_only = tool
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                .unwrap_or(false);
            assert!(read_only, "tool '{}' is not marked read_only", tool.name);
        }
    }

    /// Descriptions are what the agent reads to decide when to call a tool; an
    /// empty one turns the tool into dead weight.
    #[test]
    fn every_tool_has_a_description_and_object_schema() {
        for tool in router().list_all() {
            let desc = tool.description.as_deref().unwrap_or("");
            assert!(
                desc.len() > 30,
                "tool '{}' has a thin description: {desc:?}",
                tool.name
            );
            assert_eq!(
                tool.input_schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool '{}' has a non-object input schema",
                tool.name
            );
        }
    }

    /// `slug` is how every per-paper tool is addressed; a schema that dropped it
    /// would still compile but leave the agent unable to name a paper.
    #[test]
    fn per_paper_tools_require_a_slug() {
        let per_paper = [
            "get_paper",
            "get_paper_fulltext",
            "get_paper_sections",
            "get_paper_pdf_path",
            "list_notes",
            "get_note",
            "get_highlights",
        ];
        for tool in router().list_all() {
            if !per_paper.contains(&tool.name.as_ref()) {
                continue;
            }
            let props = tool
                .input_schema
                .get("properties")
                .and_then(|v| v.as_object())
                .unwrap_or_else(|| panic!("tool '{}' has no properties", tool.name));
            assert!(
                props.contains_key("slug"),
                "tool '{}' does not accept a slug",
                tool.name
            );
        }
    }

    /// The instructions are the first thing the agent reads. The two failure
    /// modes worth guarding are it dumping whole papers into context and it
    /// expecting PDF bytes back.
    #[test]
    fn instructions_cover_the_expensive_mistakes() {
        assert!(INSTRUCTIONS.contains("slug"));
        assert!(INSTRUCTIONS.contains("paged"));
        assert!(INSTRUCTIONS.contains("absolute path"));
        assert!(INSTRUCTIONS.contains("API keys"));
    }
}
