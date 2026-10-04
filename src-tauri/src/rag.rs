use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use rusqlite::{params, Connection};
use tauri::Emitter;

use crate::models::{AiProvider, RagSettings, VectorStoreInfo, VectorsMeta};
use crate::{ai_manager, extraction, llm, paper};

const CONFIG_KEY: &str = "rag_settings";
const VECTORS_META_FILE: &str = "vectors_meta.json";
const DB_FILE: &str = "vectors.sqlite";

// ── Batch cancel ──────────────────────────────────────────────────────────────

static BATCH_CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn batch_cancel() -> &'static Arc<AtomicBool> {
    BATCH_CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

pub fn cancel_batch_vectorize() {
    batch_cancel().store(true, Ordering::SeqCst);
}

// ── Settings ──────────────────────────────────────────────────────────────────

pub fn get_rag_settings(root: &str) -> RagSettings {
    let path = Path::new(root).join(".argus").join("config.json");
    if !path.exists() {
        return RagSettings::default();
    }
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&text).unwrap_or_default();
    map.get(CONFIG_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub fn save_rag_settings(root: &str, settings: &RagSettings) -> Result<(), String> {
    let path = Path::new(root).join(".argus").join("config.json");
    let mut map: serde_json::Map<String, serde_json::Value> = if path.exists() {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        serde_json::from_str(&text).unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    map.insert(
        CONFIG_KEY.to_string(),
        serde_json::to_value(settings).map_err(|e| e.to_string())?,
    );
    let content = serde_json::to_string_pretty(&map).map_err(|e| e.to_string())?;
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| e.to_string())
}

// ── VectorsMeta ───────────────────────────────────────────────────────────────

pub fn get_vectors_meta(root: &str) -> Option<VectorsMeta> {
    let path = Path::new(root).join(".argus").join(VECTORS_META_FILE);
    let text = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_vectors_meta(root: &str, meta: &VectorsMeta) -> Result<(), String> {
    let path = Path::new(root).join(".argus").join(VECTORS_META_FILE);
    let content = serde_json::to_string_pretty(meta).map_err(|e| e.to_string())?;
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| e.to_string())
}

// ── Text chunking ─────────────────────────────────────────────────────────────

/// Paragraph-aware chunking: splits on blank lines, groups paragraphs up to
/// `target_size` chars, keeps last paragraph as overlap into the next chunk.
/// Falls back to character sliding-window for paragraphs longer than target.
fn chunk_text(text: &str, target_size: usize, overlap: usize) -> Vec<String> {
    // Collect non-empty paragraphs (split on one or more blank lines).
    let paragraphs: Vec<String> = text
        .split("\n\n")
        .flat_map(|block| {
            // Secondary split on triple+ newlines within a block
            block.split("\n\n\n").map(|s| s.trim().to_string())
        })
        .filter(|p| !p.is_empty() && p.chars().any(|c| !c.is_whitespace()))
        .collect();

    if paragraphs.is_empty() {
        return Vec::new();
    }

    // Keep overlap below target so a chunk always makes forward progress.
    let overlap = overlap.min(target_size.saturating_sub(1));

    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_len: usize = 0;

    // After emitting a chunk, carry the trailing paragraphs that fit within
    // `overlap` chars into the next chunk so context spans the boundary. The
    // very last paragraph is always kept (even if it alone exceeds `overlap`)
    // unless overlap is 0.
    let flush = |current: &mut Vec<String>, current_len: &mut usize, chunks: &mut Vec<String>| {
        if current.is_empty() {
            return;
        }
        chunks.push(current.join("\n\n"));
        if overlap == 0 {
            current.clear();
            *current_len = 0;
            return;
        }
        let mut kept: Vec<String> = Vec::new();
        let mut kept_len: usize = 0;
        while let Some(p) = current.pop() {
            let l = p.chars().count();
            if kept_len + l > overlap && !kept.is_empty() {
                current.push(p); // doesn't fit — leave it in the flushed chunk
                break;
            }
            kept_len += l;
            kept.push(p);
            if kept_len >= overlap {
                break;
            }
        }
        kept.reverse();
        *current = kept;
        *current_len = kept_len;
    };

    for para in paragraphs {
        let plen = para.chars().count();

        // Long paragraph: character-slide it directly
        if plen > target_size {
            // Emit whatever we've accumulated as its own chunk first.
            if !current.is_empty() {
                chunks.push(current.join("\n\n"));
                current.clear();
                current_len = 0;
            }
            let chars: Vec<char> = para.chars().collect();
            // Step forward by target minus overlap so successive windows share
            // `overlap` chars; the user-configured overlap now drives this.
            let step = target_size.saturating_sub(overlap).max(1);
            let mut s = 0;
            while s < chars.len() {
                let e = (s + target_size).min(chars.len());
                let slice: String = chars[s..e].iter().collect();
                chunks.push(slice.trim().to_string());
                if e >= chars.len() {
                    break;
                }
                s += step;
            }
            continue;
        }

        let sep = if current.is_empty() { 0 } else { 2 }; // "\n\n"
        if current_len + sep + plen > target_size && !current.is_empty() {
            flush(&mut current, &mut current_len, &mut chunks);
        }

        let sep2 = if current.is_empty() { 0 } else { 2 };
        current.push(para);
        current_len += sep2 + plen;
    }

    if !current.is_empty() {
        let s = current.join("\n\n");
        if s.chars().any(|c| !c.is_whitespace()) {
            chunks.push(s);
        }
    }

    chunks
}

// ── SQLite helpers ────────────────────────────────────────────────────────────

fn db_path(root: &str) -> std::path::PathBuf {
    Path::new(root).join(".argus").join(DB_FILE)
}

// Multi-model chunks schema. Vectors are partitioned by `embedding_model` so
// switching embedding models keeps the previous model's vectors intact — the
// primary key is (chunk_id, embedding_model), letting the same logical chunk
// coexist under several models.
const CHUNKS_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS chunks (
             chunk_id        TEXT NOT NULL,
             embedding_model TEXT NOT NULL DEFAULT '',
             paper_id        TEXT NOT NULL,
             slug            TEXT NOT NULL,
             chunk_index     INTEGER NOT NULL,
             text            TEXT NOT NULL,
             vector          BLOB NOT NULL,
             source_type     TEXT NOT NULL DEFAULT 'text',
             source_id       TEXT,
             source_label    TEXT,
             paper_title     TEXT NOT NULL DEFAULT '',
             PRIMARY KEY (chunk_id, embedding_model)
         );
         CREATE INDEX IF NOT EXISTS idx_chunks_paper ON chunks(paper_id);
         CREATE INDEX IF NOT EXISTS idx_chunks_model ON chunks(embedding_model);";

fn table_has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    stmt.query_map([], |r| r.get::<_, String>(1))
        .map(|rows| rows.filter_map(|r| r.ok()).any(|c| c == column))
        .unwrap_or(false)
}

/// Bring the `chunks` table to the multi-model schema. A fresh DB gets the new
/// schema directly; a legacy single-model table (no `embedding_model` column)
/// is migrated in one atomic transaction, tagging every existing row with the
/// model recorded in vectors_meta.json (the model that originally produced it).
fn migrate_chunks_table(conn: &Connection, root: &str) -> Result<(), String> {
    // Acquire the write lock up front. Everything below — including the
    // "is it already migrated?" check — runs inside this transaction, so with
    // several connections opening concurrently only one performs the migration;
    // the others block on BEGIN IMMEDIATE and then observe the finished schema.
    conn.execute_batch("BEGIN IMMEDIATE;")
        .map_err(|e| format!("Begin migration transaction: {e}"))?;

    // Run the migration body in a closure so any error can trigger a ROLLBACK.
    let result = (|| -> Result<(), String> {
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='chunks'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if !exists {
            return conn
                .execute_batch(CHUNKS_SCHEMA)
                .map_err(|e| format!("Init vectors DB: {e}"));
        }

        if table_has_column(conn, "chunks", "embedding_model") {
            // Already multi-model (possibly migrated by a peer connection that
            // held the lock before us) — just make sure the indexes exist.
            let _ = conn.execute_batch(
                "CREATE INDEX IF NOT EXISTS idx_chunks_paper ON chunks(paper_id);
                 CREATE INDEX IF NOT EXISTS idx_chunks_model ON chunks(embedding_model);",
            );
            return Ok(());
        }

        // Legacy single-model table. Ensure it has the text columns this app
        // added over time so the copy below succeeds, then fold it into the new
        // schema.
        for (col, def) in &[
            ("source_type", "TEXT NOT NULL DEFAULT 'text'"),
            ("source_id", "TEXT"),
            ("source_label", "TEXT"),
            ("paper_title", "TEXT NOT NULL DEFAULT ''"),
        ] {
            let _ = conn.execute_batch(&format!("ALTER TABLE chunks ADD COLUMN {col} {def};"));
        }

        let legacy_model = get_vectors_meta(root)
            .map(|m| m.embedding_model)
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| "legacy".to_string());

        // Copy rows with an empty model marker first, then stamp the real model
        // via a parameterized UPDATE. Binding the value (instead of splicing it
        // into the batch SQL) removes any reliance on manual quote-escaping and
        // sidesteps edge cases like a model name containing a NUL that would
        // truncate the `execute_batch` string.
        conn.execute_batch(&format!(
            "ALTER TABLE chunks RENAME TO chunks_legacy;
             {CHUNKS_SCHEMA}
             INSERT INTO chunks
                 (chunk_id, embedding_model, paper_id, slug, chunk_index, text, vector,
                  source_type, source_id, source_label, paper_title)
             SELECT chunk_id, '', paper_id, slug, chunk_index, text, vector,
                    source_type, source_id, source_label, paper_title
             FROM chunks_legacy;
             DROP TABLE chunks_legacy;"
        ))
        .map_err(|e| format!("Migrate chunks to multi-model store: {e}"))?;

        conn.execute(
            "UPDATE chunks SET embedding_model = ?1 WHERE embedding_model = ''",
            rusqlite::params![legacy_model],
        )
        .map_err(|e| format!("Tag migrated chunks with model: {e}"))?;
        Ok(())
    })();

    match result {
        Ok(()) => conn
            .execute_batch("COMMIT;")
            .map_err(|e| format!("Commit migration transaction: {e}")),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK;");
            Err(e)
        }
    }
}

fn open_db(root: &str) -> Result<Connection, String> {
    let path = db_path(root);
    let conn = Connection::open(&path).map_err(|e| format!("Open vectors DB: {e}"))?;
    // Concurrent vectorization opens one connection per paper; WAL allows a
    // single writer at a time, so writers must wait instead of failing with
    // SQLITE_BUSY ("database is locked").
    conn.busy_timeout(std::time::Duration::from_secs(30))
        .map_err(|e| format!("Set busy timeout: {e}"))?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")
        .map_err(|e| format!("Init vectors DB: {e}"))?;
    migrate_chunks_table(&conn, root)?;
    Ok(conn)
}

/// The embedding model currently selected in RAG settings, if configured.
fn current_embedding_model(root: &str) -> Option<String> {
    get_rag_settings(root)
        .embedding_model
        .filter(|m| !m.is_empty())
}

/// Per-model chunk statistics, ordered by chunk count (largest first).
fn list_model_stats(conn: &Connection) -> Result<Vec<crate::models::EmbeddingModelStat>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT embedding_model, COUNT(*), COUNT(DISTINCT paper_id), MAX(length(vector)) \
             FROM chunks GROUP BY embedding_model ORDER BY COUNT(*) DESC",
        )
        .map_err(|e| format!("Prepare model stats: {e}"))?;
    let stats = stmt
        .query_map([], |r| {
            let byte_len: i64 = r.get(3).unwrap_or(0);
            Ok(crate::models::EmbeddingModelStat {
                embedding_model: r.get(0)?,
                total_chunks: r.get::<_, i64>(1)? as usize,
                unique_papers: r.get::<_, i64>(2)? as usize,
                dimension: (byte_len / 4) as usize,
            })
        })
        .map_err(|e| format!("Query model stats: {e}"))?
        .filter_map(|r| r.ok())
        .collect();
    Ok(stats)
}

fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

// ── Resolve embedding provider ────────────────────────────────────────────────

fn resolve_embedding_provider(
    root: &str,
    settings: &RagSettings,
) -> Result<(AiProvider, String, String), String> {
    if !settings.is_configured() {
        return Err("还没有配置向量化：请到 设置 → AI 随航 → RAG / 向量化 里选择服务商和嵌入模型。"
            .to_string());
    }
    let provider_id = settings
        .provider_id
        .as_deref()
        .ok_or("RAG provider_id is not set")?;
    let embedding_model = settings
        .embedding_model
        .as_deref()
        .ok_or("RAG embedding_model is not set")?;
    let (provider, api_key, _) =
        ai_manager::resolve_provider_model(root, Some(provider_id), Some(embedding_model))?;
    Ok((provider, api_key, embedding_model.to_string()))
}

// ── Update vectorized status ──────────────────────────────────────────────────

fn update_vectorized(root: &str, slug: &str, value: bool) -> Result<(), String> {
    let mut status = paper::read_status_for(root, slug);
    status.vectorized = value;
    status.last_updated = chrono::Utc::now().to_rfc3339();
    paper::write_status(root, slug, &status)
}

// ── Vectorize single paper ────────────────────────────────────────────────────

struct PendingChunk {
    text: String,
    source_type: &'static str,
    source_id: Option<String>,
    source_label: Option<String>,
}

pub async fn vectorize_paper(root: &str, slug: &str, app: &tauri::AppHandle) -> Result<(), String> {
    let settings = get_rag_settings(root);
    if !settings.is_configured() {
        return Err("还没有配置向量化：请到 设置 → AI 随航 → RAG / 向量化 里选择服务商和嵌入模型。"
            .to_string());
    }

    let event = format!("vectorize-{}", slug);
    let _ = app.emit(&event, serde_json::json!({"status": "chunking"}));

    let meta = paper::read_meta(root, slug).map_err(|e| format!("Read meta: {e}"))?;
    let paper_id = meta.id.clone();
    let paper_title = meta.title.clone();

    let mut pending: Vec<PendingChunk> = Vec::new();

    // ── 1. Fulltext chunks ───────────────────────────────────────────────────
    let fulltext = extraction::read_fulltext(root, slug);
    if !fulltext.is_empty() {
        for chunk in chunk_text(&fulltext, settings.chunk_size, settings.chunk_overlap) {
            pending.push(PendingChunk {
                text: chunk,
                source_type: "text",
                source_id: None,
                source_label: None,
            });
        }
    }

    // ── 2. Metadata chunk ────────────────────────────────────────────────────
    {
        let mut parts = vec![format!("标题: {}", meta.title)];
        if !meta.authors.is_empty() {
            parts.push(format!("作者: {}", meta.authors.join(", ")));
        }
        if let Some(y) = meta.year {
            parts.push(format!("年份: {y}"));
        }
        if let Some(ref v) = meta.venue {
            parts.push(format!("发表于: {v}"));
        }
        if !meta.tags.is_empty() {
            parts.push(format!("标签: {}", meta.tags.join(", ")));
        }
        if let Some(ref doi) = meta.doi {
            parts.push(format!("DOI: {doi}"));
        }
        if let Some(ref arxiv) = meta.arxiv_id {
            parts.push(format!("arXiv: {arxiv}"));
        }
        pending.push(PendingChunk {
            text: parts.join("\n"),
            source_type: "metadata",
            source_id: None,
            source_label: Some("论文基本信息".to_string()),
        });
    }

    // ── 3. Highlight chunks ──────────────────────────────────────────────────
    // Ebook highlights anchor to chapters, not pages — label accordingly.
    let unit = if crate::ebook::is_ebook_file_type(meta.file_type.as_deref()) {
        "章"
    } else {
        "页"
    };
    // One chunk per selection, not per stored record — see `highlight_inputs`,
    // which this shares with `get_paper_vectorize_input` so the two builders
    // cannot drift apart.
    for h in highlight_inputs(root, slug) {
        let mut text = format!("高亮文本 (第{}{unit}): {}", h.page, h.text);
        if let Some(ref note) = h.note {
            text.push_str(&format!("\n用户批注: {}", note.trim()));
        }
        pending.push(PendingChunk {
            text,
            source_type: "highlight",
            source_id: Some(h.id.clone()),
            source_label: Some(format!("第{}{unit}批注", h.page)),
        });
    }

    // ── 4. Notes chunks ──────────────────────────────────────────────────────
    for note in paper::list_notes(root, slug) {
        let content = paper::get_note(root, slug, &note.id);
        if content.trim().is_empty() {
            continue;
        }
        let note_chunks = chunk_text(&content, settings.chunk_size, settings.chunk_overlap);
        for (i, chunk) in note_chunks.into_iter().enumerate() {
            let label = if i == 0 {
                format!("笔记: {}", note.title)
            } else {
                format!("笔记: {} (续{})", note.title, i + 1)
            };
            pending.push(PendingChunk {
                text: chunk,
                source_type: "note",
                source_id: Some(note.id.clone()),
                source_label: Some(label),
            });
        }
    }

    if pending.is_empty() {
        return Err(
            "No content to vectorize — extract fulltext or add highlights/notes first.".to_string(),
        );
    }

    let (provider, api_key, emb_model) = resolve_embedding_provider(root, &settings)?;

    let texts: Vec<String> = pending.iter().map(|c| c.text.clone()).collect();
    let total = texts.len();
    let _ = app.emit(
        &event,
        serde_json::json!({"status": "embedding", "total": total}),
    );
    let embeddings = llm::embeddings(&provider, &api_key, &emb_model, &texts, "embedding").await?;

    // The chunk↔embedding pairing below relies on positional `zip`, which would
    // silently drop the tail (or misalign text with vectors) if the provider
    // returned a different count. Refuse rather than corrupt the index.
    if embeddings.len() != pending.len() {
        return Err(format!(
            "Embedding count mismatch: sent {} chunks, got {} vectors. Aborting to avoid text/vector misalignment.",
            pending.len(),
            embeddings.len()
        ));
    }
    let dim = embeddings.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 || embeddings.iter().any(|v| v.len() != dim) {
        return Err("Embeddings have zero or inconsistent dimensions.".to_string());
    }

    let _ = app.emit(&event, serde_json::json!({"status": "storing"}));

    let root_str = root.to_string();
    let slug_str = slug.to_string();
    let paper_title_c = paper_title.clone();
    let paper_id_c = paper_id.clone();
    let emb_model_c = emb_model.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = open_db(&root_str)?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin transaction: {e}"))?;

        // Only clear this paper's vectors for the model being (re)built —
        // other models' embeddings of the same paper stay untouched.
        tx.execute(
            "DELETE FROM chunks WHERE paper_id = ?1 AND embedding_model = ?2",
            params![paper_id_c, emb_model_c],
        )
        .map_err(|e| format!("Delete old chunks: {e}"))?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO chunks \
                     (chunk_id, embedding_model, paper_id, slug, chunk_index, text, vector, \
                      source_type, source_id, source_label, paper_title) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .map_err(|e| format!("Prepare insert: {e}"))?;

            for (i, (chunk, emb)) in pending.iter().zip(embeddings.iter()).enumerate() {
                let chunk_id = match chunk.source_type {
                    "metadata" => format!("{}-meta", paper_id_c),
                    "highlight" => format!(
                        "{}-hl-{}",
                        paper_id_c,
                        chunk.source_id.as_deref().unwrap_or(&i.to_string())
                    ),
                    "note" => format!(
                        "{}-note-{}-{}",
                        paper_id_c,
                        chunk.source_id.as_deref().unwrap_or(""),
                        i
                    ),
                    _ => format!("{}-text-{}", paper_id_c, i),
                };
                let blob = vec_to_blob(emb);
                stmt.execute(params![
                    chunk_id,
                    emb_model_c,
                    paper_id_c,
                    slug_str,
                    i as i64,
                    chunk.text,
                    blob,
                    chunk.source_type,
                    chunk.source_id,
                    chunk.source_label,
                    paper_title_c,
                ])
                .map_err(|e| format!("Insert chunk {i}: {e}"))?;
            }
        }

        tx.commit().map_err(|e| format!("Commit transaction: {e}"))
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))??;

    save_vectors_meta(
        root,
        &VectorsMeta {
            provider_id: provider.id.clone(),
            embedding_model: emb_model.clone(),
            dimension: dim,
        },
    )?;
    update_vectorized(root, slug, true)?;
    let _ = app.emit(
        &event,
        serde_json::json!({"status": "done", "chunks": total}),
    );
    Ok(())
}

// ── Frontend-orchestrated vectorize pipeline ─────────────────────────────────

/// The highlights worth embedding, one per selection, each as its display text
/// (merged lines, unless the user keeps the breaks).
///
/// A selection across a page break is stored as a record per page, each holding
/// the whole text; embedding every record would put the same passage on the map
/// twice. The entry keeps the canonical (lowest-page) record's id, so a chunk id
/// (`{paper}-hl-{id}`) is what it always was. Both paths that turn highlights
/// into chunks — `vectorize_paper` here and the frontend's `chunker.ts` via
/// `get_paper_vectorize_input` — read this one list.
fn highlight_inputs(root: &str, slug: &str) -> Vec<crate::models::HighlightInput> {
    crate::highlight_groups::read_grouped(root, slug)
        .into_iter()
        // The text as it reads — one paragraph, not a fragment per printed line —
        // so a chunk (and its embedding) is not cut at every wrap. Emptiness is
        // judged on that text, not the captured one.
        .map(|g| (g.display_text(), g.rep))
        .filter(|(text, _)| !text.trim().is_empty())
        .map(|(text, h)| crate::models::HighlightInput {
            id: h.id,
            page: h.page,
            text: text.trim().to_string(),
            note: h.note.filter(|n| !n.trim().is_empty()),
        })
        .collect()
}

/// Returns all raw content for a paper so the frontend can chunk it with
/// LlamaIndex SentenceSplitter before sending chunks back via embed_and_store_chunks.
pub fn get_paper_vectorize_input(
    root: &str,
    slug: &str,
) -> Result<crate::models::PaperVectorizeInput, String> {
    use crate::models::{NoteInput, PaperVectorizeInput};

    let meta = paper::read_meta(root, slug).map_err(|e| format!("Read meta: {e}"))?;

    // Pre-format the metadata string (will be stored as a single metadata chunk)
    let mut meta_parts = vec![format!("标题: {}", meta.title)];
    if !meta.authors.is_empty() {
        meta_parts.push(format!("作者: {}", meta.authors.join(", ")));
    }
    if let Some(y) = meta.year {
        meta_parts.push(format!("年份: {y}"));
    }
    if let Some(ref v) = meta.venue {
        meta_parts.push(format!("发表于: {v}"));
    }
    if !meta.tags.is_empty() {
        meta_parts.push(format!("标签: {}", meta.tags.join(", ")));
    }
    if let Some(ref doi) = meta.doi {
        meta_parts.push(format!("DOI: {doi}"));
    }
    if let Some(ref arxiv) = meta.arxiv_id {
        meta_parts.push(format!("arXiv: {arxiv}"));
    }

    // The same entries `vectorize_paper` writes chunks for, so the frontend
    // chunker (one chunk per input) needs no change.
    let highlights = highlight_inputs(root, slug);

    let notes = paper::list_notes(root, slug)
        .into_iter()
        .filter_map(|n| {
            let content = paper::get_note(root, slug, &n.id);
            if content.trim().is_empty() {
                None
            } else {
                Some(NoteInput {
                    id: n.id,
                    title: n.title,
                    content: content.trim().to_string(),
                })
            }
        })
        .collect();

    Ok(PaperVectorizeInput {
        paper_id: meta.id,
        paper_title: meta.title,
        meta_text: meta_parts.join("\n"),
        fulltext: extraction::read_fulltext(root, slug),
        highlights,
        notes,
        file_type: meta.file_type,
    })
}

/// Embeds pre-chunked content (produced by the frontend's SentenceSplitter)
/// and stores it in the SQLite vector database.
pub async fn embed_and_store_chunks(
    root: &str,
    slug: &str,
    paper_id: &str,
    paper_title: &str,
    chunks: Vec<crate::models::ChunkInput>,
    app: &tauri::AppHandle,
) -> Result<usize, String> {
    if chunks.is_empty() {
        return Err("No chunks to embed.".to_string());
    }

    let settings = get_rag_settings(root);
    if !settings.is_configured() {
        return Err("还没有配置向量化：请到 设置 → AI 随航 → RAG / 向量化 里选择服务商和嵌入模型。".to_string());
    }

    let (provider, api_key, emb_model) = resolve_embedding_provider(root, &settings)?;

    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let total = texts.len();

    let event = format!("vectorize-{slug}");
    let _ = app.emit(
        &event,
        serde_json::json!({"status": "embedding", "total": total}),
    );

    let embeddings = llm::embeddings(&provider, &api_key, &emb_model, &texts, "embedding").await?;
    if embeddings.len() != chunks.len() {
        return Err(format!(
            "Embedding count mismatch: sent {} chunks, got {} vectors. Aborting to avoid text/vector misalignment.",
            chunks.len(),
            embeddings.len()
        ));
    }
    let dim = embeddings.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 || embeddings.iter().any(|v| v.len() != dim) {
        return Err("Embeddings have zero or inconsistent dimensions.".to_string());
    }

    let _ = app.emit(&event, serde_json::json!({"status": "storing"}));

    let root_str = root.to_string();
    let slug_str = slug.to_string();
    let paper_id_str = paper_id.to_string();
    let paper_title_str = paper_title.to_string();
    let emb_model_c = emb_model.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = open_db(&root_str)?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin transaction: {e}"))?;

        // Only clear this paper's vectors for the model being (re)built —
        // other models' embeddings of the same paper stay untouched.
        tx.execute(
            "DELETE FROM chunks WHERE paper_id = ?1 AND embedding_model = ?2",
            params![paper_id_str, emb_model_c],
        )
        .map_err(|e| format!("Delete old chunks: {e}"))?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO chunks \
                     (chunk_id, embedding_model, paper_id, slug, chunk_index, text, vector, \
                      source_type, source_id, source_label, paper_title) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                )
                .map_err(|e| format!("Prepare insert: {e}"))?;

            for (i, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                let chunk_id = match chunk.source_type.as_str() {
                    "metadata" => format!("{}-meta", paper_id_str),
                    "highlight" => format!(
                        "{}-hl-{}",
                        paper_id_str,
                        chunk.source_id.as_deref().unwrap_or(&i.to_string())
                    ),
                    "note" => format!(
                        "{}-note-{}-{}",
                        paper_id_str,
                        chunk.source_id.as_deref().unwrap_or(""),
                        i
                    ),
                    _ => format!("{}-text-{}", paper_id_str, i),
                };
                let blob = vec_to_blob(emb);
                stmt.execute(params![
                    chunk_id,
                    emb_model_c,
                    paper_id_str,
                    slug_str,
                    i as i64,
                    chunk.text,
                    blob,
                    chunk.source_type,
                    chunk.source_id,
                    chunk.source_label,
                    paper_title_str,
                ])
                .map_err(|e| format!("Insert chunk {i}: {e}"))?;
            }
        }

        tx.commit().map_err(|e| format!("Commit transaction: {e}"))
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))??;

    save_vectors_meta(
        root,
        &VectorsMeta {
            provider_id: provider.id.clone(),
            embedding_model: emb_model.clone(),
            dimension: dim,
        },
    )?;
    update_vectorized(root, slug, true)?;
    let _ = app.emit(
        &event,
        serde_json::json!({"status": "done", "chunks": total}),
    );
    Ok(total)
}

// ── Reconcile vectorized flags with DB ───────────────────────────────────────

/// Compares each paper's `vectorized` status flag against the actual DB.
/// Papers marked as vectorized but missing from the DB are reset to false,
/// and vice versa. Returns (fixed_count, total_count).
pub async fn sync_vectorized_flags(root: &str) -> Result<(usize, usize), String> {
    // The `vectorized` flag tracks whether a paper is embedded under the
    // *currently selected* model — so switching models flips it accordingly.
    let current_model = current_embedding_model(root);
    let embedded_ids: std::collections::HashSet<String> =
        if db_path(root).exists() && current_model.is_some() {
            let root_str = root.to_string();
            let model = current_model.unwrap();
            tokio::task::spawn_blocking(
                move || -> Result<std::collections::HashSet<String>, String> {
                    let conn = open_db(&root_str)?;
                    let mut stmt = conn
                        .prepare("SELECT DISTINCT paper_id FROM chunks WHERE embedding_model = ?1")
                        .map_err(|e| e.to_string())?;
                    let ids = stmt
                        .query_map(params![model], |r| r.get::<_, String>(0))
                        .map_err(|e| e.to_string())?
                        .filter_map(|r| r.ok())
                        .collect();
                    Ok(ids)
                },
            )
            .await
            .map_err(|e| format!("Spawn blocking: {e}"))??
        } else {
            std::collections::HashSet::new()
        };

    let entries = crate::library::scan_library(root).unwrap_or_default();
    let total = entries.len();
    let mut fixed = 0usize;

    for entry in &entries {
        let status = paper::read_status_for(root, &entry.slug);
        let in_db = embedded_ids.contains(&entry.id);
        if status.vectorized != in_db {
            let _ = update_vectorized(root, &entry.slug, in_db);
            fixed += 1;
        }
    }

    Ok((fixed, total))
}

// ── Delete paper chunks ───────────────────────────────────────────────────────

pub async fn delete_paper_chunks(root: &str, paper_id: &str) -> Result<(), String> {
    let root = root.to_string();
    let paper_id = paper_id.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        if !db_path(&root).exists() {
            return Ok(());
        }
        let conn = open_db(&root)?;
        // No model filter: deleting a paper drops its vectors under *every*
        // embedding model.
        conn.execute("DELETE FROM chunks WHERE paper_id = ?1", params![paper_id])
            .map_err(|e| format!("Delete chunks: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))?
}

/// Drop every chunk stored under one embedding model, freeing its partition
/// without touching the other models' vectors. Returns the rows removed.
pub async fn delete_model_embeddings(root: &str, model: &str) -> Result<usize, String> {
    let root_c = root.to_string();
    let model_c = model.to_string();
    let removed = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        if !db_path(&root_c).exists() {
            return Ok(0);
        }
        let conn = open_db(&root_c)?;
        let n = conn
            .execute(
                "DELETE FROM chunks WHERE embedding_model = ?1",
                params![model_c],
            )
            .map_err(|e| format!("Delete model embeddings: {e}"))?;
        Ok(n)
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))??;

    // If we cleared the model that's currently selected, the per-paper
    // `vectorized` flags now overstate reality — reconcile them.
    if current_embedding_model(root).as_deref() == Some(model) {
        let _ = sync_vectorized_flags(root).await;
    }
    Ok(removed)
}

// ── Batch vectorize (rebuild) ─────────────────────────────────────────────────

pub async fn rebuild_vector_store(root: &str, app: &tauri::AppHandle) -> Result<usize, String> {
    batch_cancel().store(false, Ordering::SeqCst);

    let entries = crate::library::scan_library(root).unwrap_or_default();
    let slugs: Vec<String> = entries.iter().map(|e| e.slug.clone()).collect();
    let total = slugs.len();

    // Clean rebuild for the *current* model only — clear just its partition so
    // other models' vectors survive. (Dropping the whole DB would wipe them.)
    if let Some(model) = current_embedding_model(root) {
        let _ = delete_model_embeddings(root, &model).await;
    }

    for slug in &slugs {
        let _ = update_vectorized(root, slug, false);
    }

    let _ = app.emit(
        "vectorize-batch",
        serde_json::json!({"total": total, "done": 0, "failed": 0, "status": "running"}),
    );

    // Embedding API latency dominates each paper, so run a few papers
    // concurrently. SQLite writes serialize on the WAL writer lock (with the
    // busy_timeout set in open_db), so concurrent finishes are safe.
    const VECTORIZE_CONCURRENCY: usize = 3;
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(VECTORIZE_CONCURRENCY));
    let mut join_set: tokio::task::JoinSet<(String, Result<(), String>)> =
        tokio::task::JoinSet::new();

    for slug in slugs {
        let sem = semaphore.clone();
        let root_c = root.to_string();
        let app_c = app.clone();
        join_set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            if batch_cancel().load(Ordering::SeqCst) {
                return (slug, Err("cancelled".to_string()));
            }
            let result = vectorize_paper(&root_c, &slug, &app_c).await;
            (slug, result)
        });
    }

    let mut done = 0usize;
    let mut failed = 0usize;
    let mut cancelled = false;

    while let Some(task_result) = join_set.join_next().await {
        let Ok((slug, result)) = task_result else { continue };
        match result {
            Ok(()) => done += 1,
            Err(e) if e == "cancelled" => {
                cancelled = true;
                continue;
            }
            Err(e) => {
                eprintln!("vectorize {} failed: {}", slug, e);
                failed += 1;
            }
        }

        let _ = app.emit(
            "vectorize-batch",
            serde_json::json!({"total": total, "done": done, "failed": failed, "status": "running"}),
        );
    }

    let status = if cancelled { "cancelled" } else { "done" };
    let _ = app.emit(
        "vectorize-batch",
        serde_json::json!({"total": total, "done": done, "failed": failed, "status": status}),
    );
    Ok(done)
}

// ── Vector store info ─────────────────────────────────────────────────────────

pub async fn get_vector_store_info(root: &str) -> Result<VectorStoreInfo, String> {
    let settings = get_rag_settings(root);
    let current_model = current_embedding_model(root);
    let root = root.to_string();
    tokio::task::spawn_blocking(move || -> Result<VectorStoreInfo, String> {
        // Top-level fields describe the *currently selected* model's partition;
        // `models` lists every model that has vectors stored.
        let base = |models: Vec<crate::models::EmbeddingModelStat>| {
            let cur = current_model
                .as_ref()
                .and_then(|m| models.iter().find(|s| &s.embedding_model == m));
            VectorStoreInfo {
                total_chunks: cur.map(|s| s.total_chunks).unwrap_or(0),
                unique_papers: cur.map(|s| s.unique_papers).unwrap_or(0),
                dimension: cur.map(|s| s.dimension),
                provider_id: settings.provider_id.clone(),
                embedding_model: current_model.clone(),
                models,
            }
        };
        if !db_path(&root).exists() {
            return Ok(base(Vec::new()));
        }
        let conn = open_db(&root)?;
        let models = list_model_stats(&conn)?;
        Ok(base(models))
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))?
}

// ── Embedding map (vector space visualization) ───────────────────────────────

fn l2_normalize(v: &mut [f32]) {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Remove from `w` its projection onto unit vector `p`.
fn orthogonalize(w: &mut [f32], p: &[f32]) {
    let dot: f32 = w.iter().zip(p.iter()).map(|(a, b)| a * b).sum();
    for (wi, pi) in w.iter_mut().zip(p.iter()) {
        *wi -= dot * pi;
    }
}

/// Deterministic pseudo-random unit vector so the layout is stable across runs.
fn pseudo_rand_unit(d: usize, seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut v: Vec<f32> = (0..d)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s as f64 / u64::MAX as f64) as f32 - 0.5
        })
        .collect();
    l2_normalize(&mut v);
    v
}

/// Top principal component of `data` (rows already centered) via power
/// iteration, deflated against `prev` if given.
fn power_iteration_pc(data: &[Vec<f32>], prev: Option<&[f32]>, seed: u64) -> Vec<f32> {
    let d = data.first().map(|v| v.len()).unwrap_or(0);
    let mut w = pseudo_rand_unit(d, seed);
    if let Some(p) = prev {
        orthogonalize(&mut w, p);
        l2_normalize(&mut w);
    }
    for _ in 0..25 {
        let mut nw = vec![0f32; d];
        for x in data {
            let dot: f32 = x.iter().zip(w.iter()).map(|(a, b)| a * b).sum();
            for (ni, xi) in nw.iter_mut().zip(x.iter()) {
                *ni += xi * dot;
            }
        }
        if let Some(p) = prev {
            orthogonalize(&mut nw, p);
        }
        l2_normalize(&mut nw);
        let converged: f32 = nw.iter().zip(w.iter()).map(|(a, b)| a * b).sum();
        w = nw;
        if (1.0 - converged.abs()) < 1e-6 {
            break;
        }
    }
    w
}

pub async fn get_embedding_map(
    root: &str,
    requested_model: Option<String>,
) -> Result<crate::models::EmbeddingMapData, String> {
    use crate::models::{EmbeddingMapChunk, EmbeddingMapData, EmbeddingMapEdge, EmbeddingMapPaper};

    let current_model = current_embedding_model(root);
    // Reading status per paper id, used to tint nodes in the map.
    let reading_status: std::collections::HashMap<String, String> =
        crate::library::scan_library(root)
            .unwrap_or_default()
            .into_iter()
            .map(|e| (e.id, e.reading_status))
            .collect();
    let root = root.to_string();
    tokio::task::spawn_blocking(move || -> Result<EmbeddingMapData, String> {
        let empty = |available: Vec<crate::models::EmbeddingModelStat>| EmbeddingMapData {
            papers: Vec::new(),
            chunks: Vec::new(),
            edges: Vec::new(),
            dimension: 0,
            embedding_model: None,
            available_models: available,
        };
        if !db_path(&root).exists() {
            return Ok(empty(Vec::new()));
        }
        let conn = open_db(&root)?;
        let available = list_model_stats(&conn)?;
        if available.is_empty() {
            return Ok(empty(Vec::new()));
        }

        // Pick the model to render: an explicit request wins, then the model
        // currently selected in settings, then whichever has the most chunks.
        let has = |m: &str| available.iter().any(|s| s.embedding_model == m);
        let target = requested_model
            .filter(|m| has(m))
            .or_else(|| current_model.clone().filter(|m| has(m)))
            .or_else(|| available.first().map(|s| s.embedding_model.clone()));
        let Some(target) = target else {
            return Ok(empty(available));
        };

        struct Row {
            paper_id: String,
            slug: String,
            vector: Vec<f32>,
            source_type: String,
            source_label: Option<String>,
            paper_title: String,
            preview: String,
        }

        let mut stmt = conn
            .prepare(
                "SELECT paper_id, slug, text, vector, source_type, source_label, paper_title \
                 FROM chunks WHERE embedding_model = ?1 ORDER BY paper_id, chunk_index",
            )
            .map_err(|e| format!("Prepare map query: {e}"))?;
        let mut rows: Vec<Row> = stmt
            .query_map(params![target], |r| {
                let text: String = r.get(2)?;
                let blob: Vec<u8> = r.get(3)?;
                let mut preview: String = text.chars().take(90).collect();
                preview = preview.replace(['\n', '\r'], " ");
                if text.chars().count() > 90 {
                    preview.push('…');
                }
                Ok(Row {
                    paper_id: r.get(0)?,
                    slug: r.get(1)?,
                    vector: blob_to_vec(&blob),
                    source_type: r.get(4)?,
                    source_label: r.get(5)?,
                    paper_title: r.get(6)?,
                    preview,
                })
            })
            .map_err(|e| format!("Query chunks: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        // Keep only vectors matching the dominant dimension (guards against
        // leftovers from a previous embedding model).
        let dim = rows.first().map(|r| r.vector.len()).unwrap_or(0);
        rows.retain(|r| r.vector.len() == dim && dim > 0);
        if rows.is_empty() {
            return Ok(empty(available));
        }

        // Normalize chunk vectors so cosine geometry is consistent.
        for r in rows.iter_mut() {
            l2_normalize(&mut r.vector);
        }

        // Group rows into papers (rows are sorted by paper_id).
        let mut papers: Vec<EmbeddingMapPaper> = Vec::new();
        let mut centroids: Vec<Vec<f32>> = Vec::new();
        let mut paper_of_row: Vec<usize> = Vec::with_capacity(rows.len());
        for r in rows.iter() {
            let is_new = papers
                .last()
                .map(|p: &EmbeddingMapPaper| p.paper_id != r.paper_id)
                .unwrap_or(true);
            if is_new {
                papers.push(EmbeddingMapPaper {
                    paper_id: r.paper_id.clone(),
                    slug: r.slug.clone(),
                    title: if r.paper_title.is_empty() {
                        r.slug.clone()
                    } else {
                        r.paper_title.clone()
                    },
                    chunk_count: 0,
                    x: 0.0,
                    y: 0.0,
                    reading_status: reading_status
                        .get(&r.paper_id)
                        .cloned()
                        .unwrap_or_else(|| "unread".to_string()),
                });
                centroids.push(vec![0f32; dim]);
            }
            let idx = papers.len() - 1;
            // Prefer a non-empty title from any chunk of the paper
            if papers[idx].title == papers[idx].slug && !r.paper_title.is_empty() {
                papers[idx].title = r.paper_title.clone();
            }
            papers[idx].chunk_count += 1;
            for (c, v) in centroids[idx].iter_mut().zip(r.vector.iter()) {
                *c += v;
            }
            paper_of_row.push(idx);
        }
        for c in centroids.iter_mut() {
            l2_normalize(c);
        }

        // PCA basis from centered chunk vectors.
        let mut mean = vec![0f32; dim];
        for r in rows.iter() {
            for (m, v) in mean.iter_mut().zip(r.vector.iter()) {
                *m += v;
            }
        }
        let n = rows.len() as f32;
        for m in mean.iter_mut() {
            *m /= n;
        }
        let centered: Vec<Vec<f32>> = rows
            .iter()
            .map(|r| r.vector.iter().zip(mean.iter()).map(|(v, m)| v - m).collect())
            .collect();
        let pc1 = power_iteration_pc(&centered, None, 1);
        let pc2 = power_iteration_pc(&centered, Some(&pc1), 2);

        let project = |v: &[f32]| -> (f32, f32) {
            let cx: f32 = v
                .iter()
                .zip(mean.iter())
                .zip(pc1.iter())
                .map(|((vi, mi), wi)| (vi - mi) * wi)
                .sum();
            let cy: f32 = v
                .iter()
                .zip(mean.iter())
                .zip(pc2.iter())
                .map(|((vi, mi), wi)| (vi - mi) * wi)
                .sum();
            (cx, cy)
        };

        let chunk_xy: Vec<(f32, f32)> = centered
            .iter()
            .map(|c| {
                let x: f32 = c.iter().zip(pc1.iter()).map(|(a, b)| a * b).sum();
                let y: f32 = c.iter().zip(pc2.iter()).map(|(a, b)| a * b).sum();
                (x, y)
            })
            .collect();
        for (p, c) in papers.iter_mut().zip(centroids.iter()) {
            let (x, y) = project(c);
            p.x = x;
            p.y = y;
        }

        // Z-score both axes (based on chunk spread) so the map is roughly
        // isotropic regardless of how dominant PC1 is.
        let axis_std = |get: &dyn Fn(&(f32, f32)) -> f32| -> f32 {
            let mu: f32 = chunk_xy.iter().map(|p| get(p)).sum::<f32>() / n;
            let var: f32 = chunk_xy.iter().map(|p| (get(p) - mu).powi(2)).sum::<f32>() / n;
            var.sqrt().max(1e-6)
        };
        let sx = axis_std(&|p: &(f32, f32)| p.0);
        let sy = axis_std(&|p: &(f32, f32)| p.1);
        for p in papers.iter_mut() {
            p.x /= sx;
            p.y /= sy;
        }

        let chunks: Vec<EmbeddingMapChunk> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| EmbeddingMapChunk {
                paper: paper_of_row[i],
                x: chunk_xy[i].0 / sx,
                y: chunk_xy[i].1 / sy,
                source_type: r.source_type.clone(),
                source_label: r.source_label.clone(),
                preview: r.preview.clone(),
            })
            .collect();

        // Similarity edges: top neighbors per paper centroid.
        const EDGE_TOP_K: usize = 4;
        const EDGE_MIN_SIM: f32 = 0.25;
        let mut edge_set: std::collections::HashMap<(usize, usize), f32> =
            std::collections::HashMap::new();
        for i in 0..centroids.len() {
            let mut sims: Vec<(usize, f32)> = (0..centroids.len())
                .filter(|&j| j != i)
                .map(|j| (j, cosine_similarity(&centroids[i], &centroids[j])))
                .collect();
            sims.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            for &(j, sim) in sims.iter().take(EDGE_TOP_K) {
                if sim < EDGE_MIN_SIM {
                    break;
                }
                let key = (i.min(j), i.max(j));
                edge_set.entry(key).or_insert(sim);
            }
        }
        let mut edges: Vec<EmbeddingMapEdge> = edge_set
            .into_iter()
            .map(|((a, b), sim)| EmbeddingMapEdge { a, b, sim })
            .collect();
        edges.sort_by(|x, y| y.sim.partial_cmp(&x.sim).unwrap_or(std::cmp::Ordering::Equal));

        Ok(EmbeddingMapData {
            papers,
            chunks,
            edges,
            dimension: dim,
            embedding_model: Some(target),
            available_models: available,
        })
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))?
}

#[cfg(test)]
mod highlight_chunk_tests {
    use super::*;
    use crate::highlight_groups::fixtures;

    /// The embedding map got every cross-page selection twice: once per stored
    /// record. It must get one, under the canonical record's id.
    #[test]
    fn a_selection_across_a_page_break_is_embedded_once() {
        let mut stored = fixtures::cross_page_pair(10);
        stored[1].note = Some("我的想法".into());
        let lib = fixtures::library_with("a-paper", &stored);

        let input = get_paper_vectorize_input(lib.root(), "a-paper").unwrap();
        assert_eq!(input.highlights.len(), 1, "{:?}", input.highlights);
        let h = &input.highlights[0];
        assert_eq!((h.id.as_str(), h.page), ("hl-10", 10));
        assert_eq!(h.text, fixtures::PAIR_TEXT);
        assert_eq!(h.note.as_deref(), Some("我的想法"));

        // `vectorize_paper` reads the same list, so its chunks agree with what
        // the frontend chunker is handed.
        assert_eq!(highlight_inputs(lib.root(), "a-paper").len(), 1);
    }

    /// Ordinary highlights pass through exactly as before: trimmed text, blank
    /// notes dropped, blank highlights skipped, file order kept.
    #[test]
    fn ordinary_highlights_are_unchanged() {
        let mut a = fixtures::record("a", 2, "  padded  ", "2026-03-01T00:00:00.000Z");
        a.note = Some("   ".into());
        let mut b = fixtures::record("b", 5, "second", "2026-03-01T00:00:01.000Z");
        b.note = Some(" a note ".into());
        let blank = fixtures::record("c", 6, "   ", "2026-03-01T00:00:02.000Z");
        let lib = fixtures::library_with("a-paper", &[a, b, blank]);

        let hs = highlight_inputs(lib.root(), "a-paper");
        assert_eq!(hs.len(), 2);
        assert_eq!((hs[0].id.as_str(), hs[0].text.as_str(), hs[0].note.as_deref()), ("a", "padded", None));
        assert_eq!((hs[1].id.as_str(), hs[1].page), ("b", 5));
        assert_eq!(hs[1].note.as_deref(), Some(" a note "));
    }

    /// A PDF selection carries a line break at every printed wrap; embedding that
    /// would cut one sentence into shards. The chunk gets the merged paragraph,
    /// on both vectorize paths (they share `highlight_inputs`).
    #[test]
    fn wrapped_highlights_are_embedded_as_one_paragraph() {
        const WRAPPED: &str = "the quick brown\nfox jumps over\nthe lazy dog";
        let at = "2026-03-01T00:00:00.000Z";

        let wrapped = fixtures::record("a", 1, WRAPPED, at);
        let mut kept = fixtures::record("b", 2, WRAPPED, "2026-03-01T00:00:01.000Z");
        kept.keep_line_breaks = Some(true);
        kept.text = format!("\n{WRAPPED}\n");
        let mut ebook = fixtures::record("c", 3, "para one\npara two", "2026-03-01T00:00:02.000Z");
        ebook.start_offset = Some(0);
        ebook.end_offset = Some(17);
        let single = fixtures::record("d", 4, "already one line", "2026-03-01T00:00:03.000Z");
        // Nothing but blank lines: empty once merged, so it is skipped.
        let blank = fixtures::record("e", 5, " \n \n", "2026-03-01T00:00:04.000Z");
        let lib = fixtures::library_with("a-paper", &[wrapped, kept, ebook, single, blank]);

        let hs = highlight_inputs(lib.root(), "a-paper");
        let texts: Vec<(&str, &str)> = hs.iter().map(|h| (h.id.as_str(), h.text.as_str())).collect();
        assert_eq!(
            texts,
            vec![
                ("a", "the quick brown fox jumps over the lazy dog"),
                // Kept breaks survive, trimmed at the ends as every chunk is.
                ("b", WRAPPED),
                ("c", "para one\npara two"),
                ("d", "already one line"),
            ]
        );

        // The frontend path reads the very same list.
        let input = get_paper_vectorize_input(lib.root(), "a-paper").unwrap();
        let via_input: Vec<&str> = input.highlights.iter().map(|h| h.text.as_str()).collect();
        assert_eq!(via_input, texts.iter().map(|(_, t)| *t).collect::<Vec<_>>());
    }

    #[test]
    fn a_wrapped_selection_across_a_page_break_is_one_merged_chunk() {
        const WRAPPED: &str = "a passage that\nruns over the\npage break";
        let created = "2026-03-01T10:00:00.000Z";
        let pair = [
            fixtures::record("hl-10", 10, WRAPPED, created),
            fixtures::record("hl-11", 11, WRAPPED, created),
        ];
        let lib = fixtures::library_with("a-paper", &pair);
        let hs = highlight_inputs(lib.root(), "a-paper");
        assert_eq!(hs.len(), 1);
        assert_eq!((hs[0].id.as_str(), hs[0].page), ("hl-10", 10));
        assert_eq!(hs[0].text, "a passage that runs over the page break");
    }
}
