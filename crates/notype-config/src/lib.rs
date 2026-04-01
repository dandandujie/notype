//! Configuration management for NoType.
//!
//! Loads settings from TOML config file with environment variable overrides.
//! Config path: ~/.config/notype/config.toml (macOS/Linux) or %APPDATA%/notype/config.toml (Windows)

use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub model: ModelConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub input_mode: InputMode,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_model_name")]
    pub model_name: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum Provider {
    #[default]
    Gemini,
    Qwen,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum InputMode {
    #[default]
    Keyboard,
    Clipboard,
}

fn default_hotkey() -> String {
    "Ctrl+.".into()
}
fn default_model_name() -> String {
    "gemini-3-flash".into()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            input_mode: InputMode::default(),
        }
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: Provider::default(),
            api_key: String::new(),
            model_name: default_model_name(),
        }
    }
}

/// Get the config directory path.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("notype")
}

/// Get the config file path.
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Load config from file, with environment variable overrides.
pub fn load() -> AppConfig {
    let path = config_path();
    let mut config = load_from_file(&path);
    apply_env_overrides(&mut config);
    config
}

/// Save config to file.
pub fn save(config: &AppConfig) -> std::io::Result<()> {
    let path = config_path();
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir)?;

    let content = toml::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(&path, content)?;

    tracing::info!(path = %path.display(), "Config saved");
    Ok(())
}

fn load_from_file(path: &PathBuf) -> AppConfig {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str(&content) {
            Ok(config) => {
                tracing::info!(path = %path.display(), "Config loaded");
                config
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to parse config, using defaults"
                );
                AppConfig::default()
            }
        },
        Err(_) => {
            tracing::info!(
                path = %path.display(),
                "Config file not found, using defaults"
            );
            AppConfig::default()
        }
    }
}

fn apply_env_overrides(config: &mut AppConfig) {
    // NOTYPE_API_KEY overrides config file
    if let Ok(key) = std::env::var("NOTYPE_API_KEY") {
        if !key.is_empty() {
            tracing::info!("Using API key from NOTYPE_API_KEY env var");
            config.model.api_key = key;
        }
    }

    // NOTYPE_PROVIDER: "gemini" or "qwen"
    if let Ok(provider) = std::env::var("NOTYPE_PROVIDER") {
        match provider.to_lowercase().as_str() {
            "gemini" => config.model.provider = Provider::Gemini,
            "qwen" => config.model.provider = Provider::Qwen,
            _ => tracing::warn!(
                provider = %provider,
                "Unknown provider in NOTYPE_PROVIDER, ignoring"
            ),
        }
    }

    // NOTYPE_MODEL overrides model name
    if let Ok(model) = std::env::var("NOTYPE_MODEL") {
        if !model.is_empty() {
            config.model.model_name = model;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AppConfig::default();
        assert_eq!(config.general.hotkey, "Ctrl+.");
        assert!(config.model.api_key.is_empty());
        assert_eq!(config.model.model_name, "gemini-3-flash");
    }

    #[test]
    fn test_serialize_roundtrip() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.general.hotkey, config.general.hotkey);
    }

    #[test]
    fn test_config_path_exists() {
        let path = config_path();
        assert!(path.to_str().unwrap().contains("notype"));
    }
}
