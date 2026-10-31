//! Note lifecycle notifications and 30-day retention. No model/network work.
use tauri::{Emitter, Listener, Manager};

pub fn notify(app: &tauri::AppHandle, root: &str, slug: &str, note_id: &str, action: &str) {
    let _ = app.emit("paper-notes-changed", serde_json::json!({
        "root": root, "slug": slug, "noteId": note_id, "action": action,
    }));
}

fn prune_library(app: tauri::AppHandle) {
    let root = app.state::<crate::LibraryRoot>().0.lock().ok().and_then(|v| v.clone());
    let Some(root) = root else { return; };
    tauri::async_runtime::spawn(async move {
        let scan_root = root.clone();
        let changed = tokio::task::spawn_blocking(move || {
            let mut changed = Vec::new();
            for (slug, path) in crate::paper::list_paper_dirs(&scan_root).unwrap_or_default() {
                if !path.join("notes/.trash").exists() { continue; }
                match crate::paper::prune_deleted_notes(&scan_root, &slug, chrono::Utc::now()) {
                    Ok(n) if n > 0 => changed.push(slug),
                    Err(e) => eprintln!("[notes] retention for {slug}: {e}"),
                    _ => {}
                }
            }
            changed
        }).await.unwrap_or_default();
        for slug in changed { notify(&app, &root, &slug, "", "expired"); }
    });
}

pub fn spawn(app: &tauri::AppHandle) {
    let handle = app.clone();
    app.listen("library-changed", move |_| prune_library(handle.clone()));
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop { interval.tick().await; prune_library(handle.clone()); }
    });
}
