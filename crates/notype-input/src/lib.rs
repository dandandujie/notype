//! Text injection module for NoType.
//!
//! Simulates keyboard input via enigo to type recognized text
//! into the currently active application window.

use std::sync::Mutex;

use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("failed to simulate input: {0}")]
    SimulationFailed(String),
    #[error("clipboard error: {0}")]
    ClipboardFailed(String),
    #[error("accessibility permission denied")]
    PermissionDenied,
}

pub type Result<T> = std::result::Result<T, InputError>;

/// Thread-safe text input simulator.
/// Enigo is not Send, so we wrap it in a thread-local-like pattern.
pub struct TextInputter {
    enigo: Mutex<Option<Enigo>>,
}

impl TextInputter {
    pub fn new() -> Self {
        let enigo = Enigo::new(&Settings::default())
            .map_err(|e| tracing::warn!("Failed to init enigo: {e}"))
            .ok();
        Self {
            enigo: Mutex::new(enigo),
        }
    }

    /// Type text into the currently focused application.
    pub fn type_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        let mut guard = self.enigo.lock().unwrap();
        let enigo = guard.as_mut().ok_or(InputError::PermissionDenied)?;

        tracing::debug!(chars = text.len(), "Typing text via enigo");

        enigo
            .text(text)
            .map_err(|e| InputError::SimulationFailed(e.to_string()))?;

        tracing::info!(chars = text.len(), "Text typed successfully");
        Ok(())
    }

    /// Put text on the system clipboard without touching the focused app.
    pub fn copy_text(&self, text: &str) -> Result<()> {
        let mut clipboard =
            arboard::Clipboard::new().map_err(|e| InputError::ClipboardFailed(e.to_string()))?;
        clipboard
            .set_text(text)
            .map_err(|e| InputError::ClipboardFailed(e.to_string()))?;
        tracing::info!(chars = text.len(), "Text copied to clipboard");
        Ok(())
    }

    /// Insert text via clipboard + simulated paste shortcut.
    /// Faster and more reliable than per-character typing for long CJK text,
    /// at the cost of overwriting the user's clipboard.
    pub fn paste_text(&self, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }

        self.copy_text(text)?;
        // GOTCHA: some apps read the clipboard asynchronously; give the
        // clipboard owner a beat to settle before firing the paste chord.
        std::thread::sleep(std::time::Duration::from_millis(60));

        let mut guard = self.enigo.lock().unwrap();
        let enigo = guard.as_mut().ok_or(InputError::PermissionDenied)?;

        let modifier = if cfg!(target_os = "macos") {
            Key::Meta
        } else {
            Key::Control
        };

        enigo
            .key(modifier, Direction::Press)
            .map_err(|e| InputError::SimulationFailed(e.to_string()))?;
        let click = enigo
            .key(Key::Unicode('v'), Direction::Click)
            .map_err(|e| InputError::SimulationFailed(e.to_string()));
        // Always release the modifier, even if the 'v' click failed.
        let release = enigo
            .key(modifier, Direction::Release)
            .map_err(|e| InputError::SimulationFailed(e.to_string()));
        click?;
        release?;

        tracing::info!(chars = text.len(), "Text pasted via clipboard");
        Ok(())
    }
}

impl Default for TextInputter {
    fn default() -> Self {
        Self::new()
    }
}

// Enigo internally uses platform-specific handles that may not be Send.
// We ensure thread safety via Mutex.
unsafe impl Send for TextInputter {}
unsafe impl Sync for TextInputter {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_type_empty_string() {
        let inputter = TextInputter::new();
        assert!(inputter.type_text("").is_ok());
    }

    #[test]
    fn test_inputter_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<TextInputter>();
    }
}
