use std::sync::atomic::{AtomicBool, Ordering};

static UPDATE_CHECK_REQUESTED: AtomicBool = AtomicBool::new(false);

// Retain menu clicks until the main webview has subscribed, including when
// the menu needs to recreate a main window closed while another window stays open.
#[tauri::command]
pub fn take_update_check_request(window: tauri::Window) -> bool {
    window.label() == "main" && UPDATE_CHECK_REQUESTED.swap(false, Ordering::AcqRel)
}

#[cfg(target_os = "macos")]
pub fn install(app: &tauri::App) -> tauri::Result<()> {
    use tauri::{
        menu::{Menu, MenuItem},
        Emitter, Manager, WebviewWindowBuilder,
    };

    const CHECK_UPDATES: &str = "check-for-updates";
    let menu = Menu::default(app.handle())?;
    // Extend Tauri's standard application menu so native editing, window,
    // Services, Hide and Quit behavior stays with the platform.
    if let Some(app_menu) = menu.items()?.first().and_then(|item| item.as_submenu()) {
        app_menu.insert(
            &MenuItem::with_id(app, CHECK_UPDATES, "Check for Updates…", true, None::<&str>)?,
            1,
        )?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        if event.id().as_ref() != CHECK_UPDATES {
            return;
        }
        UPDATE_CHECK_REQUESTED.store(true, Ordering::Release);
        let main = match app.get_webview_window("main") {
            Some(window) => Ok(window),
            None => WebviewWindowBuilder::from_config(app, &app.config().app.windows[0])
                .and_then(|builder| builder.build()),
        };
        match main {
            Ok(main) => {
                let _ = app.show();
                let _ = main.show();
                let _ = main.unminimize();
                let _ = main.set_focus();
                let _ = app.emit_to("main", "argus-check-for-updates", ());
            }
            Err(error) => eprintln!("Open update check: {error}"),
        }
    });
    Ok(())
}
