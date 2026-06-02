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
    std::fs::write(&path, content).map_err(|e| e.to_string())
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
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

// ── Text chunking ─────────────────────────────────────────────────────────────

/// Paragraph-aware chunking: splits on blank lines, groups paragraphs up to
/// `target_size` chars, keeps last paragraph as overlap into the next chunk.
/// Falls back to character sliding-window for paragraphs longer than target.
fn chunk_text(text: &str, target_size: usize, _overlap: usize) -> Vec<String> {
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

    let mut chunks: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    let mut current_len: usize = 0;

    let flush = |current: &mut Vec<String>, current_len: &mut usize, chunks: &mut Vec<String>| {
        if !current.is_empty() {
            chunks.push(current.join("\n\n"));
        }
        // Keep last paragraph as overlap
        let overlap_para = current.pop().map(|p| {
            let l = p.chars().count();
            (p, l)
        });
        current.clear();
        *current_len = 0;
        if let Some((p, l)) = overlap_para {
            current.push(p);
            *current_len = l;
        }
    };

    for para in paragraphs {
        let plen = para.chars().count();

        // Long paragraph: character-slide it directly
        if plen > target_size {
            // Flush whatever we have
            if !current.is_empty() {
                chunks.push(current.join("\n\n"));
                current.clear();
                current_len = 0;
            }
            let chars: Vec<char> = para.chars().collect();
            let step = target_size.saturating_sub(target_size / 5).max(1);
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

fn open_db(root: &str) -> Result<Connection, String> {
    let path = db_path(root);
    let conn = Connection::open(&path).map_err(|e| format!("Open vectors DB: {e}"))?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS chunks (
             chunk_id     TEXT PRIMARY KEY,
             paper_id     TEXT NOT NULL,
             slug         TEXT NOT NULL,
             chunk_index  INTEGER NOT NULL,
             text         TEXT NOT NULL,
             vector       BLOB NOT NULL,
             source_type  TEXT NOT NULL DEFAULT 'text',
             source_id    TEXT,
             source_label TEXT,
             paper_title  TEXT NOT NULL DEFAULT ''
         );
         CREATE INDEX IF NOT EXISTS idx_chunks_paper ON chunks(paper_id);",
    )
    .map_err(|e| format!("Init vectors DB: {e}"))?;

    // Migrate older databases that predate the new columns (errors = already present, ignore)
    for (col, def) in &[
        ("source_type", "TEXT NOT NULL DEFAULT 'text'"),
        ("source_id", "TEXT"),
        ("source_label", "TEXT"),
        ("paper_title", "TEXT NOT NULL DEFAULT ''"),
    ] {
        let _ = conn.execute_batch(&format!("ALTER TABLE chunks ADD COLUMN {col} {def};"));
    }

    Ok(conn)
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

    // Dimension consistency guard
    if let Some(vmeta) = get_vectors_meta(root) {
        if vmeta.provider_id != provider.id || vmeta.embedding_model != emb_model {
            return Err(format!(
                "Vector store uses provider '{}' model '{}', but config uses '{}' model '{}'. \
                 Rebuild the vector store to switch embedding models.",
                vmeta.provider_id, vmeta.embedding_model, provider.id, emb_model
            ));
        }
    }

    let texts: Vec<String> = pending.iter().map(|c| c.text.clone()).collect();
    let total = texts.len();
    let _ = app.emit(
        &event,
        serde_json::json!({"status": "embedding", "total": total}),
    );
    let embeddings = llm::embeddings(&provider, &api_key, &emb_model, &texts, "embedding").await?;

    let dim = embeddings.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 {
        return Err("Embeddings returned zero-dimension vectors.".to_string());
    }

    let _ = app.emit(&event, serde_json::json!({"status": "storing"}));

    let root_str = root.to_string();
    let slug_str = slug.to_string();
    let paper_title_c = paper_title.clone();
    let paper_id_c = paper_id.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let mut conn = open_db(&root_str)?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Begin transaction: {e}"))?;

        tx.execute(
            "DELETE FROM chunks WHERE paper_id = ?1",
            params![paper_id_c],
        )
        .map_err(|e| format!("Delete old chunks: {e}"))?;

        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO chunks \
                     (chunk_id, paper_id, slug, chunk_index, text, vector, \
                      source_type, source_id, source_label, paper_title) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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

    if let Some(vmeta) = get_vectors_meta(root) {
        if vmeta.provider_id != provider.id || vmeta.embedding_model != emb_model {
            return Err(format!(
                "Vector store uses provider '{}' model '{}', but config uses '{}' model '{}'. \
                 Rebuild the vector store to switch embedding models.",
                vmeta.provider_id, vmeta.embedding_model, provider.id, emb_model
            ));
        }
    }

    let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
    let total = texts.len();

    let event = format!("vectorize-{slug}");
    let _ = app.emit(
        &event,
        serde_json::json!({"status": "embedding", "total": total}),
    );

    let embeddings = llm::embeddings(&provider, &api_key, &emb_model, &texts, "embedding").await?;
    let dim = embeddings.first().map(|v| v.len()).unwrap_or(0);
    if dim == 0 {
        return Err("Embeddings returned zero-dimension vectors.".to_string());
    }

    let _ = app.emit(&event, serde_json::json!({"status": "storing"}));

    let root_str = root.to_string();
    let slug_str = slug.to_string();
    let paper_id_str = paper_id.to_string();
    let paper_title_str = paper_title.to_string();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = open_db(&root_str)?;
        conn.execute(
            "DELETE FROM chunks WHERE paper_id = ?1",
            params![paper_id_str],
        )
        .map_err(|e| format!("Delete old chunks: {e}"))?;

        let mut stmt = conn
            .prepare(
                "INSERT OR REPLACE INTO chunks \
                 (chunk_id, paper_id, slug, chunk_index, text, vector, \
                  source_type, source_id, source_label, paper_title) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        Ok(())
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

// ── Delete paper chunks ───────────────────────────────────────────────────────

pub async fn delete_paper_chunks(root: &str, paper_id: &str) -> Result<(), String> {
    let root = root.to_string();
    let paper_id = paper_id.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        if !db_path(&root).exists() {
            return Ok(());
        }
        let conn = open_db(&root)?;
        conn.execute("DELETE FROM chunks WHERE paper_id = ?1", params![paper_id])
            .map_err(|e| format!("Delete chunks: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("Spawn blocking: {e}"))?
}

// ── Batch vectorize (rebuild) ─────────────────────────────────────────────────

pub async fn rebuild_vector_store(root: &str, app: &tauri::AppHandle) -> Result<usize, String> {
    batch_cancel().store(false, Ordering::SeqCst);

    let entries = crate::library::scan_library(root).unwrap_or_default();
    let slugs: Vec<String> = entries.iter().map(|e| e.slug.clone()).collect();
    let total = slugs.len();

    // Drop existing store for clean rebuild
    let path = db_path(root);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    let meta_path = Path::new(root).join(".argus").join(VECTORS_META_FILE);
    let _ = std::fs::remove_file(&meta_path);

    for slug in &slugs {
        let _ = update_vectorized(root, slug, false);
    }

    let _ = app.emit(
        "vectorize-batch",
        serde_json::json!({"total": total, "done": 0, "failed": 0, "status": "running"}),
    );

    let mut done = 0usize;
    let mut failed = 0usize;

    for slug in &slugs {
        if batch_cancel().load(Ordering::SeqCst) {
            let _ = app.emit(
                "vectorize-batch",
                serde_json::json!({"total": total, "done": done, "failed": failed, "status": "cancelled"}),
            );
            return Ok(done);
        }

        match vectorize_paper(root, slug, app).await {
            Ok(()) => done += 1,
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

    let _ = app.emit(
        "vectorize-batch",
        serde_json::json!({"total": total, "done": done, "failed": failed, "status": "done"}),
    );
    Ok(done)
}

// ── Vector store info ─────────────────────────────────────────────────────────

pub async fn get_vector_store_info(root: &str) -> Result<VectorStoreInfo, String> {
    let meta = get_vectors_meta(root);
    let root = root.to_string();
    tokio::task::spawn_blocking(move || -> Result<VectorStoreInfo, String> {
        let empty = VectorStoreInfo {
            total_chunks: 0,
            unique_papers: 0,
            dimension: meta.as_ref().map(|m| m.dimension),
            provider_id: meta.as_ref().map(|m| m.provider_id.clone()),
            embedding_model: meta.as_ref().map(|m| m.embedding_model.clone()),
        };
        if !db_path(&root).exists() {
            return Ok(empty);
        }
        let conn = open_db(&root)?;
        let total: usize = conn
            .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
            .unwrap_or(0);
        let unique: usize = conn
            .query_row("SELECT COUNT(DISTINCT paper_id) FROM chunks", [], |r| {
                r.get(0)
            })
            .unwrap_or(0);
        Ok(VectorStoreInfo {
            total_chunks: total,
            unique_papers: unique,
            dimension: meta.as_ref().map(|m| m.dimension),
            provider_id: meta.as_ref().map(|m| m.provider_id.clone()),
            embedding_model: meta.as_ref().map(|m| m.embedding_model.clone()),
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

        // Always load all chunks then filter in Rust — avoids stmt lifetime issues
        // and is fine for personal library sizes (few thousand chunks at most).
        let mut stmt = conn
            .prepare(
                "SELECT chunk_id, paper_id, slug, chunk_index, text, vector, \
                 source_type, source_id, source_label, paper_title FROM chunks",
            )
            .map_err(|e| e.to_string())?;

        let all_rows: Vec<Row> = stmt
            .query_map([], |r| {
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
