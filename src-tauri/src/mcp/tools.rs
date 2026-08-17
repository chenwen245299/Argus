//! Read-only data access for the MCP endpoint.
//!
//! # The security boundary lives here
//!
//! Everything an external agent can ever see is the return value of some
//! function in this file. That is a deliberate design choice, and it rests on
//! one rule:
//!
//! > **No function here accepts a filesystem path.**
//!
//! Callers pass a slug, a note id, a canvas id — never a path. Path
//! construction goes through `paper::paper_dir` and friends, which run every
//! segment past `path_guard`. There is consequently no reachable code path from
//! an MCP request to an arbitrary file, so the following stay unreachable *by
//! construction* rather than by filtering:
//!
//! | Never exposed | Why |
//! |---|---|
//! | `.argus/ai_providers.json` | AI provider endpoints and model config |
//! | `.argus/api_keys.json`, `.argus/.keymaster` | encrypted API keys + the key that decrypts them |
//! | `.argus/config.json` | carries AI / RAG / arXiv settings |
//! | `.argus/token_usage.jsonl` | spend history |
//!
//! Papers, notes, highlights, sections, collections, canvases, snippets and AI
//! conversations are exposed. AI *configuration* is not: the agent may read what
//! the user discussed, never the credentials or endpoints used to discuss it.
//!
//! Conversation records interleave content with things that fall on the wrong
//! side of that line, so `redact_answer` strips them field by field — provider
//! identity, per-call cost and token counts, and base64 attachment payloads.
//! See `list_conversations` for the full rationale.
//!
//! See `DENIED_ARTIFACTS` and its test for the enforced form of this table.

use serde::{Deserialize, Serialize};

use crate::models::{Highlight, Note, PaperMeta};
use crate::{canvas, collections, ebook, extraction, library, metadata, paper, search, sections, snippets};

/// Library artifacts that must never be reachable through an MCP tool.
///
/// This list is not consulted at runtime — nothing here takes a path, so there
/// is nothing to filter. It exists so the guarantee is written down next to a
/// test that fails if a future tool starts reading one of these files. Hence
/// `dead_code`: being unreferenced outside the test *is* the invariant.
#[allow(dead_code)]
pub const DENIED_ARTIFACTS: &[&str] = &[
    "ai_providers.json",
    "api_keys.json",
    ".keymaster",
    "config.json",
    "token_usage.jsonl",
];

/// Fields inside a conversation record that are dropped before it leaves this
/// module. They are colocated with conversation content but are not content:
/// provider identity is configuration, cost and token counts are the spend
/// history that `token_usage.jsonl` is denied for, and `dataUrl` is a base64
/// blob that would flood the caller's context.
#[allow(dead_code)]
pub const REDACTED_CONVERSATION_FIELDS: &[&str] = &[
    "providerId",
    "providerName",
    "costUsd",
    "inputTokens",
    "outputTokens",
    "totalTokens",
    "cacheHitTokens",
    "dataUrl",
];

/// Default slice size for `get_paper_fulltext`, in characters.
///
/// A full paper runs 30k–120k characters. Returning one whole would blow a
/// meaningful hole in the agent's context window on a single tool call, so the
/// text is served in slices and the agent pages through what it needs.
pub const DEFAULT_FULLTEXT_CHARS: usize = 8_000;
/// Hard ceiling on a single fulltext slice, so a large `limit` cannot be used to
/// pull an entire paper in one call.
pub const MAX_FULLTEXT_CHARS: usize = 40_000;

/// Per-message ceiling when returning a conversation.
///
/// Users paste whole papers into chat, and the largest conversation file in a
/// real library runs to several megabytes. Without a per-message cap, one
/// `get_conversation` call could return more text than the caller's entire
/// context window.
pub const MAX_MESSAGE_CHARS: usize = 6_000;
/// Preview length for a conversation in a listing.
const PREVIEW_CHARS: usize = 160;

/// How much of an abstract travels with a paper by default.
///
/// A default, not a ceiling: callers pass `abstract_detail: "full"` when they
/// want the whole thing. Enough to triage a listing without making every
/// `list_papers` carry a hundred kilobytes nobody asked for.
const SUMMARY_ABSTRACT_CHARS: usize = 400;

/// How much of an abstract a caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbstractDetail {
    /// Omit it — for callers that only need identifiers.
    None,
    /// Opening lines, enough to tell what a paper is for.
    Preview,
    /// The whole abstract.
    Full,
}

impl AbstractDetail {
    /// Anything unrecognised is a preview: a typo in this argument should not
    /// silently strip the abstracts a caller was relying on.
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::trim) {
            Some("full") => AbstractDetail::Full,
            Some("none") => AbstractDetail::None,
            _ => AbstractDetail::Preview,
        }
    }
}

// ── Output shapes ────────────────────────────────────────────────────────────

/// Wrapper for tools that return a list.
///
/// MCP requires `outputSchema` to describe an **object**, because a tool's
/// `structuredContent` is a JSON object. Returning `Vec<T>` directly produces
/// `"type": "array"`, which clients reject — and they reject the entire
/// `tools/list` response, not just the offending tool, so one bare array makes
/// every tool on the server disappear with no error shown to the user.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ItemList<T> {
    pub items: Vec<T>,
    /// How many items came back. Not the total that exist — a `limit` may have
    /// truncated this.
    pub count: usize,
}

impl<T> From<Vec<T>> for ItemList<T> {
    fn from(items: Vec<T>) -> Self {
        ItemList {
            count: items.len(),
            items,
        }
    }
}

/// Journal / conference ranking, when the user has looked it up.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VenueRank {
    /// Official datasets keyed by their code, e.g. {"ccf": "A", "sci": "Q1"}.
    pub official: std::collections::BTreeMap<String, String>,
    /// User-added datasets, e.g. [["CCF", "B"]] rendered as name → rank.
    pub custom: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaperSummary {
    /// Stable identifier used by every other tool in this server.
    pub slug: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u32>,
    /// Journal or conference.
    pub venue: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    /// Citation count, when the user recorded one.
    pub cite_count: Option<u32>,
    /// CCF / SCI / 中科院 rankings for `venue`, when looked up.
    pub venue_rank: Option<VenueRank>,
    pub tags: Vec<String>,
    /// "unread" | "reading" | "read"
    pub reading_status: String,
    /// "pdf" | "epub" | "mobi" | "azw3" | "fb2" | "txt"
    pub file_type: String,
    /// RFC3339 timestamp of when the paper entered the library.
    pub added_at: String,
    pub has_bibtex: bool,
    /// How many papers the user linked this one to by hand.
    pub related_count: usize,
    /// The abstract, at whatever detail `abstract_detail` asked for. Ends with
    /// `…` when it was cut short.
    pub paper_abstract: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaperListPage {
    /// Number of papers matching the filters, before pagination.
    pub total: usize,
    pub offset: usize,
    pub papers: Vec<PaperSummary>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RelatedPaper {
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoteSummary {
    pub note_id: String,
    pub title: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaperDetail {
    pub slug: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u32>,
    pub venue: Option<String>,
    pub doi: Option<String>,
    pub arxiv_id: Option<String>,
    pub tags: Vec<String>,
    pub reading_status: String,
    pub abstract_text: Option<String>,
    pub bibtex: Option<String>,
    /// Citation count, when the user recorded one.
    pub cite_count: Option<u32>,
    /// CCF / SCI / 中科院 rankings for `venue`, when looked up.
    pub venue_rank: Option<VenueRank>,
    pub added_at: String,
    pub file_type: String,
    /// Papers the user linked to this one by hand.
    pub related: Vec<RelatedPaper>,
    pub notes: Vec<NoteSummary>,
    pub highlight_count: usize,
    /// Characters of extracted full text available via `get_paper_fulltext`.
    /// Zero means extraction has not run (or produced nothing) for this paper.
    pub fulltext_chars: usize,
    pub has_sections: bool,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SearchResult {
    pub slug: String,
    pub title: String,
    pub authors: Vec<String>,
    pub year: Option<u32>,
    /// Journal or conference.
    pub venue: Option<String>,
    /// Citation count, when the user recorded one.
    pub cite_count: Option<u32>,
    /// Matching excerpt with the query terms in context. Says *where* the query
    /// hit; `abstract_preview` says what the paper is about.
    pub snippet: String,
    /// The abstract, at whatever detail `abstract_detail` asked for. Ends with
    /// `…` when it was cut short.
    pub paper_abstract: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FulltextSlice {
    pub slug: String,
    pub text: String,
    /// Character offset this slice starts at.
    pub offset: usize,
    /// Characters returned in `text`.
    pub returned: usize,
    /// Total characters of full text for this paper.
    pub total: usize,
    /// True when more text follows — call again with `offset = offset + returned`.
    pub has_more: bool,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SectionEntry {
    pub title: String,
    /// 1 = section, 2 = subsection, 3 = sub-subsection.
    pub level: u8,
    /// 1-based page number; 0 when unknown.
    pub page: u32,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaperSectionList {
    pub slug: String,
    /// How the outline was derived: "outline" | "heuristic" | "ai".
    pub source: String,
    pub sections: Vec<SectionEntry>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocumentLocation {
    pub slug: String,
    /// Absolute path to the PDF or ebook file on this machine.
    pub path: String,
    pub file_type: String,
    pub exists: bool,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NoteContent {
    pub slug: String,
    pub note_id: String,
    pub title: String,
    /// Markdown source.
    pub content: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct HighlightEntry {
    pub id: String,
    /// PDF: 1-based page. Ebooks: 1-based chapter index.
    pub page: u32,
    pub text: String,
    /// The user's annotation on this highlight; empty when they only marked it.
    pub note: String,
    pub color: String,
    /// "highlight" | "underline" | …
    pub style: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CollectionEntry {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    /// Papers assigned directly to this collection, not counting sub-collections.
    pub paper_count: usize,
    /// Papers in this collection or any collection beneath it, deduplicated —
    /// a paper filed in two sub-collections is counted once.
    pub total_paper_count: usize,
    /// 0 for a top-level collection, 1 for its children, and so on.
    pub depth: usize,
    /// Position in the tree, e.g. "AI / Systematicity / Milestones". Cheaper for
    /// an agent to reason about than following `parent_id` links.
    pub path: String,
}

/// One value and how many papers carry it.
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CountedValue {
    pub value: String,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct YearCount {
    pub year: u32,
    pub count: usize,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LibraryStats {
    pub total_papers: usize,

    /// Papers the user has not started.
    pub unread: usize,
    pub reading: usize,
    pub read: usize,

    /// Papers whose text has been extracted — i.e. `get_paper_fulltext` and
    /// `search_papers` can reach them.
    pub with_fulltext: usize,
    /// Papers with an AI-generated analysis.
    pub with_ai_summary: usize,
    /// Papers embedded into the vector store.
    pub vectorized: usize,
    pub with_bibtex: usize,
    /// Papers the user linked to at least one other paper by hand.
    pub with_related_links: usize,

    /// "pdf", "epub", … and how many of each.
    pub file_types: Vec<CountedValue>,

    pub earliest_year: Option<u32>,
    pub latest_year: Option<u32>,
    /// Papers with no publication year recorded.
    pub without_year: usize,
    /// Publication years present, most recent first.
    pub by_year: Vec<YearCount>,

    pub distinct_tags: usize,
    /// Most-used tags first, capped at `MAX_TOP_TAGS`. `distinct_tags` says how
    /// many exist in total, so truncation is visible rather than silent.
    pub top_tags: Vec<CountedValue>,

    /// Papers added in the last 30 days — what the user has been working on.
    pub added_last_30_days: usize,

    pub collections: usize,
    pub canvases: usize,
    pub snippet_libraries: usize,
    pub snippets: usize,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasSummary {
    pub id: String,
    pub name: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasNodeEntry {
    pub node_id: String,
    /// Present for paper nodes; absent for free-standing text/group nodes.
    pub slug: Option<String>,
    pub label: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasEdgeEntry {
    pub source_node_id: String,
    pub target_node_id: String,
    /// The user's word for the relationship ("extends", "contradicts", …).
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CanvasDetail {
    pub id: String,
    pub name: String,
    pub nodes: Vec<CanvasNodeEntry>,
    pub edges: Vec<CanvasEdgeEntry>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnippetLibraryEntry {
    pub id: String,
    pub name: String,
    pub snippet_count: usize,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnippetEntry {
    pub id: String,
    pub library_id: String,
    pub text: String,
    pub note: String,
    pub tags: Vec<String>,
    /// Title of the paper the excerpt came from.
    pub paper_title: String,
    pub page: u32,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConversationSummary {
    pub conversation_id: String,
    /// "library" for library-wide Q&A, "paper" for a conversation about one paper.
    pub scope: String,
    /// The paper this conversation is about; absent for library-wide ones.
    pub slug: Option<String>,
    pub title: String,
    pub message_count: usize,
    pub created_at: String,
    pub updated_at: String,
    /// Opening of the first user message, for recognising the conversation.
    pub preview: String,
    /// Excerpt around the query match. Only set when `query` was given.
    pub match_snippet: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConversationMessage {
    /// "user" or "assistant".
    pub role: String,
    pub content: String,
    /// Which model wrote an assistant message, when recorded.
    pub model: Option<String>,
    pub created_at: String,
    /// Names of files or images the user attached. The payloads themselves are
    /// never returned.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    /// True when the message was cut at the per-message ceiling.
    pub truncated: bool,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ConversationDetail {
    pub conversation_id: String,
    pub scope: String,
    pub slug: Option<String>,
    pub title: String,
    /// Messages in the requested window.
    pub messages: Vec<ConversationMessage>,
    pub offset: usize,
    /// Total messages in the conversation, before windowing.
    pub total: usize,
    pub has_more: bool,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// A lowercased copy of a string that can map matches back to the original.
///
/// `haystack.to_lowercase().find(needle)` returns an offset into the *copy*, and
/// lowercasing does not preserve byte length: `İ` (U+0130) is two bytes and
/// lowercases to three, `ẞ` is three and lowercases to two. Indexing the
/// original with an offset taken from the copy therefore lands mid-character
/// after any such letter, and `&original[..offset]` panics. Turkish and German
/// names in a paper's front matter are enough to hit it.
///
/// So the copy is built alongside a table saying which byte of the original each
/// byte of the copy came from. Every offset that comes back out is a real char
/// boundary in the original.
struct CaseFolded {
    lower: String,
    /// `origin[i]` is the byte offset in the original of the character that
    /// produced byte `i` of `lower`. One longer than `lower`, so a match at the
    /// very end still resolves.
    origin: Vec<usize>,
}

impl CaseFolded {
    fn new(s: &str) -> Self {
        let mut lower = String::with_capacity(s.len());
        let mut origin: Vec<usize> = Vec::with_capacity(s.len() + 1);
        for (at, c) in s.char_indices() {
            for lc in c.to_lowercase() {
                lower.push(lc);
            }
            origin.resize(lower.len(), at);
        }
        origin.push(s.len());
        CaseFolded { lower, origin }
    }

    /// Byte offset **into the original** where `needle_lower` first matches.
    fn find(&self, needle_lower: &str) -> Option<usize> {
        self.lower.find(needle_lower).map(|at| self.origin[at])
    }

}

fn file_type_of(meta: &PaperMeta) -> String {
    meta.file_type.clone().unwrap_or_else(|| "pdf".to_string())
}

fn venue_rank_of(meta: &PaperMeta) -> Option<VenueRank> {
    let rank = meta.journal_rank.as_ref()?;
    if rank.official.is_empty() && rank.custom.is_empty() {
        return None;
    }
    Some(VenueRank {
        official: rank.official.clone(),
        custom: rank
            .custom
            .iter()
            .map(|c| (c.name.clone(), c.rank.clone()))
            .collect(),
    })
}

/// Build a summary from the index, then fill in the fields the index does not
/// carry (`doi`, `arxiv_id`, rankings) by reading that paper's `meta.json`.
///
/// The read is why this runs over the returned page only, never the whole match
/// set: enriching 50 rows costs 50 small file reads, enriching 5000 would not be
/// worth it for a listing.
#[cfg(test)]
mod abstract_tests {
    use super::*;

    #[test]
    fn a_long_abstract_is_cut_and_marked() {
        let long = "字".repeat(SUMMARY_ABSTRACT_CHARS + 50);
        let out = abstract_at(Some(&long), AbstractDetail::Preview).unwrap();
        // Character count, not bytes: a CJK abstract cut on a byte boundary
        // would not be valid UTF-8 at all.
        assert_eq!(out.chars().count(), SUMMARY_ABSTRACT_CHARS + 1);
        assert!(out.ends_with('…'), "the reader must know it was cut");
    }

    #[test]
    fn a_short_abstract_arrives_whole_and_unmarked() {
        let short = "We propose the Transformer.".to_string();
        assert_eq!(
            abstract_at(Some(&short), AbstractDetail::Preview).as_deref(),
            Some(short.as_str())
        );
    }

    /// A missing abstract is `None`, not an empty string that reads to a model
    /// like "this paper's abstract is blank".
    #[test]
    fn nothing_to_preview_is_absent_not_empty() {
        assert_eq!(abstract_at(None, AbstractDetail::Preview), None);
        assert_eq!(abstract_at(Some(&"   \n ".to_string()), AbstractDetail::Preview), None);
    }

    /// The point of the detail argument: a caller with room for it gets the
    /// whole abstract, uncut and unmarked.
    #[test]
    fn full_detail_returns_the_whole_abstract() {
        let long = "字".repeat(SUMMARY_ABSTRACT_CHARS * 4);
        let out = abstract_at(Some(&long), AbstractDetail::Full).unwrap();
        assert_eq!(out.chars().count(), SUMMARY_ABSTRACT_CHARS * 4);
        assert!(!out.ends_with('…'), "nothing was cut, so nothing should say it was");
    }

    #[test]
    fn none_omits_it_entirely() {
        let text = "We propose the Transformer.".to_string();
        assert_eq!(abstract_at(Some(&text), AbstractDetail::None), None);
    }

    /// A typo must not silently strip the abstracts the caller was counting on.
    #[test]
    fn an_unrecognised_detail_falls_back_to_preview() {
        assert_eq!(AbstractDetail::parse(Some("wat")), AbstractDetail::Preview);
        assert_eq!(AbstractDetail::parse(None), AbstractDetail::Preview);
        assert_eq!(AbstractDetail::parse(Some("full")), AbstractDetail::Full);
        assert_eq!(AbstractDetail::parse(Some("none")), AbstractDetail::None);
    }
}

/// The abstract as the caller asked for it.
fn abstract_at(text: Option<&String>, detail: AbstractDetail) -> Option<String> {
    if detail == AbstractDetail::None {
        return None;
    }
    let trimmed = text.map(|s| s.trim())?;
    if trimmed.is_empty() {
        return None;
    }
    if detail == AbstractDetail::Full {
        return Some(trimmed.to_string());
    }
    let cut: String = trimmed.chars().take(SUMMARY_ABSTRACT_CHARS).collect();
    Some(if trimmed.chars().count() > SUMMARY_ABSTRACT_CHARS {
        format!("{cut}…")
    } else {
        cut
    })
}

fn summarize(
    root: &str,
    entry: &crate::models::PaperIndexEntry,
    detail: AbstractDetail,
) -> PaperSummary {
    let meta = paper::read_meta(root, &entry.slug).ok();
    PaperSummary {
        paper_abstract: abstract_at(meta.as_ref().and_then(|m| m.paper_abstract.as_ref()), detail),
        slug: entry.slug.clone(),
        title: entry.title.clone(),
        authors: entry.authors.clone(),
        year: entry.year,
        venue: entry.venue.clone(),
        doi: meta.as_ref().and_then(|m| m.doi.clone()),
        arxiv_id: meta.as_ref().and_then(|m| m.arxiv_id.clone()),
        cite_count: entry.cite_count,
        venue_rank: meta.as_ref().and_then(venue_rank_of),
        tags: entry.tags.clone(),
        reading_status: entry.reading_status.clone(),
        file_type: entry.file_type.clone().unwrap_or_else(|| "pdf".to_string()),
        added_at: entry.added_at.clone(),
        has_bibtex: entry.has_bibtex.unwrap_or(false),
        related_count: entry.related_ids.len(),
    }
}

/// Case-insensitive substring match over the fields a human would search by.
fn matches_query(entry: &crate::models::PaperIndexEntry, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    entry.title.to_lowercase().contains(&needle)
        || entry
            .authors
            .iter()
            .any(|a| a.to_lowercase().contains(&needle))
        || entry.tags.iter().any(|t| t.to_lowercase().contains(&needle))
        || entry
            .venue
            .as_deref()
            .is_some_and(|v| v.to_lowercase().contains(&needle))
}

// ── Tool implementations ─────────────────────────────────────────────────────

pub struct ListPapersArgs<'a> {
    pub query: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub collection_id: Option<&'a str>,
    pub reading_status: Option<&'a str>,
    pub year_from: Option<u32>,
    pub year_to: Option<u32>,
    /// "added" | "year" | "citations" | "title". Defaults to "added".
    pub sort_by: Option<&'a str>,
    /// "desc" (default) | "asc".
    pub order: Option<&'a str>,
    /// Only papers whose venue matches this text, case-insensitive.
    pub venue: Option<&'a str>,
    /// Only papers with at least this many citations.
    pub min_citations: Option<u32>,
    /// Only papers whose text has been extracted (i.e. readable and searchable).
    pub has_fulltext: Option<bool>,
    /// "preview" (default) | "full" | "none".
    pub abstract_detail: Option<&'a str>,
    pub limit: usize,
    pub offset: usize,
}

pub fn list_papers(root: &str, args: ListPapersArgs<'_>) -> Result<PaperListPage, String> {
    // `scan_library` is incremental — it stats meta.json and only re-reads the
    // ones whose mtime moved — so this stays cheap while still picking up papers
    // imported since the index was last written.
    let mut entries = library::scan_library(root)?;

    // A collection filter needs the assignment table, which is keyed by paper
    // `id` rather than slug.
    if let Some(cid) = args.collection_id {
        let file = collections::get_collections(root)?;
        let ids: std::collections::HashSet<&str> = file
            .assignments
            .iter()
            .filter(|a| a.collection_id == cid)
            .map(|a| a.paper_id.as_str())
            .collect();
        entries.retain(|e| ids.contains(e.id.as_str()));
    }
    if let Some(q) = args.query.filter(|q| !q.trim().is_empty()) {
        entries.retain(|e| matches_query(e, q));
    }
    if let Some(tag) = args.tag {
        entries.retain(|e| e.tags.iter().any(|t| t == tag));
    }
    if let Some(status) = args.reading_status {
        entries.retain(|e| e.reading_status == status);
    }
    if let Some(from) = args.year_from {
        entries.retain(|e| e.year.is_some_and(|y| y >= from));
    }
    if let Some(to) = args.year_to {
        entries.retain(|e| e.year.is_some_and(|y| y <= to));
    }
    if let Some(v) = args.venue.filter(|v| !v.trim().is_empty()) {
        let needle = v.to_lowercase();
        entries.retain(|e| {
            e.venue
                .as_deref()
                .is_some_and(|x| x.to_lowercase().contains(&needle))
        });
    }
    if let Some(min) = args.min_citations {
        entries.retain(|e| e.cite_count.unwrap_or(0) >= min);
    }
    if let Some(want) = args.has_fulltext {
        entries.retain(|e| e.status.text_extracted == want);
    }

    // Default: newest first. An agent asking for "my papers" usually wants
    // recent work, and truncation at `limit` should drop the oldest.
    let descending = !matches!(args.order, Some("asc"));
    match args.sort_by {
        Some("year") => {
            // Papers with no year sort last either way — an unknown year is not
            // "year zero", and burying them at the top of an ascending list
            // would hide the oldest real papers.
            entries.sort_by(|a, b| match (a.year, b.year) {
                (Some(x), Some(y)) => if descending { y.cmp(&x) } else { x.cmp(&y) },
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.title.cmp(&b.title),
            });
        }
        Some("citations") => {
            entries.sort_by(|a, b| {
                let (x, y) = (a.cite_count.unwrap_or(0), b.cite_count.unwrap_or(0));
                if descending { y.cmp(&x) } else { x.cmp(&y) }
            });
        }
        Some("title") => {
            entries.sort_by(|a, b| {
                let (x, y) = (a.title.to_lowercase(), b.title.to_lowercase());
                if descending { y.cmp(&x) } else { x.cmp(&y) }
            });
        }
        _ => {
            entries.sort_by(|a, b| {
                if descending {
                    b.added_at.cmp(&a.added_at)
                } else {
                    a.added_at.cmp(&b.added_at)
                }
            });
        }
    }

    let total = entries.len();
    let papers = entries
        .iter()
        .skip(args.offset)
        .take(args.limit)
        .map(|e| summarize(root, e, AbstractDetail::parse(args.abstract_detail)))
        .collect();
    Ok(PaperListPage {
        total,
        offset: args.offset,
        papers,
    })
}

pub fn get_paper(root: &str, slug: &str) -> Result<PaperDetail, String> {
    let meta = paper::read_meta(root, slug)?;

    // `related_ids` stores paper ids; the agent needs slugs to call back in, so
    // resolve them through the index rather than making it guess.
    let related = if meta.related_ids.is_empty() {
        Vec::new()
    } else {
        let index = library::load_library_cache(root);
        meta.related_ids
            .iter()
            .filter_map(|id| {
                index
                    .iter()
                    .find(|e| &e.id == id)
                    .map(|e| RelatedPaper {
                        slug: e.slug.clone(),
                        title: e.title.clone(),
                    })
            })
            .collect()
    };

    let notes = paper::list_notes(root, slug)
        .into_iter()
        .map(|n: Note| NoteSummary {
            note_id: n.id,
            title: n.title,
            updated_at: n.updated_at,
        })
        .collect();

    Ok(PaperDetail {
        slug: slug.to_string(),
        title: meta.title.clone(),
        authors: meta.authors.clone(),
        year: meta.year,
        venue: meta.venue.clone(),
        doi: meta.doi.clone(),
        arxiv_id: meta.arxiv_id.clone(),
        tags: meta.tags.clone(),
        reading_status: meta.reading_status.clone(),
        abstract_text: meta.paper_abstract.clone(),
        bibtex: meta.bibtex.clone(),
        cite_count: meta.cite_count,
        venue_rank: venue_rank_of(&meta),
        added_at: meta.added_at.clone(),
        file_type: file_type_of(&meta),
        related,
        notes,
        highlight_count: paper::read_highlights(root, slug).len(),
        fulltext_chars: extraction::read_fulltext(root, slug).chars().count(),
        has_sections: sections::read_sections(root, slug).is_some(),
    })
}

pub fn search_papers(
    root: &str,
    query: &str,
    limit: usize,
    detail: AbstractDetail,
) -> Result<Vec<SearchResult>, String> {
    // Keep the FTS index current before querying it; the index is a rebuildable
    // cache and may lag a library edited on another machine.
    let _ = search::ensure_current(root);
    let hits = search::search_fulltext(root, query)?;
    Ok(hits
        .into_iter()
        .take(limit)
        .map(|h| {
            // One `meta.json` read per returned hit, bounded by `limit`, so a
            // result can carry what the paper is about rather than only where
            // the query matched.
            let meta = paper::read_meta(root, &h.slug).ok();
            SearchResult {
                year: meta.as_ref().and_then(|m| m.year),
                venue: meta.as_ref().and_then(|m| m.venue.clone()),
                cite_count: meta.as_ref().and_then(|m| m.cite_count),
                paper_abstract: abstract_at(
                    meta.as_ref().and_then(|m| m.paper_abstract.as_ref()),
                    detail,
                ),
                slug: h.slug,
                title: h.title,
                authors: h.authors,
                snippet: h.snippet,
            }
        })
        .collect())
}

pub fn get_paper_fulltext(
    root: &str,
    slug: &str,
    offset: usize,
    limit: usize,
    section: Option<&str>,
) -> Result<FulltextSlice, String> {
    // Fail loudly on an unknown slug rather than returning an empty slice that
    // reads like "this paper has no text".
    paper::read_meta(root, slug)?;
    let full = extraction::read_fulltext(root, slug);

    // A section name narrows the window before paging. The heading text in
    // sections.json is what the extractor saw in the document, so a plain
    // case-insensitive search for it in the full text locates the section start.
    let (base, base_offset) = match section {
        Some(name) if !name.trim().is_empty() => {
            // Folded once and reused: the sibling-heading scan below searches
            // the same text again for every section in the document.
            let folded = CaseFolded::new(&full);
            let needle = name.trim().to_lowercase();
            match folded.find(&needle) {
                Some(byte_start) => {
                    // Byte index → char index, since the whole API is char-based.
                    let char_start = full[..byte_start].chars().count();
                    let next = sections::read_sections(root, slug)
                        .map(|s| s.sections)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|s| {
                            let n = s.title.trim().to_lowercase();
                            if n.is_empty() || n == needle {
                                return None;
                            }
                            folded
                                .find(&n)
                                .map(|b| full[..b].chars().count())
                                .filter(|c| *c > char_start)
                        })
                        .min()
                        .unwrap_or_else(|| full.chars().count());
                    let slice: String = full
                        .chars()
                        .skip(char_start)
                        .take(next.saturating_sub(char_start))
                        .collect();
                    (slice, char_start)
                }
                None => {
                    return Err(format!(
                        "Section '{name}' not found in the extracted text of '{slug}'. \
                         Call get_paper_sections to see the available headings."
                    ));
                }
            }
        }
        _ => (full, 0),
    };

    let total = base.chars().count();
    let limit = limit.clamp(1, MAX_FULLTEXT_CHARS);
    let text: String = base.chars().skip(offset).take(limit).collect();
    let returned = text.chars().count();
    Ok(FulltextSlice {
        slug: slug.to_string(),
        text,
        offset: base_offset + offset,
        returned,
        total,
        has_more: offset + returned < total,
    })
}

pub fn get_paper_sections(root: &str, slug: &str) -> Result<PaperSectionList, String> {
    paper::read_meta(root, slug)?;
    let data = sections::read_sections(root, slug).ok_or_else(|| {
        format!("No section outline has been generated for '{slug}'. Use get_paper_fulltext instead.")
    })?;
    Ok(PaperSectionList {
        slug: slug.to_string(),
        source: data.source,
        sections: data
            .sections
            .into_iter()
            .map(|s| SectionEntry {
                title: s.title,
                level: s.level,
                page: s.page,
            })
            .collect(),
    })
}

/// Locate the paper's PDF/ebook so the calling agent can open it with its own
/// file-reading tool.
///
/// Returning a path rather than bytes is the point: Claude Code and Codex both
/// read PDFs natively and page through them, whereas a base64 blob in a tool
/// result is expensive and mostly unusable.
pub fn get_document_path(root: &str, slug: &str) -> Result<DocumentLocation, String> {
    let meta = paper::read_meta(root, slug)?;
    let file_type = file_type_of(&meta);

    let path = if ebook::is_ebook_file_type(meta.file_type.as_deref()) {
        ebook::find_ebook_in_dir(root, slug)
            .map(|(p, _)| p)
            .ok_or_else(|| format!("No ebook file found for '{slug}'"))?
    } else {
        metadata::find_pdf_in_dir(root, slug)
    };

    let exists = path.is_file();
    // Canonicalize so the agent gets a path free of symlinks and `..`. Falls
    // back to the raw path when the file is missing (canonicalize would fail).
    let display = path
        .canonicalize()
        .unwrap_or(path)
        .to_string_lossy()
        .to_string();

    Ok(DocumentLocation {
        slug: slug.to_string(),
        path: display,
        file_type,
        exists,
    })
}

pub fn list_notes(root: &str, slug: &str) -> Result<Vec<NoteSummary>, String> {
    paper::read_meta(root, slug)?;
    Ok(paper::list_notes(root, slug)
        .into_iter()
        .map(|n| NoteSummary {
            note_id: n.id,
            title: n.title,
            updated_at: n.updated_at,
        })
        .collect())
}

pub fn get_note(root: &str, slug: &str, note_id: Option<&str>) -> Result<NoteContent, String> {
    paper::read_meta(root, slug)?;
    let notes = paper::list_notes(root, slug);
    if notes.is_empty() {
        return Err(format!("Paper '{slug}' has no notes."));
    }

    // With no id, hand back the most recently edited note — the one the user was
    // last working in, and the useful default for "what did I write about this".
    let note = match note_id {
        Some(id) => notes
            .iter()
            .find(|n| n.id == id)
            .ok_or_else(|| format!("Note '{id}' not found on paper '{slug}'"))?,
        None => notes
            .iter()
            .max_by(|a, b| a.updated_at.cmp(&b.updated_at))
            .expect("notes is non-empty"),
    };

    Ok(NoteContent {
        slug: slug.to_string(),
        note_id: note.id.clone(),
        title: note.title.clone(),
        content: paper::get_note(root, slug, &note.id),
        updated_at: note.updated_at.clone(),
    })
}

pub fn get_highlights(root: &str, slug: &str) -> Result<Vec<HighlightEntry>, String> {
    paper::read_meta(root, slug)?;
    Ok(paper::read_highlights(root, slug)
        .into_iter()
        .map(|h: Highlight| HighlightEntry {
            id: h.id,
            page: h.page,
            text: h.text,
            note: h.note.unwrap_or_default(),
            color: h.color,
            style: h.style,
            created_at: h.created_at,
        })
        .collect())
}

/// Collection ids in the subtree rooted at `root_id`, including itself.
///
/// Bounded by the collection count rather than recursing, so malformed or
/// cyclic `parent_id` data cannot spin here.
fn subtree_ids(all: &[crate::models::Collection], root_id: &str) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    set.insert(root_id.to_string());
    for _ in 0..=all.len() {
        let before = set.len();
        for c in all {
            if let Some(pid) = &c.parent_id {
                if set.contains(pid) {
                    set.insert(c.id.clone());
                }
            }
        }
        if set.len() == before {
            break;
        }
    }
    set
}

/// Walk up to the root, building "Parent / Child" and the nesting depth.
fn collection_path(all: &[crate::models::Collection], id: &str) -> (usize, String) {
    let mut names = Vec::new();
    let mut current = Some(id.to_string());
    // Same bound as `subtree_ids`: a cycle in `parent_id` must not hang.
    for _ in 0..=all.len() {
        let Some(cid) = current else { break };
        let Some(c) = all.iter().find(|c| c.id == cid) else {
            break;
        };
        names.push(c.name.clone());
        current = c.parent_id.clone();
    }
    names.reverse();
    (names.len().saturating_sub(1), names.join(" / "))
}

pub fn list_collections(root: &str) -> Result<Vec<CollectionEntry>, String> {
    let file = collections::get_collections(root)?;
    Ok(file
        .collections
        .iter()
        .map(|c| {
            let subtree = subtree_ids(&file.collections, &c.id);
            // Deduplicate: a paper filed in two sub-collections is one paper.
            let nested: std::collections::HashSet<&str> = file
                .assignments
                .iter()
                .filter(|a| subtree.contains(&a.collection_id))
                .map(|a| a.paper_id.as_str())
                .collect();
            let (depth, path) = collection_path(&file.collections, &c.id);
            CollectionEntry {
                id: c.id.clone(),
                name: c.name.clone(),
                parent_id: c.parent_id.clone(),
                paper_count: file
                    .assignments
                    .iter()
                    .filter(|a| a.collection_id == c.id)
                    .count(),
                total_paper_count: nested.len(),
                depth,
                path,
            }
        })
        .collect())
}

/// Cap on `LibraryStats::top_tags`, so a library with hundreds of tags cannot
/// flood the response.
pub const MAX_TOP_TAGS: usize = 30;

fn counted_desc(counts: std::collections::HashMap<String, usize>) -> Vec<CountedValue> {
    let mut out: Vec<CountedValue> = counts
        .into_iter()
        .map(|(value, count)| CountedValue { value, count })
        .collect();
    // Ties broken by name so the output is stable between calls.
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    out
}

/// A single-call overview of the library.
///
/// Everything about papers comes from one incremental index scan — the index
/// already carries reading status, tags, year, file type and the pipeline flags
/// — so this costs about the same as `list_papers` and saves the agent from
/// probing with a series of filtered listings.
pub fn library_stats(root: &str) -> Result<LibraryStats, String> {
    use std::collections::HashMap;

    let papers = library::scan_library(root)?;

    let mut stats = LibraryStats {
        total_papers: papers.len(),
        unread: 0,
        reading: 0,
        read: 0,
        with_fulltext: 0,
        with_ai_summary: 0,
        vectorized: 0,
        with_bibtex: 0,
        with_related_links: 0,
        file_types: Vec::new(),
        earliest_year: None,
        latest_year: None,
        without_year: 0,
        by_year: Vec::new(),
        distinct_tags: 0,
        top_tags: Vec::new(),
        added_last_30_days: 0,
        collections: 0,
        canvases: 0,
        snippet_libraries: 0,
        snippets: 0,
    };

    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let mut tags: HashMap<String, usize> = HashMap::new();
    let mut types: HashMap<String, usize> = HashMap::new();
    let mut years: HashMap<u32, usize> = HashMap::new();

    for p in &papers {
        match p.reading_status.as_str() {
            "read" => stats.read += 1,
            "reading" => stats.reading += 1,
            _ => stats.unread += 1,
        }

        if p.status.text_extracted {
            stats.with_fulltext += 1;
        }
        if p.status.ai_summary_done {
            stats.with_ai_summary += 1;
        }
        if p.status.vectorized {
            stats.vectorized += 1;
        }
        if p.has_bibtex.unwrap_or(false) {
            stats.with_bibtex += 1;
        }
        if !p.related_ids.is_empty() {
            stats.with_related_links += 1;
        }

        *types
            .entry(p.file_type.clone().unwrap_or_else(|| "pdf".to_string()))
            .or_insert(0) += 1;

        match p.year {
            Some(y) => {
                *years.entry(y).or_insert(0) += 1;
                stats.earliest_year = Some(stats.earliest_year.map_or(y, |e: u32| e.min(y)));
                stats.latest_year = Some(stats.latest_year.map_or(y, |l: u32| l.max(y)));
            }
            None => stats.without_year += 1,
        }

        for tag in &p.tags {
            *tags.entry(tag.clone()).or_insert(0) += 1;
        }

        // A malformed timestamp simply does not count towards "recent" rather
        // than failing the whole call.
        if chrono::DateTime::parse_from_rfc3339(&p.added_at).is_ok_and(|t| t > cutoff) {
            stats.added_last_30_days += 1;
        }
    }

    stats.file_types = counted_desc(types);
    stats.distinct_tags = tags.len();
    stats.top_tags = counted_desc(tags).into_iter().take(MAX_TOP_TAGS).collect();

    let mut by_year: Vec<YearCount> = years
        .into_iter()
        .map(|(year, count)| YearCount { year, count })
        .collect();
    by_year.sort_by(|a, b| b.year.cmp(&a.year));
    stats.by_year = by_year;

    // The other stores are small index files; read them rather than making the
    // agent call three more tools to learn whether they are worth visiting.
    stats.collections = collections::get_collections(root)
        .map(|f| f.collections.len())
        .unwrap_or(0);
    stats.canvases = canvas::list_canvases(root).map(|c| c.len()).unwrap_or(0);
    if let Ok(libs) = snippets::list_snippet_libraries(root) {
        stats.snippet_libraries = libs.len();
        stats.snippets = libs
            .iter()
            .filter_map(|l| snippets::get_snippets(root, &l.id).ok())
            .map(|s| s.len())
            .sum();
    }

    Ok(stats)
}

pub fn list_canvases(root: &str) -> Result<Vec<CanvasSummary>, String> {
    Ok(canvas::list_canvases(root)?
        .into_iter()
        .map(|c| CanvasSummary {
            id: c.id,
            name: c.name,
            updated_at: c.updated_at,
        })
        .collect())
}

pub fn get_canvas(root: &str, id: &str) -> Result<CanvasDetail, String> {
    let c = canvas::get_canvas(root, id)?;

    // Canvas nodes reference papers by id. Resolve to slug + title so the graph
    // is readable on its own and the agent can follow an edge into `get_paper`.
    let index = library::load_library_cache(root);
    let nodes = c
        .nodes
        .iter()
        .map(|n| {
            // `paper_id` is an empty string on free-standing text/shape nodes,
            // so match on it only when it actually points at something.
            let found = if n.paper_id.is_empty() {
                None
            } else {
                index.iter().find(|e| e.id == n.paper_id)
            };
            CanvasNodeEntry {
                node_id: n.node_id.clone(),
                slug: found.map(|e| e.slug.clone()),
                // Paper nodes are labelled by their title; annotation nodes carry
                // the user's own text, which is often the reasoning that explains
                // why two papers are connected.
                label: found
                    .map(|e| e.title.clone())
                    .or_else(|| n.content.clone())
                    .unwrap_or_default(),
                kind: n.node_type.clone().unwrap_or_else(|| "paper".to_string()),
            }
        })
        .collect();

    let edges = c
        .edges
        .iter()
        .map(|e| CanvasEdgeEntry {
            source_node_id: e.from_node_id.clone(),
            target_node_id: e.to_node_id.clone(),
            label: e.label.clone().unwrap_or_default(),
        })
        .collect();

    Ok(CanvasDetail {
        id: c.id,
        name: c.name,
        nodes,
        edges,
    })
}

pub fn list_snippet_libraries(root: &str) -> Result<Vec<SnippetLibraryEntry>, String> {
    Ok(snippets::list_snippet_libraries(root)?
        .into_iter()
        .map(|lib| {
            let count = snippets::get_snippets(root, &lib.id)
                .map(|s| s.len())
                .unwrap_or(0);
            SnippetLibraryEntry {
                id: lib.id,
                name: lib.name,
                snippet_count: count,
            }
        })
        .collect())
}

pub fn list_snippets(
    root: &str,
    library_id: Option<&str>,
    query: Option<&str>,
    limit: usize,
) -> Result<Vec<SnippetEntry>, String> {
    // No library id means "search everything" — the agent usually does not know
    // or care how the user partitioned their snippet libraries.
    let libs: Vec<String> = match library_id {
        Some(id) => vec![id.to_string()],
        None => snippets::list_snippet_libraries(root)?
            .into_iter()
            .map(|l| l.id)
            .collect(),
    };

    let needle = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());

    let mut out = Vec::new();
    for lib in libs {
        for s in snippets::get_snippets(root, &lib)? {
            if let Some(n) = &needle {
                let hit = s.text.to_lowercase().contains(n)
                    || s.note.to_lowercase().contains(n)
                    || s.paper_title.to_lowercase().contains(n)
                    || s.tags.iter().any(|t| t.to_lowercase().contains(n));
                if !hit {
                    continue;
                }
            }
            out.push(SnippetEntry {
                id: s.id,
                library_id: s.library_id,
                text: s.text,
                note: s.note,
                tags: s.tags,
                paper_title: s.paper_title,
                page: s.page,
                created_at: s.created_at,
            });
            if out.len() >= limit {
                return Ok(out);
            }
        }
    }
    Ok(out)
}

// ── Conversations ────────────────────────────────────────────────────────────
//
// Two stores, two shapes, both untyped JSON on disk:
//
// * `chats/<id>.json` — one per conversation: `{id, title, createdAt, updatedAt,
//   messages: [{role, content, createdAt}]}]`
// * `papers/<slug>/ai_conversations.json` — `[{id, title, slug, createdAt,
//   updatedAt, nodes: [{role: "user" | "assistantGroup", content, createdAt,
//   answers: [...], attachments: [...]}]}]`
//
// An `assistantGroup` node holds one answer per model the question was sent to,
// so it flattens into several assistant messages rather than one.
//
// The legacy per-paper `chat.json` is deliberately not exposed: the app writes
// it as a flattened mirror of the active conversation, so surfacing it would
// duplicate `ai_conversations.json` rather than add anything.

fn truncate_chars(s: &str, max: usize) -> (String, bool) {
    let mut out: String = s.chars().take(max).collect();
    let truncated = out.chars().count() < s.chars().count();
    if truncated {
        out.push_str("\n…[truncated]");
    }
    (out, truncated)
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

/// Keep an answer's content and which model produced it; drop everything else.
///
/// The dropped fields are listed in `REDACTED_CONVERSATION_FIELDS`: provider
/// identity is configuration, and cost/token counts are the spend history the
/// endpoint is not allowed to expose. `contextContent` and `reasoningContent`
/// are dropped for size — the first duplicates text available via
/// `get_paper_fulltext`, the second is chain-of-thought that can dwarf the
/// answer itself.
fn redact_answer(answer: &serde_json::Value) -> Option<ConversationMessage> {
    let content = str_field(answer, "content");
    if content.trim().is_empty() {
        // A failed or still-streaming answer carries no content worth returning.
        return None;
    }
    let (content, truncated) = truncate_chars(&content, MAX_MESSAGE_CHARS);
    Some(ConversationMessage {
        role: "assistant".to_string(),
        content,
        model: answer
            .get("modelName")
            .and_then(|m| m.as_str())
            .filter(|m| !m.is_empty())
            .map(|m| m.to_string()),
        created_at: str_field(answer, "createdAt"),
        attachments: Vec::new(),
        truncated,
    })
}

/// Attachment names only — never `dataUrl`, which holds a base64 image or PDF.
fn attachment_names(node: &serde_json::Value) -> Vec<String> {
    node.get("attachments")
        .and_then(|a| a.as_array())
        .map(|list| {
            list.iter()
                .map(|a| {
                    let name = str_field(a, "name");
                    if name.is_empty() {
                        str_field(a, "type")
                    } else {
                        name
                    }
                })
                .filter(|n| !n.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn flatten_conversation(conv: &serde_json::Value) -> Vec<ConversationMessage> {
    // Library conversations store `messages`; paper conversations store `nodes`.
    let entries = conv
        .get("messages")
        .or_else(|| conv.get("nodes"))
        .and_then(|m| m.as_array());
    let Some(entries) = entries else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for node in entries {
        let role = str_field(node, "role");
        if role == "assistantGroup" {
            if let Some(answers) = node.get("answers").and_then(|a| a.as_array()) {
                out.extend(answers.iter().filter_map(redact_answer));
            }
            continue;
        }

        let content = str_field(node, "content");
        let attachments = attachment_names(node);
        if content.trim().is_empty() && attachments.is_empty() {
            continue;
        }
        let (content, truncated) = truncate_chars(&content, MAX_MESSAGE_CHARS);
        out.push(ConversationMessage {
            role: if role.is_empty() { "user".into() } else { role },
            content,
            model: None,
            created_at: str_field(node, "createdAt"),
            attachments,
            truncated,
        });
    }
    out
}

fn summarize_conversation(
    conv: &serde_json::Value,
    scope: &str,
    slug: Option<&str>,
    needle: Option<&str>,
) -> Option<ConversationSummary> {
    let messages = flatten_conversation(conv);
    if messages.is_empty() {
        return None;
    }

    let match_snippet = match needle {
        Some(n) => {
            let title = str_field(conv, "title").to_lowercase();
            let hit = messages
                .iter()
                .find(|m| m.content.to_lowercase().contains(n));
            match hit {
                Some(m) => {
                    // Centre the excerpt on the match so the agent can judge
                    // relevance without fetching the whole conversation.
                    let at = CaseFolded::new(&m.content).find(n).unwrap_or(0);
                    let start = m.content[..at].chars().count().saturating_sub(60);
                    Some(
                        m.content
                            .chars()
                            .skip(start)
                            .take(PREVIEW_CHARS + 60)
                            .collect(),
                    )
                }
                // A title-only match still counts, it just has no excerpt.
                None if title.contains(n) => None,
                None => return None,
            }
        }
        None => None,
    };

    let preview = messages
        .iter()
        .find(|m| m.role == "user")
        .or_else(|| messages.first())
        .map(|m| m.content.chars().take(PREVIEW_CHARS).collect())
        .unwrap_or_default();

    Some(ConversationSummary {
        conversation_id: str_field(conv, "id"),
        scope: scope.to_string(),
        slug: slug.map(|s| s.to_string()),
        title: str_field(conv, "title"),
        message_count: messages.len(),
        created_at: str_field(conv, "createdAt"),
        updated_at: str_field(conv, "updatedAt"),
        preview,
        match_snippet,
    })
}

fn as_conversation_array(v: serde_json::Value) -> Vec<serde_json::Value> {
    match v {
        serde_json::Value::Array(a) => a,
        _ => Vec::new(),
    }
}

pub fn list_conversations(
    root: &str,
    scope: &str,
    slug: Option<&str>,
    query: Option<&str>,
    limit: usize,
) -> Result<Vec<ConversationSummary>, String> {
    let needle = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let mut out: Vec<ConversationSummary> = Vec::new();

    // A slug always narrows to that paper, whatever `scope` says.
    let want_library = slug.is_none() && matches!(scope, "library" | "all");
    let want_papers = slug.is_some() || matches!(scope, "paper" | "all");

    if want_library {
        for conv in as_conversation_array(crate::copilot::read_library_conversations(root)) {
            if let Some(s) = summarize_conversation(&conv, "library", None, needle.as_deref()) {
                out.push(s);
            }
        }
    }

    if want_papers {
        let slugs: Vec<String> = match slug {
            Some(s) => {
                // Validate the slug so an unknown one is an error rather than an
                // empty result the agent has to guess about.
                paper::read_meta(root, s)?;
                vec![s.to_string()]
            }
            None => paper::list_paper_dirs(root)?
                .into_iter()
                .map(|(s, _)| s)
                .collect(),
        };
        for s in slugs {
            for conv in as_conversation_array(crate::copilot::read_paper_ai_conversations(root, &s))
            {
                if let Some(summary) =
                    summarize_conversation(&conv, "paper", Some(&s), needle.as_deref())
                {
                    out.push(summary);
                }
            }
        }
    }

    // Most recently touched first — the conversation the user is thinking about
    // is almost always a recent one.
    out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    out.truncate(limit);
    Ok(out)
}

pub fn get_conversation(
    root: &str,
    conversation_id: &str,
    slug: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<ConversationDetail, String> {
    // With a slug, this is one file read. Without, the library store is checked
    // first and only then every paper — 20+ MB of JSON in a large library, which
    // is why `list_conversations` returns the slug alongside the id.
    let mut candidates: Vec<(String, Option<String>, serde_json::Value)> = Vec::new();

    match slug {
        Some(s) => {
            paper::read_meta(root, s)?;
            for conv in as_conversation_array(crate::copilot::read_paper_ai_conversations(root, s))
            {
                candidates.push(("paper".into(), Some(s.to_string()), conv));
            }
        }
        None => {
            for conv in as_conversation_array(crate::copilot::read_library_conversations(root)) {
                candidates.push(("library".into(), None, conv));
            }
            if !candidates.iter().any(|(_, _, c)| str_field(c, "id") == conversation_id) {
                for (s, _) in paper::list_paper_dirs(root)? {
                    for conv in
                        as_conversation_array(crate::copilot::read_paper_ai_conversations(root, &s))
                    {
                        candidates.push(("paper".into(), Some(s.clone()), conv));
                    }
                }
            }
        }
    }

    let (scope, found_slug, conv) = candidates
        .into_iter()
        .find(|(_, _, c)| str_field(c, "id") == conversation_id)
        .ok_or_else(|| {
            format!(
                "Conversation '{conversation_id}' not found. Call list_conversations to get valid \
                 ids, and pass the `slug` it reports for paper conversations."
            )
        })?;

    let all = flatten_conversation(&conv);
    let total = all.len();
    let messages: Vec<ConversationMessage> =
        all.into_iter().skip(offset).take(limit).collect();
    let returned = messages.len();

    Ok(ConversationDetail {
        conversation_id: conversation_id.to_string(),
        scope,
        slug: found_slug,
        title: str_field(&conv, "title"),
        messages,
        offset,
        total,
        has_more: offset + returned < total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guarantee in this module's docs is that no MCP-reachable code reads
    /// an AI-config or AI-conversation artifact. Since nothing here takes a
    /// path, the only way such a read could appear is a literal filename in this
    /// file — so assert none is present.
    #[test]
    fn no_denied_artifact_is_referenced() {
        let source = include_str!("tools.rs");
        // Skip the doc comment and the DENIED_ARTIFACTS list themselves, which
        // legitimately name these files.
        let body = source
            .split_once("// ── Helpers ─")
            .map(|(_, rest)| rest)
            .unwrap_or(source);
        for denied in DENIED_ARTIFACTS {
            assert!(
                !body.contains(denied),
                "MCP tool code references a denied artifact: {denied}"
            );
        }
    }

    /// A per-paper conversation node as the app actually writes it: an
    /// assistantGroup with several model answers, plus a user turn carrying a
    /// base64 image.
    fn paper_conversation_fixture() -> serde_json::Value {
        serde_json::json!({
            "id": "conv_1",
            "title": "Why does this method work?",
            "slug": "attention-2017",
            "createdAt": "2026-05-01T00:00:00Z",
            "updatedAt": "2026-05-02T00:00:00Z",
            "nodes": [
                {
                    "id": "n1",
                    "role": "user",
                    "content": "What is the key idea here?",
                    "createdAt": "2026-05-01T00:00:00Z",
                    "attachments": [
                        { "id": "a1", "name": "figure3.png", "type": "image",
                          "dataUrl": "data:image/png;base64,SUPERLONGBLOB" }
                    ]
                },
                {
                    "id": "n2",
                    "role": "assistantGroup",
                    "content": null,
                    "createdAt": "2026-05-01T00:01:00Z",
                    "answers": [
                        {
                            "id": "ans1",
                            "content": "Self-attention replaces recurrence.",
                            "modelName": "deepseek-v4-flash",
                            "providerId": "6f86a75b-SECRET-PROVIDER-UUID",
                            "providerName": "DeepSeek",
                            "costUsd": 0.0012,
                            "inputTokens": 900,
                            "outputTokens": 120,
                            "totalTokens": 1020,
                            "cacheHitTokens": 0,
                            "contextContent": "…the entire paper pasted in…",
                            "reasoningContent": "…long chain of thought…",
                            "createdAt": "2026-05-01T00:01:00Z"
                        },
                        {
                            "id": "ans2",
                            "content": "",
                            "error": true,
                            "errorText": "rate limited",
                            "modelName": "other-model"
                        }
                    ]
                }
            ]
        })
    }

    #[test]
    fn conversation_flattens_answer_groups_into_messages() {
        let msgs = flatten_conversation(&paper_conversation_fixture());
        // The user turn, plus one message per answer that actually has content.
        // The errored, empty answer is dropped rather than returned blank.
        assert_eq!(msgs.len(), 2, "{msgs:#?}");
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].attachments, vec!["figure3.png"]);
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].model.as_deref(), Some("deepseek-v4-flash"));
        assert!(msgs[1].content.contains("Self-attention"));
    }

    /// The load-bearing test for exposing conversations at all: content is
    /// allowed through, but provider identity, spend, and attachment payloads
    /// are not.
    #[test]
    fn conversation_redacts_provider_cost_and_blobs() {
        let msgs = flatten_conversation(&paper_conversation_fixture());
        let rendered = serde_json::to_string(&msgs).unwrap();

        for field in REDACTED_CONVERSATION_FIELDS {
            assert!(
                !rendered.contains(field),
                "redacted field '{field}' leaked into conversation output: {rendered}"
            );
        }
        for value in [
            "SUPERLONGBLOB",
            "SECRET-PROVIDER-UUID",
            "DeepSeek",
            "0.0012",
            "entire paper pasted in",
            "long chain of thought",
        ] {
            assert!(
                !rendered.contains(value),
                "'{value}' leaked into conversation output: {rendered}"
            );
        }
    }

    #[test]
    fn long_messages_are_capped_and_flagged() {
        let long = "x".repeat(MAX_MESSAGE_CHARS * 2);
        let conv = serde_json::json!({
            "id": "c", "title": "t",
            "messages": [{ "role": "user", "content": long, "createdAt": "" }]
        });
        let msgs = flatten_conversation(&conv);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].truncated, "an oversized message was not flagged");
        assert!(
            msgs[0].content.chars().count() < MAX_MESSAGE_CHARS + 40,
            "cap not applied: {} chars",
            msgs[0].content.chars().count()
        );
    }

    /// Library conversations use `messages`, paper ones use `nodes`; both must
    /// flatten through the same path.
    #[test]
    fn library_conversation_shape_is_understood() {
        let conv = serde_json::json!({
            "id": "lib1",
            "title": "Papers supporting my claim",
            "createdAt": "2026-08-01T00:00:00Z",
            "updatedAt": "2026-08-02T00:00:00Z",
            "messages": [
                { "role": "user", "content": "find supporting papers", "createdAt": "" },
                { "role": "assistant", "content": "Here are three.", "createdAt": "" }
            ]
        });
        let summary = summarize_conversation(&conv, "library", None, None).unwrap();
        assert_eq!(summary.conversation_id, "lib1");
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.scope, "library");
        assert!(summary.slug.is_none());
        assert!(summary.preview.starts_with("find supporting"));
    }

    #[test]
    fn query_filters_and_returns_an_excerpt() {
        let conv = paper_conversation_fixture();
        let hit = summarize_conversation(&conv, "paper", Some("attention-2017"), Some("recurrence"));
        assert!(hit.is_some(), "a matching conversation was filtered out");
        assert!(hit.unwrap().match_snippet.unwrap().contains("recurrence"));

        assert!(
            summarize_conversation(&conv, "paper", Some("attention-2017"), Some("zzzz")).is_none(),
            "a non-matching conversation was returned"
        );
    }

    fn collection(id: &str, name: &str, parent: Option<&str>) -> crate::models::Collection {
        crate::models::Collection {
            id: id.into(),
            name: name.into(),
            emoji: None,
            parent_id: parent.map(|p| p.into()),
            created_at: String::new(),
        }
    }

    /// AI ─ Systematicity ─ Milestones
    fn nested_tree() -> Vec<crate::models::Collection> {
        vec![
            collection("ai", "AI", None),
            collection("sys", "Systematicity", Some("ai")),
            collection("mile", "Milestones", Some("sys")),
        ]
    }

    fn entry(slug: &str, title: &str, year: Option<u32>, cites: Option<u32>) -> crate::models::PaperIndexEntry {
        crate::models::PaperIndexEntry {
            slug: slug.into(),
            id: slug.into(),
            title: title.into(),
            authors: vec![],
            year,
            venue: None,
            tags: vec![],
            status: Default::default(),
            added_at: String::new(),
            reading_status: "unread".into(),
            meta_mtime: 0,
            import_source: None,
            cite_count: cites,
            file_type: None,
            related_ids: vec![],
            has_bibtex: None,
        }
    }

    /// A paper with no year must not sort as "year zero" — that would bury the
    /// oldest real papers under unknowns in an ascending list.
    #[test]
    fn unknown_years_sort_last_in_both_directions() {
        let mut v = vec![
            entry("a", "A", None, None),
            entry("b", "B", Some(2020), None),
            entry("c", "C", Some(1999), None),
        ];
        for descending in [true, false] {
            v.sort_by(|a, b| match (a.year, b.year) {
                (Some(x), Some(y)) => if descending { y.cmp(&x) } else { x.cmp(&y) },
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.title.cmp(&b.title),
            });
            assert_eq!(v.last().unwrap().slug, "a", "descending={descending}");
        }
    }

    /// Missing citation counts are genuinely zero for ranking purposes.
    #[test]
    fn missing_citations_rank_as_zero() {
        let mut v = vec![entry("a", "A", None, None), entry("b", "B", None, Some(5))];
        v.sort_by(|a, b| b.cite_count.unwrap_or(0).cmp(&a.cite_count.unwrap_or(0)));
        assert_eq!(v[0].slug, "b");
    }

    #[test]
    fn subtree_includes_every_descendant() {
        let all = nested_tree();
        let top = subtree_ids(&all, "ai");
        assert_eq!(top.len(), 3, "a grandchild was missed: {top:?}");
        assert_eq!(subtree_ids(&all, "sys").len(), 2);
        assert_eq!(subtree_ids(&all, "mile").len(), 1, "a leaf must be alone");
    }

    #[test]
    fn collection_path_reads_root_first() {
        let all = nested_tree();
        assert_eq!(collection_path(&all, "mile"), (2, "AI / Systematicity / Milestones".into()));
        assert_eq!(collection_path(&all, "ai"), (0, "AI".into()));
    }

    /// Collections come from a JSON file the user could hand-edit or a sync
    /// could corrupt. A parent cycle must not hang the server.
    #[test]
    fn cyclic_parents_terminate() {
        let cyclic = vec![
            collection("a", "A", Some("b")),
            collection("b", "B", Some("a")),
        ];
        // The assertion that matters is that these return at all.
        assert_eq!(subtree_ids(&cyclic, "a").len(), 2);
        let (depth, path) = collection_path(&cyclic, "a");
        assert!(depth <= cyclic.len(), "depth ran away: {depth}");
        assert!(!path.is_empty());
    }

    #[test]
    fn top_tags_are_ordered_and_stable() {
        let mut counts = std::collections::HashMap::new();
        counts.insert("rare".to_string(), 1);
        counts.insert("common".to_string(), 9);
        // Equal counts must break ties by name, or the order flips between calls.
        counts.insert("b-tie".to_string(), 5);
        counts.insert("a-tie".to_string(), 5);

        let out = counted_desc(counts);
        let names: Vec<&str> = out.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(names, vec!["common", "a-tie", "b-tie", "rare"]);
    }

    #[test]
    fn tag_cap_is_disclosed_not_silent() {
        // `distinct_tags` exists precisely so a truncated `top_tags` is visible.
        assert!(MAX_TOP_TAGS > 0);
        let many: std::collections::HashMap<String, usize> =
            (0..MAX_TOP_TAGS + 10).map(|i| (format!("t{i:03}"), i)).collect();
        let distinct = many.len();
        let top: Vec<_> = counted_desc(many).into_iter().take(MAX_TOP_TAGS).collect();
        assert_eq!(top.len(), MAX_TOP_TAGS);
        assert!(distinct > top.len(), "the cap must be reportable");
    }

    #[test]
    fn fulltext_limit_is_capped() {
        assert!(DEFAULT_FULLTEXT_CHARS < MAX_FULLTEXT_CHARS);
        assert_eq!(usize::MAX.clamp(1, MAX_FULLTEXT_CHARS), MAX_FULLTEXT_CHARS);
        assert_eq!(0usize.clamp(1, MAX_FULLTEXT_CHARS), 1);
    }

    /// Lowercasing is not length-preserving, so an offset found in the folded
    /// copy cannot be used to index the original. `İ` grows from two bytes to
    /// three; `ẞ` shrinks from three to two. Either way the offset drifts, and
    /// `&original[..offset]` panics the moment it lands inside a character.
    #[test]
    fn case_folded_offsets_point_into_the_original() {
        // `İ` before the needle pushes every later offset out by one byte, and
        // the character that follows is three bytes wide — so a raw
        // `to_lowercase().find()` offset lands in the middle of `中`.
        let text = "İ中文 Introduction 正文";
        let folded = CaseFolded::new(text);
        let at = folded.find("introduction").expect("needle not found");

        assert!(text.is_char_boundary(at), "offset {at} splits a character");
        assert_eq!(&text[at..at + 12], "Introduction");
        // The naive version really does disagree, or this test proves nothing.
        assert_ne!(text.to_lowercase().find("introduction"), Some(at));

        // And the shrinking case.
        let text = "ẞTRASSE Methods";
        let folded = CaseFolded::new(text);
        let at = folded.find("methods").expect("needle not found");
        assert!(text.is_char_boundary(at));
        assert_eq!(&text[at..], "Methods");

        // The case that actually panicked: `İ` lowercases to two code points,
        // so the drift is one byte and the next character is three bytes wide —
        // the naive offset lands inside it.
        let text = "İ中";
        let naive = text.to_lowercase().find('中').expect("needle not found");
        assert!(
            !text.is_char_boundary(naive),
            "fixture no longer reproduces the panic"
        );
        let at = CaseFolded::new(text).find("中").expect("needle not found");
        assert!(text.is_char_boundary(at), "offset {at} still splits a character");
        assert_eq!(&text[..at], "İ");
    }

    /// The offsets must stay usable for the plain cases too — this is on the
    /// path of every `get_paper_fulltext` call that names a section.
    #[test]
    fn case_folded_offsets_survive_ascii_and_cjk() {
        for (text, needle) in [
            ("Abstract\n\nIntroduction\n\nMethods", "methods"),
            ("摘要\n\n引言\n\n实验方法", "实验方法"),
            ("Mixed 中文 and Results here", "results"),
        ] {
            let folded = CaseFolded::new(text);
            let at = folded.find(needle).unwrap_or_else(|| panic!("{needle} not found"));
            assert!(text.is_char_boundary(at), "{text:?} / {needle:?}");
            assert!(
                text[at..].to_lowercase().starts_with(needle),
                "offset {at} in {text:?} does not begin {needle:?}"
            );
        }
    }

    /// A needle that is not there must be reported, not guessed at.
    #[test]
    fn case_folded_reports_a_miss() {
        assert_eq!(CaseFolded::new("İ中文").find("nothing"), None);
    }
}
