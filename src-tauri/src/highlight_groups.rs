//! One selection, one highlight — for everything that *lists* highlights.
//!
//! The PDF viewer stores a selection that crosses a page break as one
//! [`Highlight`] record **per page**: each with that page's rects, the WHOLE
//! selection text, and one shared `created_at` (written by PdfViewer
//! `createHighlight`). Anything that enumerates the raw records — the export,
//! the MCP/agent tools, the embedding-map chunks — therefore reports the same
//! passage twice.
//!
//! ## Why this is a derived view and nothing else
//!
//! Storage is deliberately untouched by the grouping: no new field, no migration,
//! no rewriting or tombstoning of legacy records. Libraries already hold such pairs, and
//! older builds on other machines (a synced library) keep creating them and
//! would strip any field they do not know. `(created_at, text)` is the only
//! marker that is always there, so a group is recognised by it at read time.
//!
//! Hence this module is **pure**: it never writes, and it must never sit inside
//! `paper::read_highlights` / `commands::get_highlights` /
//! `paper::save_highlights_merged`. The frontend store round-trips every raw
//! record through `save_highlights`; collapsing there would drop the other half
//! of a pair from disk on the next save. Only read-only consumers call in here.
//!
//! The TypeScript twin is `src/utils/highlightGroups.ts`; the two implement one
//! rule and must stay identical:
//!
//! * Records bucket by (`created_at`, `text`) — exact string equality, no
//!   trimming — and only records with no `start_offset` / `end_offset` take part
//!   (ebook records carry offsets and are always one per selection).
//! * A bucket is a group only when it holds at least two DISTINCT pages. Pages
//!   need not be adjacent: an unrendered middle page leaves a gap. A single-page
//!   bucket — even one with several records — is not a group; each record stays
//!   an entry of its own. A lone survivor (its twin deleted) is therefore an
//!   ordinary highlight.
//! * Members sort by page, then `rects[0].y` (missing => 0.0), then id. The first
//!   is the canonical member: its id is the group's id (so ids stay stable as RAG
//!   `source_id`s and MCP ids) and its page is the group's page.
//! * Text is the canonical member's, never a join — every member already holds
//!   the whole selection.
//! * Colour, style and `keep_line_breaks` come from the member edited last
//!   (`updated_at`, else `created_at`, compared as plain ISO strings); a tie goes
//!   to the earlier member. The text a consumer shows is then
//!   [`HighlightGroup::display_text`]: the canonical member's text with its wrapped
//!   lines merged into one paragraph, unless that flag says to keep the breaks
//!   (see `highlight_text`).
//! * Notes are the distinct non-empty ones, in member order, joined by a blank
//!   line. "Empty" and "distinct" are judged on `trim()`; the text emitted is the
//!   original of the first member that had it.
//! * Output keeps the order of first appearance in the input.

use std::collections::HashMap;

use crate::models::Highlight;

/// A separator between the distinct notes of one group's members.
const NOTE_SEPARATOR: &str = "\n\n";

/// One logical highlight, as the user made it.
#[derive(Debug, Clone)]
pub struct HighlightGroup {
    /// The canonical member (the lowest page), with `note`, `color`, `style` and
    /// `keep_line_breaks` replaced by the group's merged values. Its `text` is
    /// still as captured — read it through [`HighlightGroup::display_text`]. Its
    /// `rects` are that member's own,
    /// i.e. only those on `rep.page` — consumers of this view do not draw.
    /// A record that is not part of a group comes through unchanged.
    pub rep: Highlight,
    /// The last page the selection covers. `Some` only for a real group, so a
    /// plain highlight (and a serialized export of one) carries no extra field.
    pub page_end: Option<u32>,
    /// Every record the entry stands for, in member order (`rep.id` first).
    /// A single element for a plain highlight. No read-only consumer needs it
    /// yet — they all key on `rep.id` — but it is the group's identity (the
    /// TypeScript twin exposes it as `ids`) and what the tests check membership
    /// against.
    #[allow(dead_code)]
    pub member_ids: Vec<String>,
}

impl HighlightGroup {
    fn single(h: Highlight) -> Self {
        HighlightGroup { member_ids: vec![h.id.clone()], rep: h, page_end: None }
    }

    /// The text this highlight shows and exports: the canonical member's text as
    /// one paragraph, unless the user chose to keep the line breaks. Ebook
    /// records come back untouched. See `highlight_text::display_text`.
    pub fn display_text(&self) -> String {
        crate::highlight_text::display_text(&self.rep)
    }

    /// Stitch the records of one selection together. `members` has at least two
    /// records on at least two distinct pages.
    fn from_members(mut members: Vec<Highlight>) -> Self {
        members.sort_by(compare_members);

        // Newest edit wins the colour, style and line-break choice; `>` keeps the
        // earlier member on a tie, so an unedited pair resolves to the lowest
        // page — the same record the canonical id belongs to.
        let mut newest = 0;
        for (i, m) in members.iter().enumerate() {
            if edit_time(m) > edit_time(&members[newest]) {
                newest = i;
            }
        }
        let color = members[newest].color.clone();
        let style = members[newest].style.clone();
        let keep_line_breaks = members[newest].keep_line_breaks;

        let page_end = members.iter().map(|m| m.page).max();
        let note = merge_notes(&members);
        let member_ids = members.iter().map(|m| m.id.clone()).collect();

        let mut rep = members.swap_remove(0);
        rep.note = note;
        rep.color = color;
        rep.style = style;
        rep.keep_line_breaks = keep_line_breaks;
        HighlightGroup { rep, page_end, member_ids }
    }
}

/// Member order: page, then the first rect's top edge, then id. The id makes it
/// total, so the canonical member never depends on input order.
fn compare_members(a: &Highlight, b: &Highlight) -> std::cmp::Ordering {
    let ay = a.rects.first().map_or(0.0, |r| r.y);
    let by = b.rects.first().map_or(0.0, |r| r.y);
    a.page
        .cmp(&b.page)
        .then_with(|| ay.total_cmp(&by))
        .then_with(|| a.id.cmp(&b.id))
}

/// When a record's content last changed: its last edit, else its creation.
fn edit_time(h: &Highlight) -> &str {
    h.updated_at.as_deref().unwrap_or(&h.created_at)
}

/// The characters JavaScript's `String.prototype.trim` strips. The TypeScript twin
/// judges "empty" and "distinct" with `trim()`, and Rust's own `str::trim` differs
/// from it in exactly two code points — it strips U+0085 and keeps U+FEFF — so a
/// note of just a BOM would be blank in the sidebar and real in an export.
fn is_js_whitespace(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}')
}

/// The distinct non-empty notes of `members` (already in member order).
fn merge_notes(members: &[Highlight]) -> Option<String> {
    let mut seen: Vec<&str> = Vec::new();
    let mut notes: Vec<&str> = Vec::new();
    for m in members {
        let Some(note) = m.note.as_deref() else { continue };
        let key = note.trim_matches(is_js_whitespace);
        if key.is_empty() || seen.contains(&key) {
            continue;
        }
        seen.push(key);
        notes.push(note);
    }
    if notes.is_empty() {
        None
    } else {
        Some(notes.join(NOTE_SEPARATOR))
    }
}

/// Collapse one-selection-per-page records into logical highlights.
///
/// Pure: takes the records by value and returns new entries; nothing is read
/// from or written to disk. Entries keep the order in which they first appear in
/// `list` — a group sits where its first record did.
pub fn collapse_highlights(list: Vec<Highlight>) -> Vec<HighlightGroup> {
    // Pass 1: bucket the records that may group, by (created_at, text).
    let mut bucket_of: Vec<Option<usize>> = Vec::with_capacity(list.len());
    let mut buckets: Vec<Vec<usize>> = Vec::new();
    {
        let mut by_key: HashMap<(&str, &str), usize> = HashMap::new();
        for (i, h) in list.iter().enumerate() {
            if h.start_offset.is_some() || h.end_offset.is_some() {
                bucket_of.push(None);
                continue;
            }
            let b = *by_key
                .entry((h.created_at.as_str(), h.text.as_str()))
                .or_insert_with(|| {
                    buckets.push(Vec::new());
                    buckets.len() - 1
                });
            buckets[b].push(i);
            bucket_of.push(Some(b));
        }
    }

    // A bucket groups only when it spans at least two distinct pages.
    let is_group: Vec<bool> = buckets
        .iter()
        .map(|idx| idx.iter().any(|&i| list[i].page != list[idx[0]].page))
        .collect();

    // Pass 2: walk the input in order, emitting each entry where it first
    // appears. Records of a non-grouping bucket stay individual entries at their
    // own positions.
    let mut cells: Vec<Option<Highlight>> = list.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(cells.len());
    for i in 0..cells.len() {
        let Some(h) = cells[i].take() else { continue }; // already folded into a group
        match bucket_of[i] {
            Some(b) if is_group[b] => {
                let mut members = vec![h];
                for &j in &buckets[b] {
                    if j != i {
                        members.extend(cells[j].take());
                    }
                }
                out.push(HighlightGroup::from_members(members));
            }
            _ => out.push(HighlightGroup::single(h)),
        }
    }
    out
}

/// A paper's effective highlights, one entry per selection.
///
/// What every read-only consumer should call instead of reading
/// `paper::read_highlights` and listing the raw records. Not for anything that
/// writes highlights back — see the module docs.
pub fn read_grouped(root: &str, slug: &str) -> Vec<HighlightGroup> {
    collapse_highlights(crate::paper::read_highlights(root, slug))
}

/// A throwaway library on disk, for the consumers' tests (export, MCP, RAG):
/// they all read a paper through `paper::read_meta` + `read_highlights`, so the
/// only honest way to test them end to end is a real `meta.json` and
/// `highlights.json`.
#[cfg(test)]
pub(crate) mod fixtures {
    use crate::models::{Highlight, Rect};

    /// The shared `created_at` of the pair `cross_page_pair` builds.
    pub const PAIR_CREATED_AT: &str = "2026-03-01T10:00:00.000Z";
    pub const PAIR_TEXT: &str = "a passage that runs over the page break";

    pub fn record(id: &str, page: u32, text: &str, created_at: &str) -> Highlight {
        Highlight {
            id: id.into(),
            page,
            rects: vec![Rect { x: 0.1, y: 0.5, width: 0.3, height: 0.02 }],
            text: text.into(),
            color: "#ffd400".into(),
            note: None,
            created_at: created_at.into(),
            updated_at: None,
            style: "highlight".into(),
            start_offset: None,
            end_offset: None,
            anchor_prefix: None,
            anchor_suffix: None,
            keep_line_breaks: None,
        }
    }

    /// What the viewer stores for one selection across pages `first` and
    /// `first + 1`: two records, the whole text in each, one `created_at`.
    pub fn cross_page_pair(first: u32) -> Vec<Highlight> {
        vec![
            record(&format!("hl-{first}"), first, PAIR_TEXT, PAIR_CREATED_AT),
            record(&format!("hl-{}", first + 1), first + 1, PAIR_TEXT, PAIR_CREATED_AT),
        ]
    }

    /// Delete-on-drop library root holding one paper.
    pub struct TestLibrary {
        pub root: std::path::PathBuf,
    }

    impl TestLibrary {
        pub fn root(&self) -> &str {
            self.root.to_str().unwrap()
        }
    }

    impl Drop for TestLibrary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Create a library with paper `slug` whose highlights file holds exactly
    /// `highlights`, in the order given — the legacy bare-array shape.
    pub fn library_with(slug: &str, highlights: &[Highlight]) -> TestLibrary {
        let root = std::env::temp_dir().join(format!(
            "argus-hlgroups-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let dir = root.join("papers").join(slug);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            r#"{"id":"paper-1","title":"A Paper","year":null,"doi":null,"arxiv_id":null,"venue":null,"original_filename":null}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("highlights.json"),
            serde_json::to_string_pretty(highlights).unwrap(),
        )
        .unwrap();
        TestLibrary { root }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Rect;

    const T0: &str = "2026-03-01T10:00:00.000Z";
    const TEXT: &str = "a passage that runs over the page break";

    fn hl(id: &str, page: u32) -> Highlight {
        Highlight {
            id: id.into(),
            page,
            rects: vec![Rect { x: 0.1, y: 0.5, width: 0.3, height: 0.02 }],
            text: TEXT.into(),
            color: "#ffd400".into(),
            note: None,
            created_at: T0.into(),
            updated_at: None,
            style: "highlight".into(),
            start_offset: None,
            end_offset: None,
            anchor_prefix: None,
            anchor_suffix: None,
            keep_line_breaks: None,
        }
    }

    fn with_note(mut h: Highlight, note: &str) -> Highlight {
        h.note = Some(note.into());
        h
    }

    fn edited(mut h: Highlight, at: &str, color: &str, style: &str) -> Highlight {
        h.updated_at = Some(at.into());
        h.color = color.into();
        h.style = style.into();
        h
    }

    fn ids(groups: &[HighlightGroup]) -> Vec<&str> {
        groups.iter().map(|g| g.rep.id.as_str()).collect()
    }

    #[test]
    fn a_legacy_pair_becomes_one_group() {
        // Stored order is whatever the merge produced; the lower page must win
        // the id no matter which half comes first.
        let groups = collapse_highlights(vec![hl("b", 11), hl("a", 10)]);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.rep.id, "a");
        assert_eq!(g.rep.page, 10);
        assert_eq!(g.page_end, Some(11));
        assert_eq!(g.member_ids, vec!["a", "b"]);
        // The text is the canonical member's, not two copies glued together.
        assert_eq!(g.rep.text, TEXT);
        assert_eq!(g.rep.created_at, T0);
    }

    #[test]
    fn a_note_on_only_one_half_is_the_groups_note() {
        for (first, second) in [(Some("重点"), None), (None, Some("重点"))] {
            let mut a = hl("a", 10);
            let mut b = hl("b", 11);
            a.note = first.map(str::to_string);
            b.note = second.map(str::to_string);
            let groups = collapse_highlights(vec![a, b]);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].rep.note.as_deref(), Some("重点"));
        }
        // Blank-only notes merge to nothing rather than to whitespace.
        let groups = collapse_highlights(vec![
            with_note(hl("a", 10), "  \n"),
            with_note(hl("b", 11), ""),
        ]);
        assert_eq!(groups[0].rep.note, None);
    }

    #[test]
    fn different_notes_join_in_member_order_and_equal_ones_dedupe() {
        // Input order is b, a — member order is a, b, so a's note comes first.
        let groups = collapse_highlights(vec![
            with_note(hl("b", 11), "second half"),
            with_note(hl("a", 10), "first half"),
        ]);
        assert_eq!(groups[0].rep.note.as_deref(), Some("first half\n\nsecond half"));

        // Identical notes — including ones differing only by surrounding
        // whitespace — collapse to one, and the emitted text is the first
        // member's original, untrimmed.
        let groups = collapse_highlights(vec![
            with_note(hl("a", 10), "  same note \n"),
            with_note(hl("b", 11), "same note"),
        ]);
        assert_eq!(groups[0].rep.note.as_deref(), Some("  same note \n"));

        // Three members, one of them blank and one a repeat.
        let groups = collapse_highlights(vec![
            with_note(hl("a", 10), "x"),
            with_note(hl("b", 11), " "),
            with_note(hl("c", 12), "x "),
        ]);
        assert_eq!(groups[0].rep.note.as_deref(), Some("x"));
    }

    /// `trim` must mean what JavaScript's `trim` means, or the sidebar and the
    /// export disagree about a note. The two differ on U+FEFF and U+0085.
    #[test]
    fn note_blankness_matches_javascript_trim() {
        // A BOM alone is blank (JS strips it; Rust's str::trim would keep it).
        let groups = collapse_highlights(vec![with_note(hl("a", 10), "\u{FEFF}"), hl("b", 11)]);
        assert_eq!(groups[0].rep.note, None);

        // U+0085 is NOT whitespace to JS (Rust's trim would strip it), so it is a
        // real, distinct note.
        let groups = collapse_highlights(vec![with_note(hl("a", 10), "\u{85}"), with_note(hl("b", 11), "real")]);
        assert_eq!(groups[0].rep.note.as_deref(), Some("\u{85}\n\nreal"));

        // A trailing BOM does not make an otherwise identical note distinct.
        let groups = collapse_highlights(vec![with_note(hl("a", 10), "x"), with_note(hl("b", 11), "x\u{FEFF}")]);
        assert_eq!(groups[0].rep.note.as_deref(), Some("x"));
    }

    #[test]
    fn a_page_gap_does_not_stop_a_group() {
        // An unrendered middle page leaves 10 and 12 only; 11 was never stored.
        let groups = collapse_highlights(vec![hl("c", 12), hl("a", 10), hl("b", 11)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rep.page, 10);
        assert_eq!(groups[0].page_end, Some(12));
        assert_eq!(groups[0].member_ids, vec!["a", "b", "c"]);

        let groups = collapse_highlights(vec![hl("a", 10), hl("c", 12)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].page_end, Some(12));
        assert_eq!(groups[0].member_ids, vec!["a", "c"]);
    }

    #[test]
    fn a_lone_survivor_is_an_ordinary_highlight() {
        // The twin was deleted (tombstoned records never reach this function).
        let groups = collapse_highlights(vec![with_note(hl("b", 11), "kept")]);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.rep.id, "b");
        assert_eq!(g.rep.page, 11);
        assert_eq!(g.page_end, None);
        assert_eq!(g.member_ids, vec!["b"]);
        assert_eq!(g.rep.note.as_deref(), Some("kept"));
    }

    #[test]
    fn a_different_created_at_is_a_different_selection() {
        let mut b = hl("b", 11);
        b.created_at = "2026-03-01T10:00:00.001Z".into();
        let groups = collapse_highlights(vec![hl("a", 10), b]);
        assert_eq!(ids(&groups), vec!["a", "b"]);
        assert!(groups.iter().all(|g| g.page_end.is_none()));
    }

    #[test]
    fn a_different_text_is_a_different_selection() {
        // Exact equality: a trailing space is not the same selection.
        let mut b = hl("b", 11);
        b.text = format!("{TEXT} ");
        let groups = collapse_highlights(vec![hl("a", 10), b]);
        assert_eq!(ids(&groups), vec!["a", "b"]);
    }

    #[test]
    fn the_same_page_twice_is_not_a_group() {
        // Two records, one (created_at, text), but a single page: two selections
        // that happen to coincide, not one selection over a page break.
        let groups = collapse_highlights(vec![
            with_note(hl("a", 10), "one"),
            with_note(hl("b", 10), "two"),
        ]);
        assert_eq!(ids(&groups), vec!["a", "b"]);
        assert!(groups.iter().all(|g| g.page_end.is_none() && g.member_ids.len() == 1));
        // And each keeps its own note, unmerged.
        assert_eq!(groups[0].rep.note.as_deref(), Some("one"));
        assert_eq!(groups[1].rep.note.as_deref(), Some("two"));
    }

    #[test]
    fn a_same_page_twin_inside_a_real_group_is_still_a_member() {
        // Pages 10, 10, 11: the bucket spans two pages, so all three belong to it.
        let mut a2 = hl("a2", 10);
        a2.rects[0].y = 0.9;
        let groups = collapse_highlights(vec![hl("a1", 10), a2, hl("b", 11)]);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rep.id, "a1");
        assert_eq!(groups[0].member_ids, vec!["a1", "a2", "b"]);
        assert_eq!(groups[0].page_end, Some(11));
    }

    #[test]
    fn members_on_one_page_order_by_the_first_rects_top_then_id() {
        let mut low = hl("z", 10);
        low.rects[0].y = 0.8;
        let mut high = hl("y", 10);
        high.rects[0].y = 0.1;
        let mut none = hl("x", 10);
        none.rects.clear(); // missing rect => 0.0, so it sorts first
        let groups = collapse_highlights(vec![low, high, none, hl("n", 11)]);
        assert_eq!(groups[0].member_ids, vec!["x", "y", "z", "n"]);
        assert_eq!(groups[0].rep.id, "x");

        // Identical position: the id decides, so the result is input-independent.
        let a = collapse_highlights(vec![hl("p", 10), hl("q", 10), hl("r", 11)]);
        let b = collapse_highlights(vec![hl("r", 11), hl("q", 10), hl("p", 10)]);
        assert_eq!(a[0].member_ids, b[0].member_ids);
        assert_eq!(a[0].rep.id, "p");
    }

    #[test]
    fn ebook_records_never_group() {
        // Same created_at and text on two chapters, but they carry offsets.
        let mut a = hl("a", 3);
        a.start_offset = Some(10);
        a.end_offset = Some(40);
        let mut b = hl("b", 4);
        b.start_offset = Some(0);
        b.end_offset = Some(30);
        let groups = collapse_highlights(vec![a, b]);
        assert_eq!(ids(&groups), vec!["a", "b"]);
        assert!(groups.iter().all(|g| g.page_end.is_none()));

        // One offset alone is enough to keep a record out...
        let mut c = hl("c", 4);
        c.end_offset = Some(9);
        let groups = collapse_highlights(vec![hl("a", 3), c]);
        assert_eq!(ids(&groups), vec!["a", "c"]);

        // ...and an offset record is not pulled into a PDF group beside it.
        let mut o = hl("o", 12);
        o.start_offset = Some(1);
        let groups = collapse_highlights(vec![hl("a", 10), o, hl("b", 11)]);
        assert_eq!(ids(&groups), vec!["a", "o"]);
        assert_eq!(groups[0].member_ids, vec!["a", "b"]);
        assert_eq!(groups[1].member_ids, vec!["o"]);
    }

    #[test]
    fn colour_and_style_follow_the_newest_edit() {
        // b was recoloured last, although a is the canonical member.
        let a = edited(hl("a", 10), "2026-03-02T00:00:00Z", "#ff0000", "underline");
        let b = edited(hl("b", 11), "2026-03-03T00:00:00Z", "#00ff00", "highlight");
        let groups = collapse_highlights(vec![a, b]);
        assert_eq!(groups[0].rep.id, "a");
        assert_eq!(groups[0].rep.color, "#00ff00");
        assert_eq!(groups[0].rep.style, "highlight");

        // A member never edited counts from its created_at, which is older than
        // any later edit.
        let a = hl("a", 10);
        let b = edited(hl("b", 11), "2026-03-03T00:00:00Z", "#0000ff", "underline");
        let groups = collapse_highlights(vec![a, b]);
        assert_eq!(groups[0].rep.color, "#0000ff");
        assert_eq!(groups[0].rep.style, "underline");

        // Equal times: the earlier member (lowest page) wins, whichever way the
        // input arrived.
        for input in [["a", "b"], ["b", "a"]] {
            let mk = |id: &str| match id {
                "a" => edited(hl("a", 10), "2026-03-02T00:00:00Z", "#aa0000", "underline"),
                _ => edited(hl("b", 11), "2026-03-02T00:00:00Z", "#00bb00", "highlight"),
            };
            let groups = collapse_highlights(input.iter().map(|id| mk(id)).collect());
            assert_eq!(groups[0].rep.color, "#aa0000", "input {input:?}");
            assert_eq!(groups[0].rep.style, "underline");
        }

        // Neither edited: the canonical member's own colour, unchanged.
        let mut b = hl("b", 11);
        b.color = "#123456".into();
        let groups = collapse_highlights(vec![b, hl("a", 10)]);
        assert_eq!(groups[0].rep.color, "#ffd400");
    }

    // ── Line breaks ─────────────────────────────────────────────────────────

    /// A wrapped selection: what a PDF hands back, a line break at every wrap.
    const WRAPPED: &str = "a passage that runs\nover the page\nbreak";
    const MERGED: &str = "a passage that runs over the page break";

    /// `hl`, but with the wrapped text. Both halves of a pair must carry the same
    /// text and `created_at` to group, so every member is built through this.
    fn wrapped(id: &str, page: u32) -> Highlight {
        let mut h = hl(id, page);
        h.text = WRAPPED.into();
        h
    }

    fn kept(mut h: Highlight, at: &str, keep: Option<bool>) -> Highlight {
        h.updated_at = Some(at.into());
        h.keep_line_breaks = keep;
        h
    }

    #[test]
    fn a_group_reads_as_one_paragraph_unless_the_breaks_are_kept() {
        let groups = collapse_highlights(vec![wrapped("a", 10), wrapped("b", 11)]);
        assert_eq!(groups.len(), 1);
        // The text is as captured; the display text is the merged paragraph.
        assert_eq!(groups[0].rep.text, WRAPPED);
        assert_eq!(groups[0].display_text(), MERGED);
        assert_eq!(groups[0].rep.keep_line_breaks, None);
    }

    #[test]
    fn keeping_the_breaks_on_only_the_newest_member_keeps_them_for_the_group() {
        // The flag was set on b — the member edited last — while a, the canonical
        // one, never had it. The group keeps the user's latest choice.
        let a = wrapped("a", 10);
        let b = kept(wrapped("b", 11), "2026-03-03T00:00:00Z", Some(true));
        for input in [vec![a.clone(), b.clone()], vec![b, a]] {
            let groups = collapse_highlights(input);
            assert_eq!(groups.len(), 1);
            assert_eq!(groups[0].rep.id, "a");
            assert_eq!(groups[0].rep.keep_line_breaks, Some(true));
            assert_eq!(groups[0].display_text(), WRAPPED);
        }
    }

    #[test]
    fn an_older_members_flag_is_overridden_by_a_newer_one_without_it() {
        // a (canonical) was set to keep at T2, then b was edited later at T3 with
        // the flag cleared — the later edit wins, so the group merges again.
        let a = kept(wrapped("a", 10), "2026-03-02T00:00:00Z", Some(true));
        let b = kept(wrapped("b", 11), "2026-03-03T00:00:00Z", None);
        let groups = collapse_highlights(vec![a, b]);
        assert_eq!(groups[0].rep.keep_line_breaks, None);
        assert_eq!(groups[0].display_text(), MERGED);

        // An explicit `false` on the newest member merges as well.
        let a = kept(wrapped("a", 10), "2026-03-02T00:00:00Z", Some(true));
        let b = kept(wrapped("b", 11), "2026-03-03T00:00:00Z", Some(false));
        let groups = collapse_highlights(vec![a, b]);
        assert_eq!(groups[0].rep.keep_line_breaks, Some(false));
        assert_eq!(groups[0].display_text(), MERGED);

        // A member never edited counts from its created_at, which is older than
        // any edit: the flag on a lone edited member still decides.
        let a = wrapped("a", 10);
        let b = kept(wrapped("b", 11), "2026-03-03T00:00:00Z", Some(true));
        let groups = collapse_highlights(vec![a, b]);
        assert_eq!(groups[0].display_text(), WRAPPED);
    }

    #[test]
    fn on_a_tie_the_lowest_page_decides_the_line_breaks() {
        // Equal edit times, opposite choices: the earlier member (lowest page)
        // wins, whichever way the input arrived.
        for (a_flag, b_flag, expected_keep) in [
            (Some(true), None, true),
            (None, Some(true), false),
            (Some(true), Some(false), true),
            (Some(false), Some(true), false),
        ] {
            for reversed in [false, true] {
                let a = kept(wrapped("a", 10), "2026-03-02T00:00:00Z", a_flag);
                let b = kept(wrapped("b", 11), "2026-03-02T00:00:00Z", b_flag);
                let input = if reversed { vec![b, a] } else { vec![a, b] };
                let groups = collapse_highlights(input);
                assert_eq!(groups.len(), 1);
                assert_eq!(groups[0].rep.id, "a");
                assert_eq!(groups[0].rep.keep_line_breaks, a_flag, "{a_flag:?} {b_flag:?}");
                assert_eq!(
                    groups[0].display_text(),
                    if expected_keep { WRAPPED } else { MERGED },
                    "{a_flag:?} {b_flag:?} reversed={reversed}"
                );
            }
        }

        // Neither edited: both fall back to created_at, which is equal for a
        // pair, so the canonical member's own flag is the group's.
        let mut a = wrapped("a", 10);
        a.keep_line_breaks = Some(true);
        let groups = collapse_highlights(vec![wrapped("b", 11), a]);
        assert_eq!(groups[0].display_text(), WRAPPED);
    }

    #[test]
    fn a_lone_record_passes_through_with_its_own_flag() {
        let solo = kept(wrapped("s", 3), "2026-03-02T00:00:00Z", Some(true));
        let groups = collapse_highlights(vec![solo.clone()]);
        assert_eq!(groups[0].rep.keep_line_breaks, Some(true));
        assert_eq!(groups[0].display_text(), WRAPPED);

        let groups = collapse_highlights(vec![wrapped("s", 3)]);
        assert_eq!(groups[0].rep.keep_line_breaks, None);
        assert_eq!(groups[0].display_text(), MERGED);

        // A lone survivor of a pair is an ordinary highlight and merges too.
        let groups = collapse_highlights(vec![wrapped("b", 11)]);
        assert_eq!(groups[0].page_end, None);
        assert_eq!(groups[0].display_text(), MERGED);
    }

    #[test]
    fn an_ebook_record_keeps_its_newlines_in_the_display_text() {
        let mut o = wrapped("o", 3);
        o.start_offset = Some(1);
        o.end_offset = Some(40);
        let groups = collapse_highlights(vec![o]);
        assert_eq!(groups[0].display_text(), WRAPPED);
    }

    #[test]
    fn entries_keep_the_order_of_first_appearance() {
        let mut other = hl("o", 4);
        other.text = "unrelated".into();
        let mut last = hl("z", 40);
        last.text = "another".into();
        let input = vec![other, hl("b", 11), last, hl("a", 10)];
        let before: Vec<String> = input.iter().map(|h| h.id.clone()).collect();

        let groups = collapse_highlights(input);
        // The pair sits where its first record (b) was; the canonical id is a.
        assert_eq!(ids(&groups), vec!["o", "a", "z"]);
        assert_eq!(groups[1].member_ids, vec!["a", "b"]);
        assert_eq!(before, vec!["o", "b", "z", "a"]);

        // Records of a non-group bucket keep their own positions even when the
        // bucket's records are not adjacent.
        let mut x = hl("x", 5);
        x.text = "between".into();
        let groups = collapse_highlights(vec![hl("p", 10), x, hl("q", 10)]);
        assert_eq!(ids(&groups), vec!["p", "x", "q"]);
    }

    #[test]
    fn the_input_records_are_not_altered() {
        // `collapse_highlights` takes ownership, so "not mutating" is checked on
        // a clone: what it returns for a group is the canonical member with the
        // merged fields, and the untouched half is exactly what was passed in.
        let a = with_note(edited(hl("a", 10), "2026-03-02T00:00:00Z", "#111111", "highlight"), "n1");
        let b = with_note(edited(hl("b", 11), "2026-03-04T00:00:00Z", "#222222", "underline"), "n2");
        let (a0, b0) = (a.clone(), b.clone());
        let groups = collapse_highlights(vec![a, b]);
        let rep = &groups[0].rep;
        assert_eq!(rep.id, a0.id);
        assert_eq!(rep.page, a0.page);
        assert_eq!(rep.text, a0.text);
        assert_eq!(rep.created_at, a0.created_at);
        assert_eq!(rep.updated_at, a0.updated_at);
        assert_eq!(rep.rects.len(), a0.rects.len());
        // Only the three merged fields differ.
        assert_eq!(rep.note.as_deref(), Some("n1\n\nn2"));
        assert_eq!(rep.color, b0.color);
        assert_eq!(rep.style, b0.style);

        // An ordinary record is handed back as it came.
        let solo = with_note(hl("s", 3), "  keep me  ");
        let groups = collapse_highlights(vec![solo.clone()]);
        assert_eq!(groups[0].rep.note, solo.note);
        assert_eq!(groups[0].rep.color, solo.color);
        assert_eq!(groups[0].rep.updated_at, solo.updated_at);
    }

    #[test]
    fn an_empty_list_collapses_to_nothing() {
        assert!(collapse_highlights(Vec::new()).is_empty());
    }

    /// The whole path on disk: a pair stored by the viewer reads back as one
    /// entry, while the raw records — what the frontend store round-trips — are
    /// still both there.
    #[test]
    fn read_grouped_collapses_a_stored_pair_without_touching_the_file() {
        let pair = vec![with_note(hl("a", 10), "n"), hl("b", 11)];
        let lib = fixtures::library_with("a-paper", &pair);
        let file = lib.root.join("papers/a-paper/highlights.json");
        let before = std::fs::read_to_string(&file).unwrap();

        let groups = read_grouped(lib.root(), "a-paper");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].rep.id, "a");
        assert_eq!(groups[0].page_end, Some(11));
        assert_eq!(groups[0].rep.note.as_deref(), Some("n"));

        // The raw read is unchanged and the file was not rewritten.
        assert_eq!(crate::paper::read_highlights(lib.root(), "a-paper").len(), 2);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    }
}
