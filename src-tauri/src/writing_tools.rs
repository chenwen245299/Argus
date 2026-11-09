//! Hand selected text to AppKit's TextKit 2 editor. No model API or disk writes.

#[derive(serde::Serialize)]
pub struct WritingToolsResult {
    action: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
}

#[tauri::command]
pub async fn system_writing_tools(
    window: tauri::WebviewWindow,
    text: String,
    can_replace: bool,
    locale: String,
) -> Result<WritingToolsResult, String> {
    if text.trim().is_empty() || text.len() > 1_000_000 || text.contains('\0') {
        return Err("Invalid or excessively large text selection".into());
    }
    #[cfg(target_os = "macos")]
    {
        macos::open(window, text, can_replace, locale).await
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, text, can_replace, locale);
        Err("Writing Tools requires macOS".into())
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::WritingToolsResult;
    use std::ffi::{c_char, c_void, CStr, CString};
    type Sender = tokio::sync::oneshot::Sender<Result<WritingToolsResult, String>>;

    extern "C" {
        fn argus_writing_tools_menu(
            window: *mut c_void,
            text: *const c_char,
            can_replace: bool,
            english: bool,
            completion: extern "C" fn(i32, *const c_char, *mut c_void),
            context: *mut c_void,
        );
    }

    // AppKit calls exactly once on cancellation, copy, replacement or failure.
    extern "C" fn completed(action: i32, text: *const c_char, context: *mut c_void) {
        let sender = unsafe { Box::from_raw(context.cast::<Sender>()) };
        let result = if action < 0 {
            Err("Could not open the system Writing Tools panel".into())
        } else {
            Ok(WritingToolsResult {
                action: match action {
                    1 => "replace",
                    2 => "copy",
                    _ => "cancel",
                },
                text: if action == 1 && !text.is_null() {
                    Some(
                        unsafe { CStr::from_ptr(text) }
                            .to_string_lossy()
                            .into_owned(),
                    )
                } else {
                    None
                },
            })
        };
        let _ = sender.send(result);
    }

    pub async fn open(
        window: tauri::WebviewWindow,
        text: String,
        can_replace: bool,
        locale: String,
    ) -> Result<WritingToolsResult, String> {
        let text = CString::new(text).map_err(|_| "Invalid text selection")?;
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let native_window = window.clone();
        window
            .run_on_main_thread(move || match native_window.ns_window() {
                Ok(parent) => unsafe {
                    argus_writing_tools_menu(
                        parent,
                        text.as_ptr(),
                        can_replace,
                        locale == "en",
                        completed,
                        Box::into_raw(Box::new(sender)).cast(),
                    );
                },
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                }
            })
            .map_err(|error| error.to_string())?;
        receiver
            .await
            .map_err(|_| "Writing Tools window closed".to_string())?
    }
}
