//! Generic id-keyed CRDT merge for JSON arrays with delete tombstones.
//!
//! Shared by the AI-conversation writers (paper `ai_conversations.json`, canvas
//! `<id>.chat.json`), which used to overwrite the whole array — so a machine with
//! a stale copy wiped conversations another machine had synced in. This mirrors
//! the typed highlight merge (`paper::merge_highlights`): union items by `id`
//! (newer timestamp wins), union tombstones (newer delete wins), and drop an item
//! a not-yet-superseded tombstone covers. A real delete propagates instead of the
//! conversation resurrecting from a machine that still has it; a re-edit after a
//! delete brings it back.
//!
//! On disk the doc is `{ "<items_field>": [...], "tombstones": [{id, deleted_at}] }`.
//! A legacy bare array reads as that doc with no tombstones.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// One lock per file, so a read-merge-write can't interleave with another and
/// drop an item. Keyed by an arbitrary string (use the file path).
pub fn lock_for(key: &str) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let locks = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = locks.lock().unwrap_or_else(|e| e.into_inner());
    map.entry(key.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

pub struct IdKeyedDoc {
    pub items: Vec<Value>,
    /// Objects shaped `{ "id": String, "deleted_at": String }`.
    pub tombstones: Vec<Value>,
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// An item's effective time for conflict resolution: `time_field`, else `createdAt`.
fn item_time(v: &Value, time_field: &str) -> String {
    str_field(v, time_field)
        .or_else(|| str_field(v, "createdAt"))
        .unwrap_or("")
        .to_string()
}

/// Parse a v2 doc (`{items_field: [...], tombstones: [...]}`) or a legacy bare array.
pub fn parse_doc(raw: &Value, items_field: &str) -> IdKeyedDoc {
    if let Some(arr) = raw.as_array() {
        return IdKeyedDoc { items: arr.clone(), tombstones: Vec::new() };
    }
    IdKeyedDoc {
        items: raw.get(items_field).and_then(|v| v.as_array()).cloned().unwrap_or_default(),
        tombstones: raw.get("tombstones").and_then(|v| v.as_array()).cloned().unwrap_or_default(),
    }
}

/// Merge `incoming` into `disk`. Items without an `id` are kept as-is (appended).
pub fn merge(disk: IdKeyedDoc, incoming: IdKeyedDoc, time_field: &str) -> IdKeyedDoc {
    let mut by_id: HashMap<String, Value> = HashMap::new();
    let mut no_id: Vec<Value> = Vec::new();
    for it in disk.items.into_iter().chain(incoming.items) {
        match str_field(&it, "id").map(str::to_string) {
            Some(id) => match by_id.get(&id) {
                Some(existing) if item_time(existing, time_field) >= item_time(&it, time_field) => {}
                _ => {
                    by_id.insert(id, it);
                }
            },
            None => no_id.push(it),
        }
    }

    let mut tombs: HashMap<String, String> = HashMap::new();
    for t in disk.tombstones.into_iter().chain(incoming.tombstones) {
        if let Some(id) = str_field(&t, "id").map(str::to_string) {
            let dt = str_field(&t, "deleted_at").unwrap_or("").to_string();
            if tombs.get(&id).map(|d| dt > *d).unwrap_or(true) {
                tombs.insert(id, dt);
            }
        }
    }
    // A re-edit at/after the delete revives the item and retires its tombstone.
    tombs.retain(|id, deleted_at| match by_id.get(id) {
        Some(it) => item_time(it, time_field).as_str() <= deleted_at.as_str(),
        None => true,
    });

    let mut items: Vec<Value> = by_id
        .into_iter()
        .filter(|(id, _)| !tombs.contains_key(id))
        .map(|(_, v)| v)
        .collect();
    // Newest first, matching how the conversation lists render.
    items.sort_by(|a, b| item_time(b, time_field).cmp(&item_time(a, time_field)));
    items.extend(no_id);

    let tombstones: Vec<Value> = tombs
        .into_iter()
        .map(|(id, deleted_at)| json!({ "id": id, "deleted_at": deleted_at }))
        .collect();

    IdKeyedDoc { items, tombstones }
}

/// Serialize a doc to the on-disk object form.
pub fn to_value(doc: &IdKeyedDoc, items_field: &str) -> Value {
    json!({ items_field: doc.items, "tombstones": doc.tombstones })
}

/// The effective (non-deleted) items, newest first — what the frontend consumes.
pub fn effective_items(doc: &IdKeyedDoc, time_field: &str) -> Vec<Value> {
    let deleted: HashMap<&str, &str> = doc
        .tombstones
        .iter()
        .filter_map(|t| Some((str_field(t, "id")?, str_field(t, "deleted_at").unwrap_or(""))))
        .collect();
    let mut items: Vec<Value> = doc
        .items
        .iter()
        .filter(|it| match str_field(it, "id").and_then(|id| deleted.get(id)) {
            Some(&dt) => item_time(it, time_field).as_str() > dt,
            None => true,
        })
        .cloned()
        .collect();
    items.sort_by(|a, b| item_time(b, time_field).cmp(&item_time(a, time_field)));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(id: &str, updated: &str) -> Value {
        json!({ "id": id, "updatedAt": updated, "messages": [] })
    }
    fn ids(items: &[Value]) -> Vec<String> {
        let mut v: Vec<String> = items.iter().filter_map(|c| c["id"].as_str().map(String::from)).collect();
        v.sort();
        v
    }
    fn doc(items: Vec<Value>) -> IdKeyedDoc {
        IdKeyedDoc { items, tombstones: vec![] }
    }

    #[test]
    fn stale_save_keeps_the_other_machines_conversation() {
        let disk = doc(vec![conv("A", "2026-01-01T00:00:00Z")]);
        let incoming = doc(vec![conv("B", "2026-01-02T00:00:00Z")]);
        let merged = merge(disk, incoming, "updatedAt");
        assert_eq!(ids(&effective_items(&merged, "updatedAt")), vec!["A", "B"]);
    }

    #[test]
    fn newer_update_wins_same_id() {
        let disk = doc(vec![conv("A", "2026-01-01T00:00:00Z")]);
        let incoming = doc(vec![conv("A", "2026-05-01T00:00:00Z")]);
        let merged = merge(disk, incoming, "updatedAt");
        let eff = effective_items(&merged, "updatedAt");
        assert_eq!(eff.len(), 1);
        assert_eq!(eff[0]["updatedAt"], "2026-05-01T00:00:00Z");
    }

    #[test]
    fn same_machine_delete_sticks_and_survives_a_stale_readd() {
        // Delete A (tombstone) — it must NOT resurrect from disk, which is the bug
        // a naive union-by-id would reintroduce.
        let disk = doc(vec![conv("A", "2026-01-01T00:00:00Z"), conv("B", "2026-01-02T00:00:00Z")]);
        let incoming = IdKeyedDoc {
            items: vec![conv("B", "2026-01-02T00:00:00Z")],
            tombstones: vec![json!({ "id": "A", "deleted_at": "2026-03-01T00:00:00Z" })],
        };
        let merged = merge(disk, incoming, "updatedAt");
        assert_eq!(ids(&effective_items(&merged, "updatedAt")), vec!["B"]);
        // A stale machine that still has A re-submits it → the delete still wins.
        let merged2 = merge(merged, doc(vec![conv("A", "2026-01-01T00:00:00Z")]), "updatedAt");
        assert_eq!(ids(&effective_items(&merged2, "updatedAt")), vec!["B"]);
    }

    #[test]
    fn re_edit_after_delete_revives() {
        let disk = IdKeyedDoc {
            items: vec![],
            tombstones: vec![json!({ "id": "A", "deleted_at": "2026-03-01T00:00:00Z" })],
        };
        let incoming = doc(vec![conv("A", "2026-04-01T00:00:00Z")]);
        let merged = merge(disk, incoming, "updatedAt");
        assert_eq!(ids(&effective_items(&merged, "updatedAt")), vec!["A"]);
        assert!(merged.tombstones.is_empty());
    }

    #[test]
    fn legacy_bare_array_parses() {
        let raw = json!([conv("A", "2026-01-01T00:00:00Z")]);
        let d = parse_doc(&raw, "conversations");
        assert_eq!(ids(&d.items), vec!["A"]);
        assert!(d.tombstones.is_empty());
    }
}
