use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, TransactionBehavior};

use crate::{models::SearchHit, paper};

const SEARCH_INDEX_VERSION: i64 = 3;
const MAX_HITS: usize = 50;

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

#[derive(Debug)]
struct QueryTerm {
    text: String,
    quoted: bool,
    prefix: bool,
}

/// Quotes mean a phrase; ordinary words must all match. User text never becomes
/// FTS syntax, including AND/OR, column names, parentheses and unmatched quotes.
fn query_terms(query: &str) -> Vec<QueryTerm> {
    fn push(terms: &mut Vec<QueryTerm>, text: &mut String, quoted: bool) {
        let prefix = !quoted && text.ends_with('*');
        let value = text.trim().trim_end_matches('*').to_string();
        text.clear();
        if value.chars().any(char::is_alphanumeric) {
            terms.push(QueryTerm {
                text: value,
                quoted,
                prefix,
            });
        }
    }
    let mut terms = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    for c in query.chars() {
        if c == '"' {
            push(&mut terms, &mut text, quoted);
            quoted = !quoted;
        } else if c.is_whitespace() && !quoted {
            push(&mut terms, &mut text, false);
        } else {
            text.push(c);
        }
    }
    push(&mut terms, &mut text, quoted);
    terms
}

fn fts_literal(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

fn match_expression(terms: &[QueryTerm], complete_last: bool) -> String {
    terms
        .iter()
        .enumerate()
        .map(|(i, term)| {
            let prefix = term.prefix || (complete_last && i + 1 == terms.len());
            format!(
                "{}{}",
                fts_literal(&term.text),
                if prefix { "*" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

fn db_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join("search.db")
}

fn version_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join("search.version")
}

fn open_db(root: &str) -> Result<Connection, String> {
    let conn = Connection::open(db_path(root)).map_err(|e| format!("Open search.db: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    // A rebuildable cache: WAL keeps the complete previous index readable during
    // rebuilds; NORMAL avoids an fsync on every note/metadata edit.
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
        .map_err(|e| format!("Search pragmas: {e}"))?;
    Ok(conn)
}

fn index_version(conn: &Connection) -> Result<i64, String> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|e| format!("Read search index version: {e}"))
}

fn open_current_db(root: &str) -> Result<Connection, String> {
    let mut conn = open_db(root)?;
    if index_version(&conn)? != SEARCH_INDEX_VERSION {
        rebuild(&mut conn, root, false)?;
    }
    Ok(conn)
}

fn all_notes_text(root: &str, slug: &str) -> String {
    let mut parts = Vec::new();
    for note in paper::list_notes(root, slug) {
        let body = paper::get_note(root, slug, &note.id);
        if !note.title.trim().is_empty() || !body.trim().is_empty() {
            parts.push(format!("{}\n{}", note.title, body));
        }
    }
    if parts.is_empty() && !paper::paper_dir(root, slug).join("notes/index.json").exists() {
        let legacy = paper::read_notes(root, slug);
        if !legacy.trim().is_empty() {
            parts.push(legacy);
        }
    }
    parts.join("\n\n")
}

fn search_content(meta: &crate::models::PaperMeta, fulltext: &str, notes: &str) -> String {
    // These already have weighted columns. Duplicating title/authors distorts
    // BM25, and artificial headings such as "Title" match every paper.
    let mut parts = Vec::new();
    for value in [
        meta.venue.as_deref(),
        meta.doi.as_deref(),
        meta.arxiv_id.as_deref(),
        meta.paper_abstract.as_deref(),
        Some(fulltext),
        Some(notes),
    ] {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            parts.push(value);
        }
    }
    parts.join("\n\n")
}

// External content stores original text once. Indexed identities replace scans
// of UNINDEXED FTS columns; triggers make updates/renames atomic with the index.
const SCHEMA: &str = "
    CREATE TABLE search_documents (
        rowid INTEGER PRIMARY KEY,
        paper_id TEXT NOT NULL UNIQUE,
        slug TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL,
        authors TEXT NOT NULL,
        content TEXT NOT NULL
    );
    CREATE VIRTUAL TABLE papers_fts USING fts5(
        paper_id UNINDEXED, slug UNINDEXED, title, authors, content,
        content='search_documents', content_rowid='rowid',
        tokenize='unicode61', prefix='3'
    );
    CREATE TRIGGER search_insert AFTER INSERT ON search_documents BEGIN
        INSERT INTO papers_fts(rowid, paper_id, slug, title, authors, content)
        VALUES (new.rowid, new.paper_id, new.slug, new.title, new.authors, new.content);
    END;
    CREATE TRIGGER search_delete AFTER DELETE ON search_documents BEGIN
        INSERT INTO papers_fts(papers_fts, rowid, paper_id, slug, title, authors, content)
        VALUES ('delete', old.rowid, old.paper_id, old.slug, old.title, old.authors, old.content);
    END;
    CREATE TRIGGER search_update AFTER UPDATE ON search_documents BEGIN
        INSERT INTO papers_fts(papers_fts, rowid, paper_id, slug, title, authors, content)
        VALUES ('delete', old.rowid, old.paper_id, old.slug, old.title, old.authors, old.content);
        INSERT INTO papers_fts(rowid, paper_id, slug, title, authors, content)
        VALUES (new.rowid, new.paper_id, new.slug, new.title, new.authors, new.content);
    END;
";

const UPSERT: &str = "
    INSERT INTO search_documents(paper_id, slug, title, authors, content)
    VALUES (?1, ?2, ?3, ?4, ?5)
    ON CONFLICT(paper_id) DO UPDATE SET
        slug=excluded.slug, title=excluded.title, authors=excluded.authors, content=excluded.content
";

pub fn index_paper(root: &str, slug: &str) -> Result<(), String> {
    let mut conn = open_current_db(root)?;
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| format!("Begin search update: {e}"))?;
    let dir = paper::paper_dir(root, slug);
    let raw = std::fs::read_to_string(dir.join("meta.json"))
        .map_err(|e| format!("Read meta.json for {slug}: {e}"))?;
    let meta: crate::models::PaperMeta =
        serde_json::from_str(&raw).map_err(|e| format!("Parse meta.json: {e}"))?;
    let fulltext = std::fs::read_to_string(dir.join("fulltext.txt")).unwrap_or_default();
    let content = search_content(&meta, &fulltext, &all_notes_text(root, slug));
    tx.execute(
        "DELETE FROM search_documents WHERE slug=?1 AND paper_id<>?2",
        params![slug, meta.id],
    )
    .map_err(|e| format!("Replace search identity: {e}"))?;
    tx.execute(
        UPSERT,
        params![meta.id, slug, meta.title, meta.authors.join(", "), content],
    )
    .map_err(|e| format!("Update search document: {e}"))?;
    tx.commit()
        .map_err(|e| format!("Commit search update: {e}"))
}

pub fn remove_paper(root: &str, slug: &str) -> Result<(), String> {
    let conn = open_current_db(root)?;
    conn.execute("DELETE FROM search_documents WHERE slug=?1", params![slug])
        .map_err(|e| format!("Delete search row: {e}"))?;
    Ok(())
}

fn rebuild(conn: &mut Connection, root: &str, force: bool) -> Result<(), String> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|e| format!("Begin search rebuild: {e}"))?;
    // Another window/process may have finished the upgrade while we waited.
    if !force && index_version(&tx)? == SEARCH_INDEX_VERSION {
        return Ok(());
    }
    tx.execute_batch("DROP TABLE IF EXISTS papers_fts; DROP TABLE IF EXISTS search_documents;")
        .map_err(|e| format!("Reset search schema: {e}"))?;
    tx.execute_batch(SCHEMA)
        .map_err(|e| format!("Create search schema: {e}"))?;
    {
        let mut insert = tx.prepare(UPSERT).map_err(|e| e.to_string())?;
        for (slug, path) in paper::list_paper_dirs(root)? {
            let meta = std::fs::read_to_string(path.join("meta.json"))
                .ok()
                .and_then(|raw| serde_json::from_str::<crate::models::PaperMeta>(&raw).ok());
            let Some(meta) = meta else { continue };
            let fulltext = std::fs::read_to_string(path.join("fulltext.txt")).unwrap_or_default();
            let content = search_content(&meta, &fulltext, &all_notes_text(root, &slug));
            insert
                .execute(params![
                    meta.id,
                    slug,
                    meta.title,
                    meta.authors.join(", "),
                    content
                ])
                .map_err(|e| format!("Index {slug}: {e}"))?;
        }
    }
    tx.execute_batch("INSERT INTO papers_fts(papers_fts) VALUES ('optimize');")
        .map_err(|e| format!("Optimize search index: {e}"))?;
    tx.pragma_update(None, "user_version", SEARCH_INDEX_VERSION)
        .map_err(|e| e.to_string())?;
    tx.commit()
        .map_err(|e| format!("Commit search rebuild: {e}"))?;
    // SQLite's version commits with the index. A stale sidecar cannot trigger
    // repeated rebuilds or hide an index which was deleted after being stamped.
    crate::fsutil::atomic_write_str(&version_path(root), &SEARCH_INDEX_VERSION.to_string())
        .map_err(|e| format!("Write search index version: {e}"))
}

pub fn rebuild_index(root: &str) -> Result<(), String> {
    rebuild(&mut open_db(root)?, root, true)
}

pub fn ensure_current(root: &str) -> Result<(), String> {
    open_current_db(root).map(|_| ())
}

/// Rank only row IDs first. FTS5's weighted rank + LIMIT can stop early without
/// generating excerpts for every matching paper in a large library.
fn candidates(conn: &Connection, expression: &str) -> Result<Vec<i64>, String> {
    let mut stmt = conn
        .prepare_cached(
            "
        SELECT rowid FROM papers_fts
        WHERE papers_fts MATCH ?1 AND rank MATCH 'bm25(0, 0, 12, 6, 1)'
        ORDER BY rank LIMIT ?2
    ",
        )
        .map_err(|e| format!("Prepare search: {e}"))?;
    let rows = stmt
        .query_map(params![expression, MAX_HITS as i64], |row| row.get(0))
        .map_err(|e| format!("Execute search: {e}"))?;
    rows.collect::<Result<_, _>>()
        .map_err(|e| format!("Read search candidates: {e}"))
}

fn ranked_candidates(
    conn: &Connection,
    terms: &[QueryTerm],
    expression: &str,
    phrases: bool,
) -> Result<Vec<i64>, String> {
    let phrase = fts_literal(
        &terms
            .iter()
            .map(|t| t.text.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    let mut tiers = vec![
        format!("title : {phrase}"),
        format!("title : ({expression})"),
    ];
    if terms.len() > 1 {
        tiers.push(phrase);
    }
    tiers.push(expression.to_string());
    if !phrases || terms.len() == 1 || terms.iter().any(|t| t.prefix) {
        tiers = vec![format!("title : ({expression})"), expression.to_string()];
    }
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for tier in tiers {
        // Each tier already implies every query term (including quoted
        // phrases). Repeating the AND expression doubles position-list work
        // for common words in long papers.
        for id in candidates(conn, &tier)? {
            if seen.insert(id) {
                ids.push(id);
            }
            if ids.len() == MAX_HITS {
                return Ok(ids);
            }
        }
    }
    Ok(ids)
}

/// Up to 50 hits. Prefer title phrases, then title keywords, then body phrases;
/// weighted BM25 orders each tier. Never broaden an AND search into an OR sweep.
pub fn search_fulltext(root: &str, query: &str) -> Result<Vec<SearchHit>, String> {
    let terms = query_terms(query);
    if terms.is_empty() {
        return Ok(vec![]);
    }
    let mut conn = open_current_db(root)?;
    // Ranking and excerpts see one snapshot even if an import commits.
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut expression = match_expression(&terms, false);
    let mut ids = ranked_candidates(&tx, &terms, &expression, true)?;
    if ids.is_empty()
        && terms.last().is_some_and(|t| {
            !t.quoted
                && !t.prefix
                && t.text.chars().count() >= 3
                && t.text.chars().all(char::is_alphanumeric)
        })
    {
        expression = match_expression(&terms, true);
        ids = ranked_candidates(&tx, &terms, &expression, false)?;
    }
    if ids.is_empty() {
        return Ok(vec![]);
    }
    // Fetch excerpts together: opening an FTS MATCH cursor per hit repeatedly
    // decodes common words' posting lists. Only selected rows reach snippet().
    let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "
        SELECT rowid, paper_id, slug, title, authors,
            snippet(papers_fts, -1, char(1), char(2), '…', 24)
        FROM papers_fts WHERE papers_fts MATCH ? AND rowid IN ({placeholders})
    "
    );
    let values = std::iter::once(rusqlite::types::Value::Text(expression))
        .chain(ids.iter().copied().map(rusqlite::types::Value::Integer));
    let mut stmt = tx
        .prepare(&sql)
        .map_err(|e| format!("Prepare search snippets: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(values), |row| {
            let id: i64 = row.get(0)?;
            let authors: String = row.get(4)?;
            let raw: String = row.get(5)?;
            Ok((
                id,
                SearchHit {
                    paper_id: row.get(1)?,
                    slug: row.get(2)?,
                    title: row.get(3)?,
                    authors: if authors.is_empty() {
                        vec![]
                    } else {
                        authors.split(", ").map(str::to_string).collect()
                    },
                    snippet: html_escape(&raw)
                        .replace('\x01', "<mark>")
                        .replace('\x02', "</mark>"),
                },
            ))
        })
        .map_err(|e| format!("Read search snippets: {e}"))?;
    let mut hits = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Read search hit: {e}"))?;
    hits.sort_by_key(|(id, _)| ids.iter().position(|candidate| candidate == id));
    Ok(hits.into_iter().map(|(_, hit)| hit).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PaperMeta;

    struct Library(String);
    impl Library {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("argus-search-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(root.join(".argus")).unwrap();
            std::fs::create_dir_all(root.join("papers")).unwrap();
            Self(root.to_string_lossy().into_owned())
        }
        fn seed(&self, slug: &str, title: &str, authors: &[&str], content: &str) {
            let dir = Path::new(&self.0).join("papers").join(slug);
            std::fs::create_dir_all(&dir).unwrap();
            let meta = serde_json::json!({"id":slug, "title":title, "authors":authors});
            std::fs::write(dir.join("meta.json"), meta.to_string()).unwrap();
            std::fs::write(dir.join("fulltext.txt"), content).unwrap();
        }
        fn slugs(&self, query: &str) -> Vec<String> {
            search_fulltext(&self.0, query)
                .unwrap()
                .into_iter()
                .map(|h| h.slug)
                .collect()
        }
    }
    impl Drop for Library {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn title_phrases_beat_repeated_citations_and_scattered_title_words() {
        let lib = Library::new();
        lib.seed(
            "paper",
            "Your Classifier Is Secretly an Energy Based Model",
            &["Will Grathwohl"],
            &"other background ".repeat(10000),
        );
        lib.seed(
            "citation",
            "A Different Paper",
            &["Other Author"],
            &"Your classifier is secretly an energy based model. ".repeat(100),
        );
        lib.seed(
            "scattered",
            "Your Model Is a Classifier Secretly Learning",
            &[],
            "background",
        );
        lib.seed("partial", "Your Classifier", &[], "unrelated");
        let hits = lib.slugs("your classifier is secretly");
        assert_eq!(hits[0], "paper");
        assert!(
            hits.iter().position(|s| s == "scattered").unwrap()
                < hits.iter().position(|s| s == "citation").unwrap()
        );
        assert!(!hits.contains(&"partial".into()));
    }

    #[test]
    fn phrases_are_exact_and_plain_words_still_use_and() {
        let lib = Library::new();
        lib.seed("phrase", "Energy Based Models", &[], "diffusion");
        lib.seed("scattered", "Energy Models", &[], "based diffusion");
        lib.seed("partial", "Diffusion", &[], "energy only");
        assert_eq!(lib.slugs("\"energy based\" diffusion"), ["phrase"]);
        assert_eq!(lib.slugs("energy based diffusion").len(), 2);
        assert!(lib.slugs("energy nonexistent").is_empty());
    }

    #[test]
    fn author_metadata_abstract_and_notes_are_searchable_without_pdf_text() {
        let lib = Library::new();
        lib.seed("author", "A Paper", &["José Grathwohl"], "");
        let dir = paper::paper_dir(&lib.0, "author");
        let mut meta: PaperMeta =
            serde_json::from_str(&std::fs::read_to_string(dir.join("meta.json")).unwrap()).unwrap();
        meta.paper_abstract = Some("quantum widgets".into());
        meta.venue = Some("ICLR 2026".into());
        meta.doi = Some("10.1234/widgets".into());
        meta.arxiv_id = Some("2601.12345".into());
        std::fs::write(dir.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        paper::create_note_with(&lib.0, "author", "Notebook", "rare personal insight").unwrap();
        for query in [
            "jose grathwohl",
            "quantum widgets",
            "ICLR 2026",
            "10.1234/widgets",
            "2601.12345",
            "personal insight",
        ] {
            assert_eq!(lib.slugs(query), ["author"], "{query}");
        }
        assert!(
            lib.slugs("Full Text").is_empty(),
            "synthetic index headings are not search terms"
        );
    }

    #[test]
    fn prefix_completion_is_a_fallback_and_does_not_relax_other_terms_or_quotes() {
        let lib = Library::new();
        lib.seed("classifier", "Energy Classifier", &[], "");
        lib.seed("classification", "Energy Classification", &[], "");
        lib.seed("elsewhere", "Other Classification", &[], "");
        assert_eq!(lib.slugs("energy classif").len(), 2);
        assert_eq!(lib.slugs("energy classifier"), ["classifier"]);
        assert!(lib.slugs("energy \"classif\"").is_empty());
        assert!(lib.slugs("missing classif").is_empty());
        assert_eq!(lib.slugs("energy classif*").len(), 2);
    }

    #[test]
    fn special_characters_are_safe_and_snippets_only_admit_our_marks() {
        let lib = Library::new();
        lib.seed(
            "html",
            "Safety",
            &[],
            "payload <img src=x onerror=alert(1)> C++ foo:bar AND OR NOT energy-based",
        );
        for query in [
            "C++",
            "foo:bar",
            "AND OR NOT",
            "\"energy-based",
            "(payload)",
            "*",
            ":",
            "\"",
            "()",
            "",
        ] {
            assert!(search_fulltext(&lib.0, query).is_ok(), "{query}");
        }
        let hit = search_fulltext(&lib.0, "payload").unwrap().remove(0);
        assert!(hit.snippet.contains("<mark>payload</mark>"));
        assert!(hit.snippet.contains("&lt;img"));
        assert!(!hit.snippet.contains("<img"));
        assert!(
            lib.slugs("(payload) OR missing").is_empty(),
            "OR is literal text, not query syntax"
        );
    }

    #[test]
    fn updates_renames_removals_and_rebuilds_keep_one_current_record() {
        let lib = Library::new();
        lib.seed("original", "Oldword", &[], "original content");
        assert_eq!(lib.slugs("oldword"), ["original"]);
        let old = paper::paper_dir(&lib.0, "original");
        let new = paper::paper_dir(&lib.0, "renamed");
        std::fs::rename(&old, &new).unwrap();
        let mut meta: PaperMeta =
            serde_json::from_str(&std::fs::read_to_string(new.join("meta.json")).unwrap()).unwrap();
        meta.title = "Newword".into();
        std::fs::write(new.join("meta.json"), serde_json::to_vec(&meta).unwrap()).unwrap();
        index_paper(&lib.0, "renamed").unwrap();
        index_paper(&lib.0, "renamed").unwrap();
        assert!(lib.slugs("oldword").is_empty());
        assert_eq!(lib.slugs("newword"), ["renamed"]);
        rebuild_index(&lib.0).unwrap();
        assert_eq!(lib.slugs("newword"), ["renamed"]);
        remove_paper(&lib.0, "renamed").unwrap();
        assert!(lib.slugs("newword").is_empty());
    }

    #[test]
    fn legacy_and_missing_databases_rebuild_automatically_even_with_current_sidecar() {
        let lib = Library::new();
        lib.seed("paper", "Migrated Search", &[], "");
        let conn = open_db(&lib.0).unwrap();
        conn.execute_batch("CREATE VIRTUAL TABLE papers_fts USING fts5(paper_id UNINDEXED, slug UNINDEXED, title, authors, content);
            INSERT INTO papers_fts VALUES ('stale','stale','Old Cache','','');").unwrap();
        drop(conn);
        std::fs::write(version_path(&lib.0), "2").unwrap();
        assert_eq!(lib.slugs("migrated"), ["paper"]);
        assert!(lib.slugs("old cache").is_empty());
        std::fs::remove_file(db_path(&lib.0)).unwrap();
        assert_eq!(lib.slugs("migrated"), ["paper"]);
        assert_eq!(
            index_version(&open_db(&lib.0).unwrap()).unwrap(),
            SEARCH_INDEX_VERSION
        );
    }

    #[test]
    fn failed_rebuild_rolls_back_and_readers_keep_a_complete_snapshot() {
        let lib = Library::new();
        lib.seed("paper", "Stable Snapshot", &[], "oldword");
        ensure_current(&lib.0).unwrap();
        let mut conn = open_current_db(&lib.0).unwrap();
        let reader = conn.transaction().unwrap();
        assert_eq!(candidates(&reader, "oldword").unwrap().len(), 1);
        lib.seed("paper", "Stable Snapshot", &[], "newword");
        rebuild_index(&lib.0).unwrap();
        assert_eq!(candidates(&reader, "oldword").unwrap().len(), 1);
        reader.commit().unwrap();
        assert_eq!(lib.slugs("newword"), ["paper"]);
        // A filesystem failure after DROP/CREATE must retain the prior index.
        let papers = Path::new(&lib.0).join("papers");
        std::fs::rename(&papers, Path::new(&lib.0).join("held-papers")).unwrap();
        std::fs::write(&papers, "not a directory").unwrap();
        assert!(rebuild_index(&lib.0).is_err());
        assert_eq!(lib.slugs("newword"), ["paper"]);
    }

    #[test]
    fn result_limit_is_unique_and_title_matches_survive_a_large_body_match_set() {
        let lib = Library::new();
        for n in 0..80 {
            lib.seed(
                &format!("body-{n}"),
                "Another Paper",
                &[],
                "energy classifier",
            );
        }
        lib.seed(
            "target",
            "Energy Classifier",
            &[],
            &"background ".repeat(10000),
        );
        let hits = lib.slugs("energy classifier");
        assert_eq!(hits.len(), MAX_HITS);
        assert_eq!(hits[0], "target");
        assert_eq!(hits.iter().collect::<HashSet<_>>().len(), MAX_HITS);
    }
}
