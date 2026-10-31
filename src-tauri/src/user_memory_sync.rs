//! File-based memory replication. Each device owns one file, so cloud sync can
//! deliver concurrent updates without replacing another device's writer cell.
//! Local learning switches, pending conversations and provider data never enter it.
use crate::user_memory::MemoryEntry;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::{Path, PathBuf}};

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Stamp {
    millis: i64,
    sequence: u64,
    device: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fact {
    entry: Option<MemoryEntry>,
    version: Stamp,
    // A manual correction/lock/delete outranks an offline automatic write that
    // never saw it. Automatic updates retain the last reviewed control stamp.
    control: Stamp,
    epoch: Stamp,
    observed: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct SyncState {
    version: u32,
    initialized: bool,
    clear: Stamp,
    manual: Stamp,
    deletion: Stamp,
    facts: BTreeMap<String, Fact>,
}
impl Default for SyncState {
    fn default() -> Self {
        Self { version: 1, initialized: false, clear: Stamp::default(), manual: Stamp::default(),
            deletion: Stamp::default(), facts: BTreeMap::new() }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum WriteKind { Automatic, Manual, Forget, Clear }

fn millis(value: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(value).map(|t| t.timestamp_millis()).unwrap_or(0)
}
fn key(entry: &MemoryEntry) -> String {
    if entry.key.is_empty() { format!("id:{}", entry.id) } else { entry.key.clone() }
}
fn observation(entry: &MemoryEntry) -> i64 {
    millis(if entry.observed_at.is_empty() { &entry.updated_at } else { &entry.observed_at })
}
fn compare(a: &Fact, b: &Fact) -> std::cmp::Ordering {
    a.control.cmp(&b.control)
        .then_with(|| if a.entry.is_some() && b.entry.is_some() { a.observed.cmp(&b.observed) } else { std::cmp::Ordering::Equal })
        .then(a.version.cmp(&b.version))
        // A duplicate cloud conflict file can contain the same stamp. Choose
        // deterministically even then; an equal-stamp deletion wins.
        .then_with(|| a.entry.is_none().cmp(&b.entry.is_none()))
        .then_with(|| serde_json::to_string(&a.entry).unwrap_or_default().cmp(&serde_json::to_string(&b.entry).unwrap_or_default()))
}

impl SyncState {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.version != 1 { return Err("Unsupported memory sync format.".into()); }
        for (key, fact) in &self.facts {
            if key.is_empty() || key.len() > 200 { return Err("Invalid memory sync key.".into()); }
            if let Some(entry) = &fact.entry {
                if *key != self::key(entry) || entry.id.is_empty() || !crate::user_memory::valid_sync_entry(entry) {
                    return Err("Invalid memory sync entry.".into());
                }
            }
        }
        Ok(())
    }
    fn tick(&self, device: &str) -> Stamp {
        let latest = self.facts.values().flat_map(|f| [&f.version, &f.control, &f.epoch])
            .chain([&self.clear, &self.manual, &self.deletion]).max().cloned().unwrap_or_default();
        let now = chrono::Utc::now().timestamp_millis();
        Stamp { millis: now.max(latest.millis), sequence: if now > latest.millis { 0 } else { latest.sequence.saturating_add(1) }, device: device.into() }
    }

    /// Upgrade the old device-local profile once. Original observation times
    /// stay original: importing an old device is not a new user correction.
    pub(crate) fn seed(&mut self, entries: &[MemoryEntry], device: &str) {
        if self.initialized { return; }
        self.initialized = true;
        for entry in entries {
            let version = Stamp { millis: millis(&entry.updated_at), sequence: 0, device: device.into() };
            let control = if entry.source == "manual" || entry.locked { version.clone() } else { Stamp::default() };
            self.manual = self.manual.clone().max(control.clone());
            let fact = Fact { entry: Some(entry.clone()), observed: observation(entry), version, control, epoch: Stamp::default() };
            let id = key(entry);
            if self.facts.get(&id).map_or(true, |old| compare(&fact, old).is_gt()) { self.facts.insert(id, fact); }
        }
    }

    pub(crate) fn merge(&mut self, other: &Self) {
        self.initialized |= other.initialized;
        self.clear = self.clear.clone().max(other.clear.clone());
        self.manual = self.manual.clone().max(other.manual.clone());
        self.deletion = self.deletion.clone().max(other.deletion.clone());
        self.facts.retain(|_, fact| fact.epoch >= self.clear);
        for (key, fact) in &other.facts {
            if fact.epoch < self.clear { continue; }
            if self.facts.get(key).map_or(true, |old| compare(fact, old).is_gt()) { self.facts.insert(key.clone(), fact.clone()); }
        }
    }

    pub(crate) fn record(&mut self, before: &[MemoryEntry], after: &[MemoryEntry], device: &str, kind: WriteKind) {
        self.seed(before, device);
        let stamp = self.tick(device);
        if matches!(kind, WriteKind::Clear) {
            self.clear = stamp.clone(); self.manual = stamp.clone(); self.deletion = stamp;
            self.facts.clear();
            return;
        }
        let manual = matches!(kind, WriteKind::Manual | WriteKind::Forget);
        for old in before {
            let id = key(old);
            if after.iter().any(|entry| key(entry) == id) { continue; }
            self.facts.insert(id, Fact { entry: None, version: stamp.clone(), control: stamp.clone(),
                epoch: self.clear.clone(), observed: if manual { stamp.millis } else { observation(old) } });
            if manual { self.manual = stamp.clone(); self.deletion = stamp.clone(); }
        }
        for entry in after {
            let id = key(entry);
            if before.iter().any(|old| key(old) == id && old == entry) { continue; }
            let prior = self.facts.get(&id);
            let control = if manual { stamp.clone() } else { prior.map(|f| f.control.clone()).unwrap_or_default() };
            self.facts.insert(id, Fact { entry: Some(entry.clone()), version: stamp.clone(), control,
                epoch: self.clear.clone(), observed: observation(entry) });
            if manual { self.manual = stamp.clone(); }
        }
    }

    pub(crate) fn entries(&self, limit: usize) -> Vec<MemoryEntry> {
        let mut facts: Vec<_> = self.facts.iter().filter(|(_, f)| f.epoch >= self.clear && f.entry.is_some()).collect();
        facts.sort_by(|(ak, a), (bk, b)| {
            let ae = a.entry.as_ref().unwrap(); let be = b.entry.as_ref().unwrap();
            be.locked.cmp(&ae.locked)
                .then((be.category == crate::user_memory::Category::Knowledge).cmp(&(ae.category == crate::user_memory::Category::Knowledge)))
                .then(b.observed.cmp(&a.observed)).then(b.version.cmp(&a.version)).then(ak.cmp(bk))
        });
        // Concurrent protected additions may exceed the ordinary cap. Retain
        // every protected fact rather than silently deleting one on another device.
        let keep = limit.max(facts.iter().filter(|(_, f)| f.entry.as_ref().unwrap().locked).count());
        facts.into_iter().take(keep).filter_map(|(_, f)| f.entry.clone()).collect()
    }

    pub(crate) fn controls_changed(&self, before: &Self) -> bool { self.manual != before.manual }
    pub(crate) fn discarded_work(&self, before: &Self) -> bool { self.clear != before.clear || self.deletion != before.deletion }
    pub(crate) fn allows_observation(&self, entry_key: &str, at: &str) -> bool {
        self.facts.get(entry_key).map_or(true, |f| f.entry.is_some() || millis(at) > f.observed)
    }
}

fn directory(root: &str, create: bool) -> Result<PathBuf, String> {
    let base = Path::new(root).join(".argus");
    let dir = base.join("user_memory");
    for path in [&base, &dir] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => return Err("Memory sync folder must be a real directory.".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(path).map_err(|e| format!("Create memory sync folder: {e}"))?;
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(format!("Read memory sync folder: {e}")),
            _ => {}
        }
    }
    Ok(dir)
}
fn writer_file(device: &str) -> String {
    // The stable device id stays outside the library. Hashing also keeps a
    // hand-edited legacy id from becoming a path segment.
    let mut hash = 0xcbf29ce484222325u64;
    for byte in device.bytes() { hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3); }
    format!("device-{hash:016x}.json")
}
fn read(path: &Path) -> Result<SyncState, String> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 16 * 1024 * 1024 { return Err("Invalid memory sync file.".into()); }
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?).map_err(|e| format!("Read memory sync record: {e}"))?;
    if value["version"] != 1 || !value["facts"].is_object() || !value["initialized"].is_boolean() {
        return Err("Invalid or unsupported memory sync record.".into());
    }
    let state: SyncState = serde_json::from_value(value).map_err(|e| format!("Read memory sync record: {e}"))?;
    state.validate()?;
    Ok(state)
}

/// Read all writer cells, including ordinary cloud conflict copies. Invalid
/// files are reported and preserved, while usable local/peer memories survive.
pub(crate) fn import(root: &str, state: &mut SyncState) -> Result<(), String> {
    let dir = directory(root, false)?;
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Read memory sync folder: {e}")),
    };
    let mut error = None;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().map_or(true, |ext| ext != "json") { continue; }
        match read(&entry.path()) { Ok(peer) => state.merge(&peer), Err(e) => error = Some(e) }
    }
    error.map_or(Ok(()), Err)
}

pub(crate) fn publish(root: &str, device: &str, state: &SyncState) -> Result<(), String> {
    state.validate()?;
    let path = directory(root, true)?.join(writer_file(device));
    match std::fs::symlink_metadata(&path) {
        Ok(_) => { if read(&path)? == *state { return Ok(()); } }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    // Only this device's cell is replaced. Do not rewrite a peer's old snapshot
    // merely because iCloud has not delivered its newest revision yet.
    crate::fsutil::atomic_write_str(&path, &serde_json::to_string_pretty(state).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::user_memory::{Category, KnowledgeLevel};
    struct Library(String);
    impl Library {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("argus-memory-sync-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(dir.join(".argus")).unwrap();
            Self(dir.to_string_lossy().to_string())
        }
        fn file(&self, device: &str) -> PathBuf { Path::new(&self.0).join(".argus/user_memory").join(writer_file(device)) }
    }
    impl Drop for Library { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    fn entry(topic: &str, date: &str) -> MemoryEntry {
        MemoryEntry { id: format!("id-{topic}"), key: topic.into(), category: Category::Knowledge,
            content: format!("Learning {topic}"), knowledge_level: KnowledgeLevel::Learning,
            evidence: format!("Learning {topic}"), source: "paper".into(), created_at: date.into(),
            updated_at: date.into(), observed_at: date.into(), locked: false }
    }
    fn seeded(entries: &[MemoryEntry], device: &str) -> SyncState { let mut state = SyncState::default(); state.seed(entries, device); state }
    fn transfer(from: &Library, to: &Library, device: &str) {
        std::fs::create_dir_all(to.file(device).parent().unwrap()).unwrap();
        std::fs::copy(from.file(device), to.file(device)).unwrap();
    }
    const OLD: &str = "2026-10-01T00:00:00Z";
    const NEW: &str = "2030-10-20T00:00:00Z";

    #[test]
    fn devices_have_separate_files_and_offline_additions_converge_without_lost_facts() {
        let a = Library::new(); let b = Library::new();
        let common = entry("algebra", OLD);
        let mut left = seeded(&[common.clone()], "mac"); let mut right = left.clone();
        let left_entries = vec![common.clone(), entry("statistics", NEW)];
        let right_entries = vec![common.clone(), entry("biology", NEW)];
        left.record(&[common.clone()], &left_entries, "mac", WriteKind::Automatic);
        right.record(&[common], &right_entries, "windows", WriteKind::Automatic);
        publish(&a.0, "mac", &left).unwrap(); publish(&b.0, "windows", &right).unwrap();
        transfer(&a, &b, "mac"); transfer(&b, &a, "windows");
        import(&a.0, &mut left).unwrap(); import(&b.0, &mut right).unwrap();
        assert_eq!(left, right); assert_eq!(left.entries(48).len(), 3);
        assert_ne!(writer_file("mac"), writer_file("windows"));
        publish(&a.0, "mac", &left).unwrap();
        assert_eq!(read(&a.file("windows")).unwrap().entries(48).len(), 2, "publishing never rewrites the other device's cell");
        let text = std::fs::read_to_string(a.file("mac")).unwrap();
        for private in ["enabled", "autoLearn", "processed", "pending", "providerId", "root", "backgroundRuns"] {
            assert!(!text.contains(&format!("\"{private}\"")), "local-only field {private} leaked into the library");
        }
    }

    #[test]
    fn same_topic_deduplicates_different_ids_and_newest_user_observation_wins() {
        let old = entry("probability", OLD);
        let mut newer = old.clone(); newer.id = "other-device-id".into(); newer.content = "Already familiar with probability".into();
        newer.knowledge_level = KnowledgeLevel::Familiar; newer.observed_at = NEW.into();
        // A slow old job can be written later than a new observation.
        let mut a = seeded(&[old.clone()], "a"); let b = seeded(&[newer.clone()], "b");
        let mut late = old.clone(); late.updated_at = "2035-01-01T00:00:00Z".into();
        a.record(&[old], &[late], "a", WriteKind::Automatic);
        let mut reverse = b.clone(); reverse.merge(&a); a.merge(&b);
        assert_eq!(a, reverse); assert_eq!(a.entries(48), vec![newer]);
    }

    #[test]
    fn manual_corrections_and_locks_beat_unaware_offline_automatic_updates() {
        let original = entry("calculus", OLD);
        let mut a = seeded(&[original.clone()], "a"); let mut b = a.clone();
        let mut corrected = original.clone(); corrected.content = "I know calculus well".into(); corrected.locked = true; corrected.source = "manual".into();
        corrected.observed_at = NEW.into(); corrected.updated_at = NEW.into();
        a.record(&[original.clone()], &[corrected.clone()], "a", WriteKind::Manual);
        let mut stale = original.clone(); stale.content = "Incorrect offline guess".into(); stale.observed_at = "2040-01-01T00:00:00Z".into();
        b.record(&[original], &[stale], "b", WriteKind::Automatic);
        let mut reverse = b.clone(); reverse.merge(&a); a.merge(&b);
        assert_eq!(a, reverse); assert_eq!(a.entries(48), vec![corrected.clone()]);
        let mut unlocked = corrected.clone(); unlocked.locked = false;
        a.record(&[corrected], &[unlocked.clone()], "a", WriteKind::Manual);
        b.merge(&a);
        let mut learned = unlocked.clone(); learned.content = "New explicit learning feedback".into(); learned.observed_at = "2041-01-01T00:00:00Z".into();
        b.record(&[unlocked], &[learned.clone()], "b", WriteKind::Automatic);
        a.merge(&b); assert_eq!(a.entries(48), vec![learned], "updates after a reviewed unlock remain possible");
    }

    #[test]
    fn tombstones_prevent_offline_resurrection_but_allow_new_post_sync_learning() {
        let original = entry("geometry", OLD);
        let mut a = seeded(&[original.clone()], "a"); let mut offline = a.clone();
        a.record(&[original.clone()], &[], "a", WriteKind::Manual);
        let mut stale = original.clone(); stale.id = "new-offline-id".into(); stale.observed_at = NEW.into();
        offline.record(&[original.clone()], &[stale], "b", WriteKind::Automatic);
        a.merge(&offline); offline.merge(&a);
        assert!(a.entries(48).is_empty()); assert_eq!(a, offline);
        assert!(!a.allows_observation("geometry", OLD));
        let mut fresh = original; fresh.observed_at = NEW.into(); fresh.content = "New explicit geometry feedback".into();
        assert!(a.allows_observation("geometry", &fresh.observed_at));
        offline.record(&[], &[fresh.clone()], "b", WriteKind::Automatic);
        a.merge(&offline); assert_eq!(a.entries(48), vec![fresh]);
    }

    #[test]
    fn clear_covers_hidden_overflow_and_every_old_device_even_if_its_file_arrives_late() {
        let entries: Vec<_> = (0..60).map(|i| entry(&format!("topic-{i}"), OLD)).collect();
        let mut a = seeded(&entries, "a"); let mut offline = a.clone();
        a.record(&a.entries(48), &[], "a", WriteKind::Clear);
        let mut offline_entries = offline.entries(48); offline_entries.push(entry("late-offline", NEW));
        offline.record(&entries[..48], &offline_entries, "b", WriteKind::Automatic);
        a.merge(&offline); assert!(a.entries(48).is_empty());
        offline.merge(&a); assert!(offline.entries(48).is_empty());
        let fresh = entry("genuinely-new", NEW);
        offline.record(&[], &[fresh.clone()], "b", WriteKind::Automatic);
        a.merge(&offline); assert_eq!(a.entries(48), vec![fresh]);
    }

    #[test]
    fn legacy_import_is_original_evidence_not_a_fresh_correction_that_undoes_clear() {
        let original = entry("legacy", OLD);
        let mut a = seeded(&[original.clone()], "a");
        a.record(&[original.clone()], &[], "a", WriteKind::Clear);
        let b = seeded(&[original], "old-device-upgrading-now");
        a.merge(&b); assert!(a.entries(48).is_empty());
    }

    #[test]
    fn automatic_evictions_retain_tombstones_without_invalidating_other_devices_pending_work() {
        let original = entry("old-concept", OLD);
        let mut state = seeded(&[original.clone()], "a"); let before = state.clone();
        state.record(&[original], &[entry("replacement", NEW)], "a", WriteKind::Automatic);
        assert!(!state.controls_changed(&before)); assert!(!state.discarded_work(&before));
        assert!(!state.allows_observation("old-concept", OLD));
        state.merge(&before); assert_eq!(state.entries(48).len(), 1);
    }

    #[test]
    fn protected_facts_are_not_silently_evicted_by_concurrent_device_additions() {
        let mut entries: Vec<_> = (0..55).map(|i| entry(&format!("locked-{i}"), OLD)).collect();
        for entry in &mut entries { entry.locked = true; }
        let state = seeded(&entries, "a"); assert_eq!(state.entries(48).len(), 55);
        assert_eq!(state.facts.len(), 55);
    }

    #[test]
    fn corrupt_and_future_files_are_preserved_and_healthy_peers_still_merge() {
        let library = Library::new();
        let state = seeded(&[entry("healthy", OLD)], "a"); publish(&library.0, "a", &state).unwrap();
        let broken = library.file("broken"); std::fs::write(&broken, "broken JSON").unwrap();
        let empty = library.file("empty"); std::fs::write(&empty, "{}").unwrap();
        let mut incoming = SyncState::default(); assert!(import(&library.0, &mut incoming).is_err());
        assert_eq!(incoming.entries(48).len(), 1);
        assert!(publish(&library.0, "broken", &state).is_err()); assert!(publish(&library.0, "empty", &state).is_err());
        assert_eq!(std::fs::read_to_string(broken).unwrap(), "broken JSON"); assert_eq!(std::fs::read_to_string(empty).unwrap(), "{}");
        let future = library.file("future"); std::fs::write(&future, r#"{"version":99,"initialized":true,"facts":{}}"#).unwrap();
        assert!(publish(&library.0, "future", &state).is_err());
    }

    #[test]
    fn stale_cloud_replacement_of_own_cell_is_repaired_from_the_local_replica() {
        let library = Library::new(); let old = entry("topic", OLD);
        let prior = seeded(&[old.clone()], "a"); let mut state = prior.clone();
        let mut corrected = old.clone(); corrected.content = "Manually corrected".into(); corrected.source = "manual".into();
        state.record(&[old], &[corrected.clone()], "a", WriteKind::Manual);
        publish(&library.0, "a", &prior).unwrap(); import(&library.0, &mut state).unwrap();
        assert_eq!(state.entries(48), vec![corrected]); publish(&library.0, "a", &state).unwrap();
        assert_eq!(read(&library.file("a")).unwrap(), state);
        let before = std::fs::metadata(library.file("a")).unwrap().modified().unwrap();
        publish(&library.0, "a", &state).unwrap();
        assert_eq!(std::fs::metadata(library.file("a")).unwrap().modified().unwrap(), before, "unchanged polling must not trigger cloud rewrites");
    }

    #[test]
    fn conflict_copies_merge_and_repeated_merge_is_idempotent() {
        let library = Library::new(); let a = seeded(&[entry("first", OLD)], "a"); let b = seeded(&[entry("second", NEW)], "b");
        publish(&library.0, "a", &a).unwrap();
        let conflict = library.file("a").with_file_name("device-a (conflicted copy).json");
        std::fs::write(conflict, serde_json::to_vec(&b).unwrap()).unwrap();
        let mut state = SyncState::default(); import(&library.0, &mut state).unwrap(); let before = state.clone();
        import(&library.0, &mut state).unwrap(); assert_eq!(state, before); assert_eq!(state.entries(48).len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_redirect_memory_sync_outside_the_library() {
        let library = Library::new(); let outside = Library::new();
        std::os::unix::fs::symlink(&outside.0, Path::new(&library.0).join(".argus/user_memory")).unwrap();
        let state = seeded(&[entry("safe", OLD)], "a");
        assert!(publish(&library.0, "a", &state).is_err());
        assert!(!Path::new(&outside.0).join(writer_file("a")).exists());
    }
}
