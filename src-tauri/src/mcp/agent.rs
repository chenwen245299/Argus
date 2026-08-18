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

use base64::Engine;
use serde::Serialize;

use super::app_tools;
use super::server;
use super::tools;

/// The one tool whose result carries images. `copilot` special-cases it: the
/// base64 PNGs are stripped out of the token-costed tool message, saved as files
/// under the conversation folder, and — for a vision model — fed back as a
/// separate image message. Kept here so both sides name it the same thing.
pub const VIEW_PAGE_TOOL: &str = "view_paper_page";

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

/// Every tool the agent may call: the MCP server's read-only surface, plus the
/// writing tools that exist only inside the app (see [`app_only_tools`]).
///
/// The read-only half is taken from the same router the MCP server serves, so
/// the two can never describe different tools.
pub fn tools() -> Vec<AgentTool> {
    server::tool_declarations()
        .into_iter()
        .map(|t| AgentTool {
            name: t.name.to_string(),
            read_only: declares_read_only(&t),
            description: t.description.map(|d| d.to_string()).unwrap_or_default(),
            input_schema: serde_json::Value::Object((*t.input_schema).clone()),
        })
        .chain(app_only_tools())
        .collect()
}

/// Tools the in-app agent has and the MCP endpoint does not.
///
/// These write, and a write only happens after the user has approved that exact
/// change in the Argus window. The stdio server has no window to ask in, so it
/// is never told these exist — external clients see the read-only list only.
///
/// The schemas are written by hand rather than derived: `server`'s `#[tool]`
/// macro is what publishes a tool to the MCP router, and using it here would
/// hand the write tool to external clients too.
fn app_only_tools() -> Vec<AgentTool> {
    vec![AgentTool {
        name: app_tools::CREATE_NOTE_TOOL.to_string(),
        read_only: false,
        description: "Create a NEW note on a paper in the user's library, holding markdown you \
                      wrote — a summary, an outline, an answer worth keeping. The user is shown \
                      the exact title and body and must approve them before anything is written, \
                      so propose the note in your reply first and call this once they ask for it. \
                      This only ever adds a note: it cannot edit, overwrite or delete an existing \
                      one, so put everything you want kept in `content`. Use `find_papers` or \
                      `get_paper` to get the `slug`."
            .to_string(),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": {
                "slug": {
                    "type": "string",
                    "description": "Slug of the paper the note belongs to, from find_papers/get_paper.",
                },
                "title": {
                    "type": "string",
                    "description": "Short title for the note, shown in the paper's note list. One line.",
                },
                "content": {
                    "type": "string",
                    "description": "The note itself, in markdown. Written verbatim — include everything you want kept.",
                },
            },
            "required": ["slug", "title", "content"],
            "additionalProperties": false,
        }),
    }]
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

/// Run one **read-only** tool against `root`.
///
/// Writing tools are refused here on purpose. They go through
/// `app_tools::PendingWrite`, which the caller must first put in front of the
/// user; routing them here as well would make "forgot to ask" a one-line
/// mistake instead of an impossible one.
///
/// Blocking file and SQLite work, so callers must keep this off the async
/// runtime's worker threads — `copilot` wraps it in `spawn_blocking`.
pub fn call(root: &str, name: &str, args: &serde_json::Value) -> Result<serde_json::Value, String> {
    if app_tools::is_write_tool(name) {
        return Err(format!(
            "'{name}' changes the library and cannot be run here — it must go through \
             the user's confirmation."
        ));
    }
    match name {
        "find_papers" => {
            let p: server::FindPapersParams = params(args)?;
            ok(tools::find_papers(
                root,
                tools::FindPapersArgs {
                    query: p.query.as_deref(),
                    content: p.content.as_deref(),
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
        "get_paper_fulltext" => {
            let p: server::FulltextParams = params(args)?;
            ok(tools::get_paper_fulltext(root, &p.slug, p.offset, p.limit)?)
        }
        "get_paper_file_path" => {
            let p: server::SlugParams = params(args)?;
            ok(tools::get_document_path(root, &p.slug)?)
        }
        "view_paper_page" => {
            let p: server::ViewPageParams = params(args)?;
            let pages = tools::render_paper_pages(root, &p.slug, &p.pages)?;
            // Return each page's text plus its PNG as base64. `copilot` decodes
            // and saves the PNGs, keeps the base64 out of the token-costed tool
            // message, and (for a vision model) feeds the images back separately.
            ok(serde_json::json!({
                "slug": p.slug,
                "pages": pages
                    .iter()
                    .map(|r| serde_json::json!({
                        "page": r.page,
                        "text": r.text,
                        "png_base64": base64::engine::general_purpose::STANDARD.encode(&r.png),
                    }))
                    .collect::<Vec<_>>(),
            }))
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
        "search_snippets" => {
            let p: server::SnippetParams = params(args)?;
            ok(tools::search_snippets(
                root,
                p.library_id.as_deref(),
                p.query.as_deref(),
                p.limit.clamp(1, 500),
            )?)
        }
        "list_conversations" => {
            let p: server::ListConversationsParams = params(args)?;
            ok(tools::ItemList::from(tools::list_conversations(
                root,
                p.scope.as_str(),
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
            // Writing tools deliberately have no arm here — they run through
            // `app_tools::PendingWrite` after the user approves. The next test
            // checks they are refused rather than missing.
            if app_tools::is_write_tool(&tool.name) {
                continue;
            }
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

    /// The read-only dispatcher must never perform a write, whatever it is
    /// handed. This is the backstop for a caller that forgets to route a write
    /// through the confirmation.
    #[test]
    fn the_read_only_dispatcher_refuses_write_tools() {
        let err = call(
            "/nonexistent-library",
            app_tools::CREATE_NOTE_TOOL,
            &serde_json::json!({"slug": "s", "title": "t", "content": "c"}),
        )
        .unwrap_err();
        assert!(err.contains("confirmation"), "{err}");
    }

    /// The write tool exists for the in-app agent only. An external MCP client
    /// must not even see it declared: that process has no window in which the
    /// user could approve anything.
    #[test]
    fn no_write_tool_is_exposed_through_the_mcp_server() {
        for tool in server::tool_declarations() {
            assert!(
                !app_tools::is_write_tool(&tool.name),
                "tool '{}' writes but is declared to external MCP clients",
                tool.name
            );
            assert!(
                declares_read_only(&tool),
                "tool '{}' is served over MCP without a read_only_hint",
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
        let p: server::FindPapersParams = params(&serde_json::Value::Null).unwrap();
        assert_eq!(p.limit, 20, "the documented default page size must apply");
        let p: server::FindPapersParams = params(&serde_json::json!({})).unwrap();
        assert_eq!(p.limit, 20);
        assert!(p.query.is_none());

        // A populated object still deserializes normally.
        let p: server::FindPapersParams =
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
        let p: server::FindPapersParams = params(&empty).unwrap();
        assert_eq!(p.limit, 20);
        let p: server::SnippetParams = params(&empty).unwrap();
        assert_eq!(p.limit, 50);
        let p: server::ListConversationsParams =
            params(&serde_json::json!({"scope": "paper"})).unwrap();
        assert_eq!(p.limit, 30);
        assert_eq!(p.scope.as_str(), "paper");
        let p: server::GetConversationParams =
            params(&serde_json::json!({"conversation_id": "c1"})).unwrap();
        assert_eq!(p.limit, 20);
    }

    /// A tool whose required argument is missing should say so, rather than
    /// silently searching for the empty string.
    #[test]
    fn a_missing_required_argument_is_an_error() {
        let err = params::<server::ListConversationsParams>(&serde_json::json!({})).unwrap_err();
        assert!(err.contains("scope"), "{err}");
        let err = params::<server::SlugParams>(&serde_json::Value::Null).unwrap_err();
        assert!(err.contains("slug"), "{err}");
    }

    /// Every tool states honestly whether it changes anything: the settings tab
    /// labels tools from this flag, and the agent loop uses it to decide what
    /// needs the user's approval. A read tool that forgot its annotation would
    /// be shown as a writer; a write tool claiming to be read-only would skip
    /// the confirmation entirely.
    #[test]
    fn every_tool_states_whether_it_writes() {
        for tool in tools() {
            assert_eq!(
                tool.read_only,
                !app_tools::is_write_tool(&tool.name),
                "tool '{}' misreports whether it writes",
                tool.name
            );
        }
    }

    /// The write tool has to be usable by a model on its own terms: named the
    /// same everywhere, and with a schema that spells out all three arguments.
    #[test]
    fn the_write_tool_is_declared_with_a_complete_schema() {
        let tool = tools()
            .into_iter()
            .find(|t| t.name == app_tools::CREATE_NOTE_TOOL)
            .expect("the create-note tool is declared to the in-app agent");
        assert!(!tool.read_only);
        let props = tool.input_schema["properties"]
            .as_object()
            .expect("object schema");
        for field in ["slug", "title", "content"] {
            assert!(props.contains_key(field), "schema is missing '{field}'");
        }
        let required: Vec<&str> = tool.input_schema["required"]
            .as_array()
            .expect("required list")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, ["slug", "title", "content"]);
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
