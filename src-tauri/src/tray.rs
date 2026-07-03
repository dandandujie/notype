use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    App, Manager,
};

const TRAY_ID: &str = "notype-tray";

pub fn create_tray(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let show = MenuItem::with_id(app, "show", "打开 NoType", true, None::<&str>)?;
    let dictate = MenuItem::with_id(app, "dictate", "开始 / 停止听写", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &dictate, &quit])?;

    let icon = tauri::include_image!("icons/icon.png");

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .menu(&menu)
        .tooltip("NoType - Ready")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "quit" => {
                tracing::info!("Quit requested from tray");
                app.exit(0);
            }
            "show" => show_main_window(app),
            "dictate" => {
                let state = app.state::<crate::AppState>();
                if state.recorder.is_recording() {
                    if let Err(e) = crate::stop_capture(app) {
                        tracing::warn!("Tray stop dictation failed: {e}");
                    }
                } else {
                    // Open the window first so the user sees the live transcript.
                    show_main_window(app);
                    if let Err(e) = crate::start_capture(app, true) {
                        tracing::warn!("Tray start dictation failed: {e}");
                    }
                }
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if matches!(event, TrayIconEvent::Click { .. }) {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    tracing::info!("System tray created");
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Update the tray tooltip to reflect current status.
pub fn update_tray_status(app: &tauri::AppHandle, status: &str) {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tooltip = format!("NoType - {status}");
        let _ = TrayIcon::set_tooltip(&tray, Some(&tooltip));
    }
}
