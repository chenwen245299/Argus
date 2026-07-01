use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

use rusqlite::{params, Connection};
use tauri::Emitter;

use crate::models::{AiProvider, RagSettings, RetrievedChunk, VectorStoreInfo, VectorsMeta};
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
        let m = legacy_model.replace('\'', "''");

        conn.execute_batch(&format!(
            "ALTER TABLE chunks RENAME TO chunks_legacy;
             {CHUNKS_SCHEMA}
             INSERT INTO chunks
                 (chunk_id, embedding_model, paper_id, slug, chunk_index, text, vector,
                  source_type, source_id, source_label, paper_title)
             SELECT chunk_id, '{m}', paper_id, slug, chunk_index, text, vector,
                    source_type, source_id, source_label, paper_title
             FROM chunks_legacy;
             DROP TABLE chunks_legacy;"
        ))
        .map_err(|e| format!("Migrate chunks to multi-model store: {e}"))
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
        return Err("RAG embedding is not configured. \
             Go to Settings → RAG and select a provider and embedding model."
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
        return Err("RAG embedding is not configured. \
             Go to Settings → RAG and select a provider and embedding model."
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
    for h in paper::read_highlights(root, slug) {
        if h.text.trim().is_empty() {
            continue;
        }
        let mut text = format!("高亮文本 (第{}页): {}", h.page, h.text.trim());
        if let Some(ref note) = h.note {
            if !note.trim().is_empty() {
                text.push_str(&format!("\n用户批注: {}", note.trim()));
            }
        }
        pending.push(PendingChunk {
            text,
            source_type: "highlight",
            source_id: Some(h.id.clone()),
            source_label: Some(format!("第{}页批注", h.page)),
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

/// Returns all raw content for a paper so the frontend can chunk it with
/// LlamaIndex SentenceSplitter before sending chunks back via embed_and_store_chunks.
pub fn get_paper_vectorize_input(
    root: &str,
    slug: &str,
) -> Result<crate::models::PaperVectorizeInput, String> {
    use crate::models::{HighlightInput, NoteInput, PaperVectorizeInput};

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

    let highlights = paper::read_highlights(root, slug)
        .into_iter()
        .filter(|h| !h.text.trim().is_empty())
        .map(|h| HighlightInput {
            id: h.id,
            page: h.page,
            text: h.text.trim().to_string(),
            note: h.note.filter(|n| !n.trim().is_empty()),
        })
        .collect();

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
        return Err("RAG embedding is not configured.".to_string());
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
        // Also drop snippet vectors tied to this paper so they don't linger as
        // orphans pointing at a deleted paper. (`snippet_chunks` may not exist
        // yet on older libraries, so ignore a missing-table error.)
        let _ = conn.execute(
            "DELETE FROM snippet_chunks WHERE paper_id = ?1",
            params![paper_id],
        );
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

// ── Search ────────────────────────────────────────────────────────────────────

pub async fn embed_query(
    root: &str,
    query: &str,
    settings: &RagSettings,
) -> Result<Vec<f32>, String> {
    let (provider, api_key, emb_model) = resolve_embedding_provider(root, settings)?;
    let mut results = llm::embeddings(
        &provider,
        &api_key,
        &emb_model,
        &[query.to_string()],
        "embedding",
    )
    .await?;
    results
        .pop()
        .ok_or_else(|| "No embedding returned".to_string())
}

pub async fn search_paper_chunks(
    root: &str,
    slug: &str,
    query: &str,
    top_k: usize,
) -> Result<Vec<RetrievedChunk>, String> {
    let settings = get_rag_settings(root);
    let query_vec = embed_query(root, query, &settings).await?;
    search_paper_chunks_with_vec(root, slug, query_vec, top_k).await
}

pub async fn search_library_chunks(
    root: &str,
    query: &str,
    top_k: usize,
) -> Result<Vec<RetrievedChunk>, String> {
    let settings = get_rag_settings(root);
    let query_vec = embed_query(root, query, &settings).await?;
    search_library_chunks_with_vec(root, query_vec, top_k).await
}

pub async fn search_paper_chunks_with_vec(
    root: &str,
    slug: &str,
    query_vec: Vec<f32>,
    top_k: usize,
) -> Result<Vec<RetrievedChunk>, String> {
    let paper_id = paper::read_meta(root, slug)
        .map(|m| m.id)
        .unwrap_or_else(|_| slug.to_string());
    search_chunks_internal(root, query_vec, top_k, Some(paper_id)).await
}

pub async fn search_library_chunks_with_vec(
    root: &str,
    query_vec: Vec<f32>,
    top_k: usize,
) -> Result<Vec<RetrievedChunk>, String> {
    search_chunks_internal(root, query_vec, top_k, None).await
}

async fn search_chunks_internal(
    root: &str,
    query_vec: Vec<f32>,
    top_k: usize,
    paper_id_filter: Option<String>,
) -> Result<Vec<RetrievedChunk>, String> {
    // The query vector was embedded with the current model, so restrict the
    // search to that model's partition — other models have different dimensions.
    let current_model = current_embedding_model(root);
    let root = root.to_string();
    tokio::task::spawn_blocking(move || -> Result<Vec<RetrievedChunk>, String> {
        if !db_path(&root).exists() {
            return Ok(Vec::new());
        }
        let conn = open_db(&root)?;

        struct Row {
            chunk_id: String,
            paper_id: String,
            slug: String,
            chunk_index: i64,
            text: String,
            vector: Vec<f32>,
            source_type: String,
            source_id: Option<String>,
            source_label: Option<String>,
            paper_title: String,
        }

        // Load this model's chunks then filter in Rust — avoids stmt lifetime
        // issues and is fine for personal library sizes (a few thousand chunks).
        // If no model is configured, fall back to scanning every partition.
        let (sql, model_param): (&str, Vec<String>) = match &current_model {
            Some(m) => (
                "SELECT chunk_id, paper_id, slug, chunk_index, text, vector, \
                 source_type, source_id, source_label, paper_title \
                 FROM chunks WHERE embedding_model = ?1",
                vec![m.clone()],
            ),
            None => (
                "SELECT chunk_id, paper_id, slug, chunk_index, text, vector, \
                 source_type, source_id, source_label, paper_title FROM chunks",
                Vec::new(),
            ),
        };
        let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;

        let all_rows: Vec<Row> = stmt
            .query_map(rusqlite::params_from_iter(model_param.iter()), |r| {
                let blob: Vec<u8> = r.get(5)?;
                Ok(Row {
                    chunk_id: r.get(0)?,
                    paper_id: r.get(1)?,
                    slug: r.get(2)?,
                    chunk_index: r.get(3)?,
                    text: r.get(4)?,
                    vector: blob_to_vec(&blob),
                    source_type: r.get(6).unwrap_or_else(|_| "text".to_string()),
                    source_id: r.get(7).ok(),
                    source_label: r.get(8).ok(),
                    paper_title: r.get(9).unwrap_or_default(),
                })
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();

        let rows: Vec<Row> = match paper_id_filter {
            Some(ref pid) => all_rows
                .into_iter()
                .filter(|r| &r.paper_id == pid)
                .collect(),
            None => all_rows,
        };

        let mut scored: Vec<(f32, Row)> = rows
            .into_iter()
            .map(|row| {
                let score = cosine_similarity(&query_vec, &row.vector);
                (score, row)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        Ok(scored
            .into_iter()
            .take(top_k)
            .map(|(score, row)| RetrievedChunk {
                chunk_id: row.chunk_id,
                paper_id: row.paper_id,
                slug: row.slug,
                chunk_index: row.chunk_index as u32,
                text: row.text,
                score,
                paper_title: row.paper_title,
                source_type: row.source_type,
                source_id: row.source_id,
                source_label: row.source_label,
            })
            .collect())
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))?
}

// ── Snippet vector store ──────────────────────────────────────────────────────

fn open_db_with_snippet_table(root: &str) -> Result<Connection, String> {
    let conn = open_db(root)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snippet_chunks (
             snippet_id  TEXT PRIMARY KEY,
             library_id  TEXT NOT NULL DEFAULT '',
             text        TEXT NOT NULL,
             vector      BLOB NOT NULL,
             paper_id    TEXT NOT NULL DEFAULT '',
             paper_title TEXT NOT NULL DEFAULT '',
             page        INTEGER NOT NULL DEFAULT 0,
             note        TEXT NOT NULL DEFAULT '',
             tags        TEXT NOT NULL DEFAULT '[]'
         );",
    )
    .map_err(|e| format!("Init snippet_chunks: {e}"))?;
    Ok(conn)
}

fn build_snippet_embed_text(s: &crate::models::Snippet) -> String {
    let mut parts = vec![s.text.trim().to_string()];
    if !s.note.trim().is_empty() {
        parts.push(s.note.trim().to_string());
    }
    if !s.tags.is_empty() {
        parts.push(s.tags.join(" "));
    }
    parts.join("\n")
}

pub async fn embed_and_store_snippets(
    root: &str,
    snippets: Vec<crate::models::Snippet>,
    app: &tauri::AppHandle,
) -> Result<(usize, usize), String> {
    if snippets.is_empty() {
        return Ok((0, 0));
    }

    let settings = get_rag_settings(root);
    let (provider, api_key, emb_model) = resolve_embedding_provider(root, &settings)?;

    let root = root.to_string();
    let total = snippets.len();
    let mut done = 0usize;
    let mut failed = 0usize;

    // Drop snippets with nothing to embed up front: an empty input string
    // would make the embeddings API reject the whole batch it lands in.
    let snippets: Vec<crate::models::Snippet> = {
        let (valid, empty): (Vec<_>, Vec<_>) = snippets
            .into_iter()
            .partition(|s| !build_snippet_embed_text(s).trim().is_empty());
        failed += empty.len();
        valid
    };

    // Batch the texts: one embeddings request per batch instead of one per
    // snippet, with one DB transaction per batch. On a batch error, count the
    // whole batch as failed and keep going.
    const BATCH_SIZE: usize = 32;

    let _ = app.emit(
        "snippet-embed-progress",
        serde_json::json!({"done": 0, "failed": 0, "total": total}),
    );

    for batch in snippets.chunks(BATCH_SIZE) {
        let texts: Vec<String> = batch.iter().map(build_snippet_embed_text).collect();
        let vecs = match llm::embeddings(&provider, &api_key, &emb_model, &texts, "embedding").await
        {
            Ok(v) if v.len() == batch.len() => v,
            Ok(_) | Err(_) => {
                failed += batch.len();
                let _ = app.emit(
                    "snippet-embed-progress",
                    serde_json::json!({"done": done, "failed": failed, "total": total}),
                );
                continue;
            }
        };

        struct SnippetRow {
            snippet_id: String,
            library_id: String,
            text: String,
            blob: Vec<u8>,
            paper_id: String,
            paper_title: String,
            page: u32,
            note: String,
            tags_json: String,
        }

        let rows: Vec<SnippetRow> = batch
            .iter()
            .zip(vecs.iter())
            .map(|(snippet, vec)| SnippetRow {
                snippet_id: snippet.id.clone(),
                library_id: snippet.library_id.clone(),
                text: snippet.text.clone(),
                blob: vec_to_blob(vec),
                paper_id: snippet.paper_id.clone(),
                paper_title: snippet.paper_title.clone(),
                page: snippet.page,
                note: snippet.note.clone(),
                tags_json: serde_json::to_string(&snippet.tags)
                    .unwrap_or_else(|_| "[]".to_string()),
            })
            .collect();

        let batch_len = rows.len();
        let root2 = root.clone();
        let res = tokio::task::spawn_blocking(move || -> Result<(), String> {
            let mut conn = open_db_with_snippet_table(&root2)?;
            let tx = conn.transaction().map_err(|e| e.to_string())?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT OR REPLACE INTO snippet_chunks
                         (snippet_id, library_id, text, vector, paper_id, paper_title, page, note, tags)
                         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    )
                    .map_err(|e| e.to_string())?;
                for r in &rows {
                    stmt.execute(params![
                        r.snippet_id,
                        r.library_id,
                        r.text,
                        r.blob,
                        r.paper_id,
                        r.paper_title,
                        r.page,
                        r.note,
                        r.tags_json,
                    ])
                    .map_err(|e| e.to_string())?;
                }
            }
            tx.commit().map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())?;

        match res {
            Ok(()) => done += batch_len,
            Err(_) => failed += batch_len,
        }

        let _ = app.emit(
            "snippet-embed-progress",
            serde_json::json!({"done": done, "failed": failed, "total": total}),
        );
    }

    Ok((done, failed))
}

pub async fn search_snippet_chunks_with_vec(
    root: &str,
    query_vec: Vec<f32>,
    top_k: usize,
) -> Result<Vec<crate::models::RetrievedSnippet>, String> {
    let root = root.to_string();
    tokio::task::spawn_blocking(move || -> Result<Vec<crate::models::RetrievedSnippet>, String> {
        let conn = open_db_with_snippet_table(&root)?;
        let mut stmt = conn
            .prepare(
                "SELECT snippet_id,library_id,text,vector,paper_id,paper_title,page,note,tags
                 FROM snippet_chunks",
            )
            .map_err(|e| e.to_string())?;

        let mut scored: Vec<(f32, crate::models::RetrievedSnippet)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, u32>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .filter_map(|(snippet_id, library_id, text, blob, paper_id, paper_title, page, note, tags_json)| {
                let vec = blob_to_vec(&blob);
                if vec.is_empty() { return None; }
                let score = cosine_similarity(&query_vec, &vec);
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
                Some((score, crate::models::RetrievedSnippet {
                    snippet_id,
                    library_id,
                    text,
                    score,
                    paper_id,
                    paper_title,
                    page,
                    note,
                    tags,
                }))
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored.into_iter().take(top_k).map(|(_, s)| s).collect())
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))?
}

pub async fn get_snippet_store_info(root: &str) -> Result<crate::models::SnippetStoreInfo, String> {
    let root = root.to_string();
    tokio::task::spawn_blocking(move || -> Result<crate::models::SnippetStoreInfo, String> {
        let conn = open_db_with_snippet_table(&root)?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snippet_chunks", [], |row| row.get(0))
            .unwrap_or(0);
        Ok(crate::models::SnippetStoreInfo { embedded_count: count as usize })
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn delete_snippet_chunk(root: &str, snippet_id: &str) -> Result<(), String> {
    let root = root.to_string();
    let snippet_id = snippet_id.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = open_db_with_snippet_table(&root)?;
        conn.execute("DELETE FROM snippet_chunks WHERE snippet_id=?1", params![snippet_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub async fn update_snippet_library_id(
    root: &str,
    snippet_id: &str,
    library_id: &str,
) -> Result<(), String> {
    let root = root.to_string();
    let snippet_id = snippet_id.to_string();
    let library_id = library_id.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = open_db_with_snippet_table(&root)?;
        conn.execute(
            "UPDATE snippet_chunks SET library_id = ?1 WHERE snippet_id = ?2",
            params![library_id, snippet_id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

pub fn get_unembedded_snippets(root: &str) -> Result<Vec<crate::models::Snippet>, String> {
    let libs = crate::snippets::list_snippet_libraries(root)?;
    let mut all: Vec<crate::models::Snippet> = vec![];
    for lib in &libs {
        all.extend(crate::snippets::get_snippets(root, &lib.id).unwrap_or_default());
    }
    if all.is_empty() {
        return Ok(vec![]);
    }

    let embedded_ids: std::collections::HashSet<String> = (|| -> Result<_, String> {
        let conn = open_db_with_snippet_table(root)?;
        let mut stmt = conn.prepare("SELECT snippet_id FROM snippet_chunks")
            .map_err(|e| e.to_string())?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        Ok(ids)
    })()
    .unwrap_or_default();

    Ok(all.into_iter().filter(|s| !embedded_ids.contains(&s.id)).collect())
}
