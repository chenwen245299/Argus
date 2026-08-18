//! The tools that write, available to the app's own agent and to nothing else.
//!
//! # Why they are not in `tools`
//!
//! `tools` is the read-only surface behind the MCP endpoint, and that module's
//! guarantee — nothing an external client reaches can change the library — is
//! worth keeping literally true. Everything here changes something, and every
//! change requires the user to approve *that exact write* in the Argus window
//! first.
//!
//! The stdio server cannot offer that. It runs in its own process, launched by
//! an external client, with no window to ask in and no way to know whether a
//! human is even at the keyboard. So these tools are never declared to it:
//! `super::server` has no idea this module exists, and an external client sees
//! only the read-only tool list.
//!
//! # The one invariant
//!
//! A tool call is parsed into a [`PendingWrite`] **once**. The confirmation the
//! user sees is rendered from that value, and the write is performed from that
//! same value — [`PendingWrite::execute`] never looks at the model's arguments
//! again. What the dialog showed is therefore what lands on disk, even if the
//! model's arguments were odd, ambiguous, or re-sent differently later.

use serde::Serialize;

use crate::paper;

/// The write tool's name, shared by the declaration, the dispatcher and the
/// confirmation UI so they cannot drift apart.
pub const CREATE_NOTE_TOOL: &str = "create_paper_note";

/// Whether `name` is a tool that changes the library.
///
/// The read-only dispatcher (`super::agent::call`) refuses these, so forgetting
/// to ask the user is not a mistake that can be made quietly — the call fails
/// instead of writing.
pub fn is_write_tool(name: &str) -> bool {
    name == CREATE_NOTE_TOOL
}

/// Longest note title accepted. Titles show up in lists and tabs; a model that
/// puts a paragraph in the title field gets told, not obeyed.
const MAX_TITLE_CHARS: usize = 120;

/// Longest note body accepted. Well past any real note, far short of anything
/// that would make the confirmation dialog useless to read.
const MAX_CONTENT_CHARS: usize = 50_000;

/// A write that has been validated and is waiting for the user's approval.
///
/// Carries everything the write needs, so nothing has to be re-derived after the
/// user says yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingWrite {
    CreateNote {
        slug: String,
        /// Resolved from the library at parse time, for the dialog. The user
        /// approves "a note on *this paper*", not on a slug they cannot read.
        paper_title: String,
        title: String,
        content: String,
    },
}

/// What the confirmation dialog renders.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WritePreview {
    /// Which tool is asking, so the dialog can title itself.
    pub tool: String,
    pub slug: String,
    pub paper_title: String,
    pub note_title: String,
    /// Markdown, rendered read-only in the dialog.
    pub content: String,
    /// Whether an existing note carries the same title. Nothing is overwritten
    /// either way — a new note is always created — but the user deserves to know
    /// they are about to have two notes with one name.
    pub duplicate_title: bool,
}

impl PendingWrite {
    /// Validate a tool call into the write it describes.
    ///
    /// Errors here are handed back to the model as an ordinary failed tool call:
    /// it can fix the arguments and try again, and the user is never shown a
    /// dialog for a call that was malformed.
    pub fn parse(root: &str, name: &str, args: &serde_json::Value) -> Result<Self, String> {
        match name {
            CREATE_NOTE_TOOL => {
                let slug = str_arg(args, "slug")?;
                let title = str_arg(args, "title")?;
                let content = str_arg(args, "content")?;

                // A title lands in index.json and in the UI, never in a path
                // (note files are named after a fresh uuid), but newlines would
                // still wreck every list it appears in.
                let title = title.replace(['\r', '\n', '\t'], " ").trim().to_string();
                if title.is_empty() {
                    return Err("`title` must not be empty.".to_string());
                }
                if title.chars().count() > MAX_TITLE_CHARS {
                    return Err(format!(
                        "`title` is too long ({} characters, limit {MAX_TITLE_CHARS}). \
                         Put the detail in `content`.",
                        title.chars().count()
                    ));
                }
                if content.trim().is_empty() {
                    return Err("`content` must not be empty.".to_string());
                }
                if content.chars().count() > MAX_CONTENT_CHARS {
                    return Err(format!(
                        "`content` is too long ({} characters, limit {MAX_CONTENT_CHARS}). \
                         Write a shorter note.",
                        content.chars().count()
                    ));
                }

                // Resolving the paper now serves two purposes: an unknown slug
                // fails before the user is interrupted, and the dialog can name
                // the paper the note would land on.
                let meta = paper::read_meta(root, &slug)
                    .map_err(|_| format!("Paper '{slug}' was not found in this library."))?;

                Ok(PendingWrite::CreateNote {
                    slug,
                    paper_title: meta.title,
                    title,
                    content,
                })
            }
            other => Err(format!("unknown write tool: {other}")),
        }
    }

    /// What to show the user. Derived from `self`, never from the raw arguments.
    pub fn preview(&self, root: &str) -> WritePreview {
        match self {
            PendingWrite::CreateNote {
                slug,
                paper_title,
                title,
                content,
            } => WritePreview {
                tool: CREATE_NOTE_TOOL.to_string(),
                slug: slug.clone(),
                paper_title: paper_title.clone(),
                note_title: title.clone(),
                content: content.clone(),
                duplicate_title: paper::list_notes(root, slug).iter().any(|n| &n.title == title),
            },
        }
    }

    /// Carry out the approved write.
    ///
    /// Blocking file IO — callers run it off the async runtime. Must only be
    /// reached after the user approved this exact value.
    pub fn execute(&self, root: &str) -> Result<serde_json::Value, String> {
        match self {
            PendingWrite::CreateNote {
                slug,
                title,
                content,
                ..
            } => {
                let note = paper::create_note_with(root, slug, title, content)?;
                Ok(serde_json::json!({
                    "created": true,
                    "slug": slug,
                    "note_id": note.id,
                    "title": note.title,
                    "characters": content.chars().count(),
                }))
            }
        }
    }
}

fn str_arg(args: &serde_json::Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("`{key}` is required and must be a string."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(slug: &str, title: &str, content: &str) -> serde_json::Value {
        serde_json::json!({ "slug": slug, "title": title, "content": content })
    }

    /// A throwaway library holding one paper, matching how the rest of the
    /// crate builds fixtures (no tempfile dependency).
    fn library_with_paper(slug: &str, title: &str) -> String {
        let root = std::env::temp_dir()
            .join(format!("argus-app-tools-{}", uuid::Uuid::new_v4().simple()));
        let dir = root.join("papers").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            serde_json::json!({
                "id": slug,
                "title": title,
                "authors": [],
                "added_at": "2026-01-01T00:00:00Z",
                "reading_status": "unread",
            })
            .to_string(),
        )
        .unwrap();
        root.to_string_lossy().to_string()
    }

    #[test]
    fn a_missing_argument_is_refused_before_the_user_is_asked() {
        let err = PendingWrite::parse("/tmp", CREATE_NOTE_TOOL, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("slug"), "{err}");
    }

    #[test]
    fn empty_title_or_content_is_refused() {
        let err = PendingWrite::parse("/tmp", CREATE_NOTE_TOOL, &args("s", "  ", "body")).unwrap_err();
        assert!(err.contains("title"), "{err}");
        let err = PendingWrite::parse("/tmp", CREATE_NOTE_TOOL, &args("s", "t", "   ")).unwrap_err();
        assert!(err.contains("content"), "{err}");
    }

    #[test]
    fn oversized_input_is_refused() {
        let long = "x".repeat(MAX_TITLE_CHARS + 1);
        let err = PendingWrite::parse("/tmp", CREATE_NOTE_TOOL, &args("s", &long, "body")).unwrap_err();
        assert!(err.contains("too long"), "{err}");
        let long = "x".repeat(MAX_CONTENT_CHARS + 1);
        let err = PendingWrite::parse("/tmp", CREATE_NOTE_TOOL, &args("s", "t", &long)).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    /// A slug that does not resolve must fail at parse time, so no dialog is
    /// ever shown for a write that could not have happened.
    #[test]
    fn an_unknown_paper_is_refused_at_parse_time() {
        let err = PendingWrite::parse(
            "/nonexistent-library",
            CREATE_NOTE_TOOL,
            &args("nope", "t", "body"),
        )
        .unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn an_unknown_tool_name_is_refused() {
        let err = PendingWrite::parse("/tmp", "delete_everything", &serde_json::json!({}))
            .unwrap_err();
        assert!(err.contains("unknown write tool"), "{err}");
    }

    /// The write goes through `create_note_with`, which appends. The property
    /// under test is that an existing note with the same title survives: the
    /// agent can only ever add.
    #[test]
    fn creating_a_note_never_touches_an_existing_one() {
        let slug = "paper-a";
        let root = library_with_paper(slug, "A Paper");

        let existing = paper::create_note_with(&root, slug, "Shared title", "user's own words")
            .expect("seed note");

        let pending = PendingWrite::parse(&root, CREATE_NOTE_TOOL, &args(slug, "Shared title", "model text"))
            .expect("parses");
        // The dialog warns about the clash rather than hiding it.
        assert!(pending.preview(&root).duplicate_title);
        pending.execute(&root).expect("write");

        let notes = paper::list_notes(&root, slug);
        assert_eq!(notes.len(), 2, "a second note is added, not merged");
        assert_eq!(
            paper::get_note(&root, slug, &existing.id),
            "user's own words",
            "the note that was already there is untouched"
        );
    }

    /// What the dialog shows and what gets written come from one value, so they
    /// cannot disagree.
    #[test]
    fn the_preview_matches_what_is_written() {
        let slug = "paper-b";
        let root = library_with_paper(slug, "Paper B");

        let pending = PendingWrite::parse(
            &root,
            CREATE_NOTE_TOOL,
            &args(slug, "  Method\nnotes  ", "# Heading\n\nbody"),
        )
        .expect("parses");
        let preview = pending.preview(&root);
        assert_eq!(preview.note_title, "Method notes", "newlines are flattened");
        assert_eq!(preview.paper_title, "Paper B");
        assert!(!preview.duplicate_title);

        let result = pending.execute(&root).expect("write");
        let note_id = result["note_id"].as_str().unwrap();
        assert_eq!(paper::get_note(&root, slug, note_id), preview.content);
        assert_eq!(
            paper::list_notes(&root, slug)[0].title,
            preview.note_title,
            "the title written is the title shown"
        );
    }
}
