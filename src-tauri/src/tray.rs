use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIcon, TrayIconBuilder, TrayIconEvent},
    App, Manager,
};

const TRAY_ID: &str = "notype-tray";

pub fn create_tray(app: &App) -> Result<(), Box<dyn std::error::Error>> {
    let quit = MenuItem::with_id(app, "quit", "Quit NoType", true, None::<&str>)?;
    let show = MenuItem::with_id(app, "show", "Show Settings", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

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
