//! Configuration management for NoType.
//!
//! Loads settings from TOML config file with environment variable overrides.
//! Config path: ~/.config/notype/config.toml (macOS/Linux) or %APPDATA%/notype/config.toml (Windows)

use std::path::PathBuf;

pub mod history;
pub mod stats;

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
    /// Voice-edit hotkey: hold with text selected, speak an instruction,
    /// the selection is replaced by the edited text. Empty = disabled.
    #[serde(default = "default_edit_hotkey")]
    pub edit_hotkey: String,
    #[serde(default)]
    pub input_mode: InputMode,
    /// Preferred microphone device name. Empty string = system default.
    #[serde(default)]
    pub audio_device: String,
    /// Also copy the final text to the clipboard after typing it.
    #[serde(default)]
    pub auto_copy: bool,
    /// How spoken audio is rendered into text.
    #[serde(default)]
    pub output_style: OutputStyle,
    /// Adapt tone/format to the app the user is typing into (Typeless-style).
    #[serde(default = "default_true")]
    pub enable_app_context: bool,
    /// Allow structured formatting (paragraphs, lists, steps) in polished
    /// output. Off = flowing text with semantic paragraph breaks only.
    #[serde(default = "default_true")]
    pub structured_output: bool,
    /// Type LLM output into the cursor as it streams (lower perceived
    /// latency). Falls back to a one-shot paste when typing fails.
    #[serde(default = "default_true")]
    pub stream_typing: bool,
    /// Press Enter automatically after injecting the final text
    /// (auto-send for chat apps).
    #[serde(default)]
    pub auto_enter: bool,
    /// User overrides for per-app tone, one per line: `应用关键词 = 语气描述`.
    /// Takes precedence over the built-in mapping.
    #[serde(default)]
    pub app_rules: String,
    /// Play subtle synthesized sounds on record start / done / error.
    #[serde(default = "default_true")]
    pub sound_feedback: bool,
    /// First-run onboarding completed.
    #[serde(default)]
    pub onboarded: bool,
}

/// Output style presets. `Polish` is the flagship mode: clean up speech into
/// carefully-written text. `Verbatim` transcribes word-for-word. `TranslateEn`
/// renders the speech as natural English.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputStyle {
    #[default]
    Polish,
    Verbatim,
    #[serde(rename = "translate_en", alias = "translate")]
    TranslateEn,
}

fn default_true() -> bool {
    true
}

/// Prompt configuration — three composable LLM modules plus deterministic
/// post-replacement rules applied outside the LLM.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct PromptsConfig {
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub rules: String,
    #[serde(default)]
    pub vocabulary: String,
    /// One rule per line: `原文 = 替换` or `/regex/ = 替换`; `#` comments.
    /// Applied deterministically outside the LLM.
    #[serde(default)]
    pub replace_rules: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub provider: Provider,
    #[serde(default)]
    pub gemini_api_key: String,
    #[serde(default)]
    pub qwen_api_key: String,
    /// OpenAI-compatible endpoint for Qwen. Default is DashScope cloud;
    /// point at a local vLLM/SGLang deployment to run Qwen models locally.
    #[serde(default = "default_qwen_base_url")]
    pub qwen_base_url: String,
    #[serde(default)]
    pub mimo_api_key: String,
    #[serde(default = "default_mimo_base_url")]
    pub mimo_base_url: String,
    /// Volcengine streaming ASR (官方豆包大模型流式识别).
    #[serde(default)]
    pub volc_app_key: String,
    #[serde(default)]
    pub volc_access_key: String,
    #[serde(default = "default_volc_resource_id")]
    pub volc_resource_id: String,
    /// OpenAI Whisper-compatible batch ASR endpoint root.
    #[serde(default = "default_whisper_base_url")]
    pub whisper_base_url: String,
    #[serde(default)]
    pub whisper_api_key: String,
    #[serde(default = "default_whisper_model")]
    pub whisper_model: String,
    /// Apple Speech locale (BCP-47, e.g. "zh-CN"); empty = system default.
    #[serde(default)]
    pub apple_locale: String,
    /// OpenAI Realtime transcription (gpt-realtime series).
    #[serde(default)]
    pub openai_api_key: String,
    #[serde(default = "default_openai_realtime_model")]
    pub openai_realtime_model: String,
    /// Polish raw ASR text with an LLM (applies to ASR engines only —
    /// multimodal providers already polish in one pass).
    #[serde(default = "default_true", alias = "enable_doubao_postprocess")]
    pub enable_postprocess: bool,
    #[serde(default, alias = "doubao_postprocess_provider")]
    pub postprocess_provider: PostprocessProvider,
    /// Custom OpenAI-compatible LLM vendor for post-processing.
    #[serde(default)]
    pub custom_llm_base_url: String,
    #[serde(default)]
    pub custom_llm_api_key: String,
    #[serde(default)]
    pub custom_llm_model: String,
    #[serde(default = "default_model_name")]
    pub model_name: String,
    /// Legacy field: migrated into provider-specific key on load.
    #[serde(default, skip_serializing)]
    api_key: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum Provider {
    #[serde(rename = "gemini", alias = "Gemini")]
    #[default]
    Gemini,
    #[serde(rename = "qwen", alias = "Qwen")]
    Qwen,
    #[serde(rename = "mimo", alias = "Mimo", alias = "MiMo", alias = "xiaomi")]
    Mimo,
    /// Volcengine streaming ASR. Aliases keep old "doubao" configs loading.
    #[serde(
        rename = "volcengine",
        alias = "volc",
        alias = "doubao",
        alias = "Doubao"
    )]
    Volcengine,
    #[serde(rename = "whisper", alias = "openai")]
    Whisper,
    #[serde(rename = "apple")]
    Apple,
    /// OpenAI Realtime transcription (gpt-realtime series, WebSocket).
    #[serde(rename = "gpt_realtime", alias = "gptrealtime", alias = "realtime")]
    GptRealtime,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum InputMode {
    #[default]
    #[serde(alias = "Keyboard")]
    Keyboard,
    #[serde(alias = "Clipboard")]
    Clipboard,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PostprocessProvider {
    #[default]
    Auto,
    Custom,
    Qwen,
    Gemini,
    Mimo,
}

fn default_hotkey() -> String {
    "Ctrl+.".into()
}
fn default_edit_hotkey() -> String {
    "Ctrl+,".into()
}
fn default_model_name() -> String {
    "gemini-3-flash-preview".into()
}

fn default_mimo_base_url() -> String {
    "https://api.xiaomimimo.com/v1".into()
}

fn default_qwen_base_url() -> String {
    "https://dashscope.aliyuncs.com/compatible-mode/v1".into()
}

fn default_volc_resource_id() -> String {
    "volc.bigasr.sauc.duration".into()
}

fn default_whisper_base_url() -> String {
    "https://api.openai.com/v1".into()
}

fn default_whisper_model() -> String {
    "whisper-1".into()
}

fn default_openai_realtime_model() -> String {
    "gpt-4o-transcribe".into()
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            edit_hotkey: default_edit_hotkey(),
            input_mode: InputMode::default(),
            audio_device: String::new(),
            auto_copy: false,
            output_style: OutputStyle::default(),
            enable_app_context: true,
            structured_output: true,
            stream_typing: true,
            auto_enter: false,
            app_rules: String::new(),
            sound_feedback: true,
            onboarded: false,
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

## 改口与口误修正
- 说话人中途改口时（如「不对」「算了」「我是说」「重新说」「刚才那个不算」），只保留最终想表达的内容，删掉被否定的部分
- 同一个意思反复说了多遍时，只保留表达最完整的一遍
- 明显的口误按上下文修正为本意

## 自动分段
- 根据语义和明显停顿自动分段，每段之间插入一个空行
- 不要把整段话挤在一起，合理断句换行
- 如果口述的是列表或步骤，自动排版为列表格式

## 数字与符号
- 中文数字转阿拉伯数字：「九块五毛」→ 9.5元，「三千五百」→ 3500
- 百分比：「百分之八十五」→ 85%
- 计算式：「四除以三」→ 4÷3
- 符号口令：「下划线」→ _，「省略号」→ ……，「破折号」→ ——，「大于等于」→ ≥";

    /// Verbatim mode: transcribe exactly, punctuation only.
    pub const VERBATIM_AGENT: &str = "\
你是一个逐字语音转文字引擎。请将音频从头到尾完整逐字转录：
- 保留说话人的每一个词，包括口头语、重复和改口，不做任何删改或润色
- 只添加标点符号和合理分段
- 保持原始语言，不要翻译
- 如果音频是静音或无法识别，输出空字符串
只输出转录后的文字，不要任何解释、前缀或额外内容。";

    /// Translate mode: render the speech as natural English.
    pub const TRANSLATE_EN_AGENT: &str = "\
你是一个语音翻译引擎。请听懂音频内容，将其翻译成地道、自然的英文：
- 输出读起来像英语母语者认真写出的文字，而不是逐字直译
- 去除口水词和改口残留，只翻译最终想表达的内容
- 专有名词、品牌名、代码术语保留原写法
- 如果音频是静音或无法识别，输出空字符串
只输出英文译文，不要任何解释、前缀或额外内容。";

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

    /// Compose the system prompt for a given output style and (optionally)
    /// the frontmost app the user is typing into.
    ///
    /// - `Polish`: user-editable agent + rules + vocabulary, plus app context.
    ///   `structured = false` forbids list/heading formatting.
    /// - `Verbatim`: built-in verbatim agent + vocabulary only (rules would
    ///   delete words, which contradicts verbatim).
    /// - `TranslateEn`: built-in translate agent + vocabulary.
    pub fn compose_for(
        &self,
        style: &OutputStyle,
        app_context: Option<(&str, &str)>,
        structured: bool,
    ) -> String {
        let mut parts: Vec<String> = Vec::new();

        match style {
            OutputStyle::Polish => {
                parts.push(self.agent_text().to_string());
                parts.push(self.rules_text().to_string());
            }
            OutputStyle::Verbatim => {
                parts.push(builtin_prompts::VERBATIM_AGENT.to_string());
            }
            OutputStyle::TranslateEn => {
                parts.push(builtin_prompts::TRANSLATE_EN_AGENT.to_string());
            }
        }

        let vocab = self.vocabulary_text();
        if !vocab.is_empty() {
            parts.push(vocab.to_string());
        }

        // Typeless-style context awareness: adapt tone to the target app.
        // Only meaningful in Polish mode — verbatim/translate have fixed voices.
        if *style == OutputStyle::Polish {
            if let Some((app, tone)) = app_context {
                if !app.trim().is_empty() {
                    parts.push(format!(
                        "## 当前输入场景\n用户正在「{}」中输入文字。{}",
                        app.trim(),
                        tone
                    ));
                }
            }
            if !structured {
                parts.push(UNSTRUCTURED_DIRECTIVE.to_string());
            }
        }

        parts.retain(|s| !s.trim().is_empty());
        parts.join("\n\n")
    }
}

/// Appended when the user turns off structured output: keep flowing prose.
pub const UNSTRUCTURED_DIRECTIVE: &str = "\
## 输出格式
输出连续的自然段文本：按语义分段即可，禁止使用列表、编号步骤、标题等排版结构。";

/// Apply deterministic replace rules to the recognized text.
/// Line format: `原文 = 替换` for plain replacement, `/pattern/ = 替换` for
/// regex (capture groups usable as $1…), `#` starts a comment. Invalid lines
/// are skipped silently so a typo can't break dictation.
pub fn apply_replace_rules(rules_text: &str, input: &str) -> String {
    let mut out = input.to_string();
    for line in rules_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((left, right)) = line.split_once('=') else {
            continue;
        };
        let (pattern, replacement) = (left.trim(), right.trim());
        if pattern.is_empty() {
            continue;
        }

        if pattern.len() >= 2 && pattern.starts_with('/') && pattern.ends_with('/') {
            let expr = &pattern[1..pattern.len() - 1];
            match regex::Regex::new(expr) {
                Ok(re) => out = re.replace_all(&out, replacement).into_owned(),
                Err(e) => tracing::debug!("Skipping invalid replace regex '{expr}': {e}"),
            }
        } else {
            out = out.replace(pattern, replacement);
        }
    }
    out
}

/// Resolve the tone hint for an app: user rules (one per line,
/// `应用关键词 = 语气描述`) take precedence over the built-in mapping.
pub fn resolve_app_tone(app_name: &str, user_rules: &str) -> String {
    let app_lower = app_name.to_lowercase();
    for line in user_rules.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((keyword, tone)) = line.split_once('=') else {
            continue;
        };
        let keyword = keyword.trim().to_lowercase();
        let tone = tone.trim();
        if !keyword.is_empty() && !tone.is_empty() && app_lower.contains(&keyword) {
            return tone.to_string();
        }
    }
    app_tone_hint(app_name).to_string()
}

/// Map an app name to a tone/format hint, mirroring Typeless's
/// "different tones for each app". Keyword matching keeps it dependency-free;
/// unknown apps get a neutral hint.
pub fn app_tone_hint(app_name: &str) -> &'static str {
    let name = app_name.to_lowercase();

    const CHAT: &[&str] = &[
        "微信",
        "wechat",
        "qq",
        "telegram",
        "discord",
        "slack",
        "whatsapp",
        "imessage",
        "信息",
        "messages",
        "钉钉",
        "dingtalk",
        "飞书",
        "lark",
        "messenger",
        "signal",
    ];
    const MAIL: &[&str] = &["mail", "邮件", "outlook", "gmail", "spark", "airmail"];
    const CODE: &[&str] = &[
        "cursor",
        "code",
        "vscode",
        "xcode",
        "intellij",
        "pycharm",
        "webstorm",
        "goland",
        "rustrover",
        "zed",
        "sublime",
        "vim",
        "neovim",
        "terminal",
        "iterm",
        "warp",
        "kitty",
        "alacritty",
        "ghostty",
    ];
    const NOTES: &[&str] = &[
        "备忘录",
        "notes",
        "notion",
        "obsidian",
        "logseq",
        "bear",
        "typora",
        "onenote",
        "evernote",
        "印象笔记",
        "flomo",
        "craft",
        "ulysses",
    ];
    const DOCS: &[&str] = &[
        "word",
        "pages",
        "docs",
        "wps",
        "石墨",
        "腾讯文档",
        "语雀",
        "yuque",
    ];
    const AI: &[&str] = &[
        "chatgpt",
        "claude",
        "gemini",
        "copilot",
        "豆包",
        "kimi",
        "元宝",
        "perplexity",
        "poe",
        "chatbox",
    ];

    let hit = |list: &[&str]| list.iter().any(|k| name.contains(k));

    if hit(CHAT) {
        "这是即时聊天场景：语气自然口语化、简洁直接，不要过度书面化，一般不需要分段和列表排版。"
    } else if hit(MAIL) {
        "这是邮件场景：语气得体、结构清晰，注意礼貌用语和完整的句子，按邮件行文习惯分段。"
    } else if hit(CODE) {
        "这是代码编辑器/终端场景：技术术语、命令、变量名、文件名保留英文原文和正确大小写，不要把术语翻译成中文。"
    } else if hit(NOTES) {
        "这是笔记场景：条理优先，善用列表和分段，把要点整理得便于日后查阅。"
    } else if hit(DOCS) {
        "这是文档写作场景：书面语，句式完整，逻辑连贯，按正式文档的标准组织段落。"
    } else if hit(AI) {
        "这是与 AI 助手对话的场景：把需求表达得明确具体，保留技术术语原文，指令性语句放在前面。"
    } else {
        "请根据该应用的常见用途，让语气和排版适配这个场景。"
    }
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            provider: Provider::default(),
            gemini_api_key: String::new(),
            qwen_api_key: String::new(),
            qwen_base_url: default_qwen_base_url(),
            mimo_api_key: String::new(),
            mimo_base_url: default_mimo_base_url(),
            volc_app_key: String::new(),
            volc_access_key: String::new(),
            volc_resource_id: default_volc_resource_id(),
            whisper_base_url: default_whisper_base_url(),
            whisper_api_key: String::new(),
            whisper_model: default_whisper_model(),
            apple_locale: String::new(),
            openai_api_key: String::new(),
            openai_realtime_model: default_openai_realtime_model(),
            enable_postprocess: true,
            postprocess_provider: PostprocessProvider::Auto,
            custom_llm_base_url: String::new(),
            custom_llm_api_key: String::new(),
            custom_llm_model: String::new(),
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
            Provider::Mimo => &self.mimo_api_key,
            Provider::Whisper => &self.whisper_api_key,
            Provider::GptRealtime => &self.openai_api_key,
            // Volcengine uses its dedicated app/access keys; Apple needs none.
            Provider::Volcengine | Provider::Apple => "",
        }
    }

    /// Whether Qwen points at a non-default (self-hosted) endpoint.
    pub fn qwen_is_custom_endpoint(&self) -> bool {
        let url = self.qwen_base_url.trim().trim_end_matches('/');
        !url.is_empty() && url != default_qwen_base_url().trim_end_matches('/')
    }

    /// True when the provider/model is a dedicated ASR engine whose raw text
    /// goes through the separate LLM polish pass. Multimodal LLMs polish
    /// inside the recognition prompt instead.
    pub fn is_asr_pipeline(&self) -> bool {
        match self.provider {
            Provider::Volcengine | Provider::Whisper | Provider::Apple | Provider::GptRealtime => {
                true
            }
            // `-asr` models are pure transcribers → route through LLM polish;
            // multimodal variants (omni/pro) polish inline via the prompt.
            Provider::Qwen | Provider::Mimo => self.model_name.to_lowercase().contains("asr"),
            Provider::Gemini => false,
        }
    }

    pub fn has_required_credentials(&self) -> bool {
        match self.provider {
            Provider::Gemini => !self.gemini_api_key.is_empty(),
            // Local Qwen deployments often run keyless.
            Provider::Qwen => !self.qwen_api_key.is_empty() || self.qwen_is_custom_endpoint(),
            Provider::Mimo => {
                !self.mimo_api_key.is_empty() && !self.mimo_base_url.trim().is_empty()
            }
            Provider::Volcengine => {
                !self.volc_app_key.trim().is_empty() && !self.volc_access_key.trim().is_empty()
            }
            // Whisper-compatible servers may be keyless (whisper.cpp, local vLLM).
            Provider::Whisper => !self.whisper_base_url.trim().is_empty(),
            Provider::Apple => cfg!(target_os = "macos"),
            Provider::GptRealtime => !self.openai_api_key.trim().is_empty(),
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
        Provider::Mimo => {
            if config.model.mimo_api_key.is_empty() {
                config.model.mimo_api_key = std::mem::take(&mut config.model.api_key);
            }
        }
        Provider::Whisper => {
            if config.model.whisper_api_key.is_empty() {
                config.model.whisper_api_key = std::mem::take(&mut config.model.api_key);
            }
        }
        Provider::GptRealtime => {
            if config.model.openai_api_key.is_empty() {
                config.model.openai_api_key = std::mem::take(&mut config.model.api_key);
            }
        }
        Provider::Volcengine | Provider::Apple => {}
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
    // NOTYPE_PROVIDER: gemini | qwen | mimo | volcengine | whisper | apple
    if let Ok(provider) = std::env::var("NOTYPE_PROVIDER") {
        match provider.to_lowercase().as_str() {
            "gemini" => config.model.provider = Provider::Gemini,
            "qwen" => config.model.provider = Provider::Qwen,
            "mimo" | "xiaomi" => config.model.provider = Provider::Mimo,
            "volcengine" | "volc" | "doubao" => config.model.provider = Provider::Volcengine,
            "whisper" => config.model.provider = Provider::Whisper,
            "apple" => config.model.provider = Provider::Apple,
            "gpt_realtime" | "gptrealtime" | "realtime" | "openai" => {
                config.model.provider = Provider::GptRealtime
            }
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
                Provider::Mimo => config.model.mimo_api_key = key,
                Provider::Whisper => config.model.whisper_api_key = key,
                Provider::GptRealtime => config.model.openai_api_key = key,
                Provider::Volcengine | Provider::Apple => {}
            }
        }
    }

    // NOTYPE_MODEL overrides model name
    if let Ok(model) = std::env::var("NOTYPE_MODEL") {
        if !model.is_empty() {
            config.model.model_name = model;
        }
    }

    type EnvSetter = fn(&mut AppConfig, String);
    let overrides: &[(&str, EnvSetter)] = &[
        ("NOTYPE_QWEN_BASE_URL", |c, v| c.model.qwen_base_url = v),
        ("NOTYPE_MIMO_API_KEY", |c, v| c.model.mimo_api_key = v),
        ("NOTYPE_MIMO_BASE_URL", |c, v| c.model.mimo_base_url = v),
        ("NOTYPE_VOLC_APP_KEY", |c, v| c.model.volc_app_key = v),
        ("NOTYPE_VOLC_ACCESS_KEY", |c, v| c.model.volc_access_key = v),
        ("NOTYPE_VOLC_RESOURCE_ID", |c, v| {
            c.model.volc_resource_id = v
        }),
        ("NOTYPE_WHISPER_BASE_URL", |c, v| {
            c.model.whisper_base_url = v
        }),
        ("NOTYPE_WHISPER_API_KEY", |c, v| c.model.whisper_api_key = v),
        ("NOTYPE_WHISPER_MODEL", |c, v| c.model.whisper_model = v),
        ("NOTYPE_OPENAI_API_KEY", |c, v| c.model.openai_api_key = v),
        ("NOTYPE_OPENAI_REALTIME_MODEL", |c, v| {
            c.model.openai_realtime_model = v
        }),
        ("NOTYPE_CUSTOM_LLM_BASE_URL", |c, v| {
            c.model.custom_llm_base_url = v
        }),
        ("NOTYPE_CUSTOM_LLM_API_KEY", |c, v| {
            c.model.custom_llm_api_key = v
        }),
        ("NOTYPE_CUSTOM_LLM_MODEL", |c, v| {
            c.model.custom_llm_model = v
        }),
    ];
    for (name, apply) in overrides {
        if let Ok(value) = std::env::var(name) {
            if !value.trim().is_empty() {
                apply(config, value);
            }
        }
    }

    if let Ok(v) = std::env::var("NOTYPE_POSTPROCESS") {
        match v.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => config.model.enable_postprocess = true,
            "0" | "false" | "no" | "off" => config.model.enable_postprocess = false,
            _ => tracing::warn!(value = %v, "Unknown NOTYPE_POSTPROCESS value"),
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
        assert!(config.general.structured_output);
        assert!(config.general.stream_typing);
        assert!(config.model.gemini_api_key.is_empty());
        assert!(config.model.qwen_api_key.is_empty());
        assert_eq!(
            config.model.qwen_base_url,
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        );
        assert!(config.model.mimo_api_key.is_empty());
        assert_eq!(config.model.mimo_base_url, "https://api.xiaomimimo.com/v1");
        assert_eq!(config.model.volc_resource_id, "volc.bigasr.sauc.duration");
        assert_eq!(config.model.whisper_base_url, "https://api.openai.com/v1");
        assert_eq!(config.model.whisper_model, "whisper-1");
        assert!(config.model.enable_postprocess);
        assert_eq!(config.model.postprocess_provider, PostprocessProvider::Auto);
        assert_eq!(config.model.active_api_key(), "");
        assert_eq!(config.model.model_name, "gemini-3-flash-preview");
        assert!(!config.model.has_required_credentials());
    }

    #[test]
    fn test_doubao_config_migrates_to_volcengine() {
        // Old configs carried provider = "doubao" + legacy postprocess keys.
        let migrated: AppConfig = toml::from_str(
            r#"
            [model]
            provider = "doubao"
            enable_doubao_postprocess = false
            "#,
        )
        .unwrap();
        assert!(matches!(migrated.model.provider, Provider::Volcengine));
        assert!(!migrated.model.enable_postprocess);
        // Volcengine needs its own keys — legacy config has none.
        assert!(!migrated.model.has_required_credentials());
    }

    #[test]
    fn test_asr_pipeline_detection() {
        let mut config = AppConfig::default();
        config.model.provider = Provider::Qwen;
        config.model.model_name = "qwen3.5-omni-flash".into();
        assert!(!config.model.is_asr_pipeline());
        config.model.model_name = "qwen3-asr-flash".into();
        assert!(config.model.is_asr_pipeline());
        // MiMo asr variant → pipeline; multimodal variant → direct.
        config.model.provider = Provider::Mimo;
        config.model.model_name = "mimo-v2.5-asr".into();
        assert!(config.model.is_asr_pipeline());
        config.model.model_name = "mimo-v2.5".into();
        assert!(!config.model.is_asr_pipeline());
        config.model.provider = Provider::GptRealtime;
        assert!(config.model.is_asr_pipeline());
        config.model.provider = Provider::Volcengine;
        assert!(config.model.is_asr_pipeline());
        config.model.provider = Provider::Whisper;
        assert!(config.model.is_asr_pipeline());
        config.model.provider = Provider::Apple;
        assert!(config.model.is_asr_pipeline());
        config.model.provider = Provider::Gemini;
        assert!(!config.model.is_asr_pipeline());
    }

    #[test]
    fn test_apply_replace_rules() {
        let rules = "含数 = 函数\n/(\\d+)块(\\d+)/ = $1.$2元\n# 注释行\n无效行没有等号";
        assert_eq!(
            apply_replace_rules(rules, "这个含数返回9块5"),
            "这个函数返回9.5元"
        );
        // 无效正则被跳过，不影响其他规则
        let bad = "/[unclosed = x\n派森 = Python";
        assert_eq!(
            apply_replace_rules(bad, "用派森写"),
            "用 Python 写".replace(" ", "")
        );
    }

    #[test]
    fn test_resolve_app_tone_user_override() {
        let rules = "微信 = 用东北话回复\n# comment";
        assert_eq!(resolve_app_tone("WeChat 微信", rules), "用东北话回复");
        // 未命中回退内置
        assert!(resolve_app_tone("Mail", rules).contains("邮件"));
    }

    #[test]
    fn test_unstructured_directive_applies() {
        let prompts = PromptsConfig::default();
        let structured = prompts.compose_for(&OutputStyle::Polish, None, true);
        assert!(!structured.contains("禁止使用列表"));
        let flat = prompts.compose_for(&OutputStyle::Polish, None, false);
        assert!(flat.contains("禁止使用列表"));
    }

    #[test]
    fn test_serialize_roundtrip() {
        let config = AppConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.general.hotkey, config.general.hotkey);
    }

    #[test]
    fn test_provider_parse_lowercase_and_legacy_case() {
        let lower: AppConfig = toml::from_str(
            r#"
            [model]
            provider = "mimo"
            model_name = "mimo-v2.5-asr"
            mimo_api_key = "test-key"
            "#,
        )
        .unwrap();
        assert!(matches!(lower.model.provider, Provider::Mimo));
        assert_eq!(lower.model.active_api_key(), "test-key");
        assert!(lower.model.has_required_credentials());

        let legacy: AppConfig = toml::from_str(
            r#"
            [model]
            provider = "Qwen"
            qwen_api_key = "test-key"
            "#,
        )
        .unwrap();
        assert!(matches!(legacy.model.provider, Provider::Qwen));
    }

    #[test]
    fn test_config_path_exists() {
        let path = config_path();
        assert!(path.to_str().unwrap().contains("notype"));
    }
}
