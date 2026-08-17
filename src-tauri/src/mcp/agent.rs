//! In-process access to the MCP tool surface, for the app's own agent mode.
//!
//! The library Q&A agent needs the same tools an external client gets, but
//! spawning `Argus --mcp-stdio` from inside Argus to talk to itself would be
//! absurd. These functions hand the same declarations and the same
//! implementations to the in-app agent loop, with no transport in between.
//!
//! # Why the dispatch is written out by hand
//!
//! `rmcp`'s router could in principle do this, but invoking a `ToolRoute`
//! requires a `RequestContext<RoleServer>`, which only exists inside a live
//! service — there is no honest way to build one here. So `call` is an explicit
//! match. The risk that comes with that is drift: a tool added to `server.rs`
//! and forgotten here. `dispatch_covers_every_tool` fails when that happens.

use serde::Serialize;

use super::server;
use super::tools;

/// A tool as the LLM needs to see it: name, purpose, and argument schema.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTool {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments. Self-contained — no `$ref`.
    pub input_schema: serde_json::Value,
    /// The tool's own claim that it changes nothing (MCP `readOnlyHint`).
    ///
    /// A tool that says nothing counts as writing. The hint is advisory in both
    /// directions, but the honest default for something the settings UI labels
    /// to the user is the one that overstates the risk rather than hides it.
    pub read_only: bool,
}

/// Whether a tool declared itself read-only.
pub fn declares_read_only(tool: &rmcp::model::Tool) -> bool {
    tool.annotations
        .as_ref()
        .and_then(|a| a.read_only_hint)
        .unwrap_or(false)
}

/// Every tool the agent may call, taken from the same router the MCP server
/// serves, so the two can never describe different tools.
pub fn tools() -> Vec<AgentTool> {
    server::tool_declarations()
        .into_iter()
        .map(|t| AgentTool {
            name: t.name.to_string(),
            read_only: declares_read_only(&t),
            description: t.description.map(|d| d.to_string()).unwrap_or_default(),
            input_schema: serde_json::Value::Object((*t.input_schema).clone()),
        })
        .collect()
}

/// Deserialize `args` into `P`, defaulting when the model sent nothing.
///
/// Models routinely omit the arguments object for a no-parameter tool, or send
/// `null`; treating that as "use the defaults" avoids failing a call over a
/// formatting quirk that has no bearing on intent.
///
/// "Defaults" means what serde would fill in for an empty object — the
/// `#[serde(default = "...")]` on each field — not `P::default()`. Those are not
/// the same thing: `usize`'s derived default is 0, while every `limit` here
/// defaults to a real page size, and the clamps downstream would turn that 0
/// into 1. The MCP server never had the bug because `rmcp` deserializes through
/// serde; only this in-process path could reach the derived default.
fn params<P: serde::de::DeserializeOwned>(args: &serde_json::Value) -> Result<P, String> {
    let empty = serde_json::Value::Object(serde_json::Map::new());
    let source = match args {
        serde_json::Value::Null => &empty,
        serde_json::Value::Object(o) if o.is_empty() => &empty,
        other => other,
    };
    serde_json::from_value(source.clone()).map_err(|e| format!("invalid arguments: {e}"))
}

fn ok<T: Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| format!("cannot serialize result: {e}"))
}

/// Run one tool against `root`.
///
/// Blocking file and SQLite work, so callers must keep this off the async
/// runtime's worker threads — `copilot` wraps it in `spawn_blocking`.
pub fn call(root: &str, name: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    match name {
        "list_papers" => {
            let p: server::ListPapersParams = params(args)?;
            ok(tools::list_papers(
                root,
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
            )?)
        }
        "get_paper" => {
            let p: server::SlugParams = params(args)?;
            ok(tools::get_paper(root, &p.slug)?)
        }
        "search_papers" => {
            let p: server::SearchParams = params(args)?;
            ok(tools::search_papers(
                root,
                &p.query,
                p.limit.clamp(1, 100),
                tools::AbstractDetail::parse(p.abstract_detail.as_deref()),
            )?)
        }
        "get_paper_fulltext" => {
            let p: server::FulltextParams = params(args)?;
            ok(tools::get_paper_fulltext(
                root,
                &p.slug,
                p.offset,
                p.limit,
                p.section.as_deref(),
            )?)
        }
        "get_paper_sections" => {
            let p: server::SlugParams = params(args)?;
            ok(tools::get_paper_sections(root, &p.slug)?)
        }
        "get_paper_pdf_path" => {
            let p: server::SlugParams = params(args)?;
            ok(tools::get_document_path(root, &p.slug)?)
        }
        "list_notes" => {
            let p: server::SlugParams = params(args)?;
            ok(tools::ItemList::from(tools::list_notes(root, &p.slug)?))
        }
        "get_note" => {
            let p: server::NoteParams = params(args)?;
            ok(tools::get_note(root, &p.slug, p.note_id.as_deref())?)
        }
        "get_highlights" => {
            let p: server::SlugParams = params(args)?;
            ok(tools::ItemList::from(tools::get_highlights(root, &p.slug)?))
        }
        "get_library_stats" => ok(tools::library_stats(root)?),
        "list_collections" => ok(tools::ItemList::from(tools::list_collections(root)?)),
        "list_canvases" => ok(tools::ItemList::from(tools::list_canvases(root)?)),
        "get_canvas" => {
            let p: server::CanvasParams = params(args)?;
            ok(tools::get_canvas(root, &p.canvas_id)?)
        }
        "list_snippet_libraries" => ok(tools::ItemList::from(tools::list_snippet_libraries(root)?)),
        "search_snippets" => {
            let p: server::SnippetParams = params(args)?;
            ok(tools::ItemList::from(tools::list_snippets(
                root,
                p.library_id.as_deref(),
                p.query.as_deref(),
                p.limit.clamp(1, 500),
            )?))
        }
        "list_conversations" => {
            let p: server::ListConversationsParams = params(args)?;
            ok(tools::ItemList::from(tools::list_conversations(
                root,
                &p.scope,
                p.slug.as_deref(),
                p.query.as_deref(),
                p.limit.clamp(1, 200),
            )?))
        }
        "get_conversation" => {
            let p: server::GetConversationParams = params(args)?;
            ok(tools::get_conversation(
                root,
                &p.conversation_id,
                p.slug.as_deref(),
                p.offset,
                p.limit.clamp(1, 100),
            )?)
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard against the one failure mode hand-written dispatch invites: a
    /// tool declared to the model that nothing here can actually run. The model
    /// would call it and get "unknown tool" back.
    #[test]
    fn dispatch_covers_every_tool() {
        for tool in tools() {
            let err = call("/nonexistent-library", &tool.name, &serde_json::json!({}))
                .err()
                .unwrap_or_default();
            assert!(
                !err.contains("unknown tool"),
                "tool '{}' is declared to the model but has no dispatch arm",
                tool.name
            );
        }
    }

    /// And the reverse: nothing dispatchable that is not declared, which would
    /// be dead code the model can never reach.
    #[test]
    fn dispatch_has_no_undeclared_arms() {
        let declared: std::collections::HashSet<String> =
            tools().into_iter().map(|t| t.name).collect();
        let source = include_str!("agent.rs");
        // Match the `"name" => {` arms in `call`, ignoring the catch-all.
        for line in source.lines() {
            let trimmed = line.trim();
            let Some(rest) = trimmed.strip_prefix('"') else {
                continue;
            };
            let Some((name, tail)) = rest.split_once('"') else {
                continue;
            };
            if !tail.trim_start().starts_with("=>") {
                continue;
            }
            assert!(
                declared.contains(name),
                "dispatch handles '{name}', but no tool by that name is declared"
            );
        }
    }

    #[test]
    fn missing_or_empty_arguments_fall_back_to_defaults() {
        // Models often omit the arguments object entirely for a no-arg tool.
        // The defaults that apply are serde's — the page size the tool
        // documents — not the derived `usize` zero.
        let p: server::ListPapersParams = params(&serde_json::Value::Null).unwrap();
        assert_eq!(p.limit, 50, "the documented default page size must apply");
        let p: server::ListPapersParams = params(&serde_json::json!({})).unwrap();
        assert_eq!(p.limit, 50);
        assert!(p.query.is_none());

        // A populated object still deserializes normally.
        let p: server::ListPapersParams =
            params(&serde_json::json!({"query": "attention", "limit": 5})).unwrap();
        assert_eq!(p.query.as_deref(), Some("attention"));
        assert_eq!(p.limit, 5);
    }

    /// The bug this guards: `P::default()` gave `limit: 0`, which the clamp in
    /// `call` turned into 1 — so an agent asking "what is in my library" with no
    /// arguments got exactly one paper back and reported that as the whole
    /// library. Every paged tool is checked, because the mistake was uniform.
    #[test]
    fn an_empty_argument_object_never_yields_a_zero_limit() {
        let empty = serde_json::json!({});
        let p: server::ListPapersParams = params(&empty).unwrap();
        assert_eq!(p.limit, 50);
        let p: server::SnippetParams = params(&empty).unwrap();
        assert_eq!(p.limit, 50);
        let p: server::ListConversationsParams = params(&empty).unwrap();
        assert_eq!(p.limit, 30);
        assert_eq!(p.scope, "all");
        let p: server::GetConversationParams =
            params(&serde_json::json!({"conversation_id": "c1"})).unwrap();
        assert_eq!(p.limit, 20);
    }

    /// A tool whose required argument is missing should say so, rather than
    /// silently searching for the empty string.
    #[test]
    fn a_missing_required_argument_is_an_error() {
        let err = params::<server::SearchParams>(&serde_json::json!({})).unwrap_err();
        assert!(err.contains("query"), "{err}");
        let err = params::<server::SlugParams>(&serde_json::Value::Null).unwrap_err();
        assert!(err.contains("slug"), "{err}");
    }

    /// The library server is read-only by construction, and the settings tab
    /// labels each tool from this flag — so a tool that forgets the annotation
    /// would be shown to the user as one that writes.
    #[test]
    fn every_library_tool_carries_its_read_only_claim() {
        for tool in tools() {
            assert!(
                tool.read_only,
                "tool '{}' does not declare read_only_hint",
                tool.name
            );
        }
    }

    #[test]
    fn declarations_are_usable_by_a_model() {
        let all = tools();
        assert!(!all.is_empty());
        for t in &all {
            assert!(!t.description.is_empty(), "tool '{}' has no description", t.name);
            assert_eq!(
                t.input_schema.get("type").and_then(|v| v.as_str()),
                Some("object"),
                "tool '{}' has a non-object argument schema",
                t.name
            );
            let rendered = t.input_schema.to_string();
            assert!(!rendered.contains("$ref"), "tool '{}' schema has a $ref", t.name);
        }
    }

    #[test]
    fn an_unknown_tool_is_reported_not_panicked() {
        let err = call("/tmp", "definitely_not_a_tool", &serde_json::json!({})).unwrap_err();
        assert!(err.contains("unknown tool"), "{err}");
    }
}
