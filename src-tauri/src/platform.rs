//! Platform integration: frontmost-app detection and permission checks.
//!
//! The frontmost app powers Typeless-style context awareness — the prompt
//! adapts tone/format to the app the user is dictating into.

/// Name of the app the user is currently in (e.g. "WeChat", "Cursor").
/// Must be called *before* we hide our own window / show the bubble, so it
/// still reflects the user's target app.
#[cfg(target_os = "macos")]
pub fn frontmost_app_name() -> Option<String> {
    use std::ffi::CStr;

    extern "C" {
        fn objc_getClass(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        fn sel_registerName(name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        #[link_name = "objc_msgSend"]
        fn objc_msg_send(
            obj: *mut std::ffi::c_void,
            sel: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
    }

    unsafe {
        let cls = objc_getClass(c"NSWorkspace".as_ptr());
        if cls.is_null() {
            return None;
        }
        let workspace = objc_msg_send(cls, sel_registerName(c"sharedWorkspace".as_ptr()));
        if workspace.is_null() {
            return None;
        }
        let app = objc_msg_send(
            workspace,
            sel_registerName(c"frontmostApplication".as_ptr()),
        );
        if app.is_null() {
            return None;
        }
        let name = objc_msg_send(app, sel_registerName(c"localizedName".as_ptr()));
        if name.is_null() {
            return None;
        }
        let utf8 = objc_msg_send(name, sel_registerName(c"UTF8String".as_ptr()))
            as *const std::ffi::c_char;
        if utf8.is_null() {
            return None;
        }
        let s = CStr::from_ptr(utf8).to_string_lossy().into_owned();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }
}

#[cfg(target_os = "windows")]
pub fn frontmost_app_name() -> Option<String> {
    extern "system" {
        fn GetForegroundWindow() -> *mut std::ffi::c_void;
        fn GetWindowTextW(hwnd: *mut std::ffi::c_void, buf: *mut u16, max: i32) -> i32;
    }

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buf[..len as usize]);
        // Window titles are usually "Document - AppName"; the tail segment is
        // the most app-identifying part, but pass the whole title — the tone
        // mapper does keyword matching anyway.
        let trimmed = title.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub fn frontmost_app_name() -> Option<String> {
    None
}

/// Whether the app has macOS Accessibility permission (needed by enigo to
/// type/paste into other apps). Non-macOS platforms report `true`.
#[cfg(target_os = "macos")]
pub fn accessibility_trusted() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> u8;
    }
    unsafe { AXIsProcessTrusted() != 0 }
}

#[cfg(not(target_os = "macos"))]
pub fn accessibility_trusted() -> bool {
    true
}

/// Deep link into the OS privacy settings for Accessibility.
#[cfg(target_os = "macos")]
pub const ACCESSIBILITY_SETTINGS_URL: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";

#[cfg(not(target_os = "macos"))]
pub const ACCESSIBILITY_SETTINGS_URL: &str = "";
