//! Text injection module for NoType.
//!
//! Simulates keyboard input via enigo to type recognized text
//! into the currently active application window.

use std::sync::Mutex;

use enigo::{Enigo, Keyboard, Settings};

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("failed to simulate input: {0}")]
    SimulationFailed(String),
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
