//! Activity-log storage with a per-device CRDT merge.
//!
//! The activity log (`{root}/.argus/activity.json`) records reading time, opens,
//! completions and AI analyses. The library folder is designed to live in an
//! iCloud/Dropbox-synced location and be opened on more than one machine (see
//! `watcher.rs`), so this file changes underneath us from the other machine.
//!
//! The old writer serialized the frontend's whole in-memory model and
//! `atomic_write`'d it over the file with no merge. That is last-writer-wins:
//! a machine holding a stale copy overwrote reading time the other machine had
//! just synced in, then synced the clobbered file back — losing data on both.
//!
//! The fix is a state-based CRDT keyed by a stable, **non-synced** device id:
//!
//! ```text
//! { version: 2, days: { "2026-09-14": { date, devices: {
//!     "<device-a>": { opened, completed, ai_analyzed, paper_reading_ms,
//!                     reading_ms, sessions, updated_at },
//!     "<device-b>": { ... } } } } }
//! ```
//!
//! Each machine only ever writes its own device slot. `save` reads the file,
//! replaces just this device's slot for each day it touches, and leaves every
//! other device's slot exactly as it is on disk. That makes the merge
//! last-writer-wins **per (day, device) cell** — a join on a lattice, so it is
//! order-independent, idempotent for a machine re-saving its own data, and
//! additive across machines. The frontend sums across device slots for display.
//!
//! `legacy` is a reserved slot: pre-v2 flat files (which had no device
//! dimension) migrate their day into `devices.legacy`, which is never written
//! again, so old single-machine history is preserved verbatim and never
//! double-counted.

use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Days older than this (relative to the local calendar day) are dropped on
/// save, matching the frontend's retention window.
const RETENTION_DAYS: i64 = 370;

/// Reserved device slot holding all pre-v2 (device-less) history.
const DEVICE_LEGACY: &str = "legacy";

/// The per-device fields. Everything else on a day object (`date`) is structural.
const SLOT_KEYS: [&str; 7] = [
    "opened",
    "completed",
    "ai_analyzed",
    "paper_reading_ms",
    "reading_ms",
    "sessions",
    "updated_at",
];

fn activity_log_path(root: &str) -> PathBuf {
    Path::new(root).join(".argus").join("activity.json")
}

fn empty_v2() -> Value {
    json!({ "version": 2, "days": {} })
}

// ── Per-root serialization ───────────────────────────────────────────────────
// `save` is read-merge-write; two concurrent saves to the same file must not
// interleave and drop a device slot. One lock per library root.

fn root_locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_for(root: &str) -> Arc<Mutex<()>> {
    let mut map = root_locks().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(root.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

// ── Schema normalization / migration ─────────────────────────────────────────

/// A day is already v2 when it carries a `devices` map.
fn is_v2_day(day: &Value) -> bool {
    day.get("devices").is_some_and(|d| d.is_object())
}

/// Normalize any accepted shape (v1 flat day-maps, or already-v2) to v2. A v1
/// day's flat fields become its `devices.legacy` slot.
fn to_v2(value: &Value) -> Value {
    let days_in = value
        .as_object()
        .and_then(|o| o.get("days"))
        .and_then(|d| d.as_object());
    let mut days_out = Map::new();
    if let Some(days) = days_in {
        for (date, day) in days {
            if is_v2_day(day) {
                let mut d = day.clone();
                if let Some(o) = d.as_object_mut() {
                    if !o.contains_key("date") {
                        o.insert("date".to_string(), json!(date));
                    }
                }
                days_out.insert(date.clone(), d);
            } else if let Some(day_obj) = day.as_object() {
                let mut slot = Map::new();
                for k in SLOT_KEYS {
                    if let Some(v) = day_obj.get(k) {
                        slot.insert(k.to_string(), v.clone());
                    }
                }
                let mut devices = Map::new();
                devices.insert(DEVICE_LEGACY.to_string(), Value::Object(slot));
                days_out.insert(date.clone(), json!({ "date": date, "devices": devices }));
            }
        }
    }
    json!({ "version": 2, "days": days_out })
}

/// A blank/unknown device id must never collide with the reserved `legacy`
/// slot, and must still be attributable rather than silently dropped.
fn sanitize_device(device_id: &str) -> String {
    let id = device_id.trim();
    if id.is_empty() || id == DEVICE_LEGACY {
        "unknown".to_string()
    } else {
        id.to_string()
    }
}

/// Merge `incoming` (this machine's full model) into `disk`, replacing ONLY
/// `device_id`'s slot per day and preserving every other device's slot on disk.
/// Both inputs are normalized to v2, so a v1 disk file migrates transparently.
/// Incoming's slots for *other* devices are ignored — disk is authoritative for
/// them — so a stale view of another machine can never clobber it.
fn merge(disk: &Value, incoming: &Value, device_id: &str) -> Value {
    let device = sanitize_device(device_id);
    let disk_v2 = to_v2(disk);
    let inc_v2 = to_v2(incoming);
    let mut days: Map<String, Value> = disk_v2
        .get("days")
        .and_then(|d| d.as_object())
        .cloned()
        .unwrap_or_default();

    if let Some(inc_days) = inc_v2.get("days").and_then(|d| d.as_object()) {
        for (date, inc_day) in inc_days {
            let Some(inc_slot) = inc_day
                .get("devices")
                .and_then(|d| d.as_object())
                .and_then(|m| m.get(&device))
            else {
                continue;
            };
            let day = days
                .entry(date.clone())
                .or_insert_with(|| json!({ "date": date, "devices": {} }));
            if let Some(day_obj) = day.as_object_mut() {
                day_obj.insert("date".to_string(), json!(date));
                let devices = day_obj
                    .entry("devices")
                    .or_insert_with(|| json!({}));
                if let Some(dev_obj) = devices.as_object_mut() {
                    dev_obj.insert(device.clone(), inc_slot.clone());
                }
            }
        }
    }
    json!({ "version": 2, "days": Value::Object(days) })
}

/// Drop days older than the retention window. The backend owns the on-disk day
/// set now, so without this a day the frontend pruned would resurrect from disk
/// on every merge.
fn prune(mut value: Value, retention_days: i64) -> Value {
    let cutoff = (chrono::Local::now().date_naive() - chrono::Duration::days(retention_days))
        .format("%Y-%m-%d")
        .to_string();
    if let Some(days) = value.get_mut("days").and_then(|d| d.as_object_mut()) {
        days.retain(|date, _| date.as_str() >= cutoff.as_str());
    }
    value
}

// ── Public API (called by the Tauri commands) ────────────────────────────────

/// Read the log, migrated to v2. A corrupt/half-synced file reads as empty
/// rather than erroring, so a bad remote copy never bricks the panel.
pub fn read_log(root: &str) -> Value {
    let path = activity_log_path(root);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|c| serde_json::from_str::<Value>(&c).ok())
        .map(|v| to_v2(&v))
        .unwrap_or_else(empty_v2)
}

/// Merge this device's slot from `incoming` into the on-disk log and write it
/// back atomically, under a per-root lock.
pub fn save_log(root: &str, device_id: &str, incoming: &Value) -> Result<(), String> {
    let lock = lock_for(root);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());

    let disk = read_log(root);
    let merged = prune(merge(&disk, incoming, device_id), RETENTION_DAYS);

    let argus_dir = Path::new(root).join(".argus");
    std::fs::create_dir_all(&argus_dir).map_err(|e| format!("Create .argus: {e}"))?;
    let content = serde_json::to_string_pretty(&merged)
        .map_err(|e| format!("Serialize activity.json: {e}"))?;
    crate::fsutil::atomic_write_str(&argus_dir.join("activity.json"), &content)
        .map_err(|e| format!("Write activity.json: {e}"))
}

/// A stable id for this machine, stored in the OS app-config dir (machine-local,
/// **never** inside the synced library — otherwise a copied library folder would
/// clone the id and two machines would collide on one slot, reviving the bug).
pub fn device_id(app: &tauri::AppHandle) -> Result<String, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app_config_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Create config dir: {e}"))?;
    let path = dir.join("device_id");
    if let Ok(existing) = std::fs::read_to_string(&path) {
        let id = existing.trim();
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    let id = uuid::Uuid::new_v4().to_string();
    std::fs::write(&path, &id).map_err(|e| format!("Write device_id: {e}"))?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day_with(device: &str, reading_ms: i64, opened: i64) -> Value {
        json!({
            "date": "2026-09-14",
            "devices": {
                device: {
                    "opened": { "p1": { "slug": "p1", "title": "P1", "count": opened, "last_at": "2026-09-14T01:00:00Z" } },
                    "completed": {},
                    "ai_analyzed": {},
                    "paper_reading_ms": { "p1": { "slug": "p1", "title": "P1", "duration_ms": reading_ms, "last_at": "2026-09-14T01:00:00Z" } },
                    "reading_ms": reading_ms,
                    "sessions": [],
                    "updated_at": "2026-09-14T01:00:00Z"
                }
            }
        })
    }

    fn model(day: Value) -> Value {
        json!({ "version": 2, "days": { "2026-09-14": day } })
    }

    fn slot_reading(m: &Value, device: &str) -> Option<i64> {
        m["days"]["2026-09-14"]["devices"][device]["reading_ms"].as_i64()
    }

    #[test]
    fn merges_two_devices_additively_without_touching_the_other_slot() {
        let disk = model(day_with("A", 100, 3));
        // Incoming from machine B — and it even carries a STALE view of A (999),
        // which must be ignored because disk is authoritative for other devices.
        let mut incoming = model(day_with("B", 200, 5));
        incoming["days"]["2026-09-14"]["devices"]["A"] =
            day_with("A", 999, 99)["devices"]["A"].clone();

        let merged = merge(&disk, &incoming, "B");
        assert_eq!(slot_reading(&merged, "A"), Some(100), "A slot from disk, not incoming's stale 999");
        assert_eq!(slot_reading(&merged, "B"), Some(200), "B slot adopted from incoming");
    }

    #[test]
    fn same_device_resave_is_idempotent_not_summed() {
        let disk = model(day_with("A", 100, 3));
        let incoming = model(day_with("A", 150, 4)); // A read more, memory total is 150
        let merged = merge(&disk, &incoming, "A");
        assert_eq!(slot_reading(&merged, "A"), Some(150), "overwrite own slot — never 250");
    }

    #[test]
    fn v1_flat_disk_migrates_to_legacy_and_survives_a_device_merge() {
        // Pre-v2 file: flat day fields, no devices map.
        let disk = json!({
            "version": 1,
            "days": { "2026-09-14": {
                "date": "2026-09-14",
                "opened": {}, "completed": {}, "ai_analyzed": {},
                "paper_reading_ms": {}, "reading_ms": 50, "sessions": [],
                "updated_at": "2026-09-14T00:00:00Z"
            } }
        });
        let incoming = model(day_with("B", 200, 5));
        let merged = merge(&disk, &incoming, "B");
        assert_eq!(slot_reading(&merged, "legacy"), Some(50), "old flat history preserved in legacy slot");
        assert_eq!(slot_reading(&merged, "B"), Some(200));

        // Merging the same incoming again must not double the legacy history.
        let merged2 = merge(&merged, &incoming, "B");
        assert_eq!(slot_reading(&merged2, "legacy"), Some(50));
        assert_eq!(slot_reading(&merged2, "B"), Some(200));
    }

    #[test]
    fn to_v2_is_stable_on_already_v2() {
        let v2 = model(day_with("A", 10, 1));
        assert_eq!(to_v2(&v2), v2);
    }

    #[test]
    fn blank_device_falls_back_and_never_hijacks_legacy() {
        let disk = empty_v2();
        let incoming = model(day_with("unknown", 30, 1));
        let merged = merge(&disk, &incoming, "");
        assert_eq!(slot_reading(&merged, "unknown"), Some(30));
        assert!(merged["days"]["2026-09-14"]["devices"].get("legacy").is_none());
    }

    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("argus-activity-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            TempRoot(dir)
        }
        fn root(&self) -> &str {
            self.0.to_str().unwrap()
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stale_machine_save_does_not_clobber_the_other_machines_reading_time() {
        // The reported bug, end to end through the real read-merge-write file path.
        let tmp = TempRoot::new();
        let root = tmp.root();

        // Machine A reads a paper: writes its slot.
        save_log(root, "A", &model(day_with("A", 100, 3))).unwrap();

        // Machine B comes back with a STALE in-memory model that never saw A's
        // reading time (its payload has only B's slot). The old code overwrote the
        // whole file here and lost A. The merge must preserve A.
        save_log(root, "B", &model(day_with("B", 200, 5))).unwrap();

        let disk = read_log(root);
        assert_eq!(slot_reading(&disk, "A"), Some(100), "A's reading time survives B's save");
        assert_eq!(slot_reading(&disk, "B"), Some(200), "B's reading time recorded");

        // And A saving again (idempotent for its own slot) doesn't disturb B.
        save_log(root, "A", &model(day_with("A", 130, 4))).unwrap();
        let disk = read_log(root);
        assert_eq!(slot_reading(&disk, "A"), Some(130));
        assert_eq!(slot_reading(&disk, "B"), Some(200));
    }

    #[test]
    fn prune_drops_ancient_days_keeps_recent() {
        let recent = chrono::Local::now().date_naive().format("%Y-%m-%d").to_string();
        let value = json!({ "version": 2, "days": {
            "2000-01-01": { "date": "2000-01-01", "devices": {} },
            recent.clone(): { "date": recent, "devices": {} }
        }});
        let pruned = prune(value, RETENTION_DAYS);
        let days = pruned["days"].as_object().unwrap();
        assert!(!days.contains_key("2000-01-01"), "ancient day dropped");
        assert!(days.contains_key(&recent), "today's day kept");
    }
}
