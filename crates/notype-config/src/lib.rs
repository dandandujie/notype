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
    #[serde(default)]
    pub prompts: PromptsConfig,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub input_mode: InputMode,
}

/// Prompt configuration — three composable modules.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct PromptsConfig {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub rules: String,
    #[serde(default)]
    pub vocabulary: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub gemini_api_key: String,
    #[serde(default)]
    pub qwen_api_key: String,
    #[serde(default)]
    pub doubao_api_key: String,
    #[serde(default = "default_doubao_base_url")]
    pub doubao_base_url: String,
    #[serde(default)]
    pub doubao_official_app_key: String,
    #[serde(default)]
    pub doubao_official_access_key: String,
    #[serde(default = "default_enable_doubao_postprocess")]
    pub enable_doubao_postprocess: bool,
    #[serde(default)]
    pub doubao_postprocess_provider: DoubaoPostprocessProvider,
    #[serde(default = "default_enable_doubao_realtime_ws")]
    pub enable_doubao_realtime_ws: bool,
    #[serde(default = "default_doubao_ime_credential_path")]
    pub doubao_ime_credential_path: String,
    #[serde(default = "default_model_name")]
    pub model_name: String,
    /// Legacy field: migrated into provider-specific key on load.
    #[serde(default, skip_serializing)]
    api_key: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum Provider {
    #[default]
    Gemini,
    Qwen,
    Doubao,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub enum InputMode {
    #[default]
    Keyboard,
    Clipboard,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DoubaoPostprocessProvider {
    #[default]
    Auto,
    Qwen,
    Gemini,
}

fn default_hotkey() -> String {
    "Ctrl+.".into()
}
fn default_model_name() -> String {
    "gemini-3-flash-preview".into()
}

fn default_doubao_base_url() -> String {
    "http://127.0.0.1:8000".into()
}

fn default_enable_doubao_postprocess() -> bool {
    true
}

fn default_enable_doubao_realtime_ws() -> bool {
    true
}

fn default_doubao_ime_credential_path() -> String {
    "~/.config/doubaoime-asr/credentials.json".into()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            input_mode: InputMode::default(),
        }
    }
}

/// Built-in default prompt content for each module.
pub mod builtin_prompts {
    pub const AGENT: &str = "\
你是一个语音转文字引擎。请将音频从头到尾完整转录为文字。
必须转录音频中的每一句话，不得省略、概括或跳过任何部分。
只输出转录后的文字，不要任何解释、前缀或额外内容。";

    pub const RULES: &str = "\
## 转录规则
- 添加正确的标点符号和大小写
- 保持原始语言（不要翻译）
- 去除口水词：嗯、啊、那个、就是说、然后呢、对吧、em、uh、um、er
- 如果音频是静音或无法识别，输出空字符串

## 自动分段
- 根据语义和明显停顿自动分段，每段之间插入一个空行
- 不要把整段话挤在一起，合理断句换行
- 如果口述的是列表或步骤，自动排版为列表格式

## 数字与符号
- 中文数字转阿拉伯数字：「九块五毛」→ 9.5元，「三千五百」→ 3500
- 百分比：「百分之八十五」→ 85%
- 计算式：「四除以三」→ 4÷3
- 符号口令：「下划线」→ _，「省略号」→ ……，「破折号」→ ——，「大于等于」→ ≥";

    pub const VOCABULARY: &str = "\
## 专有词汇校正
以下词汇在语音识别中容易出错，请根据上下文自动修正为正确写法：
- deep seek → DeepSeek
- mac book pro → MacBook Pro
- chat g p t → ChatGPT
- type script → TypeScript
- java script → JavaScript
- pie thon / 派森 → Python
- git hub → GitHub
- v s code → VS Code
- open a i → OpenAI";
}

impl PromptsConfig {
    /// Get effective agent prompt (use config value or built-in default).
    pub fn agent_text(&self) -> &str {
        if self.agent.is_empty() {
            builtin_prompts::AGENT
        } else {
            &self.agent
        }
    }

    pub fn rules_text(&self) -> &str {
        if self.rules.is_empty() {
            builtin_prompts::RULES
        } else {
            &self.rules
        }
    }

    pub fn vocabulary_text(&self) -> &str {
        if self.vocabulary.is_empty() {
            builtin_prompts::VOCABULARY
        } else {
            &self.vocabulary
        }
    }

    /// Compose the full system prompt from all modules.
    pub fn compose(&self) -> String {
        let parts: Vec<&str> = [self.agent_text(), self.rules_text(), self.vocabulary_text()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect();

        parts.join("\n\n")
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: Provider::default(),
            gemini_api_key: String::new(),
            qwen_api_key: String::new(),
            doubao_api_key: String::new(),
            doubao_base_url: default_doubao_base_url(),
            doubao_official_app_key: String::new(),
            doubao_official_access_key: String::new(),
            enable_doubao_postprocess: default_enable_doubao_postprocess(),
            doubao_postprocess_provider: DoubaoPostprocessProvider::Auto,
            enable_doubao_realtime_ws: default_enable_doubao_realtime_ws(),
            doubao_ime_credential_path: default_doubao_ime_credential_path(),
            model_name: default_model_name(),
            api_key: String::new(),
        }
    }
}

impl ModelConfig {
    /// Get the API key for the currently selected provider.
    pub fn active_api_key(&self) -> &str {
        match self.provider {
            Provider::Gemini => &self.gemini_api_key,
            Provider::Qwen => &self.qwen_api_key,
            Provider::Doubao => &self.doubao_api_key,
        }
    }

    pub fn is_doubao_official_model(&self) -> bool {
        let name = self.model_name.trim().to_lowercase();
        matches!(
            name.as_str(),
            "doubao-asr-official"
                | "doubao-asr-official-standard"
                | "doubao-asr-official-flash"
                | "official"
                | "official-standard"
                | "official-flash"
                | "standard"
                | "flash"
        )
    }

    pub fn has_required_credentials(&self) -> bool {
        match self.provider {
            Provider::Gemini => !self.gemini_api_key.is_empty(),
            Provider::Qwen => !self.qwen_api_key.is_empty(),
            Provider::Doubao => {
                if self.is_doubao_official_model() {
                    !self.doubao_official_app_key.is_empty()
                        && !self.doubao_official_access_key.is_empty()
                } else {
                    !self.doubao_base_url.trim().is_empty()
                }
            }
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
            Ok(mut config) => {
                tracing::info!(path = %path.display(), "Config loaded");
                migrate_legacy_key(&mut config);
                migrate_model_names(&mut config);
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

/// Migrate legacy single `api_key` → per-provider keys.
fn migrate_legacy_key(config: &mut AppConfig) {
    if config.model.api_key.is_empty() {
        return;
    }
    tracing::info!("Migrating legacy api_key to per-provider key");
    match config.model.provider {
        Provider::Gemini => {
            if config.model.gemini_api_key.is_empty() {
                config.model.gemini_api_key = std::mem::take(&mut config.model.api_key);
            }
        }
        Provider::Qwen => {
            if config.model.qwen_api_key.is_empty() {
                config.model.qwen_api_key = std::mem::take(&mut config.model.api_key);
            }
        }
        Provider::Doubao => {
            if config.model.doubao_api_key.is_empty() {
                config.model.doubao_api_key = std::mem::take(&mut config.model.api_key);
            }
        }
    }
    config.model.api_key.clear();
}

/// Fix old model names that are no longer valid in the API.
fn migrate_model_names(config: &mut AppConfig) {
    let name = &config.model.model_name;
    let fixed = match name.as_str() {
        "gemini-3-flash" => Some("gemini-3-flash-preview"),
        "gemini-3.1-flash-lite" => Some("gemini-3.1-flash-lite-preview"),
        _ => None,
    };
    if let Some(new_name) = fixed {
        tracing::info!(old = %name, new = new_name, "Migrating model name");
        config.model.model_name = new_name.into();
    }
}

fn apply_env_overrides(config: &mut AppConfig) {
    // NOTYPE_PROVIDER: "gemini" or "qwen" or "doubao"
    if let Ok(provider) = std::env::var("NOTYPE_PROVIDER") {
        match provider.to_lowercase().as_str() {
            "gemini" => config.model.provider = Provider::Gemini,
            "qwen" => config.model.provider = Provider::Qwen,
            "doubao" => config.model.provider = Provider::Doubao,
            _ => tracing::warn!(
                provider = %provider,
                "Unknown provider in NOTYPE_PROVIDER, ignoring"
            ),
        }
    }

    // NOTYPE_API_KEY overrides the current provider's key
    if let Ok(key) = std::env::var("NOTYPE_API_KEY") {
        if !key.is_empty() {
            tracing::info!("Using API key from NOTYPE_API_KEY env var");
            match config.model.provider {
                Provider::Gemini => config.model.gemini_api_key = key,
                Provider::Qwen => config.model.qwen_api_key = key,
                Provider::Doubao => config.model.doubao_api_key = key,
            }
        }
    }

    // NOTYPE_MODEL overrides model name
    if let Ok(model) = std::env::var("NOTYPE_MODEL") {
        if !model.is_empty() {
            config.model.model_name = model;
        }
    }

    if let Ok(url) = std::env::var("NOTYPE_DOUBAO_BASE_URL") {
        if !url.is_empty() {
            config.model.doubao_base_url = url;
        }
    }

    if let Ok(key) = std::env::var("NOTYPE_DOUBAO_API_KEY") {
        if !key.is_empty() {
            config.model.doubao_api_key = key;
        }
    }

    if let Ok(key) = std::env::var("NOTYPE_DOUBAO_OFFICIAL_APP_KEY") {
        if !key.is_empty() {
            config.model.doubao_official_app_key = key;
        }
    }

    if let Ok(key) = std::env::var("NOTYPE_DOUBAO_OFFICIAL_ACCESS_KEY") {
        if !key.is_empty() {
            config.model.doubao_official_access_key = key;
        }
    }

    if let Ok(v) = std::env::var("NOTYPE_DOUBAO_POSTPROCESS") {
        let normalized = v.trim().to_lowercase();
        match normalized.as_str() {
            "1" | "true" | "yes" | "on" => config.model.enable_doubao_postprocess = true,
            "0" | "false" | "no" | "off" => config.model.enable_doubao_postprocess = false,
            _ => tracing::warn!(
                value = %v,
                "Unknown NOTYPE_DOUBAO_POSTPROCESS value, expected true/false"
            ),
        }
    }

    if let Ok(v) = std::env::var("NOTYPE_DOUBAO_POSTPROCESS_PROVIDER") {
        match v.trim().to_lowercase().as_str() {
            "auto" => config.model.doubao_postprocess_provider = DoubaoPostprocessProvider::Auto,
            "qwen" => config.model.doubao_postprocess_provider = DoubaoPostprocessProvider::Qwen,
            "gemini" => {
                config.model.doubao_postprocess_provider = DoubaoPostprocessProvider::Gemini
            }
            _ => tracing::warn!(
                value = %v,
                "Unknown NOTYPE_DOUBAO_POSTPROCESS_PROVIDER value, expected auto/qwen/gemini"
            ),
        }
    }

    if let Ok(v) = std::env::var("NOTYPE_DOUBAO_REALTIME_WS") {
        let normalized = v.trim().to_lowercase();
        match normalized.as_str() {
            "1" | "true" | "yes" | "on" => config.model.enable_doubao_realtime_ws = true,
            "0" | "false" | "no" | "off" => config.model.enable_doubao_realtime_ws = false,
            _ => tracing::warn!(
                value = %v,
                "Unknown NOTYPE_DOUBAO_REALTIME_WS value, expected true/false"
            ),
        }
    }

    if let Ok(v) = std::env::var("NOTYPE_DOUBAO_IME_CREDENTIAL_PATH") {
        if !v.trim().is_empty() {
            config.model.doubao_ime_credential_path = v;
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
        assert!(config.model.gemini_api_key.is_empty());
        assert!(config.model.qwen_api_key.is_empty());
        assert!(config.model.doubao_api_key.is_empty());
        assert_eq!(config.model.doubao_base_url, "http://127.0.0.1:8000");
        assert!(config.model.enable_doubao_postprocess);
        assert_eq!(
            config.model.doubao_postprocess_provider,
            DoubaoPostprocessProvider::Auto
        );
        assert!(config.model.enable_doubao_realtime_ws);
        assert_eq!(
            config.model.doubao_ime_credential_path,
            "~/.config/doubaoime-asr/credentials.json"
        );
        assert_eq!(config.model.active_api_key(), "");
        assert_eq!(config.model.model_name, "gemini-3-flash-preview");
        assert!(!config.model.has_required_credentials());
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
