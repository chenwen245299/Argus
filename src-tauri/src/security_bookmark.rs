#[cfg(target_os = "macos")]
use std::{
    collections::HashSet,
    sync::{Mutex, OnceLock},
};

const BOOKMARK_KEY: &str = "last_library_bookmark";

#[cfg(target_os = "macos")]
static ACTIVE_PATHS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

#[cfg(target_os = "macos")]
pub fn ensure_library_access(app: &tauri::AppHandle, path: &str) -> String {
    if is_active(path) {
        return path.to_string();
    }

    if let Some(bookmark) = load_bookmark(app) {
        match start_bookmark_access(&bookmark, Some(path)) {
            Ok(BookmarkAccess {
                resolved_path,
                stale,
            }) => {
                if stale {
                    persist_library_bookmark(app, &resolved_path);
                }
                if resolved_path != path {
                    persist_library_path(app, &resolved_path);
                }
                return resolved_path;
            }
            Err(err) => {
                eprintln!("[security-bookmark] restore failed: {err}");
            }
        }
    }

    persist_library_bookmark(app, path);
    if let Some(bookmark) = load_bookmark(app) {
        if let Ok(access) = start_bookmark_access(&bookmark, Some(path)) {
            return access.resolved_path;
        }
    }

    path.to_string()
}

#[cfg(not(target_os = "macos"))]
pub fn ensure_library_access(_app: &tauri::AppHandle, path: &str) -> String {
    path.to_string()
}

#[cfg(target_os = "macos")]
pub fn persist_library_bookmark(app: &tauri::AppHandle, path: &str) {
    match create_bookmark(path) {
        Ok(bookmark) => {
            use tauri_plugin_store::StoreExt;
            if let Ok(store) = app.store("settings.json") {
                store.set(BOOKMARK_KEY, serde_json::json!(bookmark));
                let _ = store.save();
            }
        }
        Err(err) => {
            eprintln!("[security-bookmark] save failed: {err}");
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn persist_library_bookmark(_app: &tauri::AppHandle, _path: &str) {}

#[cfg(target_os = "macos")]
struct BookmarkAccess {
    resolved_path: String,
    stale: bool,
}

#[cfg(target_os = "macos")]
fn active_paths() -> &'static Mutex<HashSet<String>> {
    ACTIVE_PATHS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(target_os = "macos")]
fn is_active(path: &str) -> bool {
    active_paths()
        .lock()
        .map(|paths| paths.contains(path))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn mark_active(path: String) {
    if let Ok(mut paths) = active_paths().lock() {
        paths.insert(path);
    }
}

#[cfg(target_os = "macos")]
fn load_bookmark(app: &tauri::AppHandle) -> Option<String> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("settings.json").ok()?;
    store
        .get(BOOKMARK_KEY)
        .and_then(|value| value.as_str().map(|value| value.to_string()))
}

#[cfg(target_os = "macos")]
fn persist_library_path(app: &tauri::AppHandle, path: &str) {
    use tauri_plugin_store::StoreExt;
    if let Ok(store) = app.store("settings.json") {
        store.set("last_library", serde_json::json!(path));
        let _ = store.save();
    }
}

#[cfg(target_os = "macos")]
fn file_url(
    path: &str,
) -> Result<objc2_core_foundation::CFRetained<objc2_core_foundation::CFURL>, String> {
    use objc2_core_foundation::{CFString, CFURLPathStyle, CFURL};

    let path_string = CFString::from_str(path);
    CFURL::with_file_system_path(
        None,
        Some(&path_string),
        CFURLPathStyle::CFURLPOSIXPathStyle,
        true,
    )
    .ok_or_else(|| format!("Failed to create file URL for {path}"))
}

#[cfg(target_os = "macos")]
fn create_bookmark(path: &str) -> Result<String, String> {
    use base64::{engine::general_purpose, Engine as _};
    use objc2_core_foundation::{CFURLBookmarkCreationOptions, CFURL};
    use std::ptr;

    let url = file_url(path)?;
    let mut error = ptr::null_mut();
    let data = unsafe {
        CFURL::new_bookmark_data(
            None,
            Some(&url),
            CFURLBookmarkCreationOptions::WithSecurityScope,
            None,
            None,
            &mut error,
        )
    }
    .ok_or_else(|| format!("Failed to create security-scoped bookmark for {path}"))?;

    Ok(general_purpose::STANDARD.encode(data.to_vec()))
}

#[cfg(target_os = "macos")]
fn start_bookmark_access(
    bookmark: &str,
    fallback_path: Option<&str>,
) -> Result<BookmarkAccess, String> {
    use base64::{engine::general_purpose, Engine as _};
    use objc2_core_foundation::{CFData, CFURLBookmarkResolutionOptions, CFURLPathStyle, CFURL};
    use std::ptr;

    let bytes = general_purpose::STANDARD
        .decode(bookmark)
        .map_err(|err| format!("Invalid bookmark data: {err}"))?;
    let data = CFData::from_bytes(&bytes);
    let mut stale = 0u8;
    let mut error = ptr::null_mut();
    let url = unsafe {
        CFURL::new_by_resolving_bookmark_data(
            None,
            Some(&data),
            CFURLBookmarkResolutionOptions::CFURLBookmarkResolutionWithSecurityScope,
            None,
            None,
            &mut stale,
            &mut error,
        )
    }
    .ok_or_else(|| "Failed to resolve security-scoped bookmark".to_string())?;

    let resolved_path = url
        .file_system_path(CFURLPathStyle::CFURLPOSIXPathStyle)
        .map(|path| path.to_string())
        .or_else(|| fallback_path.map(|path| path.to_string()))
        .ok_or_else(|| "Resolved bookmark has no file path".to_string())?;

    if !is_active(&resolved_path) {
        let started = unsafe { url.start_accessing_security_scoped_resource() };
        if started {
            mark_active(resolved_path.clone());
        }
    }

    Ok(BookmarkAccess {
        resolved_path,
        stale: stale != 0,
    })
}
