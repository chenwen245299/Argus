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
//! The only editable fields in a marking preview are the colors: choices from
//! the user's approval are validated and applied to the captured passages;
//! paper identity, excerpts, comments, pages and geometry cannot be changed.

use serde::{Deserialize, Serialize};

use crate::paper;
use super::passage_anchors::{locate_anchor, locate_passage};

/// The write tool's name, shared by the declaration, the dispatcher and the
/// confirmation UI so they cannot drift apart.
pub const CREATE_NOTE_TOOL: &str = "create_paper_note";
pub const CREATE_HIGHLIGHTS_TOOL: &str = "create_paper_highlights";
pub const CREATE_ANNOTATIONS_TOOL: &str = "create_paper_annotations";

/// Whether `name` is a tool that changes the library.
///
/// The read-only dispatcher (`super::agent::call`) refuses these, so forgetting
/// to ask the user is not a mistake that can be made quietly — the call fails
/// instead of writing. Covers the canvas-editing tool too (see
/// [`crate::canvas_edit`]): it never runs read-only either, and lives behind the
/// same confirmation.
pub fn is_write_tool(name: &str) -> bool {
    matches!(name, CREATE_NOTE_TOOL | CREATE_HIGHLIGHTS_TOOL | CREATE_ANNOTATIONS_TOOL)
        || super::arxiv_tools::is_write(name)
        || crate::canvas_edit::is_edit_tool(name)
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
#[derive(Debug, Clone)]
pub enum PendingWrite {
    Arxiv(super::arxiv_tools::PendingArxivWrite),
    CreateNote {
        slug: String,
        /// Resolved from the library at parse time, for the dialog. The user
        /// approves "a note on *this paper*", not on a slug they cannot read.
        paper_title: String,
        title: String,
        content: String,
    },
    MarkPassages {
        tool: String,
        slug: String,
        paper_id: String,
        paper_title: String,
        pdf_path: std::path::PathBuf,
        revision: crate::render::PdfRevision,
        highlights: Vec<crate::models::Highlight>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passage {
    pub page: u32,
    #[serde(default)]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_anchor: Option<String>,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_style")]
    pub style: String,
    pub note: Option<String>,
}
fn default_color() -> String { "#FFEB3B".into() }
fn default_style() -> String { "highlight".into() }

/// Six palette colors plus red; custom RGB colors remain supported.
pub fn normalize_mark_color(color: &str) -> Result<String, String> {
    let color = color.trim().to_lowercase();
    let named = match color.as_str() {
        "yellow" | "黄色" => Some("#FFEB3B"),
        "green" | "绿色" => Some("#A5D6A7"),
        "blue" | "蓝色" => Some("#90CAF9"),
        "pink" | "粉色" => Some("#F48FB1"),
        "orange" | "橙色" => Some("#FFCC80"),
        "purple" | "紫色" => Some("#CE93D8"),
        "red" | "红色" => Some("#FF0000"),
        _ => None,
    };
    if let Some(hex) = named { return Ok(hex.into()); }
    if color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|c| c.is_ascii_hexdigit()) {
        return Ok(color.to_ascii_uppercase());
    }
    Err("`color` must be yellow, green, blue, pink, orange, purple, red, or #RRGGBB.".into())
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub passages: Vec<Passage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub recommendations: Vec<super::arxiv_tools::RecommendationPreview>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub keep_duplicate_copies: bool,
}

impl PendingWrite {
    /// Validate a tool call into the write it describes.
    ///
    /// Errors here are handed back to the model as an ordinary failed tool call:
    /// it can fix the arguments and try again, and the user is never shown a
    /// dialog for a call that was malformed.
    pub fn parse(root: &str, name: &str, args: &serde_json::Value) -> Result<Self, String> {
        if super::arxiv_tools::is_write(name) {
            return super::arxiv_tools::PendingArxivWrite::parse(root, name, args).map(Self::Arxiv);
        }
        match name {
            CREATE_HIGHLIGHTS_TOOL | CREATE_ANNOTATIONS_TOOL => {
                let slug = str_arg(args, "slug")?;
                let batch_color = match args.get("color") {
                    Some(value) => normalize_mark_color(value.as_str().ok_or("`color` must be a string.")?)?,
                    None => default_color(),
                };
                let passages = args.get("passages").and_then(|v| v.as_array())
                    .ok_or("`passages` must be an array.")?;
                if passages.is_empty() || passages.len() > 30 {
                    return Err("Provide 1–30 passages in one call.".into());
                }
                let meta = paper::read_meta(root, &slug)?;
                if meta.file_type.as_deref().is_some_and(|t| t != "pdf") {
                    return Err("These tools currently mark PDF passages only.".into());
                }
                let pdf_path = crate::metadata::find_pdf_in_dir(root, &slug);
                let revision = crate::render::pdf_revision(&pdf_path)?;
                let units = crate::render::pdf_page_units(&pdf_path)?;
                let mut pages = std::collections::HashMap::new();
                let mut highlights = Vec::new();
                let mut failures = Vec::new();
                let mut valid = Vec::new();
                for (index, raw) in passages.iter().enumerate() {
                    let result = (|| -> Result<crate::models::Highlight, String> {
                        let mut raw = raw.clone();
                        let object = raw.as_object_mut().ok_or("Each passage must be an object.")?;
                        if !object.contains_key("color") { object.insert("color".into(), batch_color.clone().into()); }
                        let mut p: Passage = serde_json::from_value(raw).map_err(|e| format!("Invalid passage: {e}"))?;
                        p.color = normalize_mark_color(&p.color)?;
                        if p.page == 0 || p.text.chars().count() > 10_000
                            || (p.text.trim().is_empty() && p.anchor.is_none()) {
                            return Err("Each passage needs a 1-based PDF page and either original text (1–10,000 characters) or an anchor from view_paper_page.".into());
                        }
                        if p.end_anchor.is_some() && p.anchor.is_none() {
                            return Err("end_anchor requires anchor on the same page.".into());
                        }
                        if !["highlight", "underline"].contains(&p.style.as_str()) {
                            return Err("`style` must be highlight or underline.".into());
                        }
                        if p.note.as_ref().is_some_and(|s| s.chars().count() > 10_000)
                            || (name == CREATE_ANNOTATIONS_TOOL && p.note.as_deref().unwrap_or("").trim().is_empty()) {
                            return Err("An annotation needs a nonempty `note` (maximum 10,000 characters).".into());
                        }
                        if name == CREATE_HIGHLIGHTS_TOOL && p.note.is_some() {
                            return Err("The highlight tool does not add comments. Use create_paper_annotations only when the user explicitly requests annotations.".into());
                        }
                        let glyphs = load_marking_page(&pdf_path, &units, &mut pages, p.page)?;
                        let (text, rects) = match p.anchor.as_deref() {
                            Some(anchor) => locate_anchor(p.page, glyphs, anchor, p.end_anchor.as_deref(), &p.text),
                            None => locate_passage(glyphs, &p.text),
                        }?;
                        Ok(crate::models::Highlight {
                            id: uuid::Uuid::new_v4().to_string(), page: p.page, text, rects,
                            color: p.color, style: p.style, note: p.note,
                            created_at: chrono::Utc::now().to_rfc3339(), updated_at: None,
                            start_offset: None, end_offset: None, anchor_prefix: None,
                            anchor_suffix: None, keep_line_breaks: None,
                        })
                    })();
                    match result {
                        Ok(highlight) => { highlights.push(highlight); valid.push(index + 1); }
                        Err(error) => {
                            let page = raw["page"].as_u64().and_then(|n| u32::try_from(n).ok());
                            let quote = raw["text"].as_str().unwrap_or("");
                            let mut hint = String::new();
                            // Diagnose off-by-one printed page labels, never move a mark silently.
                            if error.contains("Original text was not found") && !quote.trim().is_empty() {
                                if let Some(page) = page {
                                    for nearby in [page.checked_sub(1), page.checked_add(1)].into_iter().flatten() {
                                        if let Ok(glyphs) = load_marking_page(&pdf_path, &units, &mut pages, nearby) {
                                            if locate_passage(glyphs, quote).is_ok() {
                                                hint.push_str(&format!(" Exact text exists on PDF page {nearby}; inspect that page and correct `page` explicitly."));
                                            }
                                        }
                                    }
                                }
                            }
                            let excerpt = quote.chars().take(100).collect::<String>();
                            failures.push(format!("Passage #{} (PDF page {}, text {}): {error}{hint}",
                                index + 1, page.map_or("invalid".into(), |p| p.to_string()), serde_json::json!(excerpt)));
                        }
                    }
                }
                if !failures.is_empty() {
                    return Err(format!("Nothing was saved; the entire batch needs correction. Valid passage indices: {valid:?}. Keep those unchanged.\n{}\nFor missing/ambiguous/formula text, call view_paper_page for the failing PDF page and select its annotation_targets using `anchor` (and optional `end_anchor`); omit `text` to use the exact selected source lines. Never paraphrase the source or guess a target.", failures.join("\n")));
                }
                if crate::render::pdf_revision(&pdf_path)? != revision {
                    return Err("PDF changed while locating passages. Inspect it again and retry.".into());
                }
                Ok(Self::MarkPassages { tool: name.into(), slug, paper_id: meta.id,
                    paper_title: meta.title, pdf_path, revision, highlights })
            }
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
            Self::Arxiv(pending) => pending.preview(),
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
                passages: Vec::new(),
                recommendations: Vec::new(), keep_duplicate_copies: false,
            },
            Self::MarkPassages { tool, slug, paper_title, highlights, .. } => WritePreview {
                tool: tool.clone(), slug: slug.clone(), paper_title: paper_title.clone(),
                note_title: String::new(), content: String::new(), duplicate_title: false,
                recommendations: Vec::new(), keep_duplicate_copies: false,
                passages: highlights.iter().map(|h| Passage { page: h.page, text: h.text.clone(),
                    color: h.color.clone(), style: h.style.clone(), note: h.note.clone(),
                    anchor: None, end_anchor: None }).collect(),
            },
        }
    }

    /// Carry out the approved write.
    ///
    /// Blocking file IO — callers run it off the async runtime. Must only be
    /// reached after the user approved this exact value.
    pub fn execute(&self, root: &str) -> Result<serde_json::Value, String> {
        match self {
            Self::Arxiv(_) => Err("Recommendation writes require the approved async import/delete dispatcher.".into()),
            Self::MarkPassages { slug, paper_id, pdf_path, revision, highlights, .. } => {
                if paper::read_meta(root, slug)?.id != *paper_id
                    || crate::render::pdf_revision(pdf_path)? != *revision {
                    return Err("Paper or PDF changed since the preview. Nothing was saved; inspect it again.".into());
                }
                let saved = paper::append_highlights(root, slug, highlights.clone())?;
                Ok(serde_json::json!({"saved": true, "slug": slug, "highlights": saved}))
            }
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

    /// Only the user's reviewed colors may change after preview. Quote, page,
    /// comment, geometry and paper identity stay bound to the validated write.
    pub fn apply_passage_colors(&mut self, colors: Option<&[String]>) -> Result<(), String> {
        let Some(colors) = colors else { return Ok(()); };
        let Self::MarkPassages { highlights, .. } = self else {
            return Err("Color choices only apply to PDF marking.".into());
        };
        if colors.len() != highlights.len() { return Err("Color choices must match the previewed passages.".into()); }
        let normalized = colors.iter().map(|c| normalize_mark_color(c)).collect::<Result<Vec<_>, _>>()?;
        for (highlight, color) in highlights.iter_mut().zip(normalized) { highlight.color = color; }
        Ok(())
    }
}

fn load_marking_page<'a>(pdf: &std::path::Path, units: &std::collections::HashMap<u32, f32>,
    pages: &'a mut std::collections::HashMap<u32, Vec<super::passage_anchors::Glyph>>, page: u32)
    -> Result<&'a [super::passage_anchors::Glyph], String> {
    if let std::collections::hash_map::Entry::Vacant(entry) = pages.entry(page) {
        let unit = *units.get(&page).ok_or_else(|| format!("PDF page {page} does not exist."))?;
        entry.insert(crate::render::page_text_geometry(pdf, page, unit)?);
    }
    Ok(&pages[&page])
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

    fn mark_args(slug: &str, text: &str, note: Option<&str>) -> serde_json::Value {
        let mut passage = serde_json::json!({"page": 1, "text": text});
        if let Some(note) = note { passage["note"] = note.into(); }
        serde_json::json!({"slug": slug, "passages": [passage]})
    }

    #[test]
    fn marks_are_located_previewed_appended_and_retries_are_idempotent() {
        let root = library_with_paper("copy-a", "Identical title");
        let pdf = paper::paper_dir(&root, "copy-a").join("paper.pdf");
        crate::render::annotation_fixture(&pdf, "Alpha beta. A second sentence.", 0, 1.0, false);
        let args = mark_args("copy-a", "Alpha beta.", None);
        let pending = PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL, &args).unwrap();
        assert!(paper::read_highlights(&root, "copy-a").is_empty(), "preview must not write");
        let preview = pending.preview(&root);
        assert_eq!(preview.passages[0].text, "Alpha beta.");
        let saved = pending.execute(&root).unwrap();
        let first_id = saved["highlights"][0]["id"].clone();
        let retry = PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL, &args).unwrap().execute(&root).unwrap();
        assert_eq!(retry["highlights"][0]["id"], first_id);
        assert_eq!(paper::read_highlights(&root, "copy-a").len(), 1);
        assert!(!paper::read_highlights(&root, "copy-a")[0].rects.is_empty());

        let annotated = PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL,
            &mark_args("copy-a", "A second sentence.", Some("解释这句话"))).unwrap();
        assert_eq!(annotated.preview(&root).passages[0].note.as_deref(), Some("解释这句话"));
        annotated.execute(&root).unwrap();
        let marks = paper::read_highlights(&root, "copy-a");
        assert_eq!(marks.len(), 2);
        assert_eq!(marks[0].id, first_id.as_str().unwrap());
        assert_eq!(marks[1].note.as_deref(), Some("解释这句话"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn named_colors_match_the_reader_palette_and_custom_rgb_is_validated() {
        for (name, alias, hex) in [("yellow", "黄色", "#FFEB3B"), ("green", "绿色", "#A5D6A7"),
            ("blue", "蓝色", "#90CAF9"), ("pink", "粉色", "#F48FB1"),
            ("orange", "橙色", "#FFCC80"), ("purple", "紫色", "#CE93D8"), ("red", "红色", "#FF0000")] {
            assert_eq!(normalize_mark_color(name).unwrap(), hex);
            assert_eq!(normalize_mark_color(alias).unwrap(), hex);
        }
        assert_eq!(normalize_mark_color(" #12ab34 ").unwrap(), "#12AB34");
        for invalid in ["#abc", "#000000FF", "url(x)", "#12345g", "", "#中文"] {
            assert!(normalize_mark_color(invalid).is_err());
        }
    }

    #[test]
    fn both_marking_tools_inherit_batch_colors_and_save_only_reviewed_color_edits() {
        for tool in [CREATE_HIGHLIGHTS_TOOL, CREATE_ANNOTATIONS_TOOL] {
            let root = library_with_paper("colored-copy", "A paper");
            let pdf = paper::paper_dir(&root, "colored-copy").join("paper.pdf");
            crate::render::annotation_fixture(&pdf, "Alpha beta. A second sentence.", 0, 1.0, false);
            let mut args = serde_json::json!({"slug": "colored-copy", "color": "blue", "passages": [
                {"page": 1, "text": "Alpha beta."},
                {"page": 1, "text": "A second sentence.", "color": "粉色"}
            ]});
            if tool == CREATE_ANNOTATIONS_TOOL {
                for p in args["passages"].as_array_mut().unwrap() { p["note"] = "A comment".into(); }
            }
            let mut pending = PendingWrite::parse(&root, tool, &args).unwrap();
            let before = pending.preview(&root);
            assert_eq!(before.passages[0].color, "#90CAF9");
            assert_eq!(before.passages[1].color, "#F48FB1");
            assert!(pending.apply_passage_colors(Some(&["green".into()])).is_err());
            assert!(pending.apply_passage_colors(Some(&["green".into(), "invalid".into()])).is_err());
            assert_eq!(pending.preview(&root).passages[0].color, "#90CAF9", "failed changes are atomic");
            assert!(paper::read_highlights(&root, "colored-copy").is_empty());
            pending.apply_passage_colors(Some(&["green".into(), "#123abc".into()])).unwrap();
            let after = pending.preview(&root);
            for (old, new) in before.passages.iter().zip(&after.passages) {
                assert_eq!(old.text, new.text); assert_eq!(old.page, new.page); assert_eq!(old.note, new.note);
            }
            pending.execute(&root).unwrap();
            let saved = paper::read_highlights(&root, "colored-copy");
            assert_eq!(saved[0].color, "#A5D6A7");
            assert_eq!(saved[1].color, "#123ABC");
            assert_eq!(saved[0].note.as_deref(), if tool == CREATE_ANNOTATIONS_TOOL { Some("A comment") } else { None });
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn inspected_page_anchors_prepare_exact_preview_and_save_the_same_mark() {
        let root = library_with_paper("anchors", "Anchor fixture");
        let pdf = paper::paper_dir(&root, "anchors").join("paper.pdf");
        crate::render::annotation_fixture(&pdf, "Exact source sentence.", 0, 1.0, false);
        // Exercise the actual tool result the model receives, not a handmade ID.
        let view = super::super::agent::call(&root, "view_paper_page",
            &serde_json::json!({"slug":"anchors", "pages":[1]})).unwrap();
        let target = &view["pages"][0]["annotation_targets"][0];
        let mut args = serde_json::json!({"slug":"anchors", "color":"red", "passages":[{
            "page":1,"anchor":target["id"],"note":"解释原文"}]});
        let pending = PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL, &args).unwrap();
        let preview = pending.preview(&root);
        assert_eq!(preview.passages[0].text, target["text"].as_str().unwrap());
        assert_eq!(preview.passages[0].color, "#FF0000");
        assert!(paper::read_highlights(&root, "anchors").is_empty());
        pending.execute(&root).unwrap();
        let saved = paper::read_highlights(&root, "anchors");
        assert_eq!(saved[0].text, preview.passages[0].text);
        assert_eq!(saved[0].note.as_deref(), Some("解释原文"));
        args["passages"][0]["text"] = "Invented paraphrase".into();
        assert!(PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL, &args).is_err());
        args["passages"][0].as_object_mut().unwrap().remove("text");
        crate::render::annotation_fixture(&pdf, "Changed source sentence.", 0, 1.0, false);
        assert!(PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL, &args).unwrap_err().contains("stale anchor"));
        assert_eq!(paper::read_highlights(&root, "anchors").len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_batch_reports_each_failed_index_and_preserves_valid_items_without_writing() {
        let root = library_with_paper("batch", "Batch fixture");
        let pdf = paper::paper_dir(&root, "batch").join("paper.pdf");
        crate::render::annotation_fixture(&pdf, "A valid sentence.", 0, 1.0, false);
        let args = serde_json::json!({"slug":"batch","passages":[
            {"page":1,"text":"A valid sentence.","note":"good"},
            {"page":1,"text":"An invented sentence.","note":"bad"},
            {"page":1,"text":"A valid sentence.","color":"invalid","note":"bad color"},
            {"page":1,"anchor":"forged","note":"bad anchor"},
            {"page":1,"end_anchor":"orphan","text":"A valid sentence.","note":"missing start"}
        ]});
        let error = PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL, &args).unwrap_err();
        assert!(error.contains("Valid passage indices: [1]"), "{error}");
        for index in 2..=5 { assert!(error.contains(&format!("Passage #{index} (PDF page 1")), "{error}"); }
        assert!(error.contains("An invented sentence."));
        assert!(!error.contains("Passage #1 ("));
        assert!(paper::read_highlights(&root, "batch").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn wrong_page_is_diagnosed_but_never_silently_moved() {
        use lopdf::{dictionary, Object};
        let root = library_with_paper("pages", "Page fixture");
        let pdf = paper::paper_dir(&root, "pages").join("paper.pdf");
        crate::render::annotation_fixture(&pdf, "Source on PDF page one.", 0, 1.0, false);
        let mut doc = lopdf::Document::load(&pdf).unwrap();
        let first = doc.get_pages()[&1];
        let parent = doc.get_object(first).unwrap().as_dict().unwrap().get(b"Parent").unwrap().as_reference().unwrap();
        let mut second = doc.get_object(first).unwrap().as_dict().unwrap().clone();
        second.set("Contents", doc.add_object(lopdf::Stream::new(dictionary! {},
            b"BT /F1 12 Tf 100 700 Td (Different text on PDF page two.) Tj ET".to_vec())));
        let second = doc.add_object(second);
        let tree = doc.get_object_mut(parent).unwrap().as_dict_mut().unwrap();
        tree.set("Kids", vec![Object::Reference(first), Object::Reference(second)]);
        tree.set("Count", 2);
        doc.save(&pdf).unwrap();
        let mut args = mark_args("pages", "Source on PDF page one.", None);
        args["passages"][0]["page"] = 2.into();
        let error = PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL, &args).unwrap_err();
        assert!(error.contains("Passage #1 (PDF page 2"), "{error}");
        assert!(error.contains("Exact text exists on PDF page 1"), "{error}");
        assert!(paper::read_highlights(&root, "pages").is_empty());
        args["passages"][0]["page"] = 1.into();
        assert!(PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL, &args).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn marking_refuses_ambiguous_missing_text_and_changed_pdf_without_writing() {
        let root = library_with_paper("copy-b", "B");
        let pdf = paper::paper_dir(&root, "copy-b").join("paper.pdf");
        crate::render::annotation_fixture(&pdf, "Repeat. Repeat. Unique sentence.", 0, 1.0, false);
        for quote in ["Repeat.", "Invented text"] {
            assert!(PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL, &mark_args("copy-b", quote, None)).is_err());
        }
        assert!(PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL,
            &mark_args("copy-b", "Unique sentence.", None)).is_err());
        assert!(PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL,
            &mark_args("copy-b", "Unique sentence.", Some("unsolicited note"))).is_err());
        let pending = PendingWrite::parse(&root, CREATE_HIGHLIGHTS_TOOL,
            &mark_args("copy-b", "Unique sentence.", None)).unwrap();
        crate::render::annotation_fixture(&pdf, "Different text.", 0, 1.0, false);
        assert!(pending.execute(&root).unwrap_err().contains("changed"));
        assert!(paper::read_highlights(&root, "copy-b").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn passage_matching_preserves_geometry_across_wrapping_ligatures_and_cjk() {
        let text = "An efﬁcient optimi-\nzation. 中文\n段落。";
        let glyphs = text.chars().enumerate().map(|(i, c)| (c, Some(crate::models::Rect {
            x: i as f32 * 8.0, y: 10.0, width: 7.0, height: 12.0,
        }))).collect::<Vec<_>>();
        let (actual, rects) = locate_passage(&glyphs, "An efficient optimization.").unwrap();
        assert_eq!(actual, "An efﬁcient optimi-\nzation.");
        assert!(!rects.is_empty());
        assert_eq!(locate_passage(&glyphs, "中文段落。").unwrap().0, "中文\n段落。");
    }
}
