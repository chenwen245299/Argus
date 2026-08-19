//! The one tool that lets the agent change a canvas, available only when the
//! question is being asked *about* a canvas (the "问画布" chat).
//!
//! # Why this is not in `app_tools`
//!
//! `app_tools::create_paper_note` writes to disk itself, from the validated
//! [`app_tools::PendingWrite`], because a note is a file the frontend does not
//! hold open. A canvas is the opposite: it is a *live* document in the panel
//! (Vue Flow state, debounced autosave, an undo stack). If the backend rewrote
//! the canvas JSON underneath it, the next autosave would clobber the change and
//! the panel would never re-render it.
//!
//! So the split of duties here is different from the note tool, on purpose:
//!
//!   * the **backend** parses the model's call into a validated [`CanvasEdit`],
//!     renders the preview the user approves, and never touches disk;
//!   * the **frontend** draws that same edit onto the canvas as a preview, and —
//!     only if the user approves — applies those same operations to the live
//!     document and lets the normal autosave persist them.
//!
//! The safety property is kept: the operations are frozen at parse time, the
//! preview is built from them, and the frontend applies *those* operations, not
//! whatever the model might say next. What the user saw is what happens.
//!
//! # The one invariant
//!
//! A call is validated into a [`CanvasEdit`] **once**, against the canvas as it
//! is on disk. An operation that names a node or edge that does not exist, a
//! paper that is not in the library, or a colour that is not a plain hex value,
//! fails here — handed back to the model as an ordinary failed tool call, before
//! the user is ever shown a dialog.

use serde::Serialize;

/// The edit tool's name, shared by the declaration, the dispatcher and the
/// confirmation UI so they cannot drift apart.
pub const EDIT_CANVAS_TOOL: &str = "edit_canvas";

/// Whether `name` is the canvas-editing tool.
///
/// Like [`crate::mcp::app_tools::is_write_tool`], this is what keeps the
/// read-only dispatcher (`mcp::agent::call`) from ever running the edit: it
/// refuses the name outright, so a write that skipped the confirmation is not a
/// mistake that can be made quietly.
pub fn is_edit_tool(name: &str) -> bool {
    name == EDIT_CANVAS_TOOL
}

/// Most operations one call may carry. Comfortably past any real edit, short of
/// a runaway call that would make the confirmation dialog unreadable.
const MAX_OPS: usize = 60;

/// Longest text a text/shape node or an edge label may hold.
const MAX_TEXT_CHARS: usize = 4_000;

/// The canvas is effectively unbounded, but a coordinate this far out is a bug,
/// not an intention — reject it rather than scatter nodes into deep space.
const COORD_LIMIT: f64 = 200_000.0;

/// Defaults for a shape the model added without a size, matching the panel's own
/// `DEFAULT_SHAPE_WIDTH` / `DEFAULT_SHAPE_HEIGHT`.
const DEFAULT_SHAPE_W: f64 = 160.0;
const DEFAULT_SHAPE_H: f64 = 100.0;
const MIN_SIZE: f64 = 12.0;
const MAX_SIZE: f64 = 8_000.0;

/// One validated operation, in the flat shape the frontend applies verbatim.
///
/// Kept flat (a `kind` string plus optional fields) rather than an enum so it
/// serialises straight into the TypeScript `CanvasEditOp` the panel reads — no
/// bespoke deserializer on the other side.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasOp {
    /// "addText" | "addShape" | "addPaper" | "addEdge" | "updateNode" |
    /// "updateEdge" | "deleteNode" | "deleteEdge".
    pub kind: String,
    /// Batch-local id an `add*` op assigns to the node it creates, so an
    /// `addEdge` in the same call can point at it before it has a real id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    /// Existing node this op targets (update/delete node).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_id: Option<String>,
    /// Existing edge this op targets (update/delete edge).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edge_id: Option<String>,
    /// Paper slug (addPaper input) and its resolved id (for the frontend).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paper_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f64>,
    /// "rect" | "ellipse" | "diamond" for a shape node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shape_kind: Option<String>,
    /// Edge endpoints: each is either an existing node id or a batch ref.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// A short human label for the confirmation card (a paper title, the node's
    /// existing text, …), so the card can describe the op without re-resolving.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

impl CanvasOp {
    fn blank(kind: &str) -> Self {
        CanvasOp {
            kind: kind.to_string(),
            reference: None,
            node_id: None,
            edge_id: None,
            slug: None,
            paper_id: None,
            x: None,
            y: None,
            width: None,
            height: None,
            content: None,
            color: None,
            fill_color: None,
            font_size: None,
            shape_kind: None,
            from: None,
            to: None,
            label: None,
            display: None,
        }
    }
}

/// Counts and one-line descriptions, for the approval card's header and legend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEditSummary {
    pub added_nodes: usize,
    pub added_edges: usize,
    pub updated_nodes: usize,
    pub updated_edges: usize,
    pub removed_nodes: usize,
    pub removed_edges: usize,
    /// One short phrase per operation, in order, for the card's detail list.
    pub lines: Vec<String>,
}

/// What the confirmation card and the on-canvas preview are both rendered from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CanvasEditPreview {
    /// Which tool is asking, so the chat can pick the right card (this vs the
    /// note card).
    pub tool: String,
    pub canvas_id: String,
    pub canvas_name: String,
    pub ops: Vec<CanvasOp>,
    pub summary: CanvasEditSummary,
}

/// A validated canvas edit, waiting for the user's approval.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasEdit {
    pub canvas_id: String,
    pub canvas_name: String,
    pub ops: Vec<CanvasOp>,
}

impl CanvasEdit {
    /// Validate a tool call against the canvas `canvas_id`, as it is on disk.
    ///
    /// `canvas_id` comes from the request the chat window opened — never from the
    /// model — so the agent can only ever edit the canvas the user is looking at.
    pub fn parse(root: &str, canvas_id: &str, args: &serde_json::Value) -> Result<Self, String> {
        let canvas = crate::canvas::get_canvas(root, canvas_id)
            .map_err(|_| format!("Canvas '{canvas_id}' was not found."))?;

        let raw_ops = args
            .get("operations")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                "`operations` is required and must be an array of edit operations.".to_string()
            })?;
        if raw_ops.is_empty() {
            return Err("`operations` must contain at least one operation.".to_string());
        }
        if raw_ops.len() > MAX_OPS {
            return Err(format!(
                "Too many operations ({}, limit {MAX_OPS}). Make the change in smaller steps.",
                raw_ops.len()
            ));
        }

        let node_ids: std::collections::HashSet<&str> =
            canvas.nodes.iter().map(|n| n.node_id.as_str()).collect();
        let edge_ids: std::collections::HashSet<&str> =
            canvas.edges.iter().map(|e| e.edge_id.as_str()).collect();
        let library = crate::library::load_library_cache(root);

        // First pass: collect every batch ref an add op declares, so edges may
        // point at a node created later in the same call.
        let mut refs: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, raw) in raw_ops.iter().enumerate() {
            if op_kind(raw)?.starts_with("add") {
                if let Some(r) = raw.get("ref").and_then(|v| v.as_str()) {
                    let r = r.trim();
                    if !r.is_empty() && !refs.insert(r.to_string()) {
                        return Err(format!(
                            "operation {}: ref '{r}' is used twice; each new node needs its own ref.",
                            i + 1
                        ));
                    }
                }
            }
        }

        let mut ops = Vec::with_capacity(raw_ops.len());
        for (i, raw) in raw_ops.iter().enumerate() {
            let n = i + 1;
            let kind = op_kind(raw)?;
            let op = match kind {
                "add_text" | "addText" => {
                    let mut op = CanvasOp::blank("addText");
                    op.reference = opt_ref(raw);
                    op.x = Some(coord(raw, "x", n)?);
                    op.y = Some(coord(raw, "y", n)?);
                    let content = req_str(raw, "content", n)?;
                    check_text(&content, n)?;
                    op.display = Some(short(&content));
                    op.content = Some(content);
                    op.color = opt_color(raw, "color", n)?;
                    op.font_size = opt_num(raw, "font_size").or_else(|| opt_num(raw, "fontSize"));
                    op
                }
                "add_shape" | "addShape" => {
                    let mut op = CanvasOp::blank("addShape");
                    op.reference = opt_ref(raw);
                    op.x = Some(coord(raw, "x", n)?);
                    op.y = Some(coord(raw, "y", n)?);
                    op.width = Some(size(raw, "width", DEFAULT_SHAPE_W, n)?);
                    op.height = Some(size(raw, "height", DEFAULT_SHAPE_H, n)?);
                    op.shape_kind = Some(shape_kind(raw)?);
                    op.color = opt_color(raw, "color", n)?;
                    op.fill_color = opt_color(raw, "fill_color", n)?
                        .or(opt_color(raw, "fillColor", n)?);
                    if let Some(c) = raw.get("content").and_then(|v| v.as_str()) {
                        check_text(c, n)?;
                        op.content = Some(c.to_string());
                    }
                    op.display = Some(
                        op.content
                            .as_deref()
                            .map(short)
                            .unwrap_or_else(|| op.shape_kind.clone().unwrap_or_default()),
                    );
                    op
                }
                "add_paper" | "addPaper" => {
                    let mut op = CanvasOp::blank("addPaper");
                    op.reference = opt_ref(raw);
                    let slug = req_str(raw, "slug", n)?;
                    let entry = library
                        .iter()
                        .find(|e| e.slug == slug)
                        .ok_or_else(|| format!("operation {n}: paper '{slug}' is not in the library."))?;
                    op.paper_id = Some(entry.id.clone());
                    op.display = Some(entry.title.clone());
                    op.slug = Some(slug);
                    op.x = Some(coord(raw, "x", n)?);
                    op.y = Some(coord(raw, "y", n)?);
                    op
                }
                "add_edge" | "addEdge" => {
                    let mut op = CanvasOp::blank("addEdge");
                    let from = req_str(raw, "from", n)?;
                    let to = req_str(raw, "to", n)?;
                    if from == to {
                        return Err(format!("operation {n}: an edge cannot start and end on the same node."));
                    }
                    check_endpoint(&from, n, &node_ids, &refs)?;
                    check_endpoint(&to, n, &node_ids, &refs)?;
                    op.from = Some(from);
                    op.to = Some(to);
                    if let Some(l) = raw.get("label").and_then(|v| v.as_str()) {
                        check_text(l, n)?;
                        op.display = Some(short(l));
                        op.label = Some(l.to_string());
                    }
                    op.color = opt_color(raw, "color", n)?;
                    op
                }
                "update_node" | "updateNode" => {
                    let mut op = CanvasOp::blank("updateNode");
                    let id = req_str(raw, "node_id", n).or_else(|_| req_str(raw, "nodeId", n))?;
                    if !node_ids.contains(id.as_str()) {
                        return Err(format!("operation {n}: node '{id}' is not on this canvas."));
                    }
                    op.display = Some(node_label(&canvas, &library, &id));
                    op.node_id = Some(id);
                    // At least one field has to change, or the op is noise the
                    // user would be asked to approve for nothing.
                    let mut touched = false;
                    if let Some(v) = opt_num(raw, "x") {
                        bounds(v, n)?;
                        op.x = Some(v);
                        touched = true;
                    }
                    if let Some(v) = opt_num(raw, "y") {
                        bounds(v, n)?;
                        op.y = Some(v);
                        touched = true;
                    }
                    if let Some(v) = opt_num(raw, "width") {
                        op.width = Some(v.clamp(MIN_SIZE, MAX_SIZE));
                        touched = true;
                    }
                    if let Some(v) = opt_num(raw, "height") {
                        op.height = Some(v.clamp(MIN_SIZE, MAX_SIZE));
                        touched = true;
                    }
                    if let Some(c) = raw.get("content").and_then(|v| v.as_str()) {
                        check_text(c, n)?;
                        op.content = Some(c.to_string());
                        touched = true;
                    }
                    if let Some(c) = opt_color(raw, "color", n)? {
                        op.color = Some(c);
                        touched = true;
                    }
                    if let Some(c) =
                        opt_color(raw, "fill_color", n)?.or(opt_color(raw, "fillColor", n)?)
                    {
                        op.fill_color = Some(c);
                        touched = true;
                    }
                    if !touched {
                        return Err(format!(
                            "operation {n}: update_node changed nothing — give at least one of \
                             x, y, width, height, content, color, fill_color."
                        ));
                    }
                    op
                }
                "update_edge" | "updateEdge" => {
                    let mut op = CanvasOp::blank("updateEdge");
                    let id = req_str(raw, "edge_id", n).or_else(|_| req_str(raw, "edgeId", n))?;
                    if !edge_ids.contains(id.as_str()) {
                        return Err(format!("operation {n}: edge '{id}' is not on this canvas."));
                    }
                    op.edge_id = Some(id);
                    let mut touched = false;
                    if let Some(l) = raw.get("label").and_then(|v| v.as_str()) {
                        check_text(l, n)?;
                        op.display = Some(short(l));
                        op.label = Some(l.to_string());
                        touched = true;
                    }
                    if let Some(c) = opt_color(raw, "color", n)? {
                        op.color = Some(c);
                        touched = true;
                    }
                    if !touched {
                        return Err(format!(
                            "operation {n}: update_edge changed nothing — give a label or a color."
                        ));
                    }
                    op
                }
                "delete_node" | "deleteNode" => {
                    let mut op = CanvasOp::blank("deleteNode");
                    let id = req_str(raw, "node_id", n).or_else(|_| req_str(raw, "nodeId", n))?;
                    if !node_ids.contains(id.as_str()) {
                        return Err(format!("operation {n}: node '{id}' is not on this canvas."));
                    }
                    op.display = Some(node_label(&canvas, &library, &id));
                    op.node_id = Some(id);
                    op
                }
                "delete_edge" | "deleteEdge" => {
                    let mut op = CanvasOp::blank("deleteEdge");
                    let id = req_str(raw, "edge_id", n).or_else(|_| req_str(raw, "edgeId", n))?;
                    if !edge_ids.contains(id.as_str()) {
                        return Err(format!("operation {n}: edge '{id}' is not on this canvas."));
                    }
                    op.edge_id = Some(id);
                    op
                }
                other => {
                    return Err(format!(
                        "operation {n}: unknown op '{other}'. Use add_text, add_shape, add_paper, \
                         add_edge, update_node, update_edge, delete_node or delete_edge."
                    ));
                }
            };
            ops.push(op);
        }

        Ok(CanvasEdit {
            canvas_id: canvas_id.to_string(),
            canvas_name: canvas.name,
            ops,
        })
    }

    /// What the user is shown — derived from `self`, never from the raw call.
    pub fn preview(&self) -> CanvasEditPreview {
        CanvasEditPreview {
            tool: EDIT_CANVAS_TOOL.to_string(),
            canvas_id: self.canvas_id.clone(),
            canvas_name: self.canvas_name.clone(),
            ops: self.ops.clone(),
            summary: self.summary(),
        }
    }

    /// The result handed back to the model once the user approves. The write
    /// itself is the frontend's job (see the module doc); this only tells the
    /// model what it agreed to, so it can describe it in its reply.
    pub fn result(&self) -> serde_json::Value {
        let s = self.summary();
        serde_json::json!({
            "applied": true,
            "canvas_id": self.canvas_id,
            "added_nodes": s.added_nodes,
            "added_edges": s.added_edges,
            "updated_nodes": s.updated_nodes,
            "updated_edges": s.updated_edges,
            "removed_nodes": s.removed_nodes,
            "removed_edges": s.removed_edges,
        })
    }

    fn summary(&self) -> CanvasEditSummary {
        let mut s = CanvasEditSummary {
            added_nodes: 0,
            added_edges: 0,
            updated_nodes: 0,
            updated_edges: 0,
            removed_nodes: 0,
            removed_edges: 0,
            lines: Vec::with_capacity(self.ops.len()),
        };
        for op in &self.ops {
            let label = op.display.clone().unwrap_or_default();
            let line = match op.kind.as_str() {
                "addText" => {
                    s.added_nodes += 1;
                    format!("＋ text: {label}")
                }
                "addShape" => {
                    s.added_nodes += 1;
                    format!("＋ shape: {label}")
                }
                "addPaper" => {
                    s.added_nodes += 1;
                    format!("＋ paper: {label}")
                }
                "addEdge" => {
                    s.added_edges += 1;
                    if label.is_empty() {
                        "＋ edge".to_string()
                    } else {
                        format!("＋ edge: {label}")
                    }
                }
                "updateNode" => {
                    s.updated_nodes += 1;
                    format!("✎ node: {label}")
                }
                "updateEdge" => {
                    s.updated_edges += 1;
                    "✎ edge".to_string()
                }
                "deleteNode" => {
                    s.removed_nodes += 1;
                    format!("✕ node: {label}")
                }
                "deleteEdge" => {
                    s.removed_edges += 1;
                    "✕ edge".to_string()
                }
                _ => String::new(),
            };
            s.lines.push(line);
        }
        s
    }
}

// ── Argument helpers ────────────────────────────────────────────────────────────

fn op_kind(raw: &serde_json::Value) -> Result<&str, String> {
    raw.get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "each operation needs an `op` field naming what to do.".to_string())
}

fn opt_ref(raw: &serde_json::Value) -> Option<String> {
    raw.get("ref")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn req_str(raw: &serde_json::Value, key: &str, n: usize) -> Result<String, String> {
    raw.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("operation {n}: `{key}` is required and must be a non-empty string."))
}

fn opt_num(raw: &serde_json::Value, key: &str) -> Option<f64> {
    raw.get(key).and_then(|v| v.as_f64()).filter(|v| v.is_finite())
}

fn bounds(v: f64, n: usize) -> Result<(), String> {
    if v.abs() > COORD_LIMIT {
        return Err(format!("operation {n}: coordinate {v} is out of range."));
    }
    Ok(())
}

fn coord(raw: &serde_json::Value, key: &str, n: usize) -> Result<f64, String> {
    let v = opt_num(raw, key)
        .ok_or_else(|| format!("operation {n}: `{key}` is required and must be a number."))?;
    bounds(v, n)?;
    Ok(v)
}

fn size(raw: &serde_json::Value, key: &str, default: f64, n: usize) -> Result<f64, String> {
    match opt_num(raw, key) {
        Some(v) if v >= MIN_SIZE => Ok(v.min(MAX_SIZE)),
        Some(_) => Err(format!("operation {n}: `{key}` is too small (min {MIN_SIZE}).")),
        None => Ok(default),
    }
}

fn shape_kind(raw: &serde_json::Value) -> Result<String, String> {
    match raw
        .get("shape_kind")
        .or_else(|| raw.get("shapeKind"))
        .and_then(|v| v.as_str())
    {
        None => Ok("rect".to_string()),
        Some("rect") | Some("ellipse") | Some("diamond") => {
            Ok(raw
                .get("shape_kind")
                .or_else(|| raw.get("shapeKind"))
                .and_then(|v| v.as_str())
                .unwrap()
                .to_string())
        }
        Some(other) => Err(format!(
            "shape_kind '{other}' is not valid — use rect, ellipse or diamond."
        )),
    }
}

/// Accept only a plain hex colour. Everything the frontend does with this value
/// ends up in an inline `style`, so anything but `#rgb`/`#rgba`/`#rrggbb`/
/// `#rrggbbaa` is refused rather than trusted.
fn opt_color(raw: &serde_json::Value, key: &str, n: usize) -> Result<Option<String>, String> {
    match raw.get(key).and_then(|v| v.as_str()) {
        None => Ok(None),
        Some(c) => {
            let c = c.trim();
            if is_hex_color(c) {
                Ok(Some(c.to_string()))
            } else {
                Err(format!(
                    "operation {n}: `{key}` must be a hex colour like #2563eb (got '{c}')."
                ))
            }
        }
    }
}

fn is_hex_color(c: &str) -> bool {
    let Some(hex) = c.strip_prefix('#') else {
        return false;
    };
    matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

/// An edge endpoint must be a node that exists now or a node this same call
/// creates.
fn check_endpoint(
    id: &str,
    n: usize,
    node_ids: &std::collections::HashSet<&str>,
    refs: &std::collections::HashSet<String>,
) -> Result<(), String> {
    if node_ids.contains(id) || refs.contains(id) {
        Ok(())
    } else {
        Err(format!(
            "operation {n}: edge endpoint '{id}' is neither a node on this canvas nor a ref \
             created in this call. Read the canvas with get_canvas for node ids."
        ))
    }
}

fn check_text(s: &str, n: usize) -> Result<(), String> {
    if s.chars().count() > MAX_TEXT_CHARS {
        return Err(format!(
            "operation {n}: text is too long ({} chars, limit {MAX_TEXT_CHARS}).",
            s.chars().count()
        ));
    }
    Ok(())
}

/// A one-line label for a node, for the confirmation card: a paper node shows
/// its title, an annotation node its own text.
fn node_label(
    canvas: &crate::models::Canvas,
    library: &[crate::models::PaperIndexEntry],
    node_id: &str,
) -> String {
    let Some(node) = canvas.nodes.iter().find(|n| n.node_id == node_id) else {
        return node_id.to_string();
    };
    if !node.paper_id.is_empty() {
        if let Some(e) = library.iter().find(|e| e.id == node.paper_id) {
            return e.title.clone();
        }
    }
    node.content
        .as_deref()
        .map(short)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| node.node_type.clone().unwrap_or_else(|| "node".to_string()))
}

/// A compact one-liner for a preview label.
fn short(s: &str) -> String {
    let flat = s.replace(['\r', '\n', '\t'], " ");
    let trimmed = flat.trim();
    let mut out: String = trimmed.chars().take(40).collect();
    if trimmed.chars().count() > 40 {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Canvas, CanvasEdge, CanvasNode, Viewport};

    fn write_canvas(root: &str, canvas: &Canvas) {
        let dir = std::path::Path::new(root).join("canvases");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{}.json", canvas.id)),
            serde_json::to_string(canvas).unwrap(),
        )
        .unwrap();
    }

    fn paper_node(id: &str, paper_id: &str) -> CanvasNode {
        let mut n = text_node(id, "");
        n.paper_id = paper_id.to_string();
        n.node_type = None;
        n.content = None;
        n
    }

    fn text_node(id: &str, content: &str) -> CanvasNode {
        CanvasNode {
            node_id: id.to_string(),
            paper_id: String::new(),
            x: 0.0,
            y: 0.0,
            color: None,
            hover_source: None,
            node_type: Some("text".to_string()),
            content: Some(content.to_string()),
            font_size: None,
            font_bold: None,
            font_italic: None,
            width: None,
            height: None,
            shape_kind: None,
            fill_color: None,
            stroke_width: None,
            rotation: None,
            opacity: None,
            corner_radius: None,
            font_family: None,
            text_align: None,
            line_kind: None,
            line_points: vec![],
            image_src: None,
            image_alt: None,
            z_index: None,
        }
    }

    fn fixture() -> (String, String) {
        let root = std::env::temp_dir()
            .join(format!("argus-canvas-edit-{}", uuid::Uuid::new_v4().simple()))
            .to_string_lossy()
            .to_string();
        let canvas = Canvas {
            id: "cv1".to_string(),
            name: "My map".to_string(),
            nodes: vec![text_node("n-a", "existing note"), paper_node("n-b", "pid-1")],
            edges: vec![CanvasEdge {
                edge_id: "e-1".to_string(),
                from_node_id: "n-a".to_string(),
                to_node_id: "n-b".to_string(),
                source_handle: None,
                target_handle: None,
                label: Some("cites".to_string()),
                color: None,
                stroke_width: None,
                control_x: None,
                control_y: None,
                control_points: vec![],
            }],
            viewport: Viewport::default(),
            created_at: "t".to_string(),
            updated_at: "t".to_string(),
        };
        write_canvas(&root, &canvas);
        (root, "cv1".to_string())
    }

    fn ops(v: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "operations": v })
    }

    #[test]
    fn a_missing_operations_array_is_refused() {
        let (root, id) = fixture();
        let err = CanvasEdit::parse(&root, &id, &serde_json::json!({})).unwrap_err();
        assert!(err.contains("operations"), "{err}");
    }

    #[test]
    fn an_unknown_canvas_is_refused() {
        let (root, _) = fixture();
        let err = CanvasEdit::parse(&root, "nope", &ops(serde_json::json!([]))).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn adding_a_text_node_validates_and_previews() {
        let (root, id) = fixture();
        let edit = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([
                { "op": "add_text", "x": 10, "y": 20, "content": "hello", "color": "#2563eb" }
            ])),
        )
        .expect("parses");
        assert_eq!(edit.ops.len(), 1);
        let p = edit.preview();
        assert_eq!(p.summary.added_nodes, 1);
        assert_eq!(p.ops[0].kind, "addText");
        assert_eq!(p.ops[0].color.as_deref(), Some("#2563eb"));
    }

    #[test]
    fn a_non_hex_color_is_refused() {
        let (root, id) = fixture();
        let err = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([
                { "op": "add_text", "x": 0, "y": 0, "content": "x", "color": "red" }
            ])),
        )
        .unwrap_err();
        assert!(err.contains("hex colour"), "{err}");
    }

    #[test]
    fn an_edge_to_a_missing_node_is_refused() {
        let (root, id) = fixture();
        let err = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([
                { "op": "add_edge", "from": "n-a", "to": "ghost" }
            ])),
        )
        .unwrap_err();
        assert!(err.contains("neither a node"), "{err}");
    }

    #[test]
    fn an_edge_may_point_at_a_node_created_in_the_same_call() {
        let (root, id) = fixture();
        let edit = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([
                { "op": "add_text", "ref": "new1", "x": 0, "y": 0, "content": "fresh" },
                { "op": "add_edge", "from": "n-a", "to": "new1", "label": "leads to" }
            ])),
        )
        .expect("parses");
        let p = edit.preview();
        assert_eq!(p.summary.added_nodes, 1);
        assert_eq!(p.summary.added_edges, 1);
    }

    #[test]
    fn updating_a_missing_node_is_refused() {
        let (root, id) = fixture();
        let err = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([
                { "op": "update_node", "node_id": "ghost", "x": 1, "y": 2 }
            ])),
        )
        .unwrap_err();
        assert!(err.contains("not on this canvas"), "{err}");
    }

    #[test]
    fn an_update_that_changes_nothing_is_refused() {
        let (root, id) = fixture();
        let err = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([
                { "op": "update_node", "node_id": "n-a" }
            ])),
        )
        .unwrap_err();
        assert!(err.contains("changed nothing"), "{err}");
    }

    #[test]
    fn deleting_an_existing_edge_is_allowed() {
        let (root, id) = fixture();
        let edit = CanvasEdit::parse(
            &root,
            &id,
            &ops(serde_json::json!([{ "op": "delete_edge", "edge_id": "e-1" }])),
        )
        .expect("parses");
        assert_eq!(edit.preview().summary.removed_edges, 1);
    }

    #[test]
    fn too_many_operations_are_refused() {
        let (root, id) = fixture();
        let many: Vec<serde_json::Value> = (0..MAX_OPS + 1)
            .map(|_| serde_json::json!({ "op": "add_text", "x": 0, "y": 0, "content": "x" }))
            .collect();
        let err = CanvasEdit::parse(&root, &id, &ops(serde_json::json!(many))).unwrap_err();
        assert!(err.contains("Too many"), "{err}");
    }
}
