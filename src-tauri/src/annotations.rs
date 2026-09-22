//! Exporting what the reader wrote: highlights and notes, out of the library and
//! into a file the user owns.
//!
//! Everything here is read-only. It gathers a paper's metadata, its highlights
//! (including the note attached to a highlight) and its notes, and renders them
//! as Markdown or JSON. PDF is not produced here — it goes through a print
//! preview window in the frontend, because a PDF written from Rust would need an
//! embedded CJK font and this app ships none; the system's own fonts do the job
//! for free once the content is HTML.
//!
//! ## Why the structs are not the storage structs
//!
//! [`Highlight`] carries `rects` — page coordinates that mean nothing outside the
//! viewer — and both it and [`Note`] carry ids that are meaningless once the data
//! leaves the library. An export is a document, not a backup, so the shapes here
//! keep what a reader would want and drop the rest. That also makes the JSON a
//! stable thing to promise: it does not change shape when the storage format
//! does.

use serde::Serialize;

use crate::models::PaperMeta;

/// One highlight, as it appears in an export.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedHighlight {
    /// PDF: 1-based page. Ebooks: 1-based chapter index.
    pub page: u32,
    pub text: String,
    /// The comment the user attached to this highlight, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub color: String,
    pub style: String,
    pub created_at: String,
}

/// One note document, as it appears in an export.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedNote {
    pub title: String,
    /// The note's Markdown, verbatim.
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedPaper {
    pub slug: String,
    pub title: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub venue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doi: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arxiv_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub reading_status: String,
    pub highlights: Vec<ExportedHighlight>,
    pub notes: Vec<ExportedNote>,
}

impl ExportedPaper {
    /// Whether this paper has anything worth exporting. A library is mostly
    /// papers nobody has annotated yet, and a file full of empty sections is
    /// worse than one that says which papers had nothing.
    pub fn is_empty(&self) -> bool {
        self.highlights.is_empty() && self.notes.iter().all(|n| n.content.trim().is_empty())
    }
}

/// One file the export produced.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    /// Where the file goes, relative to the export root and always
    /// `/`-separated. A single-file export is just a path with no directory in
    /// it; a folder export puts `批注/…` and `笔记/…/…` in here.
    pub path: String,
    pub content: String,
}

// ── Gathering ────────────────────────────────────────────────────────────────

/// Read one paper's metadata, highlights and notes.
///
/// A paper that cannot be read (deleted between the right-click and the export,
/// a corrupt meta.json) is skipped by the caller rather than failing the whole
/// export — one bad entry should not cost the user the other twenty.
fn collect_one(root: &str, slug: &str) -> Option<ExportedPaper> {
    let meta: PaperMeta = crate::paper::read_meta(root, slug).ok()?;

    let mut highlights: Vec<ExportedHighlight> = crate::paper::read_highlights(root, slug)
        .into_iter()
        .map(|h| ExportedHighlight {
            page: h.page,
            text: h.text,
            note: h.note.filter(|n| !n.trim().is_empty()),
            color: h.color,
            style: h.style,
            created_at: h.created_at,
        })
        .collect();
    // Reading order, not creation order: an export is something you read top to
    // bottom beside the paper, and the order highlights were made in is rarely
    // the order they appear on the page.
    highlights.sort_by(|a, b| {
        a.page
            .cmp(&b.page)
            .then_with(|| a.created_at.cmp(&b.created_at))
    });

    let notes = crate::paper::list_notes(root, slug)
        .into_iter()
        .map(|n| ExportedNote {
            content: crate::paper::get_note(root, slug, &n.id),
            title: n.title,
            created_at: n.created_at,
            updated_at: n.updated_at,
        })
        .collect();

    Some(ExportedPaper {
        slug: slug.to_string(),
        title: meta.title,
        authors: meta.authors,
        year: meta.year,
        venue: meta.venue,
        doi: meta.doi,
        arxiv_id: meta.arxiv_id,
        tags: meta.tags,
        reading_status: meta.reading_status,
        highlights,
        notes,
    })
}

/// The notes worth writing out: an empty note is a note the user opened once
/// and never typed in, and it should not become a file.
///
/// Shared so the tree builder and the Markdown renderer agree on both the set
/// and its order — the renderer links to files the builder writes, and the two
/// drifting apart would produce links to nothing.
fn live_notes(p: &ExportedPaper) -> Vec<&ExportedNote> {
    p.notes
        .iter()
        .filter(|n| !n.content.trim().is_empty())
        .collect()
}

pub fn collect(root: &str, slugs: &[String]) -> Vec<ExportedPaper> {
    slugs
        .iter()
        .filter_map(|s| collect_one(root, s))
        .collect()
}

// ── Markdown ─────────────────────────────────────────────────────────────────

/// A colour swatch the highlight's own colour, named. Falls back to the raw
/// value so an unrecognised colour still says something.
fn color_name(color: &str) -> &str {
    match color.to_lowercase().trim_start_matches('#') {
        c if c.starts_with("ffd") || c.starts_with("fff") || c.starts_with("ffe") => "黄",
        c if c.starts_with("a7f") || c.starts_with("b9f") || c.starts_with("9f") => "绿",
        c if c.starts_with("a5d") || c.starts_with("9ec") || c.starts_with("bfd") => "蓝",
        c if c.starts_with("f9a") || c.starts_with("ffb") || c.starts_with("fca") => "橙",
        c if c.starts_with("f8b") || c.starts_with("fbb") || c.starts_with("ffc") => "粉",
        _ => "",
    }
}

/// Indent a block of text as a Markdown blockquote, so a multi-line highlight
/// stays one quote instead of collapsing into the surrounding paragraph.
fn blockquote(text: &str) -> String {
    text.lines()
        .map(|l| if l.trim().is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Demote every ATX heading in a note by `by` levels, so the note's own outline
/// nests *under* the heading its title occupies instead of outranking it.
///
/// `by` is the level of the note's title, which puts the note's `#` one step
/// below it. A note is authored as a standalone document and usually starts at
/// `# `, so without this a note's top heading outranks the paper it belongs to.
///
/// Clamped at six, where Markdown stops: past that the hashes stop being a
/// heading at all, and a flattened-but-visible heading beats an invisible one.
///
/// Fenced code blocks are skipped: a `#` at the start of a line inside a shell
/// snippet is a comment, not a heading, and rewriting it would corrupt the code.
fn demote_headings(md: &str, by: usize) -> String {
    let mut out = Vec::new();
    let mut in_fence = false;
    let mut fence: &str = "";
    for line in md.lines() {
        let trimmed = line.trim_start();
        if !in_fence && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            in_fence = true;
            fence = if trimmed.starts_with("```") { "```" } else { "~~~" };
            out.push(line.to_string());
            continue;
        }
        if in_fence {
            if trimmed.starts_with(fence) {
                in_fence = false;
            }
            out.push(line.to_string());
            continue;
        }
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            // Six hashes is Markdown's floor; anything already past it is not a
            // heading, so it is left exactly as the user typed it.
            if (1..=6).contains(&hashes) && trimmed.chars().nth(hashes) == Some(' ') {
                let target = (hashes + by).min(6);
                out.push(format!("{}{}", "#".repeat(target - hashes), trimmed));
                continue;
            }
        }
        out.push(line.to_string());
    }
    out.join("\n")
}

/// Where a paper's notes are, from the point of view of the section rendering
/// that paper.
enum Notes<'a> {
    /// Rendered in place, under the paper's own heading. What a single
    /// combined document wants: one file, everything in it.
    Inline,
    /// Written as their own files. `dir` is the note folder relative to the
    /// export root, and `files` are the names inside it — one per
    /// [`live_notes`] entry, in the same order. The caller supplies them
    /// rather than the renderer deriving them, so a link can never point at a
    /// name the writer deduplicated into something else.
    Folder { dir: &'a str, files: &'a [String] },
}

/// Escape the few characters that would otherwise be Markdown structure when a
/// title is dropped into running text, a link label, or a table cell.
fn md_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn citation_line(p: &ExportedPaper) -> String {
    let mut bits: Vec<String> = Vec::new();
    if !p.authors.is_empty() {
        // Long author lists are the norm and are not what anyone reads an export
        // for, so they are cut the way a citation would.
        let shown = if p.authors.len() > 4 {
            format!("{} 等", p.authors[..3].join(", "))
        } else {
            p.authors.join(", ")
        };
        bits.push(shown);
    }
    if let Some(v) = p.venue.as_ref().filter(|v| !v.trim().is_empty()) {
        bits.push(v.clone());
    }
    if let Some(y) = p.year {
        bits.push(y.to_string());
    }
    bits.join(" · ")
}

/// Render one paper as a Markdown section.
fn paper_markdown(p: &ExportedPaper, heading: &str, note_mode: Notes<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("{heading} {}\n\n", p.title));

    let cite = citation_line(p);
    if !cite.is_empty() {
        out.push_str(&format!("*{cite}*\n\n"));
    }
    let mut ids: Vec<String> = Vec::new();
    if let Some(d) = p.doi.as_ref().filter(|d| !d.trim().is_empty()) {
        ids.push(format!("DOI: {d}"));
    }
    if let Some(a) = p.arxiv_id.as_ref().filter(|a| !a.trim().is_empty()) {
        ids.push(format!("arXiv: {a}"));
    }
    if !p.tags.is_empty() {
        ids.push(format!("标签: {}", p.tags.join("、")));
    }
    if !ids.is_empty() {
        out.push_str(&format!("{}\n\n", ids.join(" · ")));
    }

    if !p.highlights.is_empty() {
        out.push_str(&format!("{heading}# 批注（{}）\n\n", p.highlights.len()));
        let mut last_page = None;
        for h in &p.highlights {
            if last_page != Some(h.page) {
                out.push_str(&format!("**第 {} 页**\n\n", h.page));
                last_page = Some(h.page);
            }
            let swatch = color_name(&h.color);
            if swatch.is_empty() {
                out.push_str(&format!("{}\n\n", blockquote(h.text.trim())));
            } else {
                out.push_str(&format!("{}\n>\n> — {swatch}\n\n", blockquote(h.text.trim())));
            }
            if let Some(note) = &h.note {
                out.push_str(&format!("{}\n\n", note.trim()));
            }
        }
    }

    let notes = live_notes(p);
    if !notes.is_empty() {
        out.push_str(&format!("{heading}# 笔记\n\n"));
        match note_mode {
            Notes::Inline => {
                // The note's title sits two levels below the paper's, and the
                // note's own headings go one below that — so a note starting at
                // `# ` cannot outrank the title of the note it is inside.
                let title_level = heading.len() + 2;
                for n in notes {
                    out.push_str(&format!("{heading}## {}\n\n", n.title));
                    out.push_str(&format!("{}\n\n", demote_headings(n.content.trim(), title_level)));
                }
            }
            Notes::Folder { dir, files } => {
                // Each note kept its own file, so it is still the standalone
                // document the user wrote — no demotion, no reflowing. This
                // section only says where they went.
                out.push_str(&format!("这篇论文的 {} 篇笔记导出成了单独的文件：\n\n", notes.len()));
                for (n, file) in notes.iter().zip(files) {
                    out.push_str(&format!("- [{}](<../{dir}/{file}>)\n", md_text(&n.title)));
                }
                out.push('\n');
            }
        }
    }

    if p.is_empty() {
        out.push_str("*这篇论文还没有批注或笔记。*\n\n");
    }
    out
}

/// Every selected paper in one Markdown document.
pub fn to_markdown_combined(papers: &[ExportedPaper], exported_at: &str) -> String {
    let mut out = String::from("# 批注与笔记导出\n\n");
    let hl: usize = papers.iter().map(|p| p.highlights.len()).sum();
    out.push_str(&format!(
        "导出时间：{exported_at} · {} 篇论文 · {hl} 条批注\n\n---\n\n",
        papers.len()
    ));
    for p in papers {
        out.push_str(&paper_markdown(p, "##", Notes::Inline));
        out.push_str("---\n\n");
    }
    out
}

// ── Filenames ────────────────────────────────────────────────────────────────

/// Turn a title into something every filesystem accepts.
///
/// The slug would be safe by construction, but it is a machine name; a user
/// exporting twenty papers wants to find them by title afterwards. Falls back to
/// the slug when a title is entirely punctuation or non-filesystem characters.
pub fn safe_stem(title: &str, slug: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| match c {
            // Reserved on Windows, plus the separators and control characters.
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            c if (c as u32) < 0x20 => ' ',
            c => c,
        })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    // Trailing dots and spaces are stripped by Windows on creation, which turns
    // two distinct names into one silently.
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    let base = if cleaned.is_empty() { slug } else { cleaned };
    // Byte-truncated, not char-truncated, because the limit filesystems enforce
    // is on bytes — and a CJK title hits 255 bytes at about 85 characters.
    let mut truncated = String::new();
    for c in base.chars() {
        if truncated.len() + c.len_utf8() > 120 {
            break;
        }
        truncated.push(c);
    }
    truncated.trim().to_string()
}

/// [`safe_stem`] with an extension on it.
pub fn safe_filename(title: &str, slug: &str, ext: &str) -> String {
    format!("{}.{ext}", safe_stem(title, slug))
}

/// Hands out names that are unique within one directory.
///
/// Two papers can share a title, and one paper can have two notes called
/// "笔记". Whoever wrote second would silently overwrite the first, so a
/// repeat gets a ` (2)` before its extension the way a file manager would.
/// Case-insensitive, because macOS and Windows are.
#[derive(Default)]
struct NameSet {
    used: std::collections::HashSet<String>,
}

impl NameSet {
    fn unique(&mut self, name: &str) -> String {
        let (stem, ext) = match name.rsplit_once('.') {
            Some((s, e)) if !s.is_empty() => (s, format!(".{e}")),
            _ => (name, String::new()),
        };
        let mut candidate = name.to_string();
        let mut n = 2;
        while !self.used.insert(candidate.to_lowercase()) {
            candidate = format!("{stem} ({n}){ext}");
            n += 1;
        }
        candidate
    }
}

// ── Folder export ────────────────────────────────────────────────────────────
//
// A pile of files dropped into whatever folder the user picked is not an
// export, it is a mess someone has to tidy. What lands is one self-contained
// folder:
//
//     批注与笔记-20260922-2210/
//     ├── 总览.md            ← what is in here, with links
//     ├── 批注/<论文>.md      ← metadata + highlights, one file per paper
//     └── 笔记/<论文>/<笔记>.md
//
// Notes get their own tree because a note is a document the user wrote, not an
// appendix to a highlight list: kept as its own file it stays importable into
// Obsidian or anything else that reads a folder of Markdown, and its content
// survives byte for byte instead of being demoted to fit under someone else's
// heading.

/// The folder name an export creates inside the directory the user picked.
pub fn export_dir_name(file_stamp: &str) -> String {
    format!("批注与笔记-{file_stamp}")
}

/// Quote a value for YAML front matter.
///
/// Double-quoted style because a title is arbitrary text: it can start with a
/// `#`, contain a `:`, or be the word `null`, and every one of those means
/// something else unquoted.
fn yaml_str(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if (c as u32) < 0x20 { ' ' } else { c })
        .collect();
    format!("\"{}\"", flat.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One note, as its own Markdown file.
///
/// The body is the user's Markdown verbatim — no demotion, no renumbering.
/// Context goes in YAML front matter instead of a heading, which keeps the
/// note's own outline intact and is what every Markdown notes app reads.
pub fn note_markdown(paper: &ExportedPaper, note: &ExportedNote) -> String {
    let mut out = String::from("---\n");
    out.push_str(&format!("title: {}\n", yaml_str(&note.title)));
    out.push_str(&format!("paper: {}\n", yaml_str(&paper.title)));
    if !paper.authors.is_empty() {
        out.push_str(&format!(
            "authors: [{}]\n",
            paper.authors.iter().map(|a| yaml_str(a)).collect::<Vec<_>>().join(", ")
        ));
    }
    if let Some(y) = paper.year {
        out.push_str(&format!("year: {y}\n"));
    }
    if let Some(a) = paper.arxiv_id.as_ref().filter(|a| !a.trim().is_empty()) {
        out.push_str(&format!("arxiv: {}\n", yaml_str(a)));
    }
    if let Some(d) = paper.doi.as_ref().filter(|d| !d.trim().is_empty()) {
        out.push_str(&format!("doi: {}\n", yaml_str(d)));
    }
    if !paper.tags.is_empty() {
        out.push_str(&format!(
            "tags: [{}]\n",
            paper.tags.iter().map(|t| yaml_str(t)).collect::<Vec<_>>().join(", ")
        ));
    }
    if !note.created_at.trim().is_empty() {
        out.push_str(&format!("created: {}\n", yaml_str(&note.created_at)));
    }
    if !note.updated_at.trim().is_empty() {
        out.push_str(&format!("updated: {}\n", yaml_str(&note.updated_at)));
    }
    out.push_str("---\n\n");
    out.push_str(note.content.trim());
    out.push('\n');
    out
}

/// What the overview says about one paper, and where its files went.
struct TreeEntry {
    title: String,
    slug: String,
    /// Relative path of the per-paper file, absent when the paper had no
    /// highlights and the Markdown tree therefore wrote none.
    paper_file: Option<String>,
    highlights: usize,
    /// Relative path of the note folder, absent when the paper has no notes.
    note_dir: Option<String>,
    notes: usize,
}

fn overview_markdown(entries: &[TreeEntry], stamp: &str) -> String {
    let hl: usize = entries.iter().map(|e| e.highlights).sum();
    let nt: usize = entries.iter().map(|e| e.notes).sum();
    let mut out = String::from("# 批注与笔记导出\n\n");
    out.push_str(&format!(
        "导出时间：{stamp} · {} 篇论文 · {hl} 条批注 · {nt} 篇笔记\n\n",
        entries.len()
    ));
    out.push_str("| 论文 | 批注 | 笔记 |\n| --- | --- | --- |\n");
    for e in entries {
        // An em dash rather than a 0: the cell is about whether there is a file
        // to open, and "0" invites the reader to go looking for an empty one.
        let title = match &e.paper_file {
            Some(f) => format!("[{}](<{f}>)", md_text(&e.title)),
            None => md_text(&e.title),
        };
        let hl = if e.highlights > 0 { e.highlights.to_string() } else { "—".into() };
        let nt = match &e.note_dir {
            Some(d) => format!("[{} 篇](<{d}/>)", e.notes),
            None => "—".into(),
        };
        out.push_str(&format!("| {title} | {hl} | {nt} |\n"));
    }
    out.push('\n');
    out
}

fn overview_json(entries: &[TreeEntry], stamp: &str) -> serde_json::Value {
    serde_json::json!({
        "exportedAt": stamp,
        "paperCount": entries.len(),
        "highlightCount": entries.iter().map(|e| e.highlights).sum::<usize>(),
        "noteCount": entries.iter().map(|e| e.notes).sum::<usize>(),
        "papers": entries.iter().map(|e| serde_json::json!({
            "title": e.title,
            "slug": e.slug,
            "file": e.paper_file,
            "highlights": e.highlights,
            "noteDir": e.note_dir,
            "notes": e.notes,
        })).collect::<Vec<_>>(),
    })
}

/// Lay the whole export out as relative paths and contents.
///
/// Returns the tree rather than writing it, so the shape is testable without
/// touching a filesystem and the caller keeps every decision about where the
/// bytes land.
pub fn build_tree(
    papers: &[ExportedPaper],
    format: &str,
    stamp: &str,
) -> Result<Vec<ExportFile>, String> {
    let json = match format {
        "json" => true,
        "markdown" => false,
        other => return Err(format!("不支持的导出格式：{other}")),
    };
    // JSON is the format someone scripts against, so its per-paper file stays
    // the whole lossless record — notes included, even though they are also
    // written as Markdown. Markdown's per-paper file is highlights only,
    // because duplicating a note there would defeat giving it its own file.
    let paper_dir = if json { "论文" } else { "批注" };

    let mut files: Vec<ExportFile> = Vec::new();
    let mut entries: Vec<TreeEntry> = Vec::new();
    let mut paper_names = NameSet::default();

    for p in papers {
        let stem = paper_names.unique(&safe_stem(&p.title, &p.slug));
        let notes = live_notes(p);

        let (note_dir, note_files) = if notes.is_empty() {
            (None, Vec::new())
        } else {
            let dir = format!("笔记/{stem}");
            let mut seen = NameSet::default();
            let names: Vec<String> = notes
                .iter()
                .map(|n| seen.unique(&safe_filename(&n.title, "笔记", "md")))
                .collect();
            for (n, name) in notes.iter().zip(&names) {
                files.push(ExportFile {
                    path: format!("{dir}/{name}"),
                    content: note_markdown(p, n),
                });
            }
            (Some(dir), names)
        };

        let paper_file = if json {
            let path = format!("{paper_dir}/{stem}.json");
            files.push(ExportFile {
                path: path.clone(),
                content: serde_json::to_string_pretty(p)
                    .map_err(|e| format!("序列化失败：{e}"))?,
            });
            Some(path)
        } else if p.highlights.is_empty() {
            // Nothing to put in it. A paper whose only content is notes is
            // already fully represented by its note folder, and an empty
            // highlights file would just be a file that says nothing.
            None
        } else {
            let path = format!("{paper_dir}/{stem}.md");
            let mode = match &note_dir {
                Some(dir) => Notes::Folder { dir, files: &note_files },
                None => Notes::Inline,
            };
            files.push(ExportFile {
                path: path.clone(),
                content: paper_markdown(p, "#", mode),
            });
            Some(path)
        };

        entries.push(TreeEntry {
            title: p.title.clone(),
            slug: p.slug.clone(),
            paper_file,
            highlights: p.highlights.len(),
            note_dir,
            notes: notes.len(),
        });
    }

    // The index goes first so a caller writing sequentially creates it before
    // anything can fail halfway.
    let overview = if json {
        ExportFile {
            path: "总览.json".into(),
            content: serde_json::to_string_pretty(&overview_json(&entries, stamp))
                .map_err(|e| format!("序列化失败：{e}"))?,
        }
    } else {
        ExportFile {
            path: "总览.md".into(),
            content: overview_markdown(&entries, stamp),
        }
    };
    files.insert(0, overview);
    Ok(files)
}

/// Write a built tree into `dir`, creating subdirectories as it goes.
///
/// `dir` must already exist and be one the caller just created, so nothing here
/// can overwrite something the user had.
pub fn write_tree(dir: &std::path::Path, files: &[ExportFile]) -> Result<(), String> {
    for f in files {
        // `build_tree` composes every path from `safe_stem`, which strips the
        // separators — so this cannot trip today. It guards the invariant for
        // whoever adds the next kind of file: one `..` component and the write
        // lands outside the folder the user chose.
        if f.path.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            return Err(format!("导出路径不合法：{}", f.path));
        }
        let target = dir.join(f.path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建 {} 失败：{e}", parent.display()))?;
        }
        std::fs::write(&target, f.content.as_bytes())
            .map_err(|e| format!("写入 {} 失败：{e}", target.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paper(title: &str, highlights: Vec<ExportedHighlight>, notes: Vec<ExportedNote>) -> ExportedPaper {
        ExportedPaper {
            slug: "a-paper".into(),
            title: title.into(),
            authors: vec!["Ada Lovelace".into(), "Alan Turing".into()],
            year: Some(2024),
            venue: Some("NeurIPS".into()),
            doi: None,
            arxiv_id: Some("2401.00001".into()),
            tags: vec!["方法".into()],
            reading_status: "read".into(),
            highlights,
            notes,
        }
    }

    fn hl(page: u32, text: &str, note: Option<&str>, created: &str) -> ExportedHighlight {
        ExportedHighlight {
            page,
            text: text.into(),
            note: note.map(str::to_string),
            color: "#ffd400".into(),
            style: "highlight".into(),
            created_at: created.into(),
        }
    }

    /// Renders a realistic paper to stdout so the output can be eyeballed.
    /// Ignored by default — a tool, not an assertion.
    #[test]
    #[ignore]
    fn dump_sample_export() {
        let p = ExportedPaper {
            slug: "knighter".into(),
            title: "KNighter: Transforming Static Analysis with LLM-Synthesized Checkers".into(),
            authors: vec!["Chenyuan Yang".into(), "Zijie Zhao".into(), "Lingming Zhang".into()],
            year: Some(2025),
            venue: Some("arXiv".into()),
            doi: None,
            arxiv_id: Some("2503.09002".into()),
            tags: vec!["方法".into()],
            reading_status: "read".into(),
            highlights: vec![
                hl(1, "We present KNighter, the first approach to\nsynthesizing static analyzers from historical bug patterns.",
                   Some("这个思路值得记一下：把历史 bug 当成 checker 的训练信号。"), "2026-01-01T10:00:00Z"),
                hl(3, "the synthesized checkers found 70 new bugs in the Linux kernel", None, "2026-01-01T11:00:00Z"),
            ],
            notes: vec![ExportedNote {
                title: "读后感".into(),
                content: "# 总体判断\n\n方法本身不复杂，价值在**数据**。\n\n```bash\n# 复现命令\nmake checkers\n```\n\n## 待办\n- [ ] 找一下他们的 checker 仓库".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-03T00:00:00Z".into(),
            }],
        };
        let empty = ExportedPaper { slug: "iris".into(), title: "IRIS: LLM-Assisted Static Analysis".into(),
            authors: vec!["A".into()], year: Some(2024), venue: None, doi: None, arxiv_id: None,
            tags: vec![], reading_status: "reading".into(), highlights: vec![], notes: vec![] };
        for f in build_tree(&[p.clone(), empty.clone()], "markdown", "2026-09-22 22:10").unwrap() {
            println!("\n══════ {} ══════\n{}", f.path, f.content);
        }
        println!("\n══════ 总览.json ══════\n{}",
            build_tree(&[p, empty], "json", "2026-09-22 22:10").unwrap()[0].content);
    }

    #[test]
    fn a_multi_line_highlight_stays_one_blockquote() {
        // Without the per-line prefix the second line would merge into the
        // paragraph after the quote.
        assert_eq!(blockquote("first\nsecond"), "> first\n> second");
        // A blank line inside the quote keeps the quote open.
        assert_eq!(blockquote("a\n\nb"), "> a\n>\n> b");
    }

    /// A note's own headings must nest under the paper's section rather than
    /// competing with it — otherwise a note starting with `# 结论` outranks the
    /// paper title in the combined document's outline.
    #[test]
    fn note_headings_are_demoted() {
        // In the combined document the note title is `####`, so the note's own
        // `#` has to land at `#####` — one below it, not above.
        assert_eq!(demote_headings("# 结论\n正文", 4), "##### 结论\n正文");
        // Not a heading: no space after the hashes.
        assert_eq!(demote_headings("#hashtag", 4), "#hashtag");
        // Clamped at six rather than emitting hashes Markdown stops honouring.
        assert_eq!(demote_headings("### 深一层", 4), "###### 深一层");
        assert_eq!(demote_headings("##### 五级", 4), "###### 五级");
        assert_eq!(demote_headings("###### 六级", 4), "###### 六级");
    }

    /// The whole point of the demotion: inside a rendered section, a note's own
    /// outline must sit strictly below the heading its title occupies.
    #[test]
    fn a_notes_headings_never_outrank_its_own_title() {
        let level_of = |line: &str| line.chars().take_while(|c| *c == '#').count();
        for heading in ["#", "##"] {
            let p = paper(
                "T",
                vec![],
                vec![ExportedNote {
                    title: "读后感".into(),
                    content: "# 总体判断\n\n## 待办".into(),
                    created_at: String::new(),
                    updated_at: String::new(),
                }],
            );
            let md = paper_markdown(&p, heading, Notes::Inline);
            let title_line = md
                .lines()
                .find(|l| l.trim_start_matches('#').trim() == "读后感")
                .unwrap();
            let body_h1 = md
                .lines()
                .find(|l| l.trim_start_matches('#').trim() == "总体判断")
                .unwrap();
            assert!(
                level_of(body_h1) > level_of(title_line),
                "heading={heading}: note body `{body_h1}` outranks its title `{title_line}`"
            );
        }
    }

    /// A `#` at the start of a line inside a fence is a shell comment. Demoting
    /// it would silently corrupt the user's code.
    #[test]
    fn headings_inside_code_fences_are_left_alone() {
        let md = "# 标题\n\n```bash\n# 这是注释\necho hi\n```\n\n# 又一个标题";
        let out = demote_headings(md, 2);
        assert!(out.contains("### 标题"));
        assert!(out.contains("\n# 这是注释\n"), "code comment was rewritten:\n{out}");
        assert!(out.contains("### 又一个标题"));
    }

    #[test]
    fn highlights_are_ordered_by_page_then_time() {
        // `collect_one` sorts; this asserts the ordering rule it applies.
        let mut hs = [
            hl(7, "later page", None, "2026-01-01T00:00:00Z"),
            hl(2, "second on page 2", None, "2026-01-02T00:00:00Z"),
            hl(2, "first on page 2", None, "2026-01-01T00:00:00Z"),
        ];
        hs.sort_by(|a, b| a.page.cmp(&b.page).then_with(|| a.created_at.cmp(&b.created_at)));
        assert_eq!(
            hs.iter().map(|h| h.text.as_str()).collect::<Vec<_>>(),
            vec!["first on page 2", "second on page 2", "later page"]
        );
    }

    #[test]
    fn a_paper_section_carries_its_citation_and_its_annotations() {
        let p = paper(
            "Attention Is All You Need",
            vec![hl(3, "the quick brown fox", Some("这里是重点"), "2026-01-01T00:00:00Z")],
            vec![ExportedNote {
                title: "读后感".into(),
                content: "# 一级标题\n内容".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-02T00:00:00Z".into(),
            }],
        );
        let md = paper_markdown(&p, "##", Notes::Inline);
        assert!(md.starts_with("## Attention Is All You Need\n"));
        assert!(md.contains("*Ada Lovelace, Alan Turing · NeurIPS · 2024*"));
        assert!(md.contains("arXiv: 2401.00001"));
        assert!(md.contains("### 批注（1）"));
        assert!(md.contains("**第 3 页**"));
        assert!(md.contains("> the quick brown fox"));
        assert!(md.contains("这里是重点"));
        assert!(md.contains("### 笔记"));
        assert!(md.contains("#### 读后感"));
        // The note's own `#` heading nested strictly under the note's `####`.
        assert!(md.contains("##### 一级标题"), "{md}");
    }

    #[test]
    fn a_paper_with_nothing_says_so_instead_of_rendering_blank() {
        let p = paper("Untouched", vec![], vec![]);
        assert!(p.is_empty());
        assert!(paper_markdown(&p, "##", Notes::Inline).contains("还没有批注或笔记"));

        // A note that exists but is empty does not count as content.
        let p = paper(
            "Untouched",
            vec![],
            vec![ExportedNote {
                title: "空笔记".into(),
                content: "   \n".into(),
                created_at: String::new(),
                updated_at: String::new(),
            }],
        );
        assert!(p.is_empty());
        assert!(!paper_markdown(&p, "##", Notes::Inline).contains("### 笔记"));
    }

    #[test]
    fn a_long_author_list_is_cut_like_a_citation() {
        let mut p = paper("X", vec![], vec![]);
        p.authors = (1..=9).map(|i| format!("Author {i}")).collect();
        assert_eq!(citation_line(&p), "Author 1, Author 2, Author 3 等 · NeurIPS · 2024");
    }

    #[test]
    fn the_combined_document_counts_what_it_contains() {
        let papers = vec![
            paper("One", vec![hl(1, "a", None, "t"), hl(2, "b", None, "t")], vec![]),
            paper("Two", vec![hl(1, "c", None, "t")], vec![]),
        ];
        let md = to_markdown_combined(&papers, "2026-09-22 14:00");
        assert!(md.contains("2 篇论文 · 3 条批注"));
        assert!(md.contains("## One"));
        assert!(md.contains("## Two"));
    }

    /// A repeated name would silently overwrite whatever wrote first.
    #[test]
    fn duplicate_names_get_a_suffix_instead_of_clobbering() {
        let mut n = NameSet::default();
        assert_eq!(n.unique("A.md"), "A.md");
        assert_eq!(n.unique("A.md"), "A (2).md");
        assert_eq!(n.unique("A.md"), "A (3).md");
        // Case-insensitive, because the two filesystems this ships on are.
        assert_eq!(n.unique("a.md"), "a (4).md");
        // A folder name has no extension to insert before.
        assert_eq!(n.unique("论文"), "论文");
        assert_eq!(n.unique("论文"), "论文 (2)");
    }

    fn tree_of(papers: &[ExportedPaper], format: &str) -> Vec<ExportFile> {
        build_tree(papers, format, "2026-09-22 22:10").unwrap()
    }

    fn paths(files: &[ExportFile]) -> Vec<&str> {
        files.iter().map(|f| f.path.as_str()).collect()
    }

    /// The shape the user asked for: one folder, highlights in theirs, notes in
    /// a folder of their own with one Markdown file each.
    #[test]
    fn the_markdown_tree_puts_notes_in_their_own_folder() {
        let p = paper(
            "Attention Is All You Need",
            vec![hl(3, "the quick brown fox", None, "t")],
            vec![
                ExportedNote {
                    title: "读后感".into(),
                    content: "# 总体判断\n很好".into(),
                    created_at: "2026-01-01T00:00:00Z".into(),
                    updated_at: "2026-01-02T00:00:00Z".into(),
                },
                ExportedNote {
                    title: "空的".into(),
                    content: "  \n".into(),
                    created_at: String::new(),
                    updated_at: String::new(),
                },
            ],
        );
        let files = tree_of(&[p], "markdown");
        assert_eq!(
            paths(&files),
            vec![
                "总览.md",
                "笔记/Attention Is All You Need/读后感.md",
                "批注/Attention Is All You Need.md",
            ],
            // The empty note produced no file.
        );
    }

    /// A note file is the user's document, not a section of someone else's:
    /// its own headings must survive untouched, with the context in front
    /// matter instead.
    #[test]
    fn a_note_file_keeps_its_markdown_verbatim() {
        let p = paper(
            "T: a study",
            vec![],
            vec![ExportedNote {
                title: "读后感".into(),
                content: "# 总体判断\n\n```bash\n# 复现\nmake\n```".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-03T00:00:00Z".into(),
            }],
        );
        let files = tree_of(&[p], "markdown");
        let note = files.iter().find(|f| f.path.starts_with("笔记/")).unwrap();
        assert!(note.content.starts_with("---\ntitle: \"读后感\"\n"));
        // A colon in a title is YAML structure unless the value is quoted.
        assert!(note.content.contains("paper: \"T: a study\""));
        assert!(note.content.contains("created: \"2026-01-01T00:00:00Z\""));
        // Verbatim: still `#`, not demoted to fit under anyone's heading.
        assert!(note.content.contains("\n# 总体判断\n"), "{}", note.content);
        assert!(note.content.contains("\n# 复现\n"));
    }

    /// A paper whose only content is notes gets no highlights file — an empty
    /// one would just be a file that says nothing.
    #[test]
    fn a_paper_with_only_notes_gets_no_highlights_file() {
        let p = paper(
            "Notes only",
            vec![],
            vec![ExportedNote {
                title: "想法".into(),
                content: "x".into(),
                created_at: String::new(),
                updated_at: String::new(),
            }],
        );
        let files = tree_of(&[p], "markdown");
        assert_eq!(paths(&files), vec!["总览.md", "笔记/Notes only/想法.md"]);
        // ...and the overview still lists it, pointing at the notes.
        let ov = &files[0].content;
        assert!(ov.contains("| Notes only | — | [1 篇](<笔记/Notes only/>) |"), "{ov}");
    }

    /// The links in the per-paper file have to name the files that were
    /// actually written, including after a duplicate title was renamed.
    #[test]
    fn note_links_match_the_files_on_disk() {
        let p = paper(
            "Dup",
            vec![hl(1, "h", None, "t")],
            vec![
                ExportedNote { title: "笔记".into(), content: "one".into(),
                    created_at: String::new(), updated_at: String::new() },
                ExportedNote { title: "笔记".into(), content: "two".into(),
                    created_at: String::new(), updated_at: String::new() },
            ],
        );
        let files = tree_of(&[p], "markdown");
        assert_eq!(
            paths(&files),
            vec!["总览.md", "笔记/Dup/笔记.md", "笔记/Dup/笔记 (2).md", "批注/Dup.md"]
        );
        let doc = &files.iter().find(|f| f.path == "批注/Dup.md").unwrap().content;
        assert!(doc.contains("(<../笔记/Dup/笔记.md>)"), "{doc}");
        assert!(doc.contains("(<../笔记/Dup/笔记 (2).md>)"), "{doc}");
        // The notes went elsewhere, so the section points instead of repeating.
        assert!(!doc.contains("one"), "note body was duplicated into the paper file:\n{doc}");
    }

    /// Two papers can share a title. Neither may overwrite the other.
    #[test]
    fn papers_sharing_a_title_get_separate_files() {
        let a = paper("Same", vec![hl(1, "a", None, "t")], vec![]);
        let b = paper("Same", vec![hl(1, "b", None, "t")], vec![]);
        let files = tree_of(&[a, b], "markdown");
        assert_eq!(paths(&files), vec!["总览.md", "批注/Same.md", "批注/Same (2).md"]);
    }

    /// JSON keeps the lossless record per paper — notes included — because it
    /// is the format someone scripts against; the readable note files are for
    /// the human and come as well, not instead.
    #[test]
    fn the_json_tree_is_lossless_and_still_writes_note_files() {
        let p = paper(
            "J",
            vec![hl(1, "h", None, "t")],
            vec![ExportedNote { title: "n".into(), content: "body".into(),
                created_at: String::new(), updated_at: String::new() }],
        );
        let files = tree_of(&[p], "json");
        assert_eq!(paths(&files), vec!["总览.json", "笔记/J/n.md", "论文/J.json"]);
        let record: serde_json::Value =
            serde_json::from_str(&files.iter().find(|f| f.path == "论文/J.json").unwrap().content)
                .unwrap();
        assert_eq!(record["notes"][0]["content"], "body");
        let index: serde_json::Value = serde_json::from_str(&files[0].content).unwrap();
        assert_eq!(index["papers"][0]["file"], "论文/J.json");
        assert_eq!(index["papers"][0]["noteDir"], "笔记/J");
        assert_eq!(index["highlightCount"], 1);
    }

    /// Nothing `build_tree` emits may climb out of the export folder, and a
    /// title full of separators is the way that would happen.
    #[test]
    fn no_path_can_escape_the_export_folder() {
        let p = paper("../../etc/passwd", vec![hl(1, "h", None, "t")], vec![]);
        for f in tree_of(&[p], "markdown") {
            assert!(!f.path.starts_with('/'), "{}", f.path);
            for part in f.path.split('/') {
                assert!(!part.is_empty() && part != "." && part != "..", "{}", f.path);
            }
        }
    }

    /// A `|` in a title would end the cell early and shear the table.
    #[test]
    fn a_pipe_in_a_title_does_not_break_the_overview_table() {
        let p = paper("A | B", vec![hl(1, "h", None, "t")], vec![]);
        let ov = &tree_of(&[p], "markdown")[0].content;
        let row = ov.lines().find(|l| l.contains("A \\| B")).unwrap();
        assert_eq!(row.matches('|').count() - row.matches("\\|").count(), 4, "{row}");
    }

    /// End to end against a real filesystem: the folder the user gets.
    #[test]
    fn the_tree_lands_on_disk_as_the_folder_it_describes() {
        let dir = std::env::temp_dir().join(format!(
            "argus-export-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();

        let p = paper(
            "Attention Is All You Need",
            vec![hl(3, "the quick brown fox", Some("重点"), "t")],
            vec![ExportedNote {
                title: "读后感".into(),
                content: "# 总体判断\n很好".into(),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-02T00:00:00Z".into(),
            }],
        );
        let files = tree_of(&[p], "markdown");
        write_tree(&dir, &files).unwrap();

        let mut on_disk: Vec<String> = Vec::new();
        fn walk(base: &std::path::Path, at: &std::path::Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(at).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    walk(base, &path, out);
                } else {
                    out.push(
                        path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"),
                    );
                }
            }
        }
        walk(&dir, &dir, &mut on_disk);
        on_disk.sort();
        assert_eq!(
            on_disk,
            vec![
                "总览.md".to_string(),
                "批注/Attention Is All You Need.md".to_string(),
                "笔记/Attention Is All You Need/读后感.md".to_string(),
            ]
        );
        let note = std::fs::read_to_string(dir.join("笔记/Attention Is All You Need/读后感.md")).unwrap();
        assert!(note.contains("\n# 总体判断\n"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The one thing `write_tree` must never do.
    #[test]
    fn write_tree_refuses_a_path_that_would_escape() {
        let dir = std::env::temp_dir().join("argus-export-escape-test");
        let _ = std::fs::create_dir(&dir);
        let err = write_tree(
            &dir,
            &[ExportFile { path: "../escaped.md".into(), content: "x".into() }],
        )
        .unwrap_err();
        assert!(err.contains("导出路径不合法"), "{err}");
        assert!(!dir.parent().unwrap().join("escaped.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn filenames_survive_titles_that_filesystems_would_refuse() {
        assert_eq!(safe_filename("A/B: C?", "slug", "md"), "A B C.md");
        // Nothing usable left -> fall back to the slug, which is safe by
        // construction.
        assert_eq!(safe_filename("///", "my-slug", "json"), "my-slug.json");
        assert_eq!(safe_filename("   ", "my-slug", "md"), "my-slug.md");
        // Windows strips a trailing dot on creation, quietly merging two names.
        assert_eq!(safe_filename("Report.", "s", "md"), "Report.md");
        // Byte-bounded, so a CJK title cannot overflow a 255-byte name.
        let long = "标".repeat(200);
        let name = safe_filename(&long, "s", "md");
        assert!(name.len() <= 124, "{} bytes", name.len());
        assert!(name.ends_with(".md"));
    }
}
