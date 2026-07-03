//! Persistent transcription history.
//!
//! Stored as JSON next to config.toml. Newest entry first, capped at
//! [`MAX_ENTRIES`] so the file stays small and loads instantly.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Upper bound on stored entries; oldest entries are dropped past this.
pub const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HistoryEntry {
    /// Unix timestamp in milliseconds; also serves as the entry id.
    pub id: u64,
    pub text: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub duration_secs: f32,
}

/// Path of the history file (`<config_dir>/history.json`).
pub fn history_path() -> PathBuf {
    crate::config_dir().join("history.json")
}

/// Load all history entries, newest first. Missing/corrupt file → empty list.
pub fn load() -> Vec<HistoryEntry> {
    let path = history_path();
    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Vec<HistoryEntry>>(&content) {
            Ok(entries) => entries,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "Failed to parse history, starting fresh");
                Vec::new()
            }
        },
        Err(_) => Vec::new(),
    }
}

fn save(entries: &[HistoryEntry]) -> std::io::Result<()> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let content = serde_json::to_string(entries).map_err(std::io::Error::other)?;
    std::fs::write(&path, content)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append a new entry (front of the list) and persist. Returns the entry.
pub fn append(
    text: &str,
    provider: &str,
    model: &str,
    duration_secs: f32,
) -> std::io::Result<HistoryEntry> {
    let mut entries = load();

    // GOTCHA: id doubles as the frontend list key — keep it strictly unique
    // even if two finalizations land within the same millisecond.
    let mut id = now_ms();
    if let Some(first) = entries.first() {
        if id <= first.id {
            id = first.id + 1;
        }
    }

    let entry = HistoryEntry {
        id,
        text: text.to_string(),
        provider: provider.to_string(),
        model: model.to_string(),
        duration_secs,
    };

    entries.insert(0, entry.clone());
    entries.truncate(MAX_ENTRIES);
    save(&entries)?;
    Ok(entry)
}

/// Delete one entry by id. Returns the remaining list.
pub fn delete(id: u64) -> std::io::Result<Vec<HistoryEntry>> {
    let mut entries = load();
    entries.retain(|e| e.id != id);
    save(&entries)?;
    Ok(entries)
}

/// Remove every entry.
pub fn clear() -> std::io::Result<()> {
    save(&[])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_path_lives_next_to_config() {
        assert!(history_path().to_str().unwrap().contains("notype"));
    }

    #[test]
    fn entry_roundtrip() {
        let entry = HistoryEntry {
            id: 42,
            text: "你好世界".into(),
            provider: "qwen".into(),
            model: "qwen3.5-omni-flash".into(),
            duration_secs: 1.5,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, 42);
        assert_eq!(parsed.text, "你好世界");
    }
}
