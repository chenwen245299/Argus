use std::path::{Path, PathBuf};

use tauri::Emitter;

use crate::models::{
    AiProvider, ChatContent, ChatContentPart, ChatMessage, FileData, PaperMeta, RetrievedChunk,
};
use crate::{ai_manager, ai_summary, extraction, llm, paper, rag};

// ── Chat history persistence ──────────────────────────────────────────────────

pub fn read_chat_history(root: &str, slug: &str) -> Vec<ChatMessage> {
    let path = paper::paper_dir(root, slug).join("chat.json");
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn write_chat_history(root: &str, slug: &str, messages: &[ChatMessage]) -> Result<(), String> {
    let path = paper::paper_dir(root, slug).join("chat.json");
    let content = serde_json::to_string_pretty(messages)
        .map_err(|e| format!("Serialize chat history: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content).map_err(|e| format!("Write chat.json: {e}"))
}

pub fn clear_chat_history(root: &str, slug: &str) -> Result<(), String> {
    let path = paper::paper_dir(root, slug).join("chat.json");
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete chat.json: {e}"))
    } else {
        Ok(())
    }
}

fn paper_ai_conversations_path(root: &str, slug: &str) -> PathBuf {
    paper::paper_dir(root, slug).join("ai_conversations.json")
}

pub fn read_paper_ai_conversations(root: &str, slug: &str) -> serde_json::Value {
    let path = paper_ai_conversations_path(root, slug);
    if !path.exists() {
        return serde_json::json!([]);
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .filter(|v: &serde_json::Value| v.is_array())
        .unwrap_or_else(|| serde_json::json!([]))
}

pub fn write_paper_ai_conversations(
    root: &str,
    slug: &str,
    conversations: &serde_json::Value,
) -> Result<(), String> {
    if !conversations.is_array() {
        return Err("Paper AI conversations must be an array.".to_string());
    }
    let path = paper_ai_conversations_path(root, slug);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create paper dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(conversations)
        .map_err(|e| format!("Serialize paper AI conversations: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write ai_conversations.json: {e}"))
}

fn library_chat_history_path(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("library_chat.json")
}

pub fn read_library_chat_history(root: &str) -> Vec<ChatMessage> {
    let path = library_chat_history_path(root);
    if !path.exists() {
        return Vec::new();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str(&c).ok())
        .unwrap_or_default()
}

pub fn write_library_chat_history(root: &str, messages: &[ChatMessage]) -> Result<(), String> {
    let path = library_chat_history_path(root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("Create .argus dir: {e}"))?;
    }
    let content = serde_json::to_string_pretty(messages)
        .map_err(|e| format!("Serialize library chat history: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write library_chat.json: {e}"))
}

pub fn clear_library_chat_history(root: &str) -> Result<(), String> {
    let path = library_chat_history_path(root);
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Delete library_chat.json: {e}"))
    } else {
        Ok(())
    }
}

// Library-wide "智能问答" conversations.
//
// # One file per conversation, beside `papers/`
//
// These used to be a single `.argus/library_chats.json` holding every
// conversation in one JSON array. That has a cost that is easy to miss: JSON
// arrays cannot be edited in place, so touching one conversation meant
// re-serializing and rewriting *all* of them — and the chat window saves several
// times per answer. The old code capped the file at 50 conversations to keep
// that bounded, which silently deleted the 51st.
//
// One file each removes both problems: a save rewrites only what changed, and
// there is no reason left to cap how many are kept.
//
// They live in `<library>/chats/` rather than `.argus/` because they are the
// user's own writing, not a rebuildable cache — the same reason `papers/` is not
// hidden either. `.argus/` is for things the app can regenerate.
//
// The frontend owns the shape; each file is stored as an opaque JSON object.

fn conversations_dir(root: &str) -> PathBuf {
    Path::new(root).join("chats")
}

/// Where every conversation used to live, in one array.
fn legacy_conversations_path(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("library_chats.json")
}

/// Filename for a conversation id.
///
/// Ids are generated as base36 and are already safe, but a file name is a real
/// path segment and anything arriving from an older version or a hand-edited
/// file has to be neutralised. Unsafe ids map to a stable hash rather than being
/// rejected, so an odd id costs its readable filename and nothing else.
fn conversation_file_stem(id: &str) -> String {
    let safe = id.len() <= 64
        && !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if safe {
        return id.to_string();
    }
    // FNV-1a: stable across runs and platforms, which `DefaultHasher` is not.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    format!("id-{hash:016x}")
}

fn conversation_path(root: &str, id: &str) -> Result<PathBuf, String> {
    let name = format!("{}.json", conversation_file_stem(id));
    crate::path_guard::validate_segment("conversation id", &name)?;
    Ok(conversations_dir(root).join(name))
}

/// Every conversation, newest first. Reads only — never migrates.
///
/// Split from [`load_library_conversations`] so the MCP tools can use it. They
/// are annotated `readOnlyHint: true`, and a client is entitled to take that at
/// its word: several auto-approve read-only calls. Migration creates a
/// directory, writes a file per conversation and renames the original, which is
/// not something a tool making that claim may do.
///
/// Anything the migration has not moved yet is still read, straight out of the
/// legacy file, so switching this path off does not hide conversations from the
/// agent — or from the user, if a migration ever stops half way.
pub fn read_library_conversations(root: &str) -> serde_json::Value {
    let mut convs: Vec<serde_json::Value> = std::fs::read_dir(conversations_dir(root))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .filter(|v| v["id"].is_string() && v["messages"].is_array())
        .collect();

    // Whatever is still only in the old single-file store. The per-file copy
    // wins: it is the one the app has been writing to since the migration.
    let migrated: std::collections::HashSet<String> = convs
        .iter()
        .filter_map(|v| v["id"].as_str().map(str::to_string))
        .collect();
    convs.extend(
        legacy_conversations(root)
            .into_iter()
            .filter(|v| v["id"].as_str().is_some_and(|id| !migrated.contains(id))),
    );

    // Order lived in the array's sequence before; now it is derived, so a file
    // written by hand or restored from a backup still lands in the right place.
    convs.sort_by(|a, b| {
        let key = |v: &serde_json::Value| {
            v["updatedAt"]
                .as_str()
                .or_else(|| v["createdAt"].as_str())
                .unwrap_or("")
                .to_string()
        };
        key(b).cmp(&key(a))
    });
    serde_json::Value::Array(convs)
}

/// What the app itself calls: migrate first, then read.
pub fn load_library_conversations(root: &str) -> serde_json::Value {
    migrate_legacy_conversations(root);
    read_library_conversations(root)
}

/// The conversations still sitting in the pre-split file, if it is there.
fn legacy_conversations(root: &str) -> Vec<serde_json::Value> {
    let Ok(text) = std::fs::read_to_string(legacy_conversations_path(root)) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(serde_json::Value::Array(convs)) => convs
            .into_iter()
            .filter(|v| v["id"].is_string() && v["messages"].is_array())
            .collect(),
        _ => Vec::new(),
    }
}

pub fn write_library_conversation(root: &str, conversation: &serde_json::Value) -> Result<(), String> {
    let id = conversation["id"]
        .as_str()
        .ok_or("Conversation is missing an id.")?;
    let path = conversation_path(root, id)?;
    std::fs::create_dir_all(conversations_dir(root))
        .map_err(|e| format!("Create chats dir: {e}"))?;
    let content = serde_json::to_string_pretty(conversation)
        .map_err(|e| format!("Serialize conversation: {e}"))?;
    crate::fsutil::atomic_write_str(&path, &content)
        .map_err(|e| format!("Write conversation: {e}"))
}

pub fn delete_library_conversation(root: &str, id: &str) -> Result<(), String> {
    let path = conversation_path(root, id)?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        // Already gone is the desired end state, not a failure.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Delete conversation: {e}")),
    }
}

/// Split the old single-file store into one file per conversation.
///
/// Runs at most once: the legacy file is renamed aside only after every
/// conversation has been written out, so an interrupted migration leaves the
/// original untouched and is simply retried next time. The backup is kept rather
/// than deleted — this moves the user's own writing, and a rename is cheap
/// insurance against a bug here.
fn migrate_legacy_conversations(root: &str) {
    let legacy = legacy_conversations_path(root);
    if !legacy.exists() {
        return;
    }
    let Ok(text) = std::fs::read_to_string(&legacy) else {
        return;
    };
    let Ok(serde_json::Value::Array(convs)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return;
    };

    for conv in &convs {
        let Some(id) = conv["id"].as_str() else {
            continue;
        };
        // If a per-file copy already exists, a previous run moved this one and
        // the user may have edited it since. Copying the legacy row over it
        // would silently undo that. Only what is still missing gets written,
        // which is what makes retrying an interrupted migration safe.
        if conversation_path(root, id).is_ok_and(|p| p.exists()) {
            continue;
        }
        if let Err(e) = write_library_conversation(root, conv) {
            eprintln!("[chats] migration aborted, leaving the original in place: {e}");
            return;
        }
    }

    let backup = legacy.with_extension("json.pre-split-backup");
    if let Err(e) = std::fs::rename(&legacy, &backup) {
        eprintln!("[chats] could not set the old file aside: {e}");
    }
}

#[cfg(test)]
mod conversation_store_tests {
    use super::*;

    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("argus-chats-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join(".argus")).unwrap();
            TempRoot(dir)
        }
        fn path(&self) -> String {
            self.0.to_string_lossy().to_string()
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn conv(id: &str, updated: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id, "title": id, "messages": [],
            "createdAt": updated, "updatedAt": updated,
        })
    }

    #[test]
    fn a_conversation_round_trips_through_its_own_file() {
        let root = TempRoot::new("roundtrip");
        write_library_conversation(&root.path(), &conv("abc123", "2026-01-01T00:00:00Z")).unwrap();
        let all = read_library_conversations(&root.path());
        assert_eq!(all.as_array().unwrap().len(), 1);
        assert_eq!(all[0]["id"], "abc123");
        assert!(
            root.0.join("chats").join("abc123.json").exists(),
            "not stored beside papers/ under its own id"
        );
    }

    /// Order used to be the array's sequence. It is derived now, so a file
    /// restored from a backup still lands where it belongs.
    #[test]
    fn conversations_come_back_newest_first() {
        let root = TempRoot::new("order");
        write_library_conversation(&root.path(), &conv("old", "2026-01-01T00:00:00Z")).unwrap();
        write_library_conversation(&root.path(), &conv("new", "2026-06-01T00:00:00Z")).unwrap();
        let all = read_library_conversations(&root.path());
        assert_eq!(all[0]["id"], "new");
        assert_eq!(all[1]["id"], "old");
    }

    #[test]
    fn saving_one_conversation_leaves_the_others_untouched() {
        let root = TempRoot::new("isolation");
        write_library_conversation(&root.path(), &conv("a", "2026-01-01T00:00:00Z")).unwrap();
        write_library_conversation(&root.path(), &conv("b", "2026-01-02T00:00:00Z")).unwrap();

        let a_file = root.0.join("chats").join("a.json");
        let before = std::fs::metadata(&a_file).unwrap().len();
        let mut updated = conv("b", "2026-02-02T00:00:00Z");
        updated["title"] = serde_json::json!("much longer title than before");
        write_library_conversation(&root.path(), &updated).unwrap();

        assert_eq!(
            std::fs::metadata(&a_file).unwrap().len(),
            before,
            "writing b rewrote a"
        );
    }

    #[test]
    fn deleting_removes_only_that_file() {
        let root = TempRoot::new("delete");
        write_library_conversation(&root.path(), &conv("a", "2026-01-01T00:00:00Z")).unwrap();
        write_library_conversation(&root.path(), &conv("b", "2026-01-02T00:00:00Z")).unwrap();
        delete_library_conversation(&root.path(), "a").unwrap();
        let all = read_library_conversations(&root.path());
        assert_eq!(all.as_array().unwrap().len(), 1);
        assert_eq!(all[0]["id"], "b");
        // Deleting what is already gone is the desired end state.
        delete_library_conversation(&root.path(), "a").unwrap();
    }

    /// The migration moves the user's own writing. Nothing may be lost, and the
    /// original has to survive as a backup.
    #[test]
    fn the_legacy_file_is_split_without_losing_anything() {
        let root = TempRoot::new("migrate");
        let legacy = serde_json::json!([
            conv("one", "2026-01-01T00:00:00Z"),
            conv("two", "2026-02-01T00:00:00Z"),
            conv("three", "2026-03-01T00:00:00Z"),
        ]);
        std::fs::write(
            legacy_conversations_path(&root.path()),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let all = load_library_conversations(&root.path());
        let ids: Vec<&str> = all
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["three", "two", "one"], "{all}");

        assert!(
            !legacy_conversations_path(&root.path()).exists(),
            "the old file would be migrated again on every read"
        );
        assert!(
            root.0.join(".argus").join("library_chats.json.pre-split-backup").exists(),
            "the original was deleted rather than set aside"
        );

        // And a second read must not resurrect or duplicate anything.
        let again = load_library_conversations(&root.path());
        assert_eq!(again.as_array().unwrap().len(), 3);
    }

    /// The MCP tools that list conversations are annotated `readOnlyHint: true`,
    /// and clients auto-approve on that basis. Reading must therefore not be
    /// what triggers the migration — and must still show everything, or the
    /// agent would be told the un-migrated conversations do not exist.
    #[test]
    fn reading_never_migrates_but_still_sees_the_legacy_file() {
        let root = TempRoot::new("readonly");
        let legacy = serde_json::json!([
            conv("one", "2026-01-01T00:00:00Z"),
            conv("two", "2026-02-01T00:00:00Z"),
        ]);
        std::fs::write(
            legacy_conversations_path(&root.path()),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let all = read_library_conversations(&root.path());
        assert_eq!(all.as_array().unwrap().len(), 2, "legacy rows were hidden");
        assert_eq!(all[0]["id"], "two", "newest first still applies");

        assert!(
            legacy_conversations_path(&root.path()).exists(),
            "a read-only call moved the user's file"
        );
        assert!(
            !root.0.join("chats").exists(),
            "a read-only call created the chats folder"
        );
    }

    /// With both stores present the per-file copy is the live one; the legacy
    /// row is a stale snapshot from before the migration.
    #[test]
    fn the_migrated_copy_wins_over_the_legacy_row() {
        let root = TempRoot::new("both");
        std::fs::write(
            legacy_conversations_path(&root.path()),
            serde_json::to_string(&serde_json::json!([conv("dup", "2026-01-01T00:00:00Z")])).unwrap(),
        )
        .unwrap();
        let mut newer = conv("dup", "2026-05-05T00:00:00Z");
        newer["title"] = serde_json::json!("edited since the migration");
        write_library_conversation(&root.path(), &newer).unwrap();

        let all = read_library_conversations(&root.path());
        assert_eq!(all.as_array().unwrap().len(), 1, "the id was listed twice");
        assert_eq!(all[0]["title"], "edited since the migration");
    }

    /// An id that is not a safe filename must not escape the chats folder.
    #[test]
    fn a_hostile_id_cannot_write_outside_the_folder() {
        let root = TempRoot::new("traversal");
        for id in ["../../escape", "a/b", "..", ""] {
            let stem = conversation_file_stem(id);
            assert!(
                !stem.contains('/') && !stem.contains('\\') && stem != "..",
                "id {id:?} produced the file stem {stem:?}"
            );
        }
        // And the mapping is stable, or a conversation would be unreachable
        // after the write that created it.
        assert_eq!(conversation_file_stem("a/b"), conversation_file_stem("a/b"));
        assert_ne!(conversation_file_stem("a/b"), conversation_file_stem("a/c"));

        write_library_conversation(&root.path(), &conv("../../escape", "2026-01-01T00:00:00Z"))
            .unwrap();
        assert_eq!(read_library_conversations(&root.path())[0]["id"], "../../escape");
    }

    /// A stray or half-written file in the folder must not take the list down.
    #[test]
    fn unreadable_files_are_skipped_not_fatal() {
        let root = TempRoot::new("junk");
        write_library_conversation(&root.path(), &conv("good", "2026-01-01T00:00:00Z")).unwrap();
        let dir = root.0.join("chats");
        std::fs::write(dir.join("broken.json"), "{ not json").unwrap();
        std::fs::write(dir.join("notes.txt"), "ignore me").unwrap();
        std::fs::write(dir.join("wrong-shape.json"), "{\"id\": 5}").unwrap();

        let all = read_library_conversations(&root.path());
        assert_eq!(all.as_array().unwrap().len(), 1);
        assert_eq!(all[0]["id"], "good");
    }
}

// ── Copilot chat ──────────────────────────────────────────────────────────────

/// Build the paper context for injection into the LLM system prompt.
/// M5: uses fulltext (truncated to fit model context).
/// M7 hook: replace `get_fulltext_context` with RAG retrieval.
pub async fn chat_with_paper(
    root: &str,
    slug: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    app: &tauri::AppHandle,
) -> Result<String, String> {
    let event_name = format!("ai-chat-{}", slug);
    chat_with_paper_on_event(
        root,
        slug,
        messages,
        provider_id,
        model_id,
        &event_name,
        app,
        false,
        None,
        "fulltext",
        false,
        &[],
        None,
        false,
    )
    .await
}

pub async fn chat_with_paper_on_event(
    root: &str,
    slug: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    event_name: &str,
    app: &tauri::AppHandle,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    context_mode: &str,
    use_pdf: bool,
    section_titles: &[String],
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    web_search: bool,
) -> Result<String, String> {
    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let meta = paper::read_meta(root, slug).ok();

    let mut all_messages: Vec<ChatMessage> = Vec::new();
    // Tracks the actual content injected per section for the transparency banner.
    let mut sent_metadata = String::new();
    let mut sent_summary = String::new();
    let mut sent_fulltext = String::new();

    match context_mode {
        "none" => {
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: "You are a research assistant. Answer the user's questions clearly and concisely.".into(),
            });
        }
        "metadata" => {
            sent_metadata = build_metadata_string(meta.as_ref());
            let system = build_system_prompt(meta.as_ref(), "", false, false, None);
            all_messages.push(ChatMessage { role: "system".to_string(), content: system.into() });
        }
        "summary" => {
            let summary = ai_summary::read_summary(root, slug);
            let summary_ctx = if summary.trim().is_empty() { None } else { Some(summary.as_str()) };
            sent_summary = summary.clone();
            // meta=None: user did not select 元数据
            let system = build_system_prompt(None, "", false, false, summary_ctx);
            all_messages.push(ChatMessage { role: "system".to_string(), content: system.into() });
        }
        "summary+fulltext" => {
            let summary = ai_summary::read_summary(root, slug);
            let (context, truncated) = get_fulltext_context(root, slug, &provider, &model);
            sent_summary = summary.clone();
            sent_fulltext = if truncated {
                format!("{context}\n\n[内容因上下文长度限制已截断]")
            } else {
                context.clone()
            };
            // Leading, cache-stable paper block (identical across tasks/turns),
            // then task-specific persona and any extra context after it.
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: build_paper_context_block(meta.as_ref(), &context).into(),
            });
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: PAPER_ASSISTANT_PERSONA.into(),
            });
            if !summary.trim().is_empty() {
                all_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: format!("--- AI SUMMARY ---\n{}", summary.trim()).into(),
                });
            }
            if truncated {
                all_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: "[Content truncated due to length. This is a partial view of the paper.]".into(),
                });
            }
        }
        _ => {
            // "fulltext"
            let (context, truncated) = get_fulltext_context(root, slug, &provider, &model);
            sent_fulltext = if truncated {
                format!("{context}\n\n[内容因上下文长度限制已截断]")
            } else {
                context.clone()
            };
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: build_paper_context_block(meta.as_ref(), &context).into(),
            });
            all_messages.push(ChatMessage {
                role: "system".to_string(),
                content: PAPER_ASSISTANT_PERSONA.into(),
            });
            if truncated {
                all_messages.push(ChatMessage {
                    role: "system".to_string(),
                    content: "[Content truncated due to length. This is a partial view of the paper.]".into(),
                });
            }
        }
    }

    // User-selected chapters — injected as an extra context block on top of
    // whatever context_mode already added (metadata / summary / fulltext).
    let sent_sections = crate::sections::extract_selected_sections_text(root, slug, section_titles);
    if !sent_sections.is_empty() {
        all_messages.push(ChatMessage {
            role: "system".to_string(),
            content: format!(
                "以下是用户选定的论文章节内容，请优先依据这些内容回答：\n\n{sent_sections}"
            )
            .into(),
        });
    }

    // Emit the actually-sent context so the frontend can display it transparently.
    let _ = app.emit(
        &format!("{event_name}-context"),
        serde_json::json!({
            "metadata": sent_metadata,
            "summary":  sent_summary,
            "fulltext": sent_fulltext,
            "sections": sent_sections,
        }),
    );

    all_messages.extend_from_slice(&messages);

    // Only OpenRouter supports inline PDF `file` content parts. Other providers
    // fall back to the text context already injected into the system prompt.
    let use_pdf = use_pdf && provider_supports_inline_pdf(&provider);

    if use_pdf {
        let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
        // Strip the system-prompt fulltext context (already not injected for openrouter+pdf),
        // but keep metadata/summary context messages that were added above.
        return llm::chat_completion_stream_with_pdf(
            &provider,
            &api_key,
            &model,
            &all_messages,
            event_name,
            app,
            use_reasoning,
            reasoning_effort,
            "copilot",
            &pdf_path,
            cancel,
        )
        .await;
    }

    llm::chat_completion_stream(
        &provider,
        &api_key,
        &model,
        &all_messages,
        event_name,
        app,
        use_reasoning,
        reasoning_effort,
        "copilot",
        cancel,
        web_search,
    )
    .await
}

// ── Library chat ──────────────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize)]
struct LibrarySentContextSection {
    kind: String,
    label: String,
    content: String,
}

#[derive(Clone, serde::Serialize)]
struct LibrarySentContextPayload {
    mode: String,
    sections: Vec<LibrarySentContextSection>,
}

fn provider_supports_inline_pdf(provider: &crate::models::AiProvider) -> bool {
    // Only OpenRouter reliably supports OpenAI-compatible inline `file`
    // content parts for PDFs.
    provider.kind == "openrouter" || provider.base_url.to_lowercase().contains("openrouter")
}

pub async fn chat_with_library(
    root: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    event_name: &str,
    sources_event_name: &str,
    knowledge_source: Option<&str>,
    selected_paper_slugs: Option<&[String]>,
    attachments: Option<&[ChatContentPart]>,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    app: &tauri::AppHandle,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    web_search: bool,
) -> Result<String, String> {
    use tauri::Emitter;

    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    let use_snippets = knowledge_source.map_or(false, |s| s == "snippets");
    let use_selected_papers = knowledge_source.map_or(false, |s| s == "papers");
    // "none" = plain conversation: no retrieval, no library context, no source
    // citations demanded of the model. Anything unrecognised still falls through
    // to the RAG default, so older callers behave as before.
    let use_no_source = knowledge_source.map_or(false, |s| s == "none");

    let system;

    let mut pdf_attachments: Vec<ChatContentPart> = Vec::new();

    if use_selected_papers {
        let slugs = selected_paper_slugs.unwrap_or(&[]);
        let use_inline_pdf = provider_supports_inline_pdf(&provider);
        // Only attach raw PDFs for providers that accept inline `file` content
        // parts (OpenRouter). For everyone else (DeepSeek, etc.) we fall back to
        // injecting the extracted fulltext as text via
        // `build_selected_papers_system_prompt` below — attaching the PDF here
        // would send an unsupported `file` part and trigger an API 400.
        if use_inline_pdf {
            pdf_attachments = slugs
                .iter()
                .filter_map(|slug| {
                    let slug = slug.trim();
                    if slug.is_empty() {
                        return None;
                    }
                    let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
                    encode_pdf_attachment(&pdf_path)
                })
                .collect();
        }
        let use_pdf = use_inline_pdf && !pdf_attachments.is_empty();
        let (selected_system, selected_sources, selected_contexts) = if use_pdf {
            build_selected_papers_pdf_system_prompt(root, slugs)
        } else {
            build_selected_papers_system_prompt(root, slugs, &provider, &model)
        };
        let _ = app.emit(sources_event_name, selected_sources);
        let context_event_name = format!("{event_name}-context");
        let _ = app.emit(
            context_event_name.as_str(),
            LibrarySentContextPayload {
                mode: "papers".to_string(),
                sections: selected_contexts,
            },
        );
        system = selected_system;
    } else if use_no_source {
        // Clear any sources the previous turn left on screen.
        let _ = app.emit(sources_event_name, Vec::<crate::models::RetrievedChunk>::new());
        system = build_plain_system_prompt();
    } else if use_snippets {
        let query_text = messages
            .iter()
            .rev()
            .find(|m| m.role == "user")
            .and_then(|m| match &m.content {
                ChatContent::Text(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_default();

        let settings = rag::get_rag_settings(root);
        let retrieved = if settings.is_configured() && !query_text.is_empty() {
            match rag::embed_query(root, &query_text, &settings).await {
                Ok(vec) => rag::search_snippet_chunks_with_vec(root, vec, 12).await.unwrap_or_default(),
                Err(_) => vec![],
            }
        } else {
            vec![]
        };

        // Surface the retrieved snippets as sources, reusing the library-chat
        // `RetrievedChunk` shape so the same UI renders them. A snippet's
        // `paper_id` stores the source paper's slug, so it doubles as `slug`
        // for click-to-open.
        let snippet_sources: Vec<crate::models::RetrievedChunk> = retrieved
            .iter()
            .map(|s| crate::models::RetrievedChunk {
                chunk_id: s.snippet_id.clone(),
                paper_id: s.paper_id.clone(),
                slug: s.paper_id.clone(),
                chunk_index: 0,
                text: s.text.clone(),
                score: s.score,
                paper_title: s.paper_title.clone(),
                source_type: "snippet".to_string(),
                source_id: Some(s.snippet_id.clone()),
                source_label: None,
            })
            .collect();
        let _ = app.emit(sources_event_name, snippet_sources);
        system = build_snippet_system_prompt(&retrieved);
    } else {
        let settings = rag::get_rag_settings(root);
        let rag_chunks = if settings.is_configured() {
            let query_text = messages
                .iter()
                .rev()
                .find(|m| m.role == "user")
                .and_then(|m| match &m.content {
                    ChatContent::Text(s) => Some(s.clone()),
                    _ => None,
                });
            if let Some(q) = query_text {
                if let Ok(vec) = rag::embed_query(root, &q, &settings).await {
                    rag::search_library_chunks_with_vec(root, vec, settings.top_k * 2)
                        .await
                        .ok()
                        .filter(|v| !v.is_empty())
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };
        let _ = app.emit(
            sources_event_name,
            rag_chunks.as_deref().unwrap_or(&[]).to_vec(),
        );
        system = build_library_system_prompt(rag_chunks.as_deref());
    }

    let mut messages = messages;
    let mut extra_parts: Vec<ChatContentPart> = Vec::new();
    extra_parts.extend(pdf_attachments);
    if let Some(attachments) = attachments {
        extra_parts.extend(attachments.iter().cloned());
    }
    if !extra_parts.is_empty() {
        if let Some(idx) = messages.iter().rposition(|m| m.role == "user") {
            let mut parts: Vec<ChatContentPart> = match &messages[idx].content {
                ChatContent::Text(t) => {
                    vec![ChatContentPart::Text { text: t.clone() }]
                }
                ChatContent::Parts(p) => p.clone(),
            };
            parts.extend(extra_parts);
            messages[idx].content = ChatContent::Parts(parts);
        }
    }

    let mut all_messages = vec![ChatMessage {
        role: "system".to_string(),
        content: system.into(),
    }];
    all_messages.extend_from_slice(&messages);

    llm::chat_completion_stream(
        &provider,
        &api_key,
        &model,
        &all_messages,
        event_name,
        app,
        use_reasoning,
        reasoning_effort,
        "library_chat",
        cancel,
        web_search,
    )
    .await
}

fn build_snippet_system_prompt(snippets: &[crate::models::RetrievedSnippet]) -> String {
    let mut prompt = String::from(
        "You are a research assistant helping the user explore their snippet library — \
         a personal collection of text excerpts saved from academic papers.\n\
         Rules:\n\
         1. Answer ONLY from the snippets provided below — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. When citing a snippet, reference the source paper title and page:\n\
            《论文标题》第 N 页\n\
         4. If multiple snippets are relevant, synthesize them.\n\n",
    );

    if snippets.is_empty() {
        prompt.push_str("[未找到相关素材。请先在「素材库」中嵌入素材（设置 → RAG 配置向量化）。]\n");
        return prompt;
    }

    prompt.push_str("--- 检索到的相关素材 ---\n\n");
    for (i, s) in snippets.iter().enumerate() {
        let tags = if s.tags.is_empty() {
            String::new()
        } else {
            format!(" | 标签: {}", s.tags.join(", "))
        };
        let note = if s.note.trim().is_empty() {
            String::new()
        } else {
            format!(" | 笔记: {}", s.note.trim())
        };
        prompt.push_str(&format!(
            "[素材 {n} | 来源: 《{title}》第 {page} 页{tags}{note}]\n{text}\n\n",
            n = i + 1,
            title = s.paper_title,
            page = s.page,
            text = s.text,
        ));
    }
    prompt
}

// ── Library chat window ──────────────────────────────────────────────────────

const WINDOW_SIZE_STORE_KEY: &str = "library_chat_window_size";
const PAPER_AI_WINDOW_SIZE_STORE_KEY: &str = "paper_ai_window_size_v3";
const DEFAULT_WINDOW_W: f64 = 760.0;
const DEFAULT_WINDOW_H: f64 = 560.0;
const LIBRARY_CHAT_MIN_WINDOW_W: f64 = 560.0;
const LIBRARY_CHAT_MIN_WINDOW_H: f64 = 400.0;
const PAPER_AI_DEFAULT_WINDOW_W: f64 = 540.0;
const PAPER_AI_DEFAULT_WINDOW_H: f64 = 660.0;
const PAPER_AI_MIN_WINDOW_W: f64 = 400.0;
const PAPER_AI_MIN_WINDOW_H: f64 = 500.0;

fn load_library_chat_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let v = store.get(WINDOW_SIZE_STORE_KEY)?;
    let w = v.get("w")?.as_f64()?;
    let h = v.get("h")?.as_f64()?;
    // Sanity check: ignore absurdly small/large values
    if w >= LIBRARY_CHAT_MIN_WINDOW_W
        && h >= LIBRARY_CHAT_MIN_WINDOW_H
        && w <= 4000.0
        && h <= 3000.0
    {
        Some((w, h))
    } else {
        None
    }
}

pub fn save_library_chat_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < LIBRARY_CHAT_MIN_WINDOW_W || height < LIBRARY_CHAT_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

fn load_paper_ai_window_size(app: &tauri::AppHandle) -> Option<(f64, f64)> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    let v = store.get(PAPER_AI_WINDOW_SIZE_STORE_KEY)?;
    let w = v.get("w")?.as_f64()?;
    let h = v.get("h")?.as_f64()?;
    if w >= PAPER_AI_MIN_WINDOW_W && h >= PAPER_AI_MIN_WINDOW_H && w <= 4000.0 && h <= 3000.0 {
        Some((w, h))
    } else {
        None
    }
}

pub fn save_paper_ai_window_size(app: &tauri::AppHandle, width: f64, height: f64) {
    use tauri_plugin_store::StoreExt;
    if width < PAPER_AI_MIN_WINDOW_W || height < PAPER_AI_MIN_WINDOW_H {
        return;
    }
    if let Ok(store) = app.store("settings.json") {
        store.set(
            PAPER_AI_WINDOW_SIZE_STORE_KEY,
            serde_json::json!({ "w": width, "h": height }),
        );
        let _ = store.save();
    }
}

pub fn open_paper_ai_window(app: &tauri::AppHandle, slug: Option<&str>) -> Result<(), String> {
    use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

    if let Some(win) = app.get_webview_window("paper-ai") {
        let _ = win.set_focus();
        if let Some(slug) = slug {
            let _ = win.emit("paper-ai-slug", slug.to_string());
        }
        return Ok(());
    }

    let (width, height) = load_paper_ai_window_size(app)
        .unwrap_or((PAPER_AI_DEFAULT_WINDOW_W, PAPER_AI_DEFAULT_WINDOW_H));

    let builder = WebviewWindowBuilder::new(
        app,
        "paper-ai",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 论文 AI")
    .inner_size(width, height)
    .min_inner_size(PAPER_AI_MIN_WINDOW_W, PAPER_AI_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    let win = builder
        .build()
        .map_err(|e| format!("Open paper AI window: {e}"))?;

    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_paper_ai_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
                }
            }
        };
        match event {
            WindowEvent::Resized(_) | WindowEvent::CloseRequested { .. } => save(&win_ref),
            _ => {}
        }
    });

    let win_c = win.clone();
    let slug = slug.map(|s| s.to_string());
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = win_c.unmaximize();
        let _ = win_c.set_size(tauri::LogicalSize::new(width, height));
        if let Some(slug) = slug {
            let _ = win_c.emit("paper-ai-slug", slug);
        }
    });

    Ok(())
}

pub fn open_library_chat_window(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

    if let Some(win) = app.get_webview_window("library-chat") {
        let _ = win.set_focus();
        return Ok(());
    }

    let (width, height) =
        load_library_chat_window_size(app).unwrap_or((DEFAULT_WINDOW_W, DEFAULT_WINDOW_H));

    let builder = WebviewWindowBuilder::new(
        app,
        "library-chat",
        WebviewUrl::App(std::path::PathBuf::from("/")),
    )
    .title("Argus — 智能问答")
    .inner_size(width, height)
    .min_inner_size(LIBRARY_CHAT_MIN_WINDOW_W, LIBRARY_CHAT_MIN_WINDOW_H);

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition { x: 14.0, y: 22.0 });

    // Windows has no overlay titlebar; drop the native decorations so the window
    // uses our custom in-app titlebar (WindowControls) instead of showing an
    // extra native title row above it — matching the main window.
    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false);

    let win = builder
        .build()
        .map_err(|e| format!("Open library chat window: {e}"))?;

    let win_ref = win.clone();
    let app_handle = app.clone();
    win.on_window_event(move |event| {
        let save = |w: &tauri::WebviewWindow| {
            if let (Ok(phys), Ok(sf)) = (w.inner_size(), w.scale_factor()) {
                if phys.width > 0 && phys.height > 0 {
                    save_library_chat_window_size(
                        &app_handle,
                        phys.width as f64 / sf,
                        phys.height as f64 / sf,
                    );
                }
            }
        };
        match event {
            WindowEvent::Resized(_) => save(&win_ref),
            WindowEvent::CloseRequested { .. } => {
                save(&win_ref);
                // The user has left agent mode by leaving entirely; stop paying
                // to hold their conversation's cache open.
                crate::cache_keepalive::disarm();  // the window is going; no one to tell
            }
            _ => {}
        }
    });

    let win_c = win.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let _ = win_c.unmaximize();
        let _ = win_c.set_size(tauri::LogicalSize::new(width, height));
    });

    Ok(())
}

// ── Context building ──────────────────────────────────────────────────────────

fn build_metadata_string(meta: Option<&crate::models::PaperMeta>) -> String {
    let Some(m) = meta else { return String::new() };
    let mut s = format!("标题：{}\n", m.title);
    if !m.authors.is_empty() {
        s.push_str(&format!("作者：{}\n", m.authors.join(", ")));
    }
    if let Some(y) = m.year { s.push_str(&format!("年份：{y}\n")); }
    if let Some(ref v) = m.venue { s.push_str(&format!("期刊/会议：{v}\n")); }
    if let Some(ref a) = m.paper_abstract {
        if !a.trim().is_empty() { s.push_str(&format!("摘要：{}\n", a.trim())); }
    }
    s
}

/// Shared budget (in chars) for how much paper fulltext to hand `model_id`.
/// Used by BOTH the chat and the summary/analysis paths so the truncated text —
/// and therefore the leading context block — is byte-identical across tasks,
/// which is what lets the provider reuse the cached prompt prefix.
pub fn paper_context_budget(provider: &crate::models::AiProvider, model_id: &str) -> usize {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.context_length)
        .map(|cl| ((cl as usize * 7 / 10) * 4).min(300_000))
        .unwrap_or(60_000)
        // Reserve space for the instructions and conversation history (~10K chars).
        .saturating_sub(10_000)
}

/// Canonical, byte-stable "paper context" block. Every full-text paper task
/// (AI chat, AI summary/analysis) emits THIS as its first system message so the
/// large paper text sits at an identical prompt prefix and the provider can
/// serve it from cache instead of re-billing it on every call. Anything
/// task-specific (persona, instructions, the user's question) must come AFTER.
pub fn build_paper_context_block(meta: Option<&PaperMeta>, fulltext: &str) -> String {
    let mut s = String::from("--- PAPER CONTEXT ---\n");
    if let Some(m) = meta {
        s.push_str(&format!("Title: {}\n", m.title));
        if !m.authors.is_empty() {
            s.push_str(&format!("Authors: {}\n", m.authors.join(", ")));
        }
        if let Some(y) = m.year {
            s.push_str(&format!("Year: {y}\n"));
        }
        if let Some(ref v) = m.venue {
            s.push_str(&format!("Venue: {v}\n"));
        }
        if let Some(ref a) = m.paper_abstract {
            if !a.trim().is_empty() {
                s.push_str(&format!("Abstract: {}\n", a.trim()));
            }
        }
    }
    s.push_str("\n--- PAPER CONTENT ---\n");
    s.push_str(fulltext);
    s
}

/// Stable research-assistant persona for single-paper chat. Sent as a system
/// message AFTER the canonical context block (task-specific, so kept out of the
/// shared cacheable prefix).
pub const PAPER_ASSISTANT_PERSONA: &str = "You are a research assistant helping the user understand and analyze a specific paper. Answer questions clearly and accurately based on the paper content provided.";

fn get_fulltext_context(
    root: &str,
    slug: &str,
    provider: &crate::models::AiProvider,
    model_id: &str,
) -> (String, bool) {
    let fulltext = extraction::read_fulltext(root, slug);
    if fulltext.is_empty() {
        return (String::new(), false);
    }
    let max_chars = paper_context_budget(provider, model_id);
    let total = fulltext.chars().count();
    let truncated = total > max_chars;
    let context: String = fulltext.chars().take(max_chars).collect();
    (context, truncated)
}

/// System prompt for "no knowledge base": a general assistant with none of the
/// library-specific citation rules, which would otherwise make the model
/// apologise for having no sources to cite.
fn build_plain_system_prompt() -> String {
    String::from(
        "You are a helpful, knowledgeable assistant.\n\
         Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         Answer from your own knowledge. No documents from the user's library are \
         attached to this conversation, so do not claim to be citing them; if a \
         question needs a specific paper the user has not provided, say so.",
    )
}

fn build_library_system_prompt(chunks: Option<&[RetrievedChunk]>) -> String {
    let mut prompt = String::from(
        "You are a research assistant managing a personal academic library.\n\
         Rules:\n\
         1. Answer ONLY from the retrieved context below — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. For every key claim, cite the source paper using this format:\n\
            **论文标题** (`slug`) — 来源: 类型\n\
            Example: **Attention Is All You Need** (`vaswani2017attention`) — 来源: PDF正文\n\
         4. When a metadata chunk directly answers the question (venue, authors, year), \
            lead with that information.\n\
         5. If multiple papers are relevant, list each separately with its citation.\n\n",
    );

    match chunks {
        Some(c) if !c.is_empty() => {
            prompt.push_str("--- 检索到的相关内容 ---\n\n");
            for (i, chunk) in c.iter().enumerate() {
                let type_label = match chunk.source_type.as_str() {
                    "metadata" => "元数据".to_string(),
                    "highlight" => chunk
                        .source_label
                        .clone()
                        .unwrap_or_else(|| "批注".to_string()),
                    "note" => chunk
                        .source_label
                        .clone()
                        .unwrap_or_else(|| "笔记".to_string()),
                    _ => "PDF正文".to_string(),
                };
                let paper_display = if chunk.paper_title.is_empty() {
                    format!("`{}`", chunk.slug)
                } else {
                    format!("**{}** (`{}`)", chunk.paper_title, chunk.slug)
                };
                prompt.push_str(&format!(
                    "[片段 {i_1} | 论文: {paper} | 类型: {src}]\n{text}\n\n",
                    i_1 = i + 1,
                    paper = paper_display,
                    src = type_label,
                    text = chunk.text,
                ));
            }
        }
        Some(_) => {
            prompt.push_str("[未找到相关内容。请先向量化文献库（设置 → RAG）。]\n");
        }
        None => {
            prompt.push_str("[RAG 未配置，将基于通用知识回答，无法引用具体文献。]\n");
        }
    }
    prompt
}

fn selected_papers_context_budget(
    provider: &crate::models::AiProvider,
    model_id: &str,
) -> usize {
    provider
        .models
        .iter()
        .find(|m| m.id == model_id)
        .and_then(|m| m.context_length)
        .map(|cl| ((cl as usize * 7 / 10) * 4).min(300_000))
        .unwrap_or(80_000)
        .saturating_sub(12_000)
}

fn take_chars(input: &str, max_chars: usize) -> (String, bool) {
    if max_chars == 0 {
        return (String::new(), !input.is_empty());
    }
    let total = input.chars().count();
    if total <= max_chars {
        (input.to_string(), false)
    } else {
        (input.chars().take(max_chars).collect(), true)
    }
}

fn build_selected_papers_system_prompt(
    root: &str,
    slugs: &[String],
    provider: &crate::models::AiProvider,
    model_id: &str,
) -> (String, Vec<RetrievedChunk>, Vec<LibrarySentContextSection>) {
    let mut prompt = String::from(
        "You are a research assistant helping the user compare and analyze a selected set of academic papers.\n\
         Rules:\n\
         1. Answer ONLY from the selected papers provided below — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. For every key claim, cite the source paper using this format:\n\
            **论文标题** (`slug`)\n\
         4. If the selected papers do not contain enough evidence, say that clearly.\n\
         5. When multiple selected papers are relevant, synthesize them and distinguish their contributions.\n\n",
    );

    if slugs.is_empty() {
        prompt.push_str("[未选择文献。请先在「文献库」模式中添加要参与问答的论文。]\n");
        return (prompt, Vec::new(), Vec::new());
    }

    let mut seen = std::collections::HashSet::new();
    let selected: Vec<&str> = slugs
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert((*s).to_string()))
        .collect();

    if selected.is_empty() {
        prompt.push_str("[未选择文献。请先在「文献库」模式中添加要参与问答的论文。]\n");
        return (prompt, Vec::new(), Vec::new());
    }

    let mut sources = Vec::new();
    let mut contexts = Vec::new();
    let mut remaining_budget = selected_papers_context_budget(provider, model_id);
    let per_paper_budget = (remaining_budget / selected.len()).max(4_000);
    let mut found_count = 0usize;

    prompt.push_str("--- 用户选择的文献 ---\n\n");

    for slug in selected {
        if remaining_budget == 0 {
            break;
        }

        let Ok(meta) = paper::read_meta(root, slug) else {
            prompt.push_str(&format!("[未找到文献: `{slug}`]\n\n"));
            continue;
        };

        let metadata = build_metadata_string(Some(&meta));
        let fulltext = extraction::read_fulltext(root, slug);

        let reserved_for_meta = metadata.chars().count() + 700;
        let text_budget = per_paper_budget
            .min(remaining_budget)
            .saturating_sub(reserved_for_meta)
            .max(1_000)
            .min(remaining_budget);
        let (text_excerpt, text_truncated) = take_chars(&fulltext, text_budget);

        found_count += 1;
        let mut paper_context = format!(
            "[文献 {n}]\nSlug: `{slug}`\n{metadata}",
            n = found_count,
            slug = slug,
            metadata = metadata,
        );

        if !text_excerpt.trim().is_empty() {
            paper_context.push_str("\n全文内容:\n");
            paper_context.push_str(&text_excerpt);
            if text_truncated {
                paper_context.push_str("\n[该文全文已因上下文长度限制截断]");
            }
            paper_context.push('\n');
        } else {
            paper_context.push_str("\n[该文暂无全文文本。只能基于元数据回答。]\n");
        }
        paper_context.push('\n');
        prompt.push_str(&paper_context);
        contexts.push(LibrarySentContextSection {
            kind: "paper".to_string(),
            label: meta.title.clone(),
            content: paper_context,
        });

        let source_text = if !text_excerpt.trim().is_empty() {
            text_excerpt.chars().take(800).collect()
        } else {
            metadata.clone()
        };
        sources.push(RetrievedChunk {
            chunk_id: format!("selected-{slug}"),
            paper_id: meta.id.clone(),
            slug: slug.to_string(),
            chunk_index: found_count.saturating_sub(1) as u32,
            text: source_text,
            score: 1.0,
            paper_title: meta.title.clone(),
            source_type: "text".to_string(),
            source_id: None,
            source_label: Some("已选文献".to_string()),
        });

        let used = metadata.chars().count() + text_excerpt.chars().count() + 700;
        remaining_budget = remaining_budget.saturating_sub(used);
    }

    if found_count == 0 {
        prompt.push_str("[所选文献未找到。请重新添加文献。]\n");
    }

    (prompt, sources, contexts)
}

/// Build a minimal system prompt for providers that accept inline PDFs.
/// The actual paper content comes from the PDF file blocks injected into the
/// user message, so the prompt only needs to tell the model what to do.
fn build_selected_papers_pdf_system_prompt(
    root: &str,
    slugs: &[String],
) -> (String, Vec<RetrievedChunk>, Vec<LibrarySentContextSection>) {
    let mut prompt = String::from(
        "You are a research assistant helping the user compare and analyze a selected set of academic papers.\n\
         The full PDFs of the selected papers are attached to the user message.\n\
         Rules:\n\
         1. Answer ONLY from the attached PDFs — do not hallucinate.\n\
         2. Respond in the same language the user uses (Chinese if asked in Chinese).\n\
         3. For every key claim, cite the source paper using this format:\n\
            **论文标题** (`slug`)\n\
         4. If the selected papers do not contain enough evidence, say that clearly.\n\
         5. When multiple selected papers are relevant, synthesize them and distinguish their contributions.\n\n",
    );

    if slugs.is_empty() {
        prompt.push_str("[未选择文献。请先在「文献库」模式中添加要参与问答的论文。]\n");
        return (prompt, Vec::new(), Vec::new());
    }

    let mut sources = Vec::new();
    let mut contexts = Vec::new();
    let mut found_count = 0usize;

    prompt.push_str("--- 用户选择的文献（PDF 已作为附件发送） ---\n\n");

    for slug in slugs {
        let slug = slug.trim();
        if slug.is_empty() {
            continue;
        }

        let Ok(meta) = paper::read_meta(root, slug) else {
            prompt.push_str(&format!("[未找到文献: `{slug}`]\n\n"));
            continue;
        };

        found_count += 1;
        let paper_line = format!("[文献 {n}] {title} (`{slug}`)\n", n = found_count, title = meta.title);
        prompt.push_str(&paper_line);

        let pdf_path = crate::metadata::find_pdf_in_dir(root, slug);
        let context_line = if pdf_path.exists() {
            format!("{paper_line}PDF 文件已直接发送给模型。\n")
        } else {
            format!("{paper_line}[未找到 PDF 文件]\n")
        };
        contexts.push(LibrarySentContextSection {
            kind: "paper".to_string(),
            label: meta.title.clone(),
            content: context_line,
        });

        sources.push(RetrievedChunk {
            chunk_id: format!("selected-{slug}"),
            paper_id: meta.id.clone(),
            slug: slug.to_string(),
            chunk_index: found_count.saturating_sub(1) as u32,
            text: meta.title.clone(),
            score: 1.0,
            paper_title: meta.title.clone(),
            source_type: "pdf".to_string(),
            source_id: None,
            source_label: Some("已选文献 PDF".to_string()),
        });
    }

    if found_count == 0 {
        prompt.push_str("[所选文献未找到。请重新添加文献。]\n");
    }

    (prompt, sources, contexts)
}

fn encode_pdf_attachment(pdf_path: &std::path::Path) -> Option<ChatContentPart> {
    use base64::Engine;
    if !pdf_path.exists() {
        return None;
    }
    let bytes = std::fs::read(pdf_path).ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let filename = pdf_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("paper.pdf")
        .to_string();
    Some(ChatContentPart::File {
        file: FileData {
            filename,
            file_data: format!("data:application/pdf;base64,{b64}"),
        },
    })
}

fn build_system_prompt(
    meta: Option<&PaperMeta>,
    context: &str,
    truncated: bool,
    rag_used: bool,
    ai_summary: Option<&str>,
) -> String {
    let mut prompt = String::from(
        "You are a research assistant helping the user understand and analyze a specific paper. \
         Answer questions clearly and accurately based on the paper content provided.\n\n",
    );

    if let Some(m) = meta {
        prompt.push_str(&format!("Paper: {}\n", m.title));
        if !m.authors.is_empty() {
            prompt.push_str(&format!("Authors: {}\n", m.authors.join(", ")));
        }
        if let Some(y) = m.year {
            prompt.push_str(&format!("Year: {y}\n"));
        }
        if let Some(ref v) = m.venue {
            prompt.push_str(&format!("Venue: {v}\n"));
        }
        if let Some(ref a) = m.paper_abstract {
            if !a.trim().is_empty() {
                prompt.push_str(&format!("Abstract: {}\n", a.trim()));
            }
        }
    }

    // Inject AI-generated summary if provided
    if let Some(summary) = ai_summary {
        if !summary.trim().is_empty() {
            prompt.push_str("\n--- AI SUMMARY ---\n");
            prompt.push_str(summary.trim());
        }
    }

    if !context.is_empty() {
        if rag_used {
            prompt.push_str("\n--- RELEVANT EXCERPTS (RAG) ---\n");
        } else {
            prompt.push_str("\n--- PAPER CONTENT ---\n");
        }
        prompt.push_str(context);
        if truncated {
            prompt.push_str(
                "\n\n[Content truncated due to length. This is a partial view of the paper.]",
            );
        }
    } else if ai_summary.map(|s| !s.trim().is_empty()).unwrap_or(false) {
        // Have summary but no fulltext — that's fine, summary is sufficient context
    } else {
        prompt.push_str(
            "\n[Full text not available. I will answer based on metadata only. \
             For richer analysis, please extract the paper's fulltext first.]",
        );
    }

    prompt
}

// ── Agent mode ───────────────────────────────────────────────────────────────
//
// Instead of retrieving context up front and hoping it contains the answer, the
// model is handed the same tools the MCP server exposes and decides for itself
// what to look at: list, search, open a paper's sections, read the part it
// needs, check the user's own notes. That turns "answer from these five RAG
// chunks" into "go find out", which is what the user actually asked for.
//
// Shape of one turn:
//
//   1. Ask the model with `tools` available (non-streaming).
//   2. If it requested tools, run them, append the results, and go back to 1.
//   3. When it stops asking for tools, stream the final answer.
//
// Each tool call is announced on `{event_name}-agent` so the UI can show what
// the model is doing rather than a blank pause.

/// Default tool rounds per answer when the caller does not say.
pub const DEFAULT_AGENT_ROUNDS: usize = 10;
/// Lower bound — one round is still a useful "look something up, then answer".
pub const MIN_AGENT_ROUNDS: usize = 1;
/// Hard ceiling, whatever the user configures.
///
/// The budget is theirs to set, but a model stuck re-reading the same paper
/// must still terminate. Five hundred rounds is enough for a genuine survey
/// across a large library, and still a bound rather than an open tab.
pub const MAX_AGENT_ROUNDS: usize = 500;

/// Clamp a user-supplied round budget into the range the loop will honour.
pub fn clamp_agent_rounds(requested: Option<usize>) -> usize {
    requested
        .unwrap_or(DEFAULT_AGENT_ROUNDS)
        .clamp(MIN_AGENT_ROUNDS, MAX_AGENT_ROUNDS)
}

// ── Context budgeting ────────────────────────────────────────────────────────
//
// What actually limits how much a tool may return is the model's context
// window, and it varies by two orders of magnitude across the models this app
// can be pointed at — a million tokens for DeepSeek V4, 131k for a local
// gpt-oss. A single constant is wrong for both: it starves the large model and
// still overruns the small one.
//
// The hazard is also not the single result. The agent loop keeps every tool
// result in the transcript for every later round, so round twenty carries
// rounds one through nineteen. Unbounded results do not merely produce one
// oversized message; they walk the whole conversation off the end of the window
// and the provider rejects the request outright, mid-answer. So there are two
// budgets: what one result may be, and what all of them together may be.

/// Roughly how many tokens a string costs.
///
/// A fixed characters-per-token ratio cannot be right for both scripts at once:
/// English runs about four characters to the token, CJK about one. Anything in
/// between is optimistic for one of them, and being optimistic here means
/// overrunning the window this budget exists to protect. So the two are counted
/// separately and the estimate follows the actual content — which matters most
/// for a tool result, where JSON structure is ASCII and the values may not be.
///
/// Deliberately an over-estimate for non-Latin scripts: a tokenizer usually does
/// better than one token per character, and the cost of guessing high is a
/// slightly smaller result rather than a rejected request.
fn estimate_tokens(s: &str) -> usize {
    let mut ascii = 0usize;
    let mut wide = 0usize;
    for c in s.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            wide += 1;
        }
    }
    wide + ascii.div_ceil(4)
}

/// Assumed window when a model's `context_length` is not configured.
///
/// Small enough to be safe on anything current, which is the right way to be
/// wrong when the answer is unknown.
const ASSUMED_CONTEXT_TOKENS: u64 = 128_000;

/// Floor under both budgets, for a model declared with an implausibly tiny
/// window — a tool result has to be able to say something.
const MIN_RESULT_TOKENS: usize = 2_000;

#[derive(Debug, Clone, Copy)]
struct ContextBudget {
    /// The most one tool result may contribute, in tokens.
    single_result: usize,
    /// The most all tool results together may occupy, in tokens, before the
    /// oldest are evicted.
    transcript: usize,
}

impl ContextBudget {
    /// Derive both budgets from the model's declared window.
    ///
    /// A quarter of the window for one result, half for all of them: the
    /// remainder is the system prompt, the tool schemas, the conversation, and
    /// the answer being generated.
    fn for_model(provider: &AiProvider, model_id: &str) -> Self {
        let tokens = provider
            .models
            .iter()
            .find(|m| m.id == model_id)
            .and_then(|m| m.context_length)
            .filter(|&n| n > 0)
            .unwrap_or(ASSUMED_CONTEXT_TOKENS) as usize;
        ContextBudget {
            single_result: (tokens / 4).max(MIN_RESULT_TOKENS),
            transcript: (tokens / 2).max(MIN_RESULT_TOKENS * 2),
        }
    }
}

/// The prompt used when the user has not written their own.
///
/// The collection-first instruction is the substantive part. A model left to
/// itself reaches for a keyword `list_papers` across the whole library, which
/// matches on titles and misses the structure the user built by hand — their
/// grouping encodes intent that a keyword cannot recover.
pub const DEFAULT_AGENT_SYSTEM_PROMPT: &str = r#"You are a research assistant with direct read access to the user's literature library through MCP tools.

Answer by looking things up. Anything you say about what is or is not in this library must come from a tool call, never from memory.

## Find papers by walking the user's own structure

The library is organised into collections, and the user has already done that work. Papers filed together are related in the way the user thinks about them, which is information no keyword search contains. So navigate down rather than searching across:

1. `list_collections` — see the structure, with each collection's readable path and paper count.
2. `list_papers` with the `collection_id` of the branch that fits the question.
3. Only then narrow, with `query`, `tag`, `year_from` / `year_to`, `venue`, `min_citations`, or by sorting on `citations` or `year`.

Descend one level at a time when a collection has children; the leaf the user filed something under is usually more specific than the question implies.

Use `search_papers` when the question is about something *inside* the papers — a method, a phrase, a finding. That searches full text, not titles.

A bare keyword `list_papers` across the whole library is the last resort, not the first move. It matches titles the user never grouped that way, and misses the ones they did.

## Reading

- `get_library_stats` when you need the shape of the library before anything else.
- `list_papers` and `search_papers` return an abstract with each paper. The default is a preview; pass `abstract_detail: "full"` when you intend to reason over the abstracts themselves rather than just pick papers from them, and `"none"` when you only need identifiers. Read those before opening anything, and open only what the question actually needs.
- `get_paper_sections`, then `get_paper_fulltext` with a `section`, to read the relevant part instead of paging through a whole paper.
- `get_note` and `get_highlights` for what the user wrote or marked themselves. Often the best answer to "what did I think about this".

## Answering

Cite the papers you used by title. If the library does not contain the answer, say so plainly instead of filling the gap from memory. Keep tool calls purposeful: each one costs the user time and money.

Reply in the language the user wrote in."#;

/// The prompt in force: the user's, or the default when they have not set one.
///
/// Shared by the agent loop and the cache keepalive — they must send byte-identical
/// system messages or the warmed prefix is not the one the next question asks for.
fn agent_system_prompt(app: &tauri::AppHandle) -> String {
    resolve_system_prompt(crate::mcp::client::read_settings(app).system_prompt)
}

/// A blank or whitespace-only prompt means the default, not silence.
///
/// Split out from the settings read so the one thing that can go wrong here —
/// clearing the box and leaving the model with no instructions at all — is
/// testable without a running Tauri app.
fn resolve_system_prompt(configured: String) -> String {
    if configured.trim().is_empty() {
        DEFAULT_AGENT_SYSTEM_PROMPT.to_string()
    } else {
        configured
    }
}

/// Trim a tool result to what one message may contribute.
///
/// Returns the text and whether anything was dropped — reported rather than
/// re-derived by the caller, so nothing has to sniff for a marker string.
fn truncate_tool_result(value: &serde_json::Value, cap_tokens: usize) -> (String, bool) {
    let rendered = serde_json::to_string(value).unwrap_or_else(|_| "null".into());
    if estimate_tokens(&rendered) <= cap_tokens {
        return (rendered, false);
    }
    // Accumulated as we go rather than re-estimating a growing prefix, which
    // would be quadratic on a result of half a million characters.
    let mut kept = String::new();
    let mut ascii = 0usize;
    let mut wide = 0usize;
    for c in rendered.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            wide += 1;
        }
        if wide + ascii.div_ceil(4) > cap_tokens {
            break;
        }
        kept.push(c);
    }
    // Tell the model the result was cut, so it pages rather than assuming it
    // has seen everything.
    (
        format!("{kept}\n…[result truncated; narrow the query or use offset/limit]"),
        true,
    )
}

/// What an evicted tool result is replaced with.
///
/// A stub rather than a deletion: removing the message would orphan the
/// `tool_calls` entry that referenced it, and providers reject that outright.
const EVICTED_NOTE: &str =
    r#"{"note":"this earlier result was dropped to stay inside the context window; call the tool again if you still need it"}"#;

/// Keep the accumulated tool output under `budget` by blanking the oldest results.
///
/// Called after each tool message is appended. Newest results are the ones the
/// model is reasoning about, so the oldest go first.
///
/// This does invalidate the prompt cache from the first evicted message onward —
/// the prefix changed. That is a real cost, and the right one to pay: the
/// alternative is the provider rejecting the request for exceeding its window,
/// which loses the whole answer rather than one cache hit. It only happens on
/// runs that would otherwise have failed.
/// `protect_from` is the index where the round in progress begins. Its results
/// count towards the total but are never blanked: the model has not seen them
/// yet, and replacing one with "call the tool again if you still need it" makes
/// it do exactly that, whereupon the re-fetched copy evicts another result from
/// the same round. That oscillates instead of converging, and every turn of it
/// is a billed round. Better to run slightly over budget for one round than to
/// take back what was just fetched.
fn evict_old_tool_results(
    convo: &mut [serde_json::Value],
    budget: usize,
    protect_from: usize,
) -> usize {
    let size = |m: &serde_json::Value| m["content"].as_str().map_or(0, estimate_tokens);
    let stub_size = estimate_tokens(EVICTED_NOTE);

    let mut total: usize = convo
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(size)
        .sum();
    if total <= budget {
        return 0;
    }

    let mut evicted = 0usize;
    for msg in convo.iter_mut().take(protect_from) {
        if total <= budget {
            break;
        }
        if msg["role"] != "tool" {
            continue;
        }
        let was = size(msg);
        if was <= stub_size {
            continue; // already a stub
        }
        msg["content"] = serde_json::json!(EVICTED_NOTE);
        total = total.saturating_sub(was).saturating_add(stub_size);
        evicted += 1;
    }
    evicted
}

/// Library Q&A where the model drives its own retrieval through the MCP tools.
///
/// Connects any external MCP servers the user configured, runs the loop, and
/// tears the connections down on every exit path — including cancellation, which
/// is why the loop lives in its own function.
#[allow(clippy::too_many_arguments)]
pub async fn chat_with_library_agent(
    root: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    event_name: &str,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    max_rounds: Option<usize>,
    // Opaque label for the conversation this answer belongs to, echoed back in
    // the cache-keepalive status so the chat window can put the indicator on
    // the right conversation.
    conversation_id: Option<&str>,
    app: &tauri::AppHandle,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    use tauri::Emitter;

    let settings = crate::mcp::client::read_settings(app);
    // The caller may override, but the round budget lives in settings so both
    // windows agree on it without having to pass it around.
    let max_rounds = clamp_agent_rounds(max_rounds.or(Some(settings.max_rounds)));

    let bridge = crate::mcp::client::ToolBridge::connect(&settings.servers).await;
    if !bridge.is_empty() || !bridge.failures().is_empty() {
        let _ = app.emit(
            format!("{event_name}-agent").as_str(),
            serde_json::json!({
                "phase": "servers",
                "extraTools": bridge.tools().len(),
                "failed": bridge.failures(),
            }),
        );
    }

    let warm_from = messages.clone();
    let outcome = run_agent_loop(
        root,
        messages,
        provider_id,
        model_id,
        event_name,
        use_reasoning,
        reasoning_effort,
        max_rounds,
        &bridge,
        app,
        cancel,
    )
    .await;

    // Snapshot before shutting the servers down: the external tool declarations
    // are part of the prefix that has to stay warm, and a keepalive sending a
    // different tools block would refresh a cache entry nothing will ask for.
    let tool_defs = agent_tool_defs(&bridge);
    bridge.shutdown().await;

    // Keep the prefix this answer just built warm, so the user can think for
    // ten minutes and still ask their follow-up at the cache-hit price.
    if let Ok(answer) = &outcome {
        arm_cache_keepalive(
            root,
            provider_id,
            model_id,
            conversation_id,
            &warm_from,
            answer,
            tool_defs,
            app,
        );
    }
    outcome
}

/// Ask the keepalive to hold this conversation's prefix, if that is worth doing.
///
/// Nothing here is allowed to fail loudly: a keepalive that cannot start is a
/// missed saving, not a broken answer.
#[allow(clippy::too_many_arguments)]
fn arm_cache_keepalive(
    root: &str,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    conversation_id: Option<&str>,
    messages: &[ChatMessage],
    answer: &str,
    tool_defs: Vec<serde_json::Value>,
    app: &tauri::AppHandle,
) {
    if !crate::mcp::client::read_settings(app).keep_cache_warm {
        crate::cache_keepalive::disarm_and_announce(app);
        return;
    }
    let Ok((provider, api_key, model)) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)
    else {
        return;
    };
    let hits = LAST_AGENT_CACHE_HITS.load(std::sync::atomic::Ordering::Relaxed);
    if !crate::cache_keepalive::is_worthwhile(&provider, hits) {
        crate::cache_keepalive::disarm_and_announce(app);
        return;
    }

    // The prefix the *next* question will send: system prompt, the turns so
    // far, and the answer just given. Not the loop's internal transcript, which
    // carries `tool` messages the next request will not repeat.
    let mut warm = vec![serde_json::json!({
        "role": "system",
        "content": agent_system_prompt(app),
    })];
    warm.extend(
        messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": &m.content})),
    );
    warm.push(serde_json::json!({ "role": "assistant", "content": answer }));

    crate::cache_keepalive::arm(
        app,
        crate::cache_keepalive::Warm {
            conversation_id: conversation_id.map(str::to_string),
            provider,
            api_key,
            model,
            messages: warm,
            tools: tool_defs,
        },
    );
}

/// Cache-hit tokens reported by the most recent agent turn.
///
/// Used to decide whether pinging this provider actually lands as a hit. A
/// static rather than a return value because the decision is made after the
/// loop has already handed back its answer.
static LAST_AGENT_CACHE_HITS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The tool array an agent turn sends: the library's own, then the external
/// servers'. Built in one place so the keepalive cannot drift from the real
/// request — a differing tools block is a different cached prefix.
fn agent_tool_defs(bridge: &crate::mcp::client::ToolBridge) -> Vec<serde_json::Value> {
    crate::mcp::agent::tools()
        .into_iter()
        .chain(bridge.tools().iter().cloned())
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.input_schema,
                }
            })
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
async fn run_agent_loop(
    root: &str,
    messages: Vec<ChatMessage>,
    provider_id: Option<&str>,
    model_id: Option<&str>,
    event_name: &str,
    use_reasoning: bool,
    reasoning_effort: Option<&str>,
    max_rounds: usize,
    bridge: &crate::mcp::client::ToolBridge,
    app: &tauri::AppHandle,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<String, String> {
    use tauri::Emitter;

    let (provider, api_key, model) =
        ai_manager::resolve_provider_model(root, provider_id, model_id)?;

    if !llm::supports_tool_calling(&provider) {
        return Err(format!(
            "「{}」暂不支持工具调用，无法使用 Agent 模式。请改用 DeepSeek、OpenRouter \
             等 OpenAI 兼容的供应商。",
            provider.name
        ));
    }

    let budget = ContextBudget::for_model(&provider, &model);
    let agent_event = format!("{event_name}-agent");
    let cancelled = || {
        cancel
            .as_ref()
            .is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    };

    // Tool declarations in OpenAI-compatible form, built once per answer: the
    // library's own tools first, then whatever the configured servers offer.
    let tool_defs = agent_tool_defs(bridge);

    let mut convo: Vec<serde_json::Value> = vec![serde_json::json!({
        "role": "system",
        "content": agent_system_prompt(app),
    })];
    convo.extend(
        messages
            .iter()
            .map(|m| serde_json::json!({"role": m.role, "content": &m.content})),
    );

    let mut rounds = 0usize;
    // Rounds whose tool calls were actually run — which is what the user's
    // "tool call limit" means. Counted separately from `rounds` because the
    // loop has to make one more provider call than that to discover the model
    // is finished, and billing the user's budget for that call would mean a
    // limit of N only ever ran N-1 rounds (and a limit of 1 ran none at all).
    let mut tool_rounds = 0usize;
    // Everything the model said across all rounds, in order. Kept so the
    // persisted answer matches what the user watched appear.
    let mut streamed = String::new();
    // Summed across rounds and emitted once at the end. Per round would show
    // the user a cost strip while the agent is still working, reporting only
    // the most recent call rather than what the answer actually cost.
    let mut usage = llm::TurnUsage::default();

    // A labelled block rather than `?` and early returns: every way out of the
    // loop has to pass through the `emit_usage` below it. A run the user stopped
    // at round thirteen still spent thirteen rounds of tokens, and dropping the
    // total meant the cost strip showed nothing at all for it.
    let outcome: Result<(), String> = 'rounds: {
    loop {
        if cancelled() {
            break 'rounds Err("cancelled".to_string());
        }

        let _ = app.emit(
            agent_event.as_str(),
            serde_json::json!({ "phase": "thinking", "round": rounds + 1 }),
        );

        // Streamed: the model's prose reaches the user as it is generated, both
        // for the running commentary before a tool call and for the final
        // answer. `done` is withheld until the loop finishes.
        let turn = llm::stream_with_tools(
            &provider,
            &api_key,
            &model,
            &convo,
            &tool_defs,
            event_name,
            app,
            use_reasoning,
            reasoning_effort,
            "library-agent",
            cancel.clone(),
        )
        .await;
        let turn = match turn {
            Ok(t) => t,
            Err(e) => break 'rounds Err(e),
        };
        rounds += 1;
        usage.add(&turn.usage);
        if !turn.content.is_empty() {
            if !streamed.is_empty() {
                streamed.push_str("\n\n");
            }
            streamed.push_str(&turn.content);
        }

        // No tool calls means the model is done looking and this is the answer.
        if turn.tool_calls.is_empty() {
            break 'rounds Ok(());
        }

        if tool_rounds >= max_rounds {
            // Stop calling tools, but give the model one turn to answer with
            // what it already gathered rather than returning nothing.
            let _ = app.emit(
                agent_event.as_str(),
                serde_json::json!({ "phase": "limit", "rounds": tool_rounds, "max": max_rounds }),
            );
            convo.push(serde_json::json!({
                "role": "user",
                "content": "Tool budget reached. Answer now with what you have gathered, \
                            and say which parts you could not verify.",
            }));
            let final_turn = llm::stream_with_tools(
                &provider,
                &api_key,
                &model,
                &convo,
                // No tools on this call: the budget is spent, and offering them
                // again invites another round the loop would have to ignore.
                &[],
                event_name,
                app,
                use_reasoning,
                reasoning_effort,
                "library-agent",
                cancel.clone(),
            )
            .await;
            let final_turn = match final_turn {
                Ok(t) => t,
                Err(e) => break 'rounds Err(e),
            };
            usage.add(&final_turn.usage);
            if !final_turn.content.is_empty() {
                if !streamed.is_empty() {
                    streamed.push_str("\n\n");
                }
                streamed.push_str(&final_turn.content);
            }
            break 'rounds Ok(());
        }

        // Record the model's own turn verbatim; providers reject a `tool`
        // message whose id has no matching `tool_calls` entry before it.
        convo.push(serde_json::json!({
            "role": "assistant",
            "content": turn.content,
            "tool_calls": turn.tool_calls.iter().map(|c| serde_json::json!({
                "id": c.id,
                "type": "function",
                "function": { "name": c.name, "arguments": c.arguments.to_string() }
            })).collect::<Vec<_>>(),
        }));

        tool_rounds += 1;
        // Where this round's results begin, so eviction can leave them alone.
        let round_start = convo.len();
        for call in &turn.tool_calls {
            if cancelled() {
                break 'rounds Err("cancelled".to_string());
            }
            let _ = app.emit(
                agent_event.as_str(),
                serde_json::json!({
                    "phase": "tool",
                    "round": rounds,
                    "tool": call.name,
                    // Absent for the library's own tools; the UI shows it as a
                    // chip so the user can see when a third party was consulted.
                    "server": bridge.server_of(&call.name),
                    "arguments": call.arguments,
                }),
            );

            let outcome = if bridge.handles(&call.name) {
                // An external server is another process on the far end of a
                // pipe — already async, and nothing to move off this thread.
                bridge.call(&call.name, &call.arguments).await
            } else {
                let root_owned = root.to_string();
                let name = call.name.clone();
                let args = call.arguments.clone();
                // Library tools do blocking file and SQLite work; keep it off
                // the async runtime that is also carrying this provider's HTTP
                // request.
                match tokio::task::spawn_blocking(move || {
                    crate::mcp::agent::call(&root_owned, &name, &args)
                })
                .await
                {
                    Ok(v) => v,
                    Err(e) => break 'rounds Err(format!("tool task failed: {e}")),
                }
            };

            // A failed tool is fed back as text rather than aborting: the model
            // can read the message and correct itself (wrong slug, bad section
            // name) on the next round.
            let (content, ok, cut) = match &outcome {
                Ok(value) => {
                    let (text, cut) = truncate_tool_result(value, budget.single_result);
                    (text, true, cut)
                }
                Err(e) => (format!("{{\"error\":{}}}", serde_json::json!(e)), false, false),
            };

            // Send exactly what the model got. There used to be a display cap
            // here, which meant the panel showed less than the model read — the
            // opposite of what an inspector is for. It costs nothing to send it
            // all: the detail box is rendered only when that step is expanded,
            // and `stripTransientContext` drops these payloads before the
            // conversation is written to disk.
            //
            // `truncated` now means the *model's* copy was cut to fit the
            // context budget, which is the only truncation worth reporting.
            let _ = app.emit(
                agent_event.as_str(),
                serde_json::json!({
                    "phase": "result",
                    "round": rounds,
                    "tool": call.name,
                    "server": bridge.server_of(&call.name),
                    "ok": ok,
                    "chars": content.chars().count(),
                    "preview": content,
                    "truncated": cut,
                }),
            );

            convo.push(serde_json::json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": content,
            }));

            // The loop retains every result for every later round, so this is
            // where a long run would otherwise walk off the end of the window.
            let dropped = evict_old_tool_results(&mut convo, budget.transcript, round_start);
            if dropped > 0 {
                let _ = app.emit(
                    agent_event.as_str(),
                    serde_json::json!({
                        "phase": "evicted",
                        "round": rounds,
                        "dropped": dropped,
                    }),
                );
            }
        }
    }
    };

    // What the answer cost, on every path out of the loop — finished, cancelled
    // or failed. Skipped only when nothing was spent, so a run that died before
    // its first response does not put an empty cost strip on screen.
    if usage.input_tokens > 0 || usage.output_tokens > 0 || usage.cost_usd.is_some() {
        llm::emit_usage(app, event_name, &usage);
    }
    LAST_AGENT_CACHE_HITS.store(usage.cache_hit_tokens, std::sync::atomic::Ordering::Relaxed);
    outcome?;

    if cancelled() {
        return Err("cancelled".to_string());
    }

    let _ = app.emit(
        agent_event.as_str(),
        serde_json::json!({ "phase": "answering", "rounds": rounds }),
    );
    // The text already reached the UI as it streamed; close the stream so the
    // frontend stops showing a caret.
    let _ = app.emit(event_name, serde_json::json!({ "delta": "", "done": true }));
    Ok(streamed)
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    fn model(id: &str, context_length: Option<u64>) -> AiProvider {
        AiProvider {
            id: "p".into(),
            name: "P".into(),
            kind: "openai_compatible".into(),
            base_url: "https://example.com/v1".into(),
            enabled: true,
            created_at: String::new(),
            models: vec![serde_json::from_value(serde_json::json!({
                "id": id,
                "display_name": id,
                "context_length": context_length,
            }))
            .expect("AiModel fixture")],
        }
    }

    #[test]
    fn oversized_tool_results_are_cut_and_flagged() {
        let cap = 24_000; // tokens
        let big = serde_json::json!({ "text": "x".repeat(cap * 8) });
        let (out, cut) = truncate_tool_result(&big, cap);
        assert!(cut, "the caller was not told it was cut");
        assert!(out.contains("truncated"), "the model must be told it was cut");
        assert!(estimate_tokens(&out) < cap + 60, "{}", estimate_tokens(&out));
    }

    /// The estimate has to follow the script, not a single ratio. A budget that
    /// assumes four characters per token hands a Chinese transcript four times
    /// the tokens it thinks it is handing over, and the request is rejected
    /// mid-answer by the provider.
    #[test]
    fn chinese_text_is_not_costed_as_though_it_were_english() {
        let english = "the quick brown fox jumps over the lazy dog. ".repeat(40);
        let chinese = "这是一段中文文本，用来检查预算估计是否按字符计费。".repeat(40);

        // Per character, not per string — the two are different lengths. The
        // point is the *rate*: the same number of characters costs about four
        // times as much in Chinese.
        let rate = |s: &str| estimate_tokens(s) as f64 / s.chars().count() as f64;
        assert!(
            rate(&chinese) > rate(&english) * 3.5,
            "{:.2} tokens/char for Chinese vs {:.2} for English",
            rate(&chinese),
            rate(&english)
        );
        // Roughly one token per character, and never fewer.
        assert!(estimate_tokens(&chinese) >= chinese.chars().count());
    }

    /// Truncating a Chinese result must respect the token budget and must not
    /// split a character.
    #[test]
    fn truncation_of_chinese_stays_within_budget_and_on_boundaries() {
        let value = serde_json::json!({ "text": "文献综述与实验结果".repeat(500) });
        let (out, cut) = truncate_tool_result(&value, 400);
        assert!(cut);
        assert!(estimate_tokens(&out) < 460, "{}", estimate_tokens(&out));
        // Reconstructing it proves nothing was cut mid-character.
        assert!(out.chars().count() > 0);
    }

    #[test]
    fn small_results_pass_through_untouched() {
        let small = serde_json::json!({ "total": 3 });
        assert_eq!(
            truncate_tool_result(&small, 24_000),
            (r#"{"total":3}"#.to_string(), false)
        );
    }

    /// The budget has to follow the model. A million-token window and a 131k one
    /// cannot share a constant: one would be starved, the other overrun.
    #[test]
    fn the_budget_scales_with_the_declared_window() {
        let big = ContextBudget::for_model(&model("v4", Some(1_000_000)), "v4");
        let small = ContextBudget::for_model(&model("oss", Some(131_072)), "oss");
        assert!(
            big.single_result > small.single_result * 5,
            "a 1M model got {} and a 131k model got {}",
            big.single_result,
            small.single_result
        );
        // Room for every abstract in a large library in one call.
        assert!(big.single_result > 100_000, "{}", big.single_result);
        assert!(big.transcript > big.single_result);
    }

    /// An unconfigured window must be assumed small, not unlimited.
    #[test]
    fn an_unknown_window_is_assumed_conservative() {
        let unknown = ContextBudget::for_model(&model("m", None), "m");
        let known = ContextBudget::for_model(&model("m", Some(1_000_000)), "m");
        assert!(unknown.single_result < known.single_result);
        assert!(unknown.single_result >= MIN_RESULT_TOKENS);

        // A model declaring something absurd still gets a usable budget.
        let tiny = ContextBudget::for_model(&model("m", Some(10)), "m");
        assert_eq!(tiny.single_result, MIN_RESULT_TOKENS);
    }

    /// Eviction has to blank the oldest results, not remove the messages: a
    /// `tool` message removed from the transcript orphans the `tool_calls` entry
    /// that referenced it, and providers reject that outright.
    #[test]
    fn eviction_blanks_the_oldest_and_keeps_every_message() {
        let mut convo = vec![
            serde_json::json!({"role": "system", "content": "s"}),
            serde_json::json!({"role": "tool", "tool_call_id": "a", "content": "x".repeat(4000)}),
            serde_json::json!({"role": "tool", "tool_call_id": "b", "content": "y".repeat(4000)}),
            serde_json::json!({"role": "tool", "tool_call_id": "c", "content": "z".repeat(4000)}),
        ];
        let before = convo.len();
        // Everything is fair game: nothing belongs to a round in progress.
        let dropped = evict_old_tool_results(&mut convo, 1500, before);

        assert_eq!(convo.len(), before, "a message was removed, orphaning its call id");
        assert!(dropped >= 1);
        assert!(convo[1]["content"].as_str().unwrap().contains("dropped"), "the oldest survived");
        assert_eq!(
            convo[3]["content"].as_str().unwrap().chars().count(),
            4000,
            "the newest result — the one being reasoned about — was evicted"
        );
        for msg in &convo {
            assert!(msg["role"].is_string());
        }
    }

    #[test]
    fn eviction_leaves_a_transcript_within_budget_alone() {
        let mut convo = vec![
            serde_json::json!({"role": "tool", "tool_call_id": "a", "content": "small"}),
        ];
        assert_eq!(evict_old_tool_results(&mut convo, 10_000, 1), 0);
        assert_eq!(convo[0]["content"], "small");
    }

    /// Results from the round in progress must survive even when the transcript
    /// is over budget. Blanking one the model has not read yet tells it to call
    /// the tool again, and the re-fetched copy evicts another result from the
    /// same round — a loop that burns a billed round every time round it goes.
    #[test]
    fn the_round_in_progress_is_never_evicted() {
        let mut convo = vec![
            serde_json::json!({"role": "tool", "tool_call_id": "old", "content": "o".repeat(8000)}),
            serde_json::json!({"role": "assistant", "content": "", "tool_calls": []}),
            serde_json::json!({"role": "tool", "tool_call_id": "n1", "content": "a".repeat(8000)}),
            serde_json::json!({"role": "tool", "tool_call_id": "n2", "content": "b".repeat(8000)}),
        ];
        // This round's results begin at index 2. The budget is far too small for
        // all three, so without the guard the parallel calls would eat each other.
        let dropped = evict_old_tool_results(&mut convo, 100, 2);

        assert_eq!(dropped, 1, "only the previous round's result was available");
        assert!(convo[0]["content"].as_str().unwrap().contains("dropped"));
        assert_eq!(convo[2]["content"].as_str().unwrap().chars().count(), 8000);
        assert_eq!(convo[3]["content"].as_str().unwrap().chars().count(), 8000);
    }

    /// The loop must be bounded: a model that keeps calling tools cannot be
    /// allowed to spend the user's credits without end.
    /// Chinese answers are multi-byte; chunking on bytes would panic mid-character.
    #[test]
    fn chunking_respects_character_boundaries() {
        let answer = "这是一个中文回答，混合 English 和标点。".repeat(5);
        let chars: Vec<char> = answer.chars().collect();
        let rejoined: String = chars.chunks(24).flat_map(|c| c.iter()).collect();
        assert_eq!(rejoined, answer, "chunking lost or corrupted characters");
    }

    /// The budget is the user's to set, but it must stay bounded on both ends:
    /// zero rounds would answer nothing, and an unbounded one lets a looping
    /// model spend without limit.
    #[test]
    fn round_budget_is_clamped_both_ways() {
        assert_eq!(clamp_agent_rounds(None), DEFAULT_AGENT_ROUNDS);
        assert_eq!(clamp_agent_rounds(Some(0)), MIN_AGENT_ROUNDS);
        assert_eq!(clamp_agent_rounds(Some(usize::MAX)), MAX_AGENT_ROUNDS);
        assert_eq!(clamp_agent_rounds(Some(25)), 25, "a valid budget must pass through");
        assert!(MIN_AGENT_ROUNDS >= 1 && DEFAULT_AGENT_ROUNDS <= MAX_AGENT_ROUNDS);
    }

    /// The prompt is what stops the model from answering from memory.
    /// Clearing the box must restore the built-in prompt. Sending an empty
    /// system message instead would leave the model to guess what the tools are
    /// for, and it would go straight back to keyword-sweeping the library.
    #[test]
    fn an_empty_prompt_falls_back_rather_than_leaving_the_model_bare() {
        assert_eq!(resolve_system_prompt(String::new()), DEFAULT_AGENT_SYSTEM_PROMPT);
        assert_eq!(
            resolve_system_prompt("  \n\t ".to_string()),
            DEFAULT_AGENT_SYSTEM_PROMPT,
            "whitespace is not a prompt"
        );
        assert_eq!(
            resolve_system_prompt("Answer in haiku.".to_string()),
            "Answer in haiku.",
            "a real prompt must win outright, not be appended to the default"
        );
    }

    #[test]
    fn system_prompt_sets_the_ground_rules() {
        let p = DEFAULT_AGENT_SYSTEM_PROMPT;
        assert!(p.contains("get_library_stats"));
        assert!(p.contains("say so plainly"), "must forbid filling gaps from memory");
        assert!(p.contains("Cite"), "answers should name their sources");
        // The whole point of the rewrite: the user's grouping is the index, and
        // a keyword sweep over the library is the fallback rather than the reflex.
        assert!(p.contains("list_collections"), "must point at the collection tree");
        assert!(
            p.contains("last resort"),
            "must demote the whole-library keyword sweep"
        );
    }
}
