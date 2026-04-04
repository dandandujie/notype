//! Floating bubble window that follows the mouse cursor.

use tauri::{Manager, Webview, WebviewUrl, WebviewWindowBuilder};

const BUBBLE_LABEL: &str = "bubble";
const BUBBLE_WIDTH: f64 = 380.0;
const BUBBLE_RECORDING_H: f64 = 140.0;
const BUBBLE_RESULT_MAX_H: f64 = 280.0;

/// Show the bubble near the current mouse cursor.
/// MUST NOT steal focus from the user's active application.
pub fn show_bubble(app: &tauri::AppHandle) {
    let (mx, my) = get_mouse_position();
    let x = (mx - BUBBLE_WIDTH / 2.0).max(0.0);
    let y = my + 24.0;

    if let Some(win) = app.get_webview_window(BUBBLE_LABEL) {
        let _ = win.set_position(tauri::LogicalPosition::new(x, y));
        let _ = win.set_size(tauri::LogicalSize::new(BUBBLE_WIDTH, BUBBLE_RECORDING_H));
        let _ = win.show();
        let _ = win.set_ignore_cursor_events(true);
    } else {
        let url = WebviewUrl::App("src/bubble.html".into());
        if let Ok(win) = WebviewWindowBuilder::new(app, BUBBLE_LABEL, url)
            .title("")
            .inner_size(BUBBLE_WIDTH, BUBBLE_RECORDING_H)
            .position(x, y)
            .decorations(false)
            .transparent(true)
            .always_on_top(true)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(true)
            .build()
        {
            // Prevent bubble from intercepting clicks or stealing focus
            let _ = win.set_ignore_cursor_events(true);
        }
    }

    // Deactivate our app so the user's target app stays focused
    deactivate_app();
}

/// On macOS, deactivate the app so the previously focused app stays in front.
#[cfg(target_os = "macos")]
fn deactivate_app() {
    extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
    }

    // Use a single signature for objc_msgSend (returns id, ignoring it for void calls is fine)
    extern "C" {
        #[link_name = "objc_msgSend"]
        fn objc_msg_send(
            obj: *mut std::ffi::c_void,
            sel: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
    }

    unsafe {
        let cls = objc_getClass(c"NSApplication".as_ptr());
        let sel_shared = sel_registerName(c"sharedApplication".as_ptr());
        let ns_app = objc_msg_send(cls, sel_shared);
        if !ns_app.is_null() {
            let sel_deactivate = sel_registerName(c"deactivate".as_ptr());
            let _ = objc_msg_send(ns_app, sel_deactivate);
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn deactivate_app() {}

/// Hide the bubble.
pub fn hide_bubble(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window(BUBBLE_LABEL) {
        let _ = win.hide();
    }
}

/// Update bubble state via JS eval — avoids event timing issues.
pub fn set_recording(app: &tauri::AppHandle) {
    eval_bubble(app, "showRecording()");
    set_bubble_size(app, BUBBLE_RECORDING_H);
}

pub fn set_recognizing(app: &tauri::AppHandle) {
    eval_bubble(app, "showRecognizing()");
}

/// Show interim (partial) transcription while still recording.
pub fn set_interim(app: &tauri::AppHandle, text: &str) {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    eval_bubble(app, &format!("showInterim('{escaped}')"));
    resize_for_text(app, text);
}

/// Resize bubble to fit current text length.
pub fn resize_for_text(app: &tauri::AppHandle, full_text: &str) {
    let char_count = full_text.len();
    let wrap_lines = (char_count as f64 / 30.0).ceil().max(1.0);
    let line_breaks = full_text.lines().count().max(1) as f64;
    let lines = wrap_lines.max(line_breaks);
    let estimated = 70.0 + (lines * 25.0);
    set_bubble_size(app, estimated.min(BUBBLE_RESULT_MAX_H));
}

pub fn set_result(app: &tauri::AppHandle, text: &str) {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    eval_bubble(app, &format!("showResult('{escaped}')"));
    resize_for_text(app, text);
}

pub fn set_error(app: &tauri::AppHandle, text: &str) {
    let escaped = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    eval_bubble(app, &format!("showError('{escaped}')"));
    set_bubble_size(app, 100.0);
}

fn eval_bubble(app: &tauri::AppHandle, script: &str) {
    if let Some(win) = app.get_webview_window(BUBBLE_LABEL) {
        let webview: &Webview = win.as_ref();
        let _ = webview.eval(script);
    }
}

fn set_bubble_size(app: &tauri::AppHandle, height: f64) {
    if let Some(win) = app.get_webview_window(BUBBLE_LABEL) {
        let _ = win.set_size(tauri::LogicalSize::new(BUBBLE_WIDTH, height));
    }
}

// -- Mouse position (cross-platform) --

fn get_mouse_position() -> (f64, f64) {
    #[cfg(target_os = "macos")]
    {
        macos_mouse_position()
    }

    #[cfg(target_os = "windows")]
    {
        windows_mouse_position()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        (500.0, 300.0)
    }
}

#[cfg(target_os = "macos")]
fn macos_mouse_position() -> (f64, f64) {
    #[repr(C)]
    #[derive(Copy, Clone)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    extern "C" {
        fn CGEventCreate(source: *const std::ffi::c_void) -> *mut std::ffi::c_void;
        fn CGEventGetLocation(event: *const std::ffi::c_void) -> CGPoint;
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return (500.0, 300.0);
        }
        let p = CGEventGetLocation(event);
        CFRelease(event);
        (p.x, p.y)
    }
}

#[cfg(target_os = "windows")]
fn windows_mouse_position() -> (f64, f64) {
    #[repr(C)]
    struct POINT {
        x: i32,
        y: i32,
    }

    extern "system" {
        fn GetCursorPos(point: *mut POINT) -> i32;
    }

    let mut p = POINT { x: 0, y: 0 };
    unsafe {
        GetCursorPos(&mut p);
    }
    (p.x as f64, p.y as f64)
}
