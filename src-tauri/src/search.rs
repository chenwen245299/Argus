use std::path::Path;

use rusqlite::{params, Connection};

use crate::{models::SearchHit, paper};

const SEARCH_INDEX_VERSION: &str = "2";

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

fn db_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join("search.db")
}

fn version_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join("search.version")
}

// ── Connection + schema ───────────────────────────────────────────────────────

fn open_db(root: &str) -> Result<Connection, String> {
    let path = db_path(root);
    let conn = Connection::open(&path).map_err(|e| format!("Open search.db: {e}"))?;
    // WAL mode for better concurrent access.
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|e| format!("WAL pragma: {e}"))?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS papers_fts USING fts5(
            paper_id UNINDEXED,
            slug     UNINDEXED,
            title,
            authors,
            content,
            tokenize = 'unicode61'
        );",
    )
    .map_err(|e| format!("Create FTS5 table: {e}"))
}

/// Open DB; if it fails (corrupt), delete and recreate.
fn get_or_recreate_db(root: &str) -> Result<Connection, String> {
    match open_db(root) {
        Ok(c) => Ok(c),
        Err(_) => {
            let path = db_path(root);
            let _ = std::fs::remove_file(&path);
            open_db(root)
        }
    }
}

fn all_notes_text(root: &str, slug: &str) -> String {
    let mut parts = Vec::new();

    for note in paper::list_notes(root, slug) {
        let body = paper::get_note(root, slug, &note.id);
        if note.title.trim().is_empty() && body.trim().is_empty() {
            continue;
        }
        parts.push(format!("{}\n{}", note.title, body));
    }

    if parts.is_empty() {
        let legacy = paper::read_notes(root, slug);
        if !legacy.trim().is_empty() {
            parts.push(legacy);
        }
    }

    parts.join("\n\n")
}

fn search_content(meta: &crate::models::PaperMeta, fulltext: &str, notes: &str) -> String {
    let mut parts = Vec::new();
    parts.push(format!("Title\n{}", meta.title.as_str()));

    if !meta.authors.is_empty() {
        parts.push(format!("Authors\n{}", meta.authors.join(", ")));
    }
    if let Some(venue) = meta.venue.as_deref().filter(|v| !v.trim().is_empty()) {
        parts.push(format!("Venue\n{venue}"));
    }
    if let Some(doi) = meta.doi.as_deref().filter(|v| !v.trim().is_empty()) {
        parts.push(format!("DOI\n{doi}"));
    }
    if let Some(arxiv_id) = meta.arxiv_id.as_deref().filter(|v| !v.trim().is_empty()) {
        parts.push(format!("arXiv\n{arxiv_id}"));
    }
    if let Some(abstract_text) = meta
        .paper_abstract
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        parts.push(format!("Abstract\n{abstract_text}"));
    }
    if !fulltext.trim().is_empty() {
        parts.push(format!("Full Text\n{fulltext}"));
    }
    if !notes.trim().is_empty() {
        parts.push(format!("Notes\n{notes}"));
    }

    parts.join("\n\n")
}

// ── Index operations ──────────────────────────────────────────────────────────

/// Add or update a single paper in the FTS index.
/// Reads `fulltext.txt` and `meta.json` from the paper dir.
pub fn index_paper(root: &str, slug: &str) -> Result<(), String> {
    let paper_dir = paper::paper_dir(root, slug);

    let meta_content = std::fs::read_to_string(paper_dir.join("meta.json"))
        .map_err(|e| format!("Read meta.json for {slug}: {e}"))?;
    let meta: crate::models::PaperMeta =
        serde_json::from_str(&meta_content).map_err(|e| format!("Parse meta.json: {e}"))?;

    let fulltext = std::fs::read_to_string(paper_dir.join("fulltext.txt")).unwrap_or_default();
    let notes = all_notes_text(root, slug);
    let content = search_content(&meta, &fulltext, &notes);

    let authors_str = meta.authors.join(", ");

    let conn = get_or_recreate_db(root)?;

    // Delete existing row for this paper (upsert via delete+insert).
    conn.execute(
        "DELETE FROM papers_fts WHERE paper_id = ?1",
        params![meta.id],
    )
    .map_err(|e| format!("Delete existing: {e}"))?;

    conn.execute(
        "INSERT INTO papers_fts (paper_id, slug, title, authors, content)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![meta.id, slug, meta.title, authors_str, content],
    )
    .map_err(|e| format!("Insert FTS row: {e}"))?;

    Ok(())
}

pub fn remove_paper(root: &str, slug: &str) -> Result<(), String> {
    let conn = get_or_recreate_db(root)?;
    conn.execute("DELETE FROM papers_fts WHERE slug = ?1", params![slug])
        .map_err(|e| format!("Delete search row: {e}"))?;
    Ok(())
}

/// Rebuild the entire FTS index from all papers in the library.
pub fn rebuild_index(root: &str) -> Result<(), String> {
    let conn = get_or_recreate_db(root)?;

    // Clear existing data.
    conn.execute("DELETE FROM papers_fts", [])
        .map_err(|e| format!("Clear index: {e}"))?;

    for (slug, path) in paper::list_paper_dirs(root)? {
        let meta_content = match std::fs::read_to_string(path.join("meta.json")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let meta: crate::models::PaperMeta = match serde_json::from_str(&meta_content) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let authors_str = meta.authors.join(", ");
        let fulltext = std::fs::read_to_string(path.join("fulltext.txt")).unwrap_or_default();
        let notes = all_notes_text(root, &slug);
        let content = search_content(&meta, &fulltext, &notes);

        conn.execute(
            "INSERT INTO papers_fts (paper_id, slug, title, authors, content)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![meta.id, slug, meta.title, authors_str, content],
        )
        .map_err(|e| {
            eprintln!("[search] index {slug}: {e}");
            e.to_string()
        })
        .ok();
    }

    Ok(())
}

pub fn ensure_current(root: &str) -> Result<(), String> {
    let path = version_path(root);
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    if current.trim() == SEARCH_INDEX_VERSION {
        return Ok(());
    }

    rebuild_index(root)?;
    std::fs::write(&path, SEARCH_INDEX_VERSION)
        .map_err(|e| format!("Write search index version: {e}"))?;
    Ok(())
}

// ── Search ────────────────────────────────────────────────────────────────────

/// Search the FTS index. Returns up to 50 hits with snippet highlighting.
pub fn search_fulltext(root: &str, query: &str) -> Result<Vec<SearchHit>, String> {
    if query.trim().is_empty() {
        return Ok(vec![]);
    }

    let conn = get_or_recreate_db(root)?;

    // Use control-character markers so we can HTML-escape the full snippet text
    // and then substitute safe <mark> tags — preventing any HTML in PDF content
    // from being injected into the rendered snippet.
    let sql = "
        SELECT
            paper_id,
            slug,
            title,
            authors,
            snippet(papers_fts, 4, char(1), char(2), '…', 24)
        FROM papers_fts
        WHERE papers_fts MATCH ?1
        ORDER BY rank
        LIMIT 50
    ";

    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Prepare search: {e}"))?;

    let rows = stmt
        .query_map(params![query], |row| {
            let paper_id: String = row.get(0)?;
            let slug: String = row.get(1)?;
            let title: String = row.get(2)?;
            let authors_str: String = row.get(3)?;
            let raw: String = row.get(4)?;
            Ok((paper_id, slug, title, authors_str, raw))
        })
        .map_err(|e| format!("Execute search: {e}"))?;

    let mut hits = Vec::new();
    for row_result in rows {
        match row_result {
            Ok((paper_id, slug, title, authors_str, raw)) => {
                let authors: Vec<String> = if authors_str.is_empty() {
                    vec![]
                } else {
                    authors_str.split(", ").map(|s| s.to_string()).collect()
                };
                // HTML-escape the raw snippet, then restore the mark tags
                let snippet = html_escape(&raw)
                    .replace('\x01', "<mark>")
                    .replace('\x02', "</mark>");
                hits.push(SearchHit {
                    paper_id,
                    slug,
                    title,
                    authors,
                    snippet,
                });
            }
            Err(e) => eprintln!("[search] row error: {e}"),
        }
    }

    Ok(hits)
}
