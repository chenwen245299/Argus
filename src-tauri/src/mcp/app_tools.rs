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
//! the user's approval are validated and applied to the captured passages (or,
//! for a change to existing marks, to their new color); paper identity,
//! excerpts, comments, pages and geometry cannot be changed.
//!
//! # Changing marks that already exist
//!
//! `update_paper_highlights` recolors, restyles or rewrites the note of existing
//! marks — the user's too, since they approve each change with its before and
//! after. `remove_paper_highlights` retracts only marks the agent created and
//! nobody has edited since ([`MarkOrigin::Ai`]); the user's own marks cannot be
//! deleted through it at all. Both address a whole selection: a highlight that
//! crosses a page break is several records (see `highlight_groups`), and every
//! one of them changes together, with one shared stamp. The records exactly as
//! previewed are captured, and the write refuses if any changed meanwhile.

use serde::{Deserialize, Serialize};

use crate::highlight_groups::{collapse_highlights, MarkOrigin};
use crate::models::{AgentMark, Highlight, HighlightTombstone};
use crate::paper;
use super::passage_anchors::{locate_anchor, locate_passage};

/// The write tool's name, shared by the declaration, the dispatcher and the
/// confirmation UI so they cannot drift apart.
pub const CREATE_NOTE_TOOL: &str = "create_paper_note";
pub const CREATE_HIGHLIGHTS_TOOL: &str = "create_paper_highlights";
pub const CREATE_ANNOTATIONS_TOOL: &str = "create_paper_annotations";
pub const UPDATE_MARKS_TOOL: &str = "update_paper_highlights";
pub const REMOVE_MARKS_TOOL: &str = "remove_paper_highlights";

/// Whether `name` is a tool that changes the library.
///
/// The read-only dispatcher (`super::agent::call`) refuses these, so forgetting
/// to ask the user is not a mistake that can be made quietly — the call fails
/// instead of writing. Covers the canvas-editing tool too (see
/// [`crate::canvas_edit`]): it never runs read-only either, and lives behind the
/// same confirmation.
pub fn is_write_tool(name: &str) -> bool {
    matches!(name, CREATE_NOTE_TOOL | CREATE_HIGHLIGHTS_TOOL | CREATE_ANNOTATIONS_TOOL
        | UPDATE_MARKS_TOOL | REMOVE_MARKS_TOOL)
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
    /// Changes to existing marks, one entry per selection. An entry without
    /// `after` retracts that mark.
    ChangeMarks {
        tool: String,
        slug: String,
        paper_id: String,
        paper_title: String,
        changes: Vec<MarkChange>,
    },
}

/// One existing mark as the confirmation shows it: before and after.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkChange {
    /// The selection's canonical id, as `get_highlights` reports it.
    pub id: String,
    pub page: u32,
    /// Last page of a selection across a page break.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_end: Option<u32>,
    /// The display text (one paragraph unless the user keeps line breaks).
    pub text: String,
    pub origin: MarkOrigin,
    pub before: MarkState,
    /// What the mark becomes; absent when it is retracted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<MarkState>,
    /// Every record of the selection exactly as previewed. Nothing is written
    /// if any of them changed or vanished before the approval arrived.
    #[serde(skip)]
    members: Vec<Highlight>,
}

/// The parts of a mark the agent may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MarkState {
    pub color: String,
    pub style: String,
    pub note: Option<String>,
}

/// One entry of `update_paper_highlights`; an omitted field stays as it is.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MarkEditArgs {
    id: String,
    color: Option<String>,
    style: Option<String>,
    note: Option<String>,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub changes: Vec<MarkChange>,
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
                        let created_at = chrono::Utc::now().to_rfc3339();
                        Ok(crate::models::Highlight {
                            id: uuid::Uuid::new_v4().to_string(), page: p.page, text, rects,
                            color: p.color, style: p.style, note: p.note,
                            agent: Some(AgentMark { revision: created_at.clone() }),
                            created_at, updated_at: None,
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
            UPDATE_MARKS_TOOL | REMOVE_MARKS_TOOL => parse_mark_changes(root, name, args),
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
                recommendations: Vec::new(), keep_duplicate_copies: false, changes: Vec::new(),
            },
            Self::MarkPassages { tool, slug, paper_title, highlights, .. } => WritePreview {
                tool: tool.clone(), slug: slug.clone(), paper_title: paper_title.clone(),
                note_title: String::new(), content: String::new(), duplicate_title: false,
                recommendations: Vec::new(), keep_duplicate_copies: false, changes: Vec::new(),
                passages: highlights.iter().map(|h| Passage { page: h.page, text: h.text.clone(),
                    color: h.color.clone(), style: h.style.clone(), note: h.note.clone(),
                    anchor: None, end_anchor: None }).collect(),
            },
            Self::ChangeMarks { tool, slug, paper_title, changes, .. } => WritePreview {
                tool: tool.clone(), slug: slug.clone(), paper_title: paper_title.clone(),
                note_title: String::new(), content: String::new(), duplicate_title: false,
                recommendations: Vec::new(), keep_duplicate_copies: false,
                passages: Vec::new(), changes: changes.clone(),
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
            Self::ChangeMarks { slug, paper_id, changes, .. } => {
                if paper::read_meta(root, slug)?.id != *paper_id {
                    return Err("Paper changed since the preview. Nothing was saved; inspect it again.".into());
                }
                execute_mark_changes(root, slug, changes)
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
        let normalized = colors.iter().map(|c| normalize_mark_color(c)).collect::<Result<Vec<_>, _>>()?;
        match self {
            Self::MarkPassages { highlights, .. } => {
                if colors.len() != highlights.len() { return Err("Color choices must match the previewed passages.".into()); }
                for (highlight, color) in highlights.iter_mut().zip(normalized) { highlight.color = color; }
            }
            // A retraction has no color to choose; only an update's new color.
            Self::ChangeMarks { changes, .. } if changes.iter().all(|c| c.after.is_some()) => {
                if colors.len() != changes.len() { return Err("Color choices must match the previewed marks.".into()); }
                for (change, color) in changes.iter_mut().zip(normalized) {
                    if let Some(after) = change.after.as_mut() { after.color = color; }
                }
            }
            _ => return Err("Color choices only apply to PDF marking.".into()),
        }
        Ok(())
    }
}

/// Most marks one change call may touch, like a creation batch.
const MAX_MARK_CHANGES: usize = 30;

/// Validate `update_paper_highlights` / `remove_paper_highlights` against the
/// marks on disk now. Every entry is checked and every failure reported, so
/// the model fixes the batch in one round; nothing is shown to the user
/// until all of it is valid.
fn parse_mark_changes(root: &str, name: &str, args: &serde_json::Value) -> Result<PendingWrite, String> {
    let remove = name == REMOVE_MARKS_TOOL;
    let slug = str_arg(args, "slug")?;
    let key = if remove { "ids" } else { "changes" };
    let items = args.get(key).and_then(|v| v.as_array())
        .ok_or_else(|| format!("`{key}` must be an array."))?;
    if items.is_empty() || items.len() > MAX_MARK_CHANGES {
        return Err(format!("Provide 1–{MAX_MARK_CHANGES} {key} in one call."));
    }
    let meta = paper::read_meta(root, &slug)?;
    if meta.file_type.as_deref().is_some_and(|t| t != "pdf") {
        return Err("These tools currently change PDF marks only.".into());
    }
    let records = paper::read_highlights(root, &slug);
    let groups = collapse_highlights(records.clone());
    let mut seen = std::collections::HashSet::new();
    let (mut changes, mut failures, mut valid) = (Vec::new(), Vec::new(), Vec::new());
    for (index, raw) in items.iter().enumerate() {
        let id = if remove { raw.as_str() } else { raw.get("id").and_then(|v| v.as_str()) };
        let result = (|| -> Result<MarkChange, String> {
            let edit = if remove {
                None
            } else {
                Some(serde_json::from_value::<MarkEditArgs>(raw.clone()).map_err(|e| format!("Invalid change: {e}"))?)
            };
            let id = edit.as_ref().map_or(id, |e| Some(e.id.as_str()))
                .ok_or("Each entry needs a highlight `id` string from get_highlights.")?;
            // Any member id names the whole selection.
            let group = groups.iter().find(|g| g.member_ids.iter().any(|m| m == id))
                .ok_or("No highlight with this id exists on this paper; call get_highlights for current ids.")?;
            let before = MarkState { color: group.rep.color.clone(), style: group.rep.style.clone(), note: group.rep.note.clone() };
            let after = match edit {
                None => match group.origin {
                    MarkOrigin::Ai => None,
                    MarkOrigin::AiUserEdited => return Err("The user edited this mark after you created it, so it is theirs; do not remove it. Explain in the conversation instead.".into()),
                    MarkOrigin::User => return Err("This is the user's own mark (or its origin is unknown). Only marks you created can be removed; never delete the user's marks. Explain in the conversation instead.".into()),
                },
                Some(edit) => {
                    if edit.color.is_none() && edit.style.is_none() && edit.note.is_none() {
                        return Err("Give at least one of `color`, `style` or `note` to change.".into());
                    }
                    let mut after = before.clone();
                    if let Some(color) = edit.color { after.color = normalize_mark_color(&color)?; }
                    if let Some(style) = edit.style {
                        if !["highlight", "underline"].contains(&style.as_str()) {
                            return Err("`style` must be highlight or underline.".into());
                        }
                        after.style = style;
                    }
                    if let Some(note) = edit.note {
                        if note.chars().count() > 10_000 {
                            return Err("`note` exceeds 10,000 characters.".into());
                        }
                        // An empty note removes the comment and keeps the mark.
                        after.note = (!note.trim().is_empty()).then_some(note);
                    }
                    if after == before {
                        return Err("This mark already has these values; leave it out.".into());
                    }
                    Some(after)
                }
            };
            if !seen.insert(group.rep.id.clone()) {
                return Err("This highlight is already listed in this call (a cross-page highlight is one entry).".into());
            }
            let members = group.member_ids.iter()
                .filter_map(|m| records.iter().find(|r| &r.id == m).cloned()).collect();
            Ok(MarkChange { id: group.rep.id.clone(), page: group.rep.page, page_end: group.page_end,
                text: group.display_text(), origin: group.origin, before, after, members })
        })();
        match result {
            Ok(change) => { changes.push(change); valid.push(index + 1); }
            Err(error) => failures.push(format!("Entry #{} (id {}): {error}", index + 1,
                serde_json::json!(id.unwrap_or("missing")))),
        }
    }
    if !failures.is_empty() {
        return Err(format!("Nothing was changed; the entire batch needs correction. Valid entry indices: {valid:?}. Keep those unchanged.\n{}", failures.join("\n")));
    }
    Ok(PendingWrite::ChangeMarks { tool: name.into(), slug, paper_id: meta.id, paper_title: meta.title, changes })
}

/// Apply approved changes under the highlights lock. The selection must still
/// be exactly the records the user reviewed; a mark edited, deleted or
/// regrouped meanwhile fails the whole batch rather than being half applied.
fn execute_mark_changes(root: &str, slug: &str, changes: &[MarkChange]) -> Result<serde_json::Value, String> {
    let same = |a: &Highlight, b: &Highlight| serde_json::to_value(a).ok() == serde_json::to_value(b).ok();
    paper::change_highlights(root, slug, |current| {
        let groups = collapse_highlights(current.to_vec());
        let (mut records, mut tombstones) = (Vec::new(), Vec::new());
        let (mut updated, mut removed, mut unchanged) = (Vec::new(), Vec::new(), Vec::new());
        for change in changes {
            let group = groups.iter().find(|g| g.rep.id == change.id).filter(|g|
                g.member_ids.len() == change.members.len()
                    && change.members.iter().all(|m| current.iter().any(|c| c.id == m.id && same(c, m))));
            let Some(group) = group else {
                return Err(format!("The highlight {} changed since the preview. Nothing was saved; call get_highlights and try again.",
                    serde_json::json!(change.id)));
            };
            // Newer than every record it replaces, so the change wins the
            // newest-edit merge; one stamp for the whole selection.
            let stamp = stamp_after(change.members.iter().map(paper::highlight_time));
            match &change.after {
                None => {
                    if group.origin != MarkOrigin::Ai {
                        return Err("Only marks the AI created and nobody edited can be removed. Nothing was saved.".into());
                    }
                    tombstones.extend(change.members.iter().map(|m| HighlightTombstone { id: m.id.clone(), deleted_at: stamp.clone() }));
                    removed.push(serde_json::json!({"id": change.id, "page": change.page}));
                }
                // The user picked the old color back in the card.
                Some(after) if *after == change.before => unchanged.push(change.id.clone()),
                Some(after) => {
                    for member in &change.members {
                        let mut h = member.clone();
                        h.color = after.color.clone();
                        h.style = after.style.clone();
                        h.note = after.note.clone();
                        // What the group shows, written to every member, as the
                        // reader's own group edits do (`groupAppearance`).
                        h.keep_line_breaks = group.rep.keep_line_breaks;
                        h.updated_at = Some(stamp.clone());
                        // An untouched agent mark stays retractable; a user's
                        // mark does not become the agent's by being changed.
                        if group.origin == MarkOrigin::Ai {
                            h.agent = Some(AgentMark { revision: stamp.clone() });
                        }
                        records.push(h);
                    }
                    updated.push(serde_json::json!({"id": change.id, "page": change.page,
                        "color": after.color, "style": after.style, "has_note": after.note.is_some()}));
                }
            }
        }
        let result = serde_json::json!({"saved": true, "slug": slug, "updated": updated,
            "removed": removed, "unchanged": unchanged, "highlights": records, "tombstones": tombstones});
        Ok((records, tombstones, result))
    })
}

/// A timestamp in the frontend's `toISOString` form, strictly later than every
/// edit time given — so a change wins the newest-edit merge even against a
/// clock that ran ahead on another machine.
fn stamp_after<'a>(times: impl IntoIterator<Item = &'a str>) -> String {
    let mut stamp = chrono::Utc::now();
    for time in times {
        if let Ok(time) = chrono::DateTime::parse_from_rfc3339(time) {
            let next = time.with_timezone(&chrono::Utc) + chrono::Duration::milliseconds(1);
            if next > stamp { stamp = next; }
        }
    }
    stamp.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
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

    fn stored_mark(id: &str, page: u32, text: &str, agent: bool) -> Highlight {
        let created_at = "2026-10-01T08:00:00.000Z".to_string();
        Highlight {
            id: id.into(), page, text: text.into(), rects: Vec::new(), color: "#FFEB3B".into(),
            style: "highlight".into(), note: None, updated_at: None,
            agent: agent.then(|| AgentMark { revision: created_at.clone() }), created_at,
            start_offset: None, end_offset: None, anchor_prefix: None, anchor_suffix: None,
            keep_line_breaks: None,
        }
    }

    fn library_with_marks(slug: &str, marks: Vec<Highlight>) -> String {
        let root = library_with_paper(slug, "Marked paper");
        paper::save_highlights_merged(&root, slug, marks, Vec::new()).unwrap();
        root
    }

    /// Records with the same page and creation time come back in no fixed order.
    fn stored(root: &str, slug: &str, id: &str) -> Highlight {
        paper::read_highlights(root, slug).into_iter().find(|h| h.id == id).unwrap()
    }

    fn origins(root: &str, slug: &str) -> Vec<(String, String)> {
        let mut list: Vec<_> = super::super::tools::get_highlights(root, slug).unwrap()
            .into_iter().map(|h| (h.id, h.origin)).collect();
        list.sort();
        list
    }

    #[test]
    fn agent_marks_carry_provenance_and_can_be_recolored_revised_and_retracted() {
        let root = library_with_paper("guided", "Guided paper");
        let pdf = paper::paper_dir(&root, "guided").join("paper.pdf");
        crate::render::annotation_fixture(&pdf, "Alpha beta. A second sentence.", 0, 1.0, false);
        PendingWrite::parse(&root, CREATE_ANNOTATIONS_TOOL,
            &mark_args("guided", "Alpha beta.", Some("Yellow method note"))).unwrap().execute(&root).unwrap();
        let created = paper::read_highlights(&root, "guided").remove(0);
        assert_eq!(created.agent.as_ref().unwrap().revision, created.created_at, "the agent's own write is its revision");
        let id = created.id.clone();
        assert_eq!(origins(&root, "guided"), vec![(id.clone(), "ai".into())]);

        // The user designates it an innovation and the annotation gains an example.
        let args = serde_json::json!({"slug": "guided", "changes": [{"id": id, "color": "purple",
            "note": "Yellow method note\n\nExample: $x_1 = 2$."}]});
        let pending = PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &args).unwrap();
        let preview = serde_json::to_value(pending.preview(&root)).unwrap();
        let change = &preview["changes"][0];
        assert_eq!(change["origin"], "ai");
        assert_eq!(change["before"], serde_json::json!({"color": "#FFEB3B", "style": "highlight", "note": "Yellow method note"}));
        assert_eq!(change["after"]["color"], "#CE93D8");
        assert_eq!(change["after"]["note"], "Yellow method note\n\nExample: $x_1 = 2$.");
        assert_eq!(change["text"], "Alpha beta.");
        assert!(preview.get("passages").is_none() && change.get("members").is_none());
        assert_eq!(paper::read_highlights(&root, "guided")[0].color, "#FFEB3B", "preview must not write");

        let result = pending.execute(&root).unwrap();
        assert_eq!(result["updated"][0]["id"], id.as_str());
        assert_eq!(result["tombstones"], serde_json::json!([]));
        let updated = paper::read_highlights(&root, "guided").remove(0);
        assert_eq!((updated.color.as_str(), updated.note.as_deref()), ("#CE93D8", Some("Yellow method note\n\nExample: $x_1 = 2$.")));
        let stamp = updated.updated_at.clone().unwrap();
        assert!(stamp.as_str() > created.created_at.as_str() && stamp.ends_with('Z'), "{stamp}");
        assert_eq!(updated.agent.unwrap().revision, stamp, "an untouched agent mark stays retractable");
        assert_eq!((updated.text, updated.page), (created.text, created.page), "excerpt and page are never changed");
        assert_eq!(origins(&root, "guided")[0].1, "ai");

        let pending = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "guided", "ids": [id]})).unwrap();
        let preview = serde_json::to_value(pending.preview(&root)).unwrap();
        assert_eq!(preview["changes"][0]["before"]["color"], "#CE93D8");
        assert!(preview["changes"][0].get("after").is_none());
        let result = pending.execute(&root).unwrap();
        assert_eq!(result["removed"][0]["id"], id.as_str());
        assert_eq!(result["tombstones"][0]["id"], id.as_str());
        assert!(paper::read_highlights(&root, "guided").is_empty());
        assert_eq!(paper::read_highlights_doc(&root, "guided").tombstones.len(), 1, "the delete propagates to synced copies");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn user_marks_can_be_changed_with_approval_but_never_removed() {
        let root = library_with_marks("users", vec![stored_mark("mine", 1, "User passage.", false)]);
        assert_eq!(origins(&root, "users"), vec![("mine".into(), "user".into())]);
        let error = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "users", "ids": ["mine"]})).unwrap_err();
        assert!(error.contains("user's own mark") && error.contains("Entry #1"), "{error}");

        let pending = PendingWrite::parse(&root, UPDATE_MARKS_TOOL,
            &serde_json::json!({"slug": "users", "changes": [{"id": "mine", "color": "紫色", "style": "underline"}]})).unwrap();
        assert_eq!(serde_json::to_value(pending.preview(&root)).unwrap()["changes"][0]["origin"], "user");
        pending.execute(&root).unwrap();
        let mark = paper::read_highlights(&root, "users").remove(0);
        assert_eq!((mark.color.as_str(), mark.style.as_str()), ("#CE93D8", "underline"));
        assert!(mark.agent.is_none(), "changing a user's mark never makes it the agent's");
        assert!(PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "users", "ids": ["mine"]})).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_agent_mark_the_user_edited_or_an_older_build_saved_is_no_longer_retractable() {
        let root = library_with_marks("edited", vec![stored_mark("ai-1", 1, "First.", true), stored_mark("ai-2", 1, "Second.", true)]);
        // The reader's own edit: a newer stamp, provenance carried along untouched.
        let mut edited = stored(&root, "edited", "ai-1");
        edited.note = Some("My own words".into());
        edited.updated_at = Some("2026-10-02T08:00:00.000Z".into());
        paper::save_highlights_merged(&root, "edited", vec![edited], Vec::new()).unwrap();
        assert_eq!(origins(&root, "edited")[0], ("ai-1".into(), "ai_user_edited".into()));
        let error = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "edited", "ids": ["ai-1"]})).unwrap_err();
        assert!(error.contains("edited this mark"), "{error}");
        // Updating it is still allowed, and does not make it retractable again.
        PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &serde_json::json!({"slug": "edited",
            "changes": [{"id": "ai-1", "color": "green"}]})).unwrap().execute(&root).unwrap();
        assert_eq!(origins(&root, "edited")[0].1, "ai_user_edited");

        // An older build reads the file into a struct without `agent` and writes
        // it back whole: the field is gone, and the mark counts as the user's —
        // even a removal approved before that save.
        let pending = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "edited", "ids": ["ai-2"]})).unwrap();
        let file = paper::paper_dir(&root, "edited").join("highlights.json");
        let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        for h in doc["highlights"].as_array_mut().unwrap() { h.as_object_mut().unwrap().remove("agent"); }
        std::fs::write(&file, doc.to_string()).unwrap();
        assert!(pending.execute(&root).unwrap_err().contains("changed since the preview"));
        assert_eq!(paper::read_highlights(&root, "edited").len(), 2);
        assert!(origins(&root, "edited").iter().all(|(_, o)| o == "user"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_cross_page_highlight_changes_and_retracts_as_one_selection() {
        let mut first = stored_mark("half-a", 3, "A sentence over the break", true);
        let mut second = stored_mark("half-b", 4, "A sentence over the break", true);
        first.note = Some("Old note".into());
        second.keep_line_breaks = Some(true);
        second.updated_at = Some("2026-10-01T09:00:00.000Z".into());
        second.agent = Some(AgentMark { revision: "2026-10-01T09:00:00.000Z".into() });
        let root = library_with_marks("groups", vec![first, second]);
        assert_eq!(origins(&root, "groups"), vec![("half-a".into(), "ai".into())], "one entry for the selection");

        // Either half's id names the whole; listing both halves is a mistake.
        let twice = serde_json::json!({"slug": "groups", "changes": [{"id": "half-a", "color": "red"}, {"id": "half-b", "color": "red"}]});
        assert!(PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &twice).unwrap_err().contains("already listed"));
        let pending = PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &serde_json::json!({"slug": "groups",
            "changes": [{"id": "half-b", "color": "red", "note": "New note"}]})).unwrap();
        let preview = serde_json::to_value(pending.preview(&root)).unwrap();
        assert_eq!((preview["changes"][0]["id"].as_str(), preview["changes"][0]["pageEnd"].as_u64()), (Some("half-a"), Some(4)));
        let result = pending.execute(&root).unwrap();
        assert_eq!(result["highlights"].as_array().unwrap().len(), 2, "the reader receives every record");
        let marks = paper::read_highlights(&root, "groups");
        assert_eq!(marks.len(), 2);
        let stamp = marks[0].updated_at.clone();
        for mark in &marks {
            assert_eq!((mark.color.as_str(), mark.note.as_deref()), ("#FF0000", Some("New note")));
            assert_eq!(mark.updated_at, stamp, "one shared stamp for the selection");
            assert_eq!(mark.keep_line_breaks, Some(true), "the group's line-break choice is kept on every half");
        }
        assert_eq!(origins(&root, "groups")[0].1, "ai");

        let result = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "groups", "ids": ["half-b"]}))
            .unwrap().execute(&root).unwrap();
        assert_eq!(result["tombstones"].as_array().unwrap().len(), 2);
        assert!(paper::read_highlights(&root, "groups").is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_change_batches_report_every_entry_and_stale_previews_write_nothing() {
        let root = library_with_marks("checks", vec![stored_mark("one", 1, "One.", true), stored_mark("two", 1, "Two.", false)]);
        let before = std::fs::read_to_string(paper::paper_dir(&root, "checks").join("highlights.json")).unwrap();
        let args = serde_json::json!({"slug": "checks", "changes": [
            {"id": "one", "color": "blue"}, {"id": "missing", "color": "blue"}, {"id": "two"},
            {"id": "two", "color": "yellow"}, {"id": "two", "color": "url(x)"}, {"id": "two", "text": "rewritten"}
        ]});
        let error = PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &args).unwrap_err();
        assert!(error.contains("Valid entry indices: [1]"), "{error}");
        for index in 2..=6 { assert!(error.contains(&format!("Entry #{index} (")), "{error}"); }
        assert!(error.contains("already has these values") && error.contains("unknown field"), "{error}");
        for bad in [serde_json::json!({"slug": "checks", "changes": []}), serde_json::json!({"slug": "checks", "ids": "one"}),
            serde_json::json!({"slug": "checks", "changes": (0..31).map(|_| serde_json::json!({"id": "one", "color": "red"})).collect::<Vec<_>>()})] {
            assert!(PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &bad).is_err() || PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &bad).is_err());
        }

        let pending = PendingWrite::parse(&root, UPDATE_MARKS_TOOL,
            &serde_json::json!({"slug": "checks", "changes": [{"id": "one", "color": "blue"}]})).unwrap();
        let removal = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "checks", "ids": ["one"]})).unwrap();
        // The user edits the mark while the card is open.
        let mut mark = stored(&root, "checks", "one");
        mark.color = "#A5D6A7".into();
        mark.updated_at = Some("2026-10-03T00:00:00.000Z".into());
        paper::save_highlights_merged(&root, "checks", vec![mark], Vec::new()).unwrap();
        let edited = std::fs::read_to_string(paper::paper_dir(&root, "checks").join("highlights.json")).unwrap();
        assert_ne!(edited, before);
        for stale in [pending, removal] {
            assert!(stale.execute(&root).unwrap_err().contains("changed since the preview"));
        }
        assert_eq!(std::fs::read_to_string(paper::paper_dir(&root, "checks").join("highlights.json")).unwrap(), edited);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reviewed_colors_apply_to_updates_and_never_to_retractions() {
        let root = library_with_marks("colors", vec![stored_mark("a", 1, "A.", true), stored_mark("b", 1, "B.", true)]);
        let mut pending = PendingWrite::parse(&root, UPDATE_MARKS_TOOL, &serde_json::json!({"slug": "colors", "changes": [
            {"id": "a", "color": "purple"}, {"id": "b", "note": "Explained"}]})).unwrap();
        assert!(pending.apply_passage_colors(Some(&["blue".into()])).is_err());
        assert!(pending.apply_passage_colors(Some(&["blue".into(), "bad".into()])).is_err());
        // The user keeps a's old yellow and also recolors b in the card.
        pending.apply_passage_colors(Some(&["#ffeb3b".into(), "green".into()])).unwrap();
        let result = pending.execute(&root).unwrap();
        assert_eq!(result["unchanged"], serde_json::json!(["a"]));
        assert!(stored(&root, "colors", "a").updated_at.is_none(), "a reverted change writes nothing");
        let b = stored(&root, "colors", "b");
        assert_eq!((b.color.as_str(), b.note.as_deref()), ("#A5D6A7", Some("Explained")));

        let mut removal = PendingWrite::parse(&root, REMOVE_MARKS_TOOL, &serde_json::json!({"slug": "colors", "ids": ["a"]})).unwrap();
        assert!(removal.apply_passage_colors(Some(&["blue".into()])).is_err());
        removal.apply_passage_colors(None).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn provenance_is_optional_on_disk_so_older_files_and_builds_keep_working() {
        // A record written before provenance existed loads as the user's.
        let legacy: Highlight = serde_json::from_value(serde_json::json!({"id": "old", "page": 1, "rects": [],
            "text": "t", "color": "#FFEB3B", "note": null, "created_at": "2026-01-01"})).unwrap();
        assert!(legacy.agent.is_none());
        // A user's mark serializes exactly as before: no new key at all.
        assert!(serde_json::to_value(&legacy).unwrap().get("agent").is_none());
        // The provenance is a plain extra key, which serde in older builds ignores.
        let agent = serde_json::to_value(stored_mark("new", 1, "t", true)).unwrap();
        assert_eq!(agent["agent"], serde_json::json!({"revision": "2026-10-01T08:00:00.000Z"}));
    }

    #[test]
    fn stamps_use_the_frontend_format_and_outrun_existing_edit_times() {
        let now = stamp_after(std::iter::empty());
        assert_eq!(now.len(), "2026-10-25T08:00:00.000Z".len());
        assert!(now.ends_with('Z'));
        let future = "2099-01-01T00:00:00.999999999+00:00";
        assert_eq!(stamp_after([future, "2026-01-01"]), "2099-01-01T00:00:01.000Z");
        assert!(stamp_after(["2099-01-01T00:00:00.000Z"]).as_str() > "2099-01-01T00:00:00.000Z");
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
