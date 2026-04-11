mod bubble;
mod tray;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use std::{path::PathBuf, process::Stdio};

use base64::Engine;
use notype_audio::Recorder;
use notype_input::TextInputter;
use notype_llm::VoiceRecognizer;
use tauri::{Emitter, Manager};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

struct AppState {
    recorder: Arc<Recorder>,
    inputter: Arc<TextInputter>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    runtime: tokio::runtime::Handle,
    bubble_generation: Arc<AtomicU64>,
    hotkey_down: Arc<AtomicBool>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    doubao_gateway_process: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
}

const DEFAULT_POSTPROCESS_QWEN_MODEL: &str = "qwen3.5-omni-flash";
const DEFAULT_POSTPROCESS_GEMINI_MODEL: &str = "gemini-3-flash-preview";
const DEFAULT_LOCAL_DOUBAO_BASE_URL: &str = "http://127.0.0.1:8000";
const DOUBAO_QUOTA_RETRY_ATTEMPTS: usize = 5;
const DOUBAO_QUOTA_RETRY_BASE_DELAY_MS: u64 = 700;
const POSTPROCESS_SYSTEM_PROMPT: &str = r#"你是实时语音转写后处理器。
输入是 ASR 粗转写文本，请在不改变原意的前提下做语义和表达修正：
- 修正同音/近音误识别，尤其是技术词汇、人名、品牌、代码术语
- 补全标点与断句，优化段落与可读性
- 清理明显口头语、重复词、改口残留（保持语义）
- 保持原语言，不翻译
- 只输出处理后的最终文本，不要解释"#;

static DOUBAO_ASR_SERIAL_MUTEX: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static DOUBAO_WS_ACTIVE_SESSIONS: AtomicUsize = AtomicUsize::new(0);
static DOUBAO_WS_DISABLED: AtomicBool = AtomicBool::new(false);

fn doubao_asr_serial_mutex() -> &'static tokio::sync::Mutex<()> {
    DOUBAO_ASR_SERIAL_MUTEX.get_or_init(|| tokio::sync::Mutex::new(()))
}

struct DoubaoWsSessionGuard;

impl Drop for DoubaoWsSessionGuard {
    fn drop(&mut self) {
        let _ = DOUBAO_WS_ACTIVE_SESSIONS.fetch_update(
            Ordering::SeqCst,
            Ordering::SeqCst,
            |value| Some(value.saturating_sub(1)),
        );
    }
}

fn is_doubao_concurrency_quota_error(msg: &str) -> bool {
    let lower = msg.to_lowercase();
    lower.contains("exceededconcurrentquota")
        || lower.contains("concurrentquota")
        || lower.contains("exceeded concurrent quota")
        || lower.contains("concurrent quota exceeded")
        || (lower.contains("concurrent") && lower.contains("quota"))
}

fn disable_doubao_ws_for_session() {
    if !DOUBAO_WS_DISABLED.swap(true, Ordering::SeqCst) {
        tracing::warn!("Disable Doubao realtime WS for this app session after quota error");
    }
}

fn read_cached_interim_text(latest_interim_text: &std::sync::Mutex<String>) -> String {
    latest_interim_text
        .lock()
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

async fn wait_for_doubao_ws_idle(max_wait: std::time::Duration) -> bool {
    if DOUBAO_WS_ACTIVE_SESSIONS.load(Ordering::SeqCst) == 0 {
        return true;
    }

    let deadline = Instant::now() + max_wait;
    while Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        if DOUBAO_WS_ACTIVE_SESSIONS.load(Ordering::SeqCst) == 0 {
            return true;
        }
    }

    DOUBAO_WS_ACTIVE_SESSIONS.load(Ordering::SeqCst) == 0
}

async fn wait_for_nonempty_interim_text(
    latest_interim_text: &std::sync::Mutex<String>,
    max_wait: std::time::Duration,
) -> String {
    let current = read_cached_interim_text(latest_interim_text);
    if !current.is_empty() {
        return current;
    }

    let deadline = Instant::now() + max_wait;
    while Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let next = read_cached_interim_text(latest_interim_text);
        if !next.is_empty() {
            return next;
        }
    }

    String::new()
}

#[derive(Clone)]
struct PostprocessSpec {
    provider: notype_llm::Provider,
    api_key: String,
    model_name: String,
}

#[derive(serde::Serialize)]
struct DoubaoBridgeInMessage<'a> {
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pcm_b64: Option<String>,
}

#[derive(serde::Deserialize)]
struct DoubaoBridgeOutMessage {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    message: Option<String>,
}

// -- Frontend event payloads --

#[derive(Clone, serde::Serialize)]
struct StatusEvent {
    status: String,
    detail: Option<String>,
}

fn emit_status(app: &tauri::AppHandle, status: &str, detail: Option<&str>) {
    tray::update_tray_status(app, status);
    let _ = app.emit(
        "notype://status",
        StatusEvent {
            status: status.into(),
            detail: detail.map(String::from),
        },
    );
}

fn set_interim_with_cache(
    app: &tauri::AppHandle,
    latest_interim_text: &Arc<std::sync::Mutex<String>>,
    text: &str,
) {
    let mut should_emit = true;
    if let Ok(mut guard) = latest_interim_text.lock() {
        if guard.as_str() == text {
            should_emit = false;
        } else {
            *guard = text.to_string();
        }
    }
    if should_emit {
        bubble::set_interim(app, text);
    }
}

// -- Tauri Commands --

#[tauri::command]
fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn list_audio_devices() -> Vec<String> {
    notype_audio::list_input_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            if d.is_default {
                format!("{} (default)", d.name)
            } else {
                d.name
            }
        })
        .collect()
}

#[tauri::command]
fn is_recording(state: tauri::State<'_, AppState>) -> bool {
    state.recorder.is_recording()
}

#[tauri::command]
async fn get_prompts(state: tauri::State<'_, AppState>) -> Result<PromptsDto, String> {
    let config = state.config.read().await;
    Ok(PromptsDto {
        agent: config.prompts.agent_text().to_string(),
        rules: config.prompts.rules_text().to_string(),
        vocabulary: config.prompts.vocabulary_text().to_string(),
    })
}

#[tauri::command]
async fn save_prompts(state: tauri::State<'_, AppState>, dto: PromptsDto) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.prompts.agent = dto.agent;
    config.prompts.rules = dto.rules;
    config.prompts.vocabulary = dto.vocabulary;
    notype_config::save(&config).map_err(|e| e.to_string())?;
    tracing::info!("Prompts saved");
    Ok(())
}

#[tauri::command]
fn get_builtin_prompts() -> PromptsDto {
    PromptsDto {
        agent: notype_config::builtin_prompts::AGENT.to_string(),
        rules: notype_config::builtin_prompts::RULES.to_string(),
        vocabulary: notype_config::builtin_prompts::VOCABULARY.to_string(),
    }
}

#[tauri::command]
async fn get_config(state: tauri::State<'_, AppState>) -> Result<ConfigDto, String> {
    let config = state.config.read().await;
    Ok(ConfigDto::from(&config))
}

#[tauri::command]
async fn save_config(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    dto: ConfigDto,
) -> Result<SaveResult, String> {
    let mut config = state.config.write().await;

    let hotkey_changed = config.general.hotkey != dto.hotkey;

    config.model.provider = match dto.provider.to_lowercase().as_str() {
        "qwen" => notype_config::Provider::Qwen,
        "doubao" => notype_config::Provider::Doubao,
        _ => notype_config::Provider::Gemini,
    };
    if !dto.gemini_api_key.is_empty() {
        config.model.gemini_api_key = dto.gemini_api_key;
    }
    if !dto.qwen_api_key.is_empty() {
        config.model.qwen_api_key = dto.qwen_api_key;
    }
    if !dto.doubao_api_key.is_empty() {
        config.model.doubao_api_key = dto.doubao_api_key;
    }
    if !dto.doubao_base_url.trim().is_empty() {
        config.model.doubao_base_url = dto.doubao_base_url;
    }
    if !dto.doubao_official_app_key.is_empty() {
        config.model.doubao_official_app_key = dto.doubao_official_app_key;
    }
    if !dto.doubao_official_access_key.is_empty() {
        config.model.doubao_official_access_key = dto.doubao_official_access_key;
    }
    config.model.enable_doubao_postprocess = dto.enable_doubao_postprocess;
    config.model.doubao_postprocess_provider = match dto.doubao_postprocess_provider.as_str() {
        "qwen" => notype_config::DoubaoPostprocessProvider::Qwen,
        "gemini" => notype_config::DoubaoPostprocessProvider::Gemini,
        _ => notype_config::DoubaoPostprocessProvider::Auto,
    };
    config.model.enable_doubao_realtime_ws = dto.enable_doubao_realtime_ws;
    if !dto.doubao_ime_credential_path.trim().is_empty() {
        config.model.doubao_ime_credential_path = dto.doubao_ime_credential_path;
    }
    config.model.model_name = dto.model_name;
    config.general.hotkey = dto.hotkey.clone();

    notype_config::save(&config).map_err(|e| e.to_string())?;

    // Rebuild recognizer
    let new_recognizer = build_recognizer(&config);
    drop(config);

    let mut rec = state.recognizer.write().await;
    *rec = new_recognizer;
    drop(rec);

    let cfg_snapshot = state.config.read().await.clone();
    if should_manage_local_doubao_gateway(&cfg_snapshot) {
        let gateway = Arc::clone(&state.doubao_gateway_process);
        let base_url = cfg_snapshot.model.doubao_base_url.clone();
        let credential_path = cfg_snapshot.model.doubao_ime_credential_path.clone();
        let api_key = cfg_snapshot.model.doubao_api_key.clone();
        state.runtime.spawn(async move {
            if let Err(e) =
                ensure_local_doubao_gateway_running(gateway, base_url, credential_path, api_key)
                    .await
            {
                tracing::warn!("Failed to auto-start local doubao-asr2api gateway: {e}");
            }
        });
    }

    // Re-register shortcut if changed
    if hotkey_changed {
        if let Err(e) = reregister_shortcut(&app, &dto.hotkey) {
            tracing::error!(error = %e, "Failed to update hotkey, restart required");
            return Ok(SaveResult {
                restart_needed: true,
            });
        }
        tracing::info!(hotkey = %dto.hotkey, "Hotkey updated");
    }

    tracing::info!("Config saved and recognizer rebuilt");
    Ok(SaveResult {
        restart_needed: false,
    })
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PromptsDto {
    agent: String,
    rules: String,
    vocabulary: String,
}

/// DTO for frontend config exchange. Keys are never sent back — only has_* flags.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ConfigDto {
    provider: String,
    gemini_api_key: String,
    qwen_api_key: String,
    doubao_api_key: String,
    doubao_base_url: String,
    doubao_official_app_key: String,
    doubao_official_access_key: String,
    enable_doubao_postprocess: bool,
    doubao_postprocess_provider: String,
    enable_doubao_realtime_ws: bool,
    doubao_ime_credential_path: String,
    model_name: String,
    hotkey: String,
    has_gemini_key: bool,
    has_qwen_key: bool,
    has_doubao_key: bool,
    has_doubao_official_app_key: bool,
    has_doubao_official_access_key: bool,
}

impl ConfigDto {
    fn from(config: &notype_config::AppConfig) -> Self {
        let provider = match config.model.provider {
            notype_config::Provider::Gemini => "gemini",
            notype_config::Provider::Qwen => "qwen",
            notype_config::Provider::Doubao => "doubao",
        };
        Self {
            provider: provider.into(),
            gemini_api_key: String::new(),
            qwen_api_key: String::new(),
            doubao_api_key: String::new(),
            doubao_base_url: config.model.doubao_base_url.clone(),
            doubao_official_app_key: String::new(),
            doubao_official_access_key: String::new(),
            enable_doubao_postprocess: config.model.enable_doubao_postprocess,
            doubao_postprocess_provider: match config.model.doubao_postprocess_provider {
                notype_config::DoubaoPostprocessProvider::Auto => "auto",
                notype_config::DoubaoPostprocessProvider::Qwen => "qwen",
                notype_config::DoubaoPostprocessProvider::Gemini => "gemini",
            }
            .to_string(),
            enable_doubao_realtime_ws: config.model.enable_doubao_realtime_ws,
            doubao_ime_credential_path: config.model.doubao_ime_credential_path.clone(),
            model_name: config.model.model_name.clone(),
            hotkey: config.general.hotkey.clone(),
            has_gemini_key: !config.model.gemini_api_key.is_empty(),
            has_qwen_key: !config.model.qwen_api_key.is_empty(),
            has_doubao_key: !config.model.doubao_api_key.is_empty(),
            has_doubao_official_app_key: !config.model.doubao_official_app_key.is_empty(),
            has_doubao_official_access_key: !config.model.doubao_official_access_key.is_empty(),
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct SaveResult {
    restart_needed: bool,
}

#[derive(Clone, serde::Serialize)]
struct DoubaoRealtimeSetupResult {
    python: String,
    credential_path: String,
    base_url: String,
    model_name: String,
    installed: bool,
    credential_ready: bool,
    gateway_running: bool,
    message: String,
}

#[derive(Clone)]
struct PythonLauncher {
    program: String,
    prefix_args: Vec<String>,
}

#[tauri::command]
async fn setup_doubao_realtime_runtime(
    state: tauri::State<'_, AppState>,
    credential_path: Option<String>,
    base_url: Option<String>,
    gateway_api_key: Option<String>,
) -> Result<DoubaoRealtimeSetupResult, String> {
    let (configured_path, configured_base_url, configured_api_key) = {
        let cfg = state.config.read().await;
        (
            cfg.model.doubao_ime_credential_path.clone(),
            cfg.model.doubao_base_url.clone(),
            cfg.model.doubao_api_key.clone(),
        )
    };

    let chosen_path = credential_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            let s = configured_path.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .unwrap_or_else(|| "~/.config/doubaoime-asr/credentials.json".to_string());

    let base_url_candidate = base_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            let s = configured_base_url.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .unwrap_or_else(|| DEFAULT_LOCAL_DOUBAO_BASE_URL.to_string());

    let chosen_base_url = if parse_local_base_url(&base_url_candidate).is_some() {
        base_url_candidate
    } else {
        tracing::warn!(
            base_url = %base_url_candidate,
            "One-click setup received non-local base_url, forcing local gateway endpoint"
        );
        DEFAULT_LOCAL_DOUBAO_BASE_URL.to_string()
    };

    let chosen_api_key = gateway_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .or_else(|| {
            let s = configured_api_key.trim();
            if s.is_empty() {
                None
            } else {
                Some(s.to_string())
            }
        })
        .unwrap_or_default();

    let launchers = python_launchers();
    let pypi_install_args = [
        "-m",
        "pip",
        "install",
        "--disable-pip-version-check",
        "--upgrade",
        "doubaoime-asr",
    ];
    let github_install_args = [
        "-m",
        "pip",
        "install",
        "--disable-pip-version-check",
        "--upgrade",
        "git+https://github.com/starccy/doubaoime-asr.git",
    ];

    let (launcher, install_source) = {
        let (primary_launcher, primary_output) =
            run_python_command(&launchers, &pypi_install_args).await?;
        if primary_output.status.success() {
            (primary_launcher, "PyPI".to_string())
        } else {
            let primary_detail = command_output_detail(&primary_output);
            tracing::warn!(
                launcher = %format_python_launcher(&primary_launcher),
                "PyPI install failed, trying GitHub fallback: {primary_detail}"
            );

            let (fallback_launcher, fallback_output) =
                run_python_command(&launchers, &github_install_args).await?;
            if fallback_output.status.success() {
                (fallback_launcher, "GitHub".to_string())
            } else {
                let fallback_detail = command_output_detail(&fallback_output);
                return Err(format!(
                    "Failed to install doubaoime-asr.\nPyPI: {primary_detail}\nGitHub fallback: {fallback_detail}\nHint: doubaoime-asr requires Python >= 3.11, and GitHub install needs network access to github.com."
                ));
            }
        }
    };

    let bootstrap_script = r#"import os
from doubaoime_asr import ASRConfig
p = os.environ.get("DOUBAO_IME_CREDENTIAL_PATH", "").strip()
if not p:
    raise RuntimeError("empty credential path")
cfg = ASRConfig(credential_path=p)
cfg.ensure_credentials()
print(p)
"#;

    let setup_output = run_python_command_with_launcher(
        &launcher,
        &["-c", bootstrap_script],
        &[("DOUBAO_IME_CREDENTIAL_PATH", chosen_path.clone())],
    )
    .await?;

    if !setup_output.status.success() {
        let stderr = String::from_utf8_lossy(&setup_output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&setup_output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(format!(
            "Failed to initialize doubaoime-asr credential using {}: {}",
            format_python_launcher(&launcher),
            detail
        ));
    }

    let expanded_path = expand_home_path(&chosen_path);
    if !expanded_path.is_file() {
        return Err(format!(
            "Credential initialization completed, but file not found at {}",
            expanded_path.display()
        ));
    }

    let gateway_dep_args = [
        "-m",
        "pip",
        "install",
        "--disable-pip-version-check",
        "--upgrade",
        "fastapi",
        "uvicorn[standard]",
        "python-multipart",
    ];
    let gateway_dep_output =
        run_python_command_with_launcher(&launcher, &gateway_dep_args, &[]).await?;
    if !gateway_dep_output.status.success() {
        return Err(format!(
            "Failed to install gateway dependencies using {}: {}",
            format_python_launcher(&launcher),
            command_output_detail(&gateway_dep_output)
        ));
    }

    let (new_recognizer, effective_model_name) = {
        let mut cfg = state.config.write().await;
        cfg.model.provider = notype_config::Provider::Doubao;
        cfg.model.doubao_ime_credential_path = chosen_path.clone();
        cfg.model.doubao_base_url = chosen_base_url.clone();
        cfg.model.enable_doubao_realtime_ws = true;

        let normalized_model = cfg.model.model_name.trim().to_lowercase();
        if cfg.model.is_doubao_official_model() || !normalized_model.starts_with("doubao-asr") {
            cfg.model.model_name = "doubao-asr".to_string();
        }

        if !chosen_api_key.trim().is_empty() {
            cfg.model.doubao_api_key = chosen_api_key.clone();
        }

        notype_config::save(&cfg).map_err(|e| format!("Failed to persist runtime config: {e}"))?;
        let effective_model_name = cfg.model.model_name.clone();
        (build_recognizer(&cfg), effective_model_name)
    };

    {
        let mut rec = state.recognizer.write().await;
        *rec = new_recognizer;
    }

    ensure_local_doubao_gateway_running(
        Arc::clone(&state.doubao_gateway_process),
        chosen_base_url.clone(),
        chosen_path.clone(),
        chosen_api_key.clone(),
    )
    .await?;
    let gateway_running = true;

    Ok(DoubaoRealtimeSetupResult {
        python: format_python_launcher(&launcher),
        credential_path: expanded_path.to_string_lossy().to_string(),
        base_url: chosen_base_url.clone(),
        model_name: effective_model_name,
        installed: true,
        credential_ready: true,
        gateway_running,
        message: format!(
            "doubaoime-asr installed ({install_source}), credentials initialized, runtime switched to Doubao local ASR, gateway running at {chosen_base_url}"
        ),
    })
}

fn python_launchers() -> Vec<PythonLauncher> {
    if let Ok(value) = std::env::var("NOTYPE_DOUBAO_PYTHON") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            return vec![PythonLauncher {
                program: trimmed.to_string(),
                prefix_args: Vec::new(),
            }];
        }
    }

    if cfg!(target_os = "windows") {
        vec![
            PythonLauncher {
                program: "python".to_string(),
                prefix_args: Vec::new(),
            },
            PythonLauncher {
                program: "py".to_string(),
                prefix_args: vec!["-3".to_string()],
            },
        ]
    } else {
        vec![
            PythonLauncher {
                program: "python3".to_string(),
                prefix_args: Vec::new(),
            },
            PythonLauncher {
                program: "python".to_string(),
                prefix_args: Vec::new(),
            },
        ]
    }
}

fn format_python_launcher(launcher: &PythonLauncher) -> String {
    if launcher.prefix_args.is_empty() {
        launcher.program.clone()
    } else {
        format!("{} {}", launcher.program, launcher.prefix_args.join(" "))
    }
}

fn expand_home_path(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home);
        }
    }

    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            return PathBuf::from(home).join(rest);
        }
    }

    PathBuf::from(path)
}

fn command_output_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("process exited with status {}", output.status)
    }
}

fn parse_local_base_url(base_url: &str) -> Option<(String, u16)> {
    let normalized = base_url.trim().trim_end_matches('/');
    let rest = normalized.strip_prefix("http://")?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return None;
    }

    let (host_raw, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().ok()?),
        None => (authority, 80),
    };
    let host = host_raw.trim_matches(|c| c == '[' || c == ']').to_string();
    let is_local = matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1");
    if !is_local {
        return None;
    }
    Some((host, port))
}

fn resolve_doubao_gateway_script_path() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/doubao_asr_api_gateway.py"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/doubao_asr_api_gateway.py"),
    ];
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scripts/doubao_asr_api_gateway.py"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

async fn check_local_gateway_health(host: &str, port: u16) -> bool {
    let connect = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::net::TcpStream::connect((host, port)),
    )
    .await;

    let Ok(Ok(mut stream)) = connect else {
        return false;
    };

    let req = format!("GET /health HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).await.is_err() {
        return false;
    }

    let mut buf = vec![0u8; 256];
    match tokio::time::timeout(std::time::Duration::from_millis(800), stream.read(&mut buf)).await
    {
        Ok(Ok(n)) if n > 0 => {
            let head = String::from_utf8_lossy(&buf[..n]);
            head.contains("200")
        }
        _ => false,
    }
}

fn should_manage_local_doubao_gateway(config: &notype_config::AppConfig) -> bool {
    matches!(config.model.provider, notype_config::Provider::Doubao)
        && !config.model.is_doubao_official_model()
        && parse_local_base_url(&config.model.doubao_base_url).is_some()
}

async fn ensure_local_doubao_gateway_running(
    gateway_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    base_url: String,
    credential_path: String,
    api_key: String,
) -> Result<(), String> {
    let (host, port) = parse_local_base_url(&base_url)
        .ok_or_else(|| "doubao_base_url is not a local http endpoint".to_string())?;

    if check_local_gateway_health(&host, port).await {
        return Ok(());
    }

    {
        let mut slot = gateway_slot.lock().await;
        let dead = if let Some(child) = slot.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    tracing::warn!("Existing doubao gateway exited: {status}");
                    true
                }
                Ok(None) => false,
                Err(e) => {
                    tracing::warn!("Failed to inspect doubao gateway process: {e}");
                    true
                }
            }
        } else {
            false
        };
        if dead {
            *slot = None;
        }
    }

    if check_local_gateway_health(&host, port).await {
        return Ok(());
    }

    let script = resolve_doubao_gateway_script_path()
        .ok_or_else(|| "doubao gateway script not found".to_string())?;
    let python = std::env::var("NOTYPE_DOUBAO_PYTHON").unwrap_or_else(|_| "python3".to_string());

    let mut cmd = tokio::process::Command::new(&python);
    cmd.arg("-u")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("DOUBAO_ASR_HOST", host.clone())
        .env("DOUBAO_ASR_PORT", port.to_string())
        .env("DOUBAO_ASR_CREDENTIAL_PATH", credential_path);
    if !api_key.trim().is_empty() {
        cmd.env("DOUBAO_ASR_API_KEY", api_key);
    }
    apply_doubao_network_env(&mut cmd);
    apply_opus_runtime_env(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| {
        format!("Failed to start local doubao-asr2api gateway via {python}: {e}")
    })?;

    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if !line.trim().is_empty() => {
                        tracing::info!("doubao-gateway: {line}");
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!("doubao-gateway stdout read error: {e}");
                        break;
                    }
                }
            }
        });
    }

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if !line.trim().is_empty() => {
                        tracing::warn!("doubao-gateway stderr: {line}");
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!("doubao-gateway stderr read error: {e}");
                        break;
                    }
                }
            }
        });
    }

    {
        let mut slot = gateway_slot.lock().await;
        if let Some(mut old) = slot.take() {
            let _ = old.kill().await;
        }
        *slot = Some(child);
    }

    for _ in 0..24 {
        if check_local_gateway_health(&host, port).await {
            tracing::info!(host = %host, port, "Local doubao-asr2api gateway ready");
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    {
        let mut slot = gateway_slot.lock().await;
        if let Some(mut child) = slot.take() {
            let _ = child.kill().await;
        }
    }

    Err("Local doubao-asr2api gateway failed health check on startup".to_string())
}

async fn run_doubao_gateway_maintainer(
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    gateway_slot: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        let cfg = config.read().await.clone();
        if !should_manage_local_doubao_gateway(&cfg) {
            continue;
        }
        if let Err(e) = ensure_local_doubao_gateway_running(
            Arc::clone(&gateway_slot),
            cfg.model.doubao_base_url.clone(),
            cfg.model.doubao_ime_credential_path.clone(),
            cfg.model.doubao_api_key.clone(),
        )
        .await
        {
            tracing::warn!("Failed to keep local doubao-asr2api gateway alive: {e}");
        }
    }
}

async fn run_python_command(
    launchers: &[PythonLauncher],
    args: &[&str],
) -> Result<(PythonLauncher, std::process::Output), String> {
    let mut attempted = Vec::new();

    for launcher in launchers {
        attempted.push(format_python_launcher(launcher));
        let mut cmd = tokio::process::Command::new(&launcher.program);
        cmd.args(&launcher.prefix_args)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        apply_doubao_network_env(&mut cmd);
        apply_opus_runtime_env(&mut cmd);

        match cmd.output().await {
            Ok(output) => return Ok((launcher.clone(), output)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(format!(
                    "Failed to start {}: {e}",
                    format_python_launcher(launcher)
                ));
            }
        }
    }

    Err(format!(
        "No usable Python found (tried: {}). Set NOTYPE_DOUBAO_PYTHON to specify one.",
        attempted.join(", ")
    ))
}

async fn run_python_command_with_launcher(
    launcher: &PythonLauncher,
    args: &[&str],
    envs: &[(&str, String)],
) -> Result<std::process::Output, String> {
    let mut cmd = tokio::process::Command::new(&launcher.program);
    cmd.args(&launcher.prefix_args)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_doubao_network_env(&mut cmd);
    apply_opus_runtime_env(&mut cmd);
    for (k, v) in envs {
        cmd.env(k, v);
    }

    cmd.output().await.map_err(|e| {
        format!(
            "Failed to run {}: {e}",
            format_python_launcher(launcher)
        )
    })
}

fn apply_opus_runtime_env(cmd: &mut tokio::process::Command) {
    #[cfg(target_os = "macos")]
    {
        let mut dirs = Vec::new();
        for dir in ["/opt/homebrew/lib", "/usr/local/lib", "/opt/local/lib"] {
            if std::path::Path::new(dir).is_dir() {
                dirs.push(dir.to_string());
            }
        }
        if !dirs.is_empty() {
            let prefixes = dirs.join(":");
            let fallback = std::env::var("DYLD_FALLBACK_LIBRARY_PATH").unwrap_or_default();
            let fallback_value = if fallback.is_empty() {
                prefixes.clone()
            } else {
                format!("{prefixes}:{fallback}")
            };
            let dyld = std::env::var("DYLD_LIBRARY_PATH").unwrap_or_default();
            let dyld_value = if dyld.is_empty() {
                prefixes
            } else {
                format!("{fallback_value}:{dyld}")
            };
            cmd.env("DYLD_FALLBACK_LIBRARY_PATH", fallback_value);
            cmd.env("DYLD_LIBRARY_PATH", dyld_value);
        }
    }
}

fn apply_doubao_network_env(cmd: &mut tokio::process::Command) {
    let required_hosts = [
        "log.snssdk.com",
        "is.snssdk.com",
        "frontier-audio-ime-ws.doubao.com",
        "ime.oceancloudapi.com",
        "keyhub.zijieapi.com",
        "speech.bytedance.com",
        ".snssdk.com",
        ".doubao.com",
        ".oceancloudapi.com",
        ".zijieapi.com",
        ".bytedance.com",
    ];

    let mut merged = std::env::var("NO_PROXY")
        .or_else(|_| std::env::var("no_proxy"))
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<String>>();

    for host in required_hosts {
        if !merged.iter().any(|v| v.eq_ignore_ascii_case(host)) {
            merged.push(host.to_string());
        }
    }

    let value = merged.join(",");
    cmd.env("NO_PROXY", &value);
    cmd.env("no_proxy", value);
}

// -- App Lifecycle --

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    tracing::info!("NoType v{} starting", env!("CARGO_PKG_VERSION"));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to create tokio runtime");

    let config = notype_config::load();
    let recognizer = build_recognizer(&config);

    tracing::info!(
        provider = ?config.model.provider,
        model = %config.model.model_name,
        has_gemini_key = !config.model.gemini_api_key.is_empty(),
        has_qwen_key = !config.model.qwen_api_key.is_empty(),
        has_doubao_key = !config.model.doubao_api_key.is_empty(),
        has_doubao_official_app_key = !config.model.doubao_official_app_key.is_empty(),
        has_doubao_official_access_key = !config.model.doubao_official_access_key.is_empty(),
        "Config loaded"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            recorder: Arc::new(Recorder::new(None)),
            inputter: Arc::new(TextInputter::new()),
            recognizer: Arc::new(tokio::sync::RwLock::new(recognizer)),
            config: Arc::new(tokio::sync::RwLock::new(config)),
            runtime: runtime.handle().clone(),
            bubble_generation: Arc::new(AtomicU64::new(0)),
            hotkey_down: Arc::new(AtomicBool::new(false)),
            latest_interim_text: Arc::new(std::sync::Mutex::new(String::new())),
            doubao_gateway_process: Arc::new(tokio::sync::Mutex::new(None)),
        })
        .on_window_event(|window, event| {
            // Only intercept close on main window (hide to tray)
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            tray::create_tray(app)?;
            setup_global_shortcut(app)?;

            // Show settings on first launch if required recognizer credentials are missing.
            let show_on_start = {
                let config = app.state::<AppState>();
                let rt = config.runtime.clone();
                let cfg = Arc::clone(&config.config);
                rt.block_on(async { !cfg.read().await.model.has_required_credentials() })
            };
            if show_on_start {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

            // Keep local doubao-asr2api gateway alive for doubao-asr mode.
            let state = app.state::<AppState>();
            let cfg = Arc::clone(&state.config);
            let gateway = Arc::clone(&state.doubao_gateway_process);
            let rt = state.runtime.clone();
            rt.spawn(async move {
                run_doubao_gateway_maintainer(cfg, gateway).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            list_audio_devices,
            get_prompts,
            save_prompts,
            get_builtin_prompts,
            is_recording,
            get_config,
            save_config,
            setup_doubao_realtime_runtime,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// -- Global Shortcut --

#[cfg(desktop)]
fn setup_global_shortcut(app: &tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    // Read hotkey from config
    let hotkey_str = {
        let state = app.state::<AppState>();
        let rt = state.runtime.clone();
        let cfg = Arc::clone(&state.config);
        rt.block_on(async { cfg.read().await.general.hotkey.clone() })
    };

    let shortcut =
        parse_hotkey(&hotkey_str).map_err(|e| format!("Invalid hotkey '{hotkey_str}': {e}"))?;

    app.handle().plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, _sc, event| {
                let state = app.state::<AppState>();
                let recorder = &state.recorder;
                let hotkey_down = &state.hotkey_down;

                match event.state() {
                    ShortcutState::Pressed => {
                        if hotkey_down
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_err()
                        {
                            return;
                        }

                        if !recorder.is_recording() {
                            state.bubble_generation.fetch_add(1, Ordering::SeqCst);
                            if let Ok(mut guard) = state.latest_interim_text.lock() {
                                guard.clear();
                            }
                            if let Some(window) = app.get_webview_window("main") {
                                let _ = window.hide();
                            }
                            if let Err(e) = recorder.start() {
                                hotkey_down.store(false, Ordering::SeqCst);
                                tracing::error!("Failed to start recording: {e}");
                                emit_status(app, "Error", Some(&e.to_string()));
                            } else {
                                bubble::hide_bubble(app);
                                bubble::show_bubble(app);
                                bubble::set_recording(app);
                                emit_status(app, "Recording", None);

                                // Start interim transcription loop
                                let rec_clone = Arc::clone(&state.recorder);
                                let recognizer = Arc::clone(&state.recognizer);
                                let cfg = Arc::clone(&state.config);
                                let handle = app.clone();
                                let gen = Arc::clone(&state.bubble_generation);
                                let latest_interim_text = Arc::clone(&state.latest_interim_text);
                                let gen_val = gen.load(Ordering::SeqCst);
                                state.runtime.spawn(async move {
                                    interim_loop(
                                        handle,
                                        rec_clone,
                                        recognizer,
                                        cfg,
                                        latest_interim_text,
                                        gen,
                                        gen_val,
                                    )
                                    .await;
                                });
                            }
                        } else {
                            hotkey_down.store(false, Ordering::SeqCst);
                        }
                    }
                    ShortcutState::Released => {
                        if !hotkey_down.swap(false, Ordering::SeqCst) {
                            return;
                        }

                        if recorder.is_recording() {
                            match recorder.stop() {
                                Ok(audio) => {
                                    tracing::info!(
                                        duration = audio.duration_secs,
                                        bytes = audio.wav_bytes.len(),
                                        "Audio captured"
                                    );
                                    bubble::set_recognizing(app);
                                    emit_status(app, "Recognizing", None);
                                    let recognizer = Arc::clone(&state.recognizer);
                                    let cfg = Arc::clone(&state.config);
                                    let inputter = Arc::clone(&state.inputter);
                                    let gen = Arc::clone(&state.bubble_generation);
                                    let latest_interim_text = Arc::clone(&state.latest_interim_text);
                                    let gateway_process = Arc::clone(&state.doubao_gateway_process);
                                    let rt = state.runtime.clone();
                                    let handle = app.clone();
                                    rt.spawn(async move {
                                        process_audio(
                                            &handle,
                                            &recognizer,
                                            &cfg,
                                            &inputter,
                                            gateway_process,
                                            latest_interim_text.as_ref(),
                                            &gen,
                                            audio,
                                        )
                                        .await;
                                    });
                                }
                                Err(e) => {
                                    tracing::error!("Failed to stop recording: {e}");
                                    emit_status(app, "Error", Some(&e.to_string()));
                                }
                            }
                        }
                    }
                }
            })
            .build(),
    )?;

    app.global_shortcut().register(shortcut)?;
    tracing::info!("Global shortcut Ctrl+. registered");
    Ok(())
}

#[cfg(not(desktop))]
fn setup_global_shortcut(_app: &tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

// -- Interim Transcription (live preview while recording) --

const INTERIM_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_millis(2000);
const INTERIM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(2500);
const INTERIM_INITIAL_DELAY_DOUBAO: std::time::Duration = std::time::Duration::from_millis(300);
const INTERIM_INTERVAL_DOUBAO: std::time::Duration = std::time::Duration::from_millis(900);
const INTERIM_MIN_DURATION: f32 = 1.0;
const INTERIM_DOUBAO_MIN_CHUNK_DURATION: f32 = 0.75;
const INTERIM_DOUBAO_CONTEXT_SECS: f32 = 1.2;
const INTERIM_DOUBAO_QUOTA_COOLDOWN: std::time::Duration = std::time::Duration::from_secs(3);
const INTERIM_DOUBAO_WS_PCM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(80);

async fn interim_loop(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
) {
    let provider = { config.read().await.model.provider.clone() };

    if matches!(provider, notype_config::Provider::Doubao) {
        interim_loop_doubao_streaming(
            app,
            recorder,
            recognizer,
            config,
            latest_interim_text,
            generation,
            gen_val,
        )
        .await;
    } else {
        interim_loop_default(
            app,
            recorder,
            recognizer,
            config,
            latest_interim_text,
            generation,
            gen_val,
        )
        .await;
    }
}

async fn interim_loop_default(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
) {
    let prompt = { config.read().await.prompts.compose() };
    let mut last_asr_text = String::new();

    tokio::time::sleep(INTERIM_INITIAL_DELAY).await;

    loop {
        if generation.load(Ordering::SeqCst) != gen_val || !recorder.is_recording() {
            break;
        }

        let snapshot = recorder.snapshot();
        let Some(Ok(audio)) = snapshot else {
            break;
        };

        if audio.duration_secs < INTERIM_MIN_DURATION {
            tokio::time::sleep(INTERIM_INTERVAL).await;
            continue;
        }

        let guard = recognizer.read().await;
        let Some(rec) = guard.as_ref() else {
            break;
        };

        tracing::info!(
            duration = audio.duration_secs,
            "Interim transcription request"
        );

        let result = rec
            .recognize(audio.wav_bytes, "audio/wav".into(), prompt.clone())
            .await;

        drop(guard);

        match result {
            Ok(result) if !result.text.is_empty() => {
                if result.text != last_asr_text
                    && generation.load(Ordering::SeqCst) == gen_val
                    && recorder.is_recording()
                {
                    last_asr_text = result.text.clone();
                    tracing::info!(text = %result.text, "Interim result");
                    set_interim_with_cache(&app, &latest_interim_text, &result.text);
                }
            }
            Err(e) => {
                tracing::warn!("Interim transcription failed: {e}");
            }
            _ => {}
        }

        tokio::time::sleep(INTERIM_INTERVAL).await;
    }
}

async fn interim_loop_doubao_streaming(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
) {
    let cfg_snapshot = { config.read().await.clone() };
    let prompt = cfg_snapshot.prompts.compose();
    let postprocess_spec = build_postprocess_spec(&cfg_snapshot);

    if should_use_doubao_ws_realtime(&cfg_snapshot) {
        let used_ws = interim_loop_doubao_ws_realtime(
            app.clone(),
            Arc::clone(&recorder),
            Arc::clone(&latest_interim_text),
            Arc::clone(&generation),
            gen_val,
            cfg_snapshot.model.doubao_ime_credential_path.clone(),
            postprocess_spec.clone(),
        )
        .await;
        if used_ws {
            return;
        }
        tracing::warn!("Doubao realtime WS unavailable, fallback to chunked interim mode");
    }

    tokio::time::sleep(INTERIM_INITIAL_DELAY_DOUBAO).await;

    let mut last_end_sample = 0usize;
    let mut sample_rate = 16_000u32;
    let mut channels = 1u16;
    let mut rough_text = String::new();
    let mut displayed_text = String::new();
    let mut completed_source_sentences: Vec<String> = Vec::new();
    let mut corrected_display_sentences: Vec<String> = Vec::new();
    let mut last_llm_index: Option<usize> = None;
    let mut last_llm_target = String::new();
    let mut quota_cooldown_until: Option<Instant> = None;
    let stable_revision = Arc::new(AtomicU64::new(0));
    let mut llm_task: Option<tokio::task::JoinHandle<(usize, String, Option<String>)>> = None;
    let mut interval = tokio::time::interval(INTERIM_INTERVAL_DOUBAO);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let is_valid_session =
        || generation.load(Ordering::SeqCst) == gen_val && recorder.is_recording();

    loop {
        interval.tick().await;

        if !is_valid_session() {
            break;
        }

        if let Some(until) = quota_cooldown_until {
            if Instant::now() < until {
                continue;
            }
            quota_cooldown_until = None;
        }

        if llm_task.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(task) = llm_task.take() {
                last_llm_index = None;
                last_llm_target.clear();
                match task.await {
                    Ok((target_index, target_sentence, Some(processed_sentence)))
                        if !processed_sentence.trim().is_empty() && is_valid_session() =>
                    {
                        if completed_source_sentences
                            .get(target_index)
                            .is_some_and(|current| same_live_sentence(current, &target_sentence))
                        {
                            if corrected_display_sentences.len() > target_index {
                                corrected_display_sentences[target_index] =
                                    compact_repeated_sentences(&processed_sentence);
                                corrected_display_sentences.truncate(target_index + 1);
                            } else {
                                while corrected_display_sentences.len() < target_index {
                                    corrected_display_sentences.push(String::new());
                                }
                                corrected_display_sentences
                                    .push(compact_repeated_sentences(&processed_sentence));
                            }
                            maybe_emit_live_display(
                                &app,
                                &latest_interim_text,
                                &mut displayed_text,
                                &rough_text,
                                &corrected_display_sentences,
                                is_valid_session(),
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => tracing::warn!("Doubao live post-process task failed: {e}"),
                }
            }
        }

        let context_samples =
            ((sample_rate as f32 * channels as f32 * INTERIM_DOUBAO_CONTEXT_SECS) as usize)
                .max(1);
        let from_sample = last_end_sample.saturating_sub(context_samples);

        let snapshot = recorder.snapshot_from(from_sample);
        let Some(snapshot_result) = snapshot else {
            break;
        };
        let slice = match snapshot_result {
            Ok(slice) => slice,
            Err(e) => {
                tracing::warn!("Doubao interim snapshot failed: {e}");
                tokio::time::sleep(INTERIM_INTERVAL_DOUBAO).await;
                continue;
            }
        };

        sample_rate = slice.audio.sample_rate.max(1);
        channels = slice.audio.channels.max(1);

        let new_samples = slice.end_sample.saturating_sub(last_end_sample);
        if new_samples == 0 {
            maybe_schedule_live_postprocess(
                &mut llm_task,
                &postprocess_spec,
                &completed_source_sentences,
                &corrected_display_sentences,
                &mut last_llm_index,
                &mut last_llm_target,
                &app,
                &recorder,
                &generation,
                gen_val,
                &stable_revision,
            );
            continue;
        }

        let new_duration = new_samples as f32 / (sample_rate as f32 * channels as f32);
        last_end_sample = slice.end_sample;

        if new_duration < INTERIM_DOUBAO_MIN_CHUNK_DURATION {
            continue;
        }

        let guard = recognizer.read().await;
        let Some(rec) = guard.as_ref() else {
            break;
        };

        tracing::info!(
            chunk_secs = new_duration,
            start_sample = slice.start_sample,
            end_sample = slice.end_sample,
            "Doubao incremental interim transcription request"
        );

        let _serial_guard = doubao_asr_serial_mutex().lock().await;
        let result = rec
            .recognize(slice.audio.wav_bytes, "audio/wav".into(), prompt.clone())
            .await;
        drop(_serial_guard);

        drop(guard);

        match result {
            Ok(result) if !result.text.trim().is_empty() => {
                let merged = merge_incremental_asr_text(&rough_text, &result.text);
                if merged != rough_text {
                    let compacted = compact_repeated_sentences(&merged);
                    if compacted.len() < merged.len() {
                        tracing::info!(
                            before_chars = merged.chars().count(),
                            after_chars = compacted.chars().count(),
                            "Compacted repeated fragments in chunked Doubao rough text"
                        );
                    }
                    rough_text = compacted;
                    let (next_completed_source_sentences, _) =
                        split_live_completed_sentences(&rough_text);
                    let shared_prefix = common_live_sentence_prefix_len(
                        &completed_source_sentences,
                        &next_completed_source_sentences,
                    );
                    if shared_prefix < completed_source_sentences.len() {
                        stable_revision.fetch_add(1, Ordering::SeqCst);
                        completed_source_sentences
                            .truncate(shared_prefix);
                        corrected_display_sentences
                            .truncate(shared_prefix);
                        last_llm_index = None;
                        last_llm_target.clear();
                        if let Some(task) = llm_task.take() {
                            task.abort();
                        }
                    }
                    if next_completed_source_sentences.len() < corrected_display_sentences.len() {
                        corrected_display_sentences
                            .truncate(next_completed_source_sentences.len());
                    }
                    completed_source_sentences = next_completed_source_sentences;
                    maybe_emit_live_display(
                        &app,
                        &latest_interim_text,
                        &mut displayed_text,
                        &rough_text,
                        &corrected_display_sentences,
                        is_valid_session(),
                    );
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                if is_doubao_concurrency_quota_error(&error_msg) {
                    quota_cooldown_until = Some(Instant::now() + INTERIM_DOUBAO_QUOTA_COOLDOWN);
                    tracing::warn!(
                        cooldown_ms = INTERIM_DOUBAO_QUOTA_COOLDOWN.as_millis(),
                        "Doubao interim throttled by ExceededConcurrentQuota, entering cooldown"
                    );
                } else {
                    tracing::debug!("Doubao incremental interim transcription failed: {error_msg}");
                }
            }
            _ => {}
        }

        maybe_schedule_live_postprocess(
            &mut llm_task,
            &postprocess_spec,
            &completed_source_sentences,
            &corrected_display_sentences,
            &mut last_llm_index,
            &mut last_llm_target,
            &app,
            &recorder,
            &generation,
            gen_val,
            &stable_revision,
        );
    }

    if let Some(task) = llm_task.take() {
        task.abort();
    }
}

fn should_use_doubao_ws_realtime(config: &notype_config::AppConfig) -> bool {
    matches!(config.model.provider, notype_config::Provider::Doubao)
        && config.model.enable_doubao_realtime_ws
        && !config.model.is_doubao_official_model()
        && !DOUBAO_WS_DISABLED.load(Ordering::SeqCst)
}

fn resolve_doubao_bridge_script_path() -> Option<PathBuf> {
    let mut candidates = vec![
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/doubao_realtime_bridge.py"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/doubao_realtime_bridge.py"),
    ];
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("scripts/doubao_realtime_bridge.py"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

async fn send_doubao_bridge_message(
    stdin: &mut tokio::process::ChildStdin,
    msg: &DoubaoBridgeInMessage<'_>,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(msg).map_err(std::io::Error::other)?;
    line.push(b'\n');
    stdin.write_all(&line).await?;
    stdin.flush().await
}

async fn interim_loop_doubao_ws_realtime(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
    credential_path: String,
    postprocess_spec: Option<PostprocessSpec>,
) -> bool {
    let Some(bridge_script) = resolve_doubao_bridge_script_path() else {
        tracing::warn!("Doubao realtime bridge script not found");
        return false;
    };

    let python = std::env::var("NOTYPE_DOUBAO_PYTHON").unwrap_or_else(|_| "python3".to_string());
    let mut cmd = tokio::process::Command::new(&python);
    cmd.arg("-u")
        .arg(&bridge_script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    apply_doubao_network_env(&mut cmd);
    apply_opus_runtime_env(&mut cmd);

    if !credential_path.trim().is_empty() {
        cmd.env("DOUBAO_IME_CREDENTIAL_PATH", credential_path);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Failed to start Doubao realtime bridge ({python}): {e}");
            return false;
        }
    };

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) if !line.trim().is_empty() => {
                        tracing::debug!("doubao-bridge stderr: {line}");
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(e) => {
                        tracing::debug!("doubao-bridge stderr read error: {e}");
                        break;
                    }
                }
            }
        });
    }

    let mut stdin = match child.stdin.take() {
        Some(v) => v,
        None => {
            let _ = child.kill().await;
            return false;
        }
    };
    let stdout = match child.stdout.take() {
        Some(v) => v,
        None => {
            let _ = child.kill().await;
            return false;
        }
    };
    let mut lines = BufReader::new(stdout).lines();

    let ready_line = tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
        .await
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(ready_line) = ready_line else {
        let _ = child.kill().await;
        tracing::warn!("Doubao realtime bridge did not become ready");
        return false;
    };

    let ready_msg = serde_json::from_str::<DoubaoBridgeOutMessage>(&ready_line).ok();
    if !ready_msg.as_ref().is_some_and(|m| m.kind == "ready") {
        if let Some(m) = ready_msg {
            let message = m.message.unwrap_or_else(|| "unknown".to_string());
            tracing::warn!(
                "Doubao realtime bridge startup error: {}",
                message
            );
            if is_doubao_concurrency_quota_error(&message) {
                tracing::warn!(
                    "Doubao realtime WS quota exceeded on startup; disable WS and fallback to chunked interim mode"
                );
                disable_doubao_ws_for_session();
                let _ = child.kill().await;
                return false;
            }
        } else {
            tracing::warn!("Doubao realtime bridge invalid ready payload: {ready_line}");
        }
        let _ = child.kill().await;
        return false;
    }

    DOUBAO_WS_ACTIVE_SESSIONS.fetch_add(1, Ordering::SeqCst);
    let _ws_guard = DoubaoWsSessionGuard;

    let is_valid_session =
        || generation.load(Ordering::SeqCst) == gen_val && recorder.is_recording();

    let mut last_end_sample = 0usize;
    let mut rough_text = String::new();
    let mut displayed_text = String::new();
    let mut completed_source_sentences: Vec<String> = Vec::new();
    let mut corrected_display_sentences: Vec<String> = Vec::new();
    let mut last_llm_index: Option<usize> = None;
    let mut last_llm_target = String::new();
    let mut bridge_failed = false;
    let mut logged_resample = false;
    let stable_revision = Arc::new(AtomicU64::new(0));
    let mut llm_task: Option<tokio::task::JoinHandle<(usize, String, Option<String>)>> = None;
    let mut pcm_interval = tokio::time::interval(INTERIM_DOUBAO_WS_PCM_INTERVAL);
    pcm_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if !is_valid_session() {
            break;
        }

        if llm_task.as_ref().is_some_and(|h| h.is_finished()) {
            if let Some(task) = llm_task.take() {
                last_llm_index = None;
                last_llm_target.clear();
                match task.await {
                    Ok((target_index, target_sentence, Some(processed_sentence)))
                        if !processed_sentence.trim().is_empty() && is_valid_session() =>
                    {
                        if completed_source_sentences
                            .get(target_index)
                            .is_some_and(|current| same_live_sentence(current, &target_sentence))
                        {
                            if corrected_display_sentences.len() > target_index {
                                corrected_display_sentences[target_index] =
                                    compact_repeated_sentences(&processed_sentence);
                                corrected_display_sentences.truncate(target_index + 1);
                            } else {
                                while corrected_display_sentences.len() < target_index {
                                    corrected_display_sentences.push(String::new());
                                }
                                corrected_display_sentences
                                    .push(compact_repeated_sentences(&processed_sentence));
                            }
                            maybe_emit_live_display(
                                &app,
                                &latest_interim_text,
                                &mut displayed_text,
                                &rough_text,
                                &corrected_display_sentences,
                                is_valid_session(),
                            );
                        }
                    }
                    Ok(_) => {}
                    Err(e) if e.is_cancelled() => {}
                    Err(e) => tracing::warn!("Doubao ws post-process task failed: {e}"),
                }
            }
        }

        maybe_schedule_live_postprocess(
            &mut llm_task,
            &postprocess_spec,
            &completed_source_sentences,
            &corrected_display_sentences,
            &mut last_llm_index,
            &mut last_llm_target,
            &app,
            &recorder,
            &generation,
            gen_val,
            &stable_revision,
        );

        tokio::select! {
            _ = pcm_interval.tick() => {
                let snapshot = recorder.snapshot_pcm_from(last_end_sample);
                let Some(snapshot_result) = snapshot else {
                    break;
                };
                let pcm = match snapshot_result {
                    Ok(pcm) => pcm,
                    Err(e) => {
                        tracing::debug!("Doubao ws pcm snapshot failed: {e}");
                        continue;
                    }
                };

                if pcm.end_sample <= last_end_sample || pcm.pcm_s16le.is_empty() {
                    continue;
                }
                last_end_sample = pcm.end_sample;

                if !logged_resample && (pcm.sample_rate != 16000 || pcm.channels != 1) {
                    tracing::info!(
                        sample_rate = pcm.sample_rate,
                        channels = pcm.channels,
                        "Resampling realtime PCM to 16kHz mono for doubao ws bridge"
                    );
                    logged_resample = true;
                }

                let bridge_pcm = convert_pcm_chunk_to_16k_mono_s16le(
                    &pcm.pcm_s16le,
                    pcm.sample_rate,
                    pcm.channels,
                );
                if bridge_pcm.is_empty() {
                    continue;
                }

                let msg = DoubaoBridgeInMessage {
                    kind: "audio",
                    pcm_b64: Some(base64::engine::general_purpose::STANDARD.encode(&bridge_pcm)),
                };
                if let Err(e) = send_doubao_bridge_message(&mut stdin, &msg).await {
                    tracing::warn!("Failed to send pcm to doubao bridge: {e}");
                    bridge_failed = true;
                    break;
                }
            }
            line_result = lines.next_line() => {
                let line = match line_result {
                    Ok(Some(line)) => line,
                    Ok(None) => {
                        bridge_failed = true;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("Failed reading doubao bridge output: {e}");
                        bridge_failed = true;
                        break;
                    }
                };

                let msg = match serde_json::from_str::<DoubaoBridgeOutMessage>(&line) {
                    Ok(msg) => msg,
                    Err(e) => {
                        tracing::debug!("Ignore invalid doubao bridge payload: {e}");
                        continue;
                    }
                };

                match msg.kind.as_str() {
                    "interim" | "final" => {
                        let Some(text) = msg.text else {
                            continue;
                        };
                        let merged = merge_incremental_asr_text(&rough_text, &text);
                        if merged != rough_text {
                            let compacted = compact_repeated_sentences(&merged);
                            if compacted.len() < merged.len() {
                                tracing::info!(
                                    before_chars = merged.chars().count(),
                                    after_chars = compacted.chars().count(),
                                    "Compacted repeated fragments in ws Doubao rough text"
                                );
                            }
                            rough_text = compacted;
                            let (next_completed_source_sentences, _) =
                                split_live_completed_sentences(&rough_text);
                            let shared_prefix = common_live_sentence_prefix_len(
                                &completed_source_sentences,
                                &next_completed_source_sentences,
                            );
                            if shared_prefix < completed_source_sentences.len() {
                                stable_revision.fetch_add(1, Ordering::SeqCst);
                                completed_source_sentences
                                    .truncate(shared_prefix);
                                corrected_display_sentences
                                    .truncate(shared_prefix);
                                last_llm_index = None;
                                last_llm_target.clear();
                                if let Some(task) = llm_task.take() {
                                    task.abort();
                                }
                            }
                            if next_completed_source_sentences.len()
                                < corrected_display_sentences.len()
                            {
                                corrected_display_sentences
                                    .truncate(next_completed_source_sentences.len());
                            }
                            completed_source_sentences = next_completed_source_sentences;
                            maybe_emit_live_display(
                                &app,
                                &latest_interim_text,
                                &mut displayed_text,
                                &rough_text,
                                &corrected_display_sentences,
                                is_valid_session(),
                            );
                        }
                    }
                    "error" => {
                        let message = msg.message.unwrap_or_else(|| "unknown".to_string());
                        tracing::warn!(
                            "Doubao bridge error: {}",
                            message
                        );
                        if is_doubao_concurrency_quota_error(&message) {
                            tracing::warn!(
                                "Doubao realtime WS quota exceeded; disable WS and fallback to chunked interim mode"
                            );
                            disable_doubao_ws_for_session();
                            let _ = child.kill().await;
                            return false;
                        }
                        if rough_text.is_empty() {
                            let _ = child.kill().await;
                            return false;
                        }
                        bridge_failed = true;
                        break;
                    }
                    "ready" => {}
                    _ => {}
                }
            }
        }
    }

    if let Some(task) = llm_task.take() {
        task.abort();
    }

    let _ = send_doubao_bridge_message(
        &mut stdin,
        &DoubaoBridgeInMessage {
            kind: "end",
            pcm_b64: None,
        },
    )
    .await;
    let _ = stdin.shutdown().await;

    // Recorder has stopped, but bridge may still flush the last interim/final packets.
    // Drain a short tail window so finalization can use the most complete preview text.
    let tail_deadline = Instant::now() + std::time::Duration::from_millis(1200);
    while Instant::now() < tail_deadline {
        let line = match tokio::time::timeout(std::time::Duration::from_millis(90), lines.next_line())
            .await
        {
            Ok(Ok(Some(line))) => line,
            Ok(Ok(None)) => break,
            Ok(Err(e)) => {
                tracing::debug!("doubao bridge tail read error: {e}");
                break;
            }
            Err(_) => break,
        };

        let msg = match serde_json::from_str::<DoubaoBridgeOutMessage>(&line) {
            Ok(msg) => msg,
            Err(_) => continue,
        };

        if !matches!(msg.kind.as_str(), "interim" | "final") {
            continue;
        }
        let Some(text) = msg.text else {
            continue;
        };

        let merged = merge_incremental_asr_text(&rough_text, &text);
        if merged != rough_text {
            rough_text = compact_repeated_sentences(&merged);
        }
    }

    if !rough_text.trim().is_empty() {
        if let Ok(mut guard) = latest_interim_text.lock() {
            let composed = compose_live_display_text(&rough_text, &corrected_display_sentences);
            if !composed.trim().is_empty() {
                *guard = composed;
            } else {
                *guard = rough_text.clone();
            }
        }
    }

    match tokio::time::timeout(std::time::Duration::from_millis(800), child.wait()).await {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => tracing::debug!("doubao bridge wait error: {e}"),
        Err(_) => {
            let _ = child.kill().await;
        }
    }

    if bridge_failed && is_valid_session() {
        tracing::warn!("Doubao realtime bridge ended unexpectedly, fallback to chunked interim mode");
        return false;
    }

    true
}

fn convert_pcm_chunk_to_16k_mono_s16le(input: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    if input.len() < 2 || sample_rate == 0 || channels == 0 {
        return Vec::new();
    }

    let mut pcm = Vec::with_capacity(input.len() / 2);
    for bytes in input.chunks_exact(2) {
        pcm.push(i16::from_le_bytes([bytes[0], bytes[1]]));
    }
    if pcm.is_empty() {
        return Vec::new();
    }

    let channels = channels as usize;
    let mut mono = if channels == 1 {
        pcm
    } else {
        let frames = pcm.len() / channels;
        if frames == 0 {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(frames);
        for frame in 0..frames {
            let base = frame * channels;
            let mut acc = 0i32;
            for c in 0..channels {
                acc += pcm[base + c] as i32;
            }
            out.push((acc / channels as i32) as i16);
        }
        out
    };

    if sample_rate != 16_000 {
        let src_len = mono.len();
        if src_len == 0 {
            return Vec::new();
        }
        let dst_len = ((src_len as u64 * 16_000 + sample_rate as u64 - 1) / sample_rate as u64)
            as usize;
        if dst_len == 0 {
            return Vec::new();
        }

        let ratio = sample_rate as f64 / 16_000f64;
        let mut resampled = Vec::with_capacity(dst_len);
        for i in 0..dst_len {
            let src_pos = i as f64 * ratio;
            let idx = src_pos.floor() as usize;
            let frac = src_pos - idx as f64;
            let a = mono[idx.min(src_len - 1)] as f64;
            let b = mono[(idx + 1).min(src_len - 1)] as f64;
            let mixed = a + (b - a) * frac;
            resampled.push(mixed.clamp(i16::MIN as f64, i16::MAX as f64).round() as i16);
        }
        mono = resampled;
    }

    let mut out = Vec::with_capacity(mono.len() * 2);
    for sample in mono {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn merge_incremental_asr_text(existing: &str, latest_window: &str) -> String {
    let existing_compacted = compact_repeated_sentences(existing.trim());
    let latest_compacted = compact_repeated_sentences(latest_window.trim());
    let existing = existing_compacted.trim();
    let latest = latest_compacted.trim();

    if existing.is_empty() {
        return latest.to_string();
    }
    if latest.is_empty() {
        return existing.to_string();
    }
    if existing == latest || existing.ends_with(latest) {
        return existing.to_string();
    }
    if latest.starts_with(existing) {
        return latest.to_string();
    }

    let existing_norm = normalized_alnum_chars_with_offsets(existing);
    let latest_norm = normalized_alnum_chars_with_offsets(latest);
    if !existing_norm.is_empty() && !latest_norm.is_empty() {
        let existing_norm_chars: Vec<char> = existing_norm.iter().map(|(ch, _)| *ch).collect();
        let latest_norm_chars: Vec<char> = latest_norm.iter().map(|(ch, _)| *ch).collect();
        let existing_norm_text: String = existing_norm_chars.iter().collect();
        let latest_norm_text: String = latest_norm_chars.iter().collect();

        if existing_norm_chars == latest_norm_chars
            || ends_with_chars(&existing_norm_chars, &latest_norm_chars)
        {
            return existing.to_string();
        }
        if starts_with_chars(&latest_norm_chars, &existing_norm_chars) {
            return latest.to_string();
        }
        if same_or_similar_norm(&existing_norm_text, &latest_norm_text) {
            return if latest_norm_chars.len() >= existing_norm_chars.len() {
                latest.to_string()
            } else {
                existing.to_string()
            };
        }

        let normalized_overlap =
            longest_suffix_prefix_overlap(&existing_norm_chars, &latest_norm_chars);
        if normalized_overlap > 0 {
            let cut_byte = latest_norm[normalized_overlap - 1].1;
            let latest_tail = latest[cut_byte..].trim_start();
            if latest_tail.is_empty() {
                return existing.to_string();
            }

            let mut merged = existing.to_string();
            if should_insert_space_between(existing, latest_tail) {
                merged.push(' ');
            }
            merged.push_str(latest_tail);
            return merged;
        }
    }

    let existing_chars: Vec<char> = existing.chars().collect();
    let latest_chars: Vec<char> = latest.chars().collect();
    let overlap = longest_suffix_prefix_overlap(&existing_chars, &latest_chars);

    if overlap > 0 {
        let mut merged: String = existing_chars[..existing_chars.len() - overlap]
            .iter()
            .collect();
        merged.push_str(latest);
        return merged;
    }

    if same_or_similar_norm(existing, latest) {
        return if latest_chars.len() >= existing_chars.len() {
            latest.to_string()
        } else {
            existing.to_string()
        };
    }

    if existing_chars.len() > 40 && latest_chars.len() > 40 {
        tracing::warn!(
            existing_chars = existing_chars.len(),
            latest_chars = latest_chars.len(),
            "Doubao merge fallback append without overlap"
        );
    }

    let stable_prefix = stable_prefix_for_incremental_merge(existing);
    if !stable_prefix.is_empty() {
        return compact_repeated_sentences(&combine_prefix_tail(&stable_prefix, latest));
    }

    if latest_chars.len() >= existing_chars.len() {
        latest.to_string()
    } else {
        existing.to_string()
    }
}

fn normalized_alnum_chars_with_offsets(text: &str) -> Vec<(char, usize)> {
    let mut out = Vec::new();
    for (idx, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            out.push((ch.to_ascii_lowercase(), idx + ch.len_utf8()));
        }
    }
    out
}

fn starts_with_chars(haystack: &[char], needle: &[char]) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()] == *needle
}

fn ends_with_chars(haystack: &[char], needle: &[char]) -> bool {
    haystack.len() >= needle.len() && haystack[haystack.len() - needle.len()..] == *needle
}

fn stable_prefix_for_incremental_merge(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut boundary_end = 0usize;
    for (idx, ch) in trimmed.char_indices() {
        if is_compaction_boundary(ch) {
            boundary_end = idx + ch.len_utf8();
        }
    }

    if boundary_end > 0 && boundary_end < trimmed.len() {
        return trimmed[..boundary_end].trim_end().to_string();
    }

    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() <= 48 {
        return String::new();
    }

    chars[..chars.len() - 48]
        .iter()
        .collect::<String>()
        .trim_end()
        .to_string()
}


fn longest_suffix_prefix_overlap(left: &[char], right: &[char]) -> usize {
    let max = left.len().min(right.len());
    for overlap in (1..=max).rev() {
        let left_suffix = &left[left.len() - overlap..];
        let right_prefix = &right[..overlap];
        if left_suffix == right_prefix {
            return overlap;
        }
        
        let dist = levenshtein_distance(left_suffix, right_prefix);
        let allowed_errors = if overlap >= 10 {
            overlap / 4
        } else if overlap >= 4 {
            1
        } else {
            0
        };
        
        if dist <= allowed_errors {
            return overlap;
        }
    }
    0
}

#[cfg(test)]
fn split_text_tail_chars(text: &str, tail_chars: usize) -> (String, String) {
    if tail_chars == 0 {
        return (text.to_string(), String::new());
    }

    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= tail_chars {
        return (String::new(), text.to_string());
    }

    let split = chars.len() - tail_chars;
    let prefix: String = chars[..split].iter().collect();
    let tail: String = chars[split..].iter().collect();
    (prefix, tail)
}

fn combine_prefix_tail(prefix: &str, processed_tail: &str) -> String {
    let prefix = prefix.trim_end();
    let tail = processed_tail.trim_start();

    if prefix.is_empty() {
        return tail.to_string();
    }

    if tail.is_empty() {
        return prefix.to_string();
    }

    if tail.starts_with(prefix) {
        return tail.to_string();
    }
    if prefix.ends_with(tail) {
        return prefix.to_string();
    }

    let prefix_chars: Vec<char> = prefix.chars().collect();
    let tail_chars: Vec<char> = tail.chars().collect();
    let overlap = longest_suffix_prefix_overlap(&prefix_chars, &tail_chars);

    if overlap > 0 {
        let mut out = prefix.to_string();
        for ch in tail_chars.iter().skip(overlap) {
            out.push(*ch);
        }
        return out;
    }

    let mut out = prefix.to_string();
    if should_insert_space_between(prefix, tail) {
        out.push(' ');
    }
    out.push_str(tail);
    out
}

fn should_insert_space_between(left: &str, right: &str) -> bool {
    let Some(l) = left.chars().last() else {
        return false;
    };
    let Some(r) = right.chars().next() else {
        return false;
    };
    l.is_ascii_alphanumeric() && r.is_ascii_alphanumeric()
}

fn is_live_llm_boundary(ch: char) -> bool {
    matches!(
        ch,
        '。' | '！' | '？' | '；' | '.' | '!' | '?' | ';' | '\n'
    )
}

fn is_compaction_boundary(ch: char) -> bool {
    matches!(
        ch,
        '。' | '！' | '？' | '；' | '，' | '、' | ',' | ';' | ':' | '：' | '.' | '!' | '?'
            | '\n'
    )
}

#[cfg(test)]
fn stable_live_prefix(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let mut boundary_end = None;
    for (idx, ch) in trimmed.char_indices() {
        if is_live_llm_boundary(ch) {
            boundary_end = Some(idx + ch.len_utf8());
        }
    }

    boundary_end
        .map(|end| trimmed[..end].to_string())
        .unwrap_or_default()
}

fn push_live_sentence_segment(segments: &mut Vec<String>, segment: String) {
    let norm = normalize_sentence(&segment);
    if norm.is_empty() {
        segments.push(segment);
        return;
    }

    if let Some(prev) = segments.last_mut() {
        let prev_norm = normalize_sentence(prev);
        if same_or_similar_norm(&prev_norm, &norm) || prev_norm.starts_with(&norm) {
            return;
        }
        if norm.starts_with(&prev_norm) || similar_sentence_score(&prev_norm, &norm) >= 0.82 {
            *prev = segment;
            return;
        }
    }

    segments.push(segment);
}

fn split_live_completed_sentences(text: &str) -> (Vec<String>, String) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return (Vec::new(), String::new());
    }

    let mut completed = Vec::new();
    let mut start = 0usize;
    let mut last_complete_end = 0usize;
    for (idx, ch) in trimmed.char_indices() {
        if is_live_llm_boundary(ch) {
            let end = idx + ch.len_utf8();
            let segment = trimmed[start..end].trim();
            if !segment.is_empty() {
                push_live_sentence_segment(&mut completed, segment.to_string());
            }
            start = end;
            last_complete_end = end;
        }
    }

    let tail = trimmed[last_complete_end..].trim().to_string();
    (completed, tail)
}

fn same_live_sentence(left: &str, right: &str) -> bool {
    let left_norm = normalize_sentence(left);
    let right_norm = normalize_sentence(right);
    if left_norm.is_empty() || right_norm.is_empty() {
        return left.trim() == right.trim();
    }

    same_or_similar_norm(&left_norm, &right_norm)
}

fn common_live_sentence_prefix_len(existing: &[String], next: &[String]) -> usize {
    existing
        .iter()
        .zip(next.iter())
        .take_while(|(left, right)| same_live_sentence(left, right))
        .count()
}

fn compose_live_display_text(
    rough_text: &str,
    corrected_display_sentences: &[String],
) -> String {
    let (completed_source_sentences, rough_tail) = split_live_completed_sentences(rough_text);
    if completed_source_sentences.is_empty() && rough_tail.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    for (idx, source_sentence) in completed_source_sentences.iter().enumerate() {
        let sentence = corrected_display_sentences
            .get(idx)
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.as_str())
            .unwrap_or(source_sentence.as_str());
        out = combine_prefix_tail(&out, sentence);
    }

    out = combine_prefix_tail(&out, &rough_tail);
    compact_repeated_sentences(&out)
}

fn normalize_sentence(sentence: &str) -> String {
    sentence
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(|ch| ch.to_lowercase())
        .collect()
}

fn common_prefix_chars(left: &[char], right: &[char]) -> usize {
    left.iter()
        .zip(right.iter())
        .take_while(|(l, r)| l == r)
        .count()
}

fn levenshtein_distance(left: &[char], right: &[char]) -> usize {
    if left.is_empty() {
        return right.len();
    }
    if right.is_empty() {
        return left.len();
    }

    let mut prev: Vec<usize> = (0..=right.len()).collect();
    let mut curr = vec![0usize; right.len() + 1];

    for (i, left_ch) in left.iter().enumerate() {
        curr[0] = i + 1;
        for (j, right_ch) in right.iter().enumerate() {
            let cost = usize::from(left_ch != right_ch);
            curr[j + 1] = (prev[j + 1] + 1)
                .min(curr[j] + 1)
                .min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[right.len()]
}

fn similar_sentence_score(left_norm: &str, right_norm: &str) -> f32 {
    if left_norm.is_empty() || right_norm.is_empty() {
        return 0.0;
    }
    if left_norm == right_norm {
        return 1.0;
    }
    if left_norm.contains(right_norm) || right_norm.contains(left_norm) {
        let min_len = left_norm.chars().count().min(right_norm.chars().count()) as f32;
        let max_len = left_norm.chars().count().max(right_norm.chars().count()) as f32;
        return min_len / max_len;
    }

    let left_chars: Vec<char> = left_norm.chars().collect();
    let right_chars: Vec<char> = right_norm.chars().collect();
    let prefix = common_prefix_chars(&left_chars, &right_chars) as f32;
    let max_len = left_chars.len().max(right_chars.len()) as f32;
    let prefix_ratio = prefix / max_len;
    let distance = levenshtein_distance(&left_chars, &right_chars) as f32;
    let edit_ratio = 1.0 - distance / max_len;
    prefix_ratio.max(edit_ratio)
}

fn same_or_similar_norm(left_norm: &str, right_norm: &str) -> bool {
    left_norm == right_norm
        || left_norm.starts_with(right_norm)
        || right_norm.starts_with(left_norm)
        || (left_norm.chars().count().min(right_norm.chars().count()) >= 8
            && similar_sentence_score(left_norm, right_norm) >= 0.82)
}

fn sentence_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut start = 0usize;
    for (idx, ch) in text.char_indices() {
        if is_compaction_boundary(ch) {
            let end = idx + ch.len_utf8();
            let segment = text[start..end].trim();
            if !segment.is_empty() {
                segments.push(segment.to_string());
            }
            start = end;
        }
    }

    let tail = text[start..].trim();
    if !tail.is_empty() {
        segments.push(tail.to_string());
    }
    segments
}

fn collapse_repeated_tail_fuzzy(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let total = chars.len();
    if total < 24 {
        return text.to_string();
    }

    let max_block = 120.min(total / 2);
    for block in (12..=max_block).rev() {
        let left: String = chars[total - block * 2..total - block].iter().collect();
        let right: String = chars[total - block..].iter().collect();
        let left_norm = normalize_sentence(&left);
        let right_norm = normalize_sentence(&right);
        if left_norm.is_empty() || right_norm.is_empty() {
            continue;
        }
        if same_or_similar_norm(&left_norm, &right_norm) {
            let mut out: String = chars[..total - block * 2].iter().collect();
            out.push_str(right.trim_start());
            return out;
        }
    }
    text.to_string()
}

fn compact_repeated_sentences(text: &str) -> String {
    let mut working = text.trim().to_string();
    for _ in 0..4 {
        let collapsed = collapse_repeated_tail_fuzzy(&working);
        if collapsed == working {
            break;
        }
        working = collapsed;
    }

    let mut kept: Vec<String> = Vec::new();
    for segment in sentence_segments(&working) {
        let norm = normalize_sentence(&segment);
        if norm.is_empty() {
            kept.push(segment);
            continue;
        }

        let mut handled = false;
        let start = kept.len().saturating_sub(12);
        for idx in (start..kept.len()).rev() {
            let prev_norm = normalize_sentence(&kept[idx]);
            if prev_norm.is_empty() {
                continue;
            }

            if norm == prev_norm {
                handled = true;
                break;
            }
            if same_or_similar_norm(&norm, &prev_norm) {
                if norm.len() >= prev_norm.len() {
                    kept[idx] = segment.clone();
                }
                handled = true;
                break;
            }
            if norm.starts_with(&prev_norm) {
                kept[idx] = segment.clone();
                handled = true;
                break;
            }
            if prev_norm.starts_with(&norm) {
                handled = true;
                break;
            }
        }

        if !handled {
            kept.push(segment);
        }
    }

    let joined = kept.join("");
    collapse_repeated_tail_fuzzy(&joined)
}

fn maybe_emit_live_display(
    app: &tauri::AppHandle,
    latest_interim_text: &Arc<std::sync::Mutex<String>>,
    displayed_text: &mut String,
    rough_text: &str,
    corrected_display_sentences: &[String],
    can_emit: bool,
) {
    if !can_emit {
        return;
    }

    let next = compose_live_display_text(rough_text, corrected_display_sentences);
    if *displayed_text != next {
        *displayed_text = next.clone();
        set_interim_with_cache(app, latest_interim_text, &next);
    }
}

fn maybe_schedule_live_postprocess(
    llm_task: &mut Option<tokio::task::JoinHandle<(usize, String, Option<String>)>>,
    postprocess_spec: &Option<PostprocessSpec>,
    completed_source_sentences: &[String],
    corrected_display_sentences: &[String],
    last_llm_index: &mut Option<usize>,
    last_llm_target: &mut String,
    app: &tauri::AppHandle,
    recorder: &Arc<Recorder>,
    generation: &Arc<AtomicU64>,
    gen_val: u64,
    stable_revision: &Arc<AtomicU64>,
) {
    if llm_task.is_some() || postprocess_spec.is_none() {
        return;
    }

    let Some((target_index, target_sentence)) = completed_source_sentences
        .iter()
        .enumerate()
        .find(|(idx, sentence)| {
            !sentence.trim().is_empty() && corrected_display_sentences.get(*idx).is_none()
        })
    else {
        return;
    };

    if Some(target_index) == *last_llm_index && target_sentence == last_llm_target {
        return;
    }

    let app_handle = app.clone();
    let recorder_ref = Arc::clone(recorder);
    let generation_ref = Arc::clone(generation);
    let revision_ref = Arc::clone(stable_revision);
    let expected_revision = stable_revision.load(Ordering::SeqCst);
    let target = target_sentence.to_string();
    let spec = postprocess_spec.clone();
    *llm_task = Some(tokio::spawn(async move {
        let processed = postprocess_asr_text_streaming_live(
            &app_handle,
            target.clone(),
            spec,
            generation_ref.as_ref(),
            gen_val,
            recorder_ref.as_ref(),
            Some((revision_ref.as_ref(), expected_revision)),
        )
        .await;
        (target_index, target, processed)
    }));
    *last_llm_index = Some(target_index);
    *last_llm_target = target_sentence.to_string();
}

fn build_postprocess_spec(config: &notype_config::AppConfig) -> Option<PostprocessSpec> {
    if !config.model.enable_doubao_postprocess {
        return None;
    }

    let choose_qwen = || {
        if config.model.qwen_api_key.is_empty() {
            return None;
        }
        let model = if config.model.model_name.starts_with("qwen") {
            config.model.model_name.clone()
        } else {
            DEFAULT_POSTPROCESS_QWEN_MODEL.to_string()
        };
        Some(PostprocessSpec {
            provider: notype_llm::Provider::Qwen,
            api_key: config.model.qwen_api_key.clone(),
            model_name: model,
        })
    };

    let choose_gemini = || {
        if config.model.gemini_api_key.is_empty() {
            return None;
        }
        let model = if config.model.model_name.starts_with("gemini") {
            config.model.model_name.clone()
        } else {
            DEFAULT_POSTPROCESS_GEMINI_MODEL.to_string()
        };
        Some(PostprocessSpec {
            provider: notype_llm::Provider::Gemini,
            api_key: config.model.gemini_api_key.clone(),
            model_name: model,
        })
    };

    match config.model.doubao_postprocess_provider {
        notype_config::DoubaoPostprocessProvider::Auto
        | notype_config::DoubaoPostprocessProvider::Qwen => choose_qwen().or_else(choose_gemini),
        notype_config::DoubaoPostprocessProvider::Gemini => {
            choose_gemini().or_else(choose_qwen)
        }
    }
}

#[allow(dead_code)]
async fn postprocess_asr_text_streaming(
    app: &tauri::AppHandle,
    raw_text: String,
    spec: Option<PostprocessSpec>,
) -> String {
    let Some(spec) = spec else {
        tracing::debug!("Doubao ASR post-process skipped");
        return raw_text;
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let postprocess_future = notype_llm::postprocess_text_stream(
        spec.provider,
        spec.api_key,
        Some(spec.model_name),
        POSTPROCESS_SYSTEM_PROMPT.to_string(),
        raw_text.clone(),
        tx,
    );
    tokio::pin!(postprocess_future);

    let mut streamed = String::new();

    loop {
        tokio::select! {
            result = &mut postprocess_future => {
                while let Ok(chunk) = rx.try_recv() {
                    streamed.push_str(&chunk);
                }

                if !streamed.is_empty() {
                    bubble::set_interim(app, &streamed);
                }

                match result {
                    Ok(processed) if !processed.text.trim().is_empty() => {
                        tracing::info!(chars = processed.text.len(), "LLM post-process completed");
                        return processed.text;
                    }
                    Ok(_) => {
                        if !streamed.trim().is_empty() {
                            return streamed;
                        }
                        tracing::warn!("LLM post-process returned empty text; fallback to raw ASR");
                        return raw_text;
                    }
                    Err(e) => {
                        tracing::warn!("LLM post-process failed, fallback to raw ASR: {e}");
                        if !streamed.trim().is_empty() {
                            return streamed;
                        }
                        return raw_text;
                    }
                }
            }
            maybe_chunk = rx.recv() => {
                match maybe_chunk {
                    Some(chunk) => {
                        streamed.push_str(&chunk);
                        bubble::set_interim(app, &streamed);
                    }
                    None => {
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
    }
}

async fn postprocess_asr_text_streaming_live(
    _app: &tauri::AppHandle,
    raw_text: String,
    spec: Option<PostprocessSpec>,
    generation: &AtomicU64,
    gen_val: u64,
    recorder: &Recorder,
    revision: Option<(&AtomicU64, u64)>,
) -> Option<String> {
    let Some(spec) = spec else {
        return Some(raw_text);
    };

    let is_valid_session = || {
        let session_ok = generation.load(Ordering::SeqCst) == gen_val && recorder.is_recording();
        let revision_ok = revision
            .as_ref()
            .map(|(counter, expected)| counter.load(Ordering::SeqCst) == *expected)
            .unwrap_or(true);
        session_ok && revision_ok
    };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let postprocess_future = notype_llm::postprocess_text_stream(
        spec.provider,
        spec.api_key,
        Some(spec.model_name),
        POSTPROCESS_SYSTEM_PROMPT.to_string(),
        raw_text.clone(),
        tx,
    );
    tokio::pin!(postprocess_future);

    let mut streamed = String::new();

    loop {
        tokio::select! {
            result = &mut postprocess_future => {
                if !is_valid_session() {
                    return None;
                }

                while let Ok(chunk) = rx.try_recv() {
                    streamed.push_str(&chunk);
                }

                return match result {
                    Ok(processed) if !processed.text.trim().is_empty() => Some(processed.text),
                    Ok(_) => {
                        if !streamed.trim().is_empty() {
                            Some(streamed)
                        } else {
                            Some(raw_text)
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Live LLM post-process failed, fallback to ASR interim: {e}");
                        if !streamed.trim().is_empty() {
                            Some(streamed)
                        } else {
                            Some(raw_text)
                        }
                    }
                };
            }
            maybe_chunk = rx.recv() => {
                if !is_valid_session() {
                    return None;
                }

                match maybe_chunk {
                    Some(chunk) => {
                        streamed.push_str(&chunk);
                    }
                    None => {
                        tokio::task::yield_now().await;
                    }
                }
            }
        }
    }
}

// -- Audio Processing Pipeline --

async fn emit_final_text(
    app: &tauri::AppHandle,
    inputter: &TextInputter,
    generation: &AtomicU64,
    gen_at_start: u64,
    text: &str,
) {
    bubble::set_result(app, text);
    emit_status(app, "Done", Some(text));
    if let Err(e) = inputter.type_text(text) {
        tracing::error!("Failed to type text: {e}");
        emit_status(app, "Error", Some(&e.to_string()));
    }
    bubble::enable_result_interaction(app);
    let display_secs = (5 + text.len() as u64 / 50).min(15);
    auto_hide_bubble(app, generation, gen_at_start, display_secs).await;
    emit_status(app, "Ready", None);
}

async fn process_audio(
    app: &tauri::AppHandle,
    recognizer: &tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>,
    config: &tokio::sync::RwLock<notype_config::AppConfig>,
    inputter: &TextInputter,
    gateway_process: Arc<tokio::sync::Mutex<Option<tokio::process::Child>>>,
    latest_interim_text: &std::sync::Mutex<String>,
    generation: &AtomicU64,
    audio: notype_audio::AudioData,
) {
    let gen_at_start = generation.load(Ordering::SeqCst);
    let (
        system_prompt,
        active_provider,
        base_url,
        credential_path,
        gateway_api_key,
        should_manage_gateway,
        using_doubao_ws_realtime,
    ) = {
        let cfg = config.read().await;
        (
            cfg.prompts.compose(),
            cfg.model.provider.clone(),
            cfg.model.doubao_base_url.clone(),
            cfg.model.doubao_ime_credential_path.clone(),
            cfg.model.doubao_api_key.clone(),
            should_manage_local_doubao_gateway(&cfg),
            should_use_doubao_ws_realtime(&cfg),
        )
    };
    let is_doubao_provider = matches!(active_provider, notype_config::Provider::Doubao);
    let mut latest_preview = read_cached_interim_text(latest_interim_text);

    // WS realtime mode should finalize from cached preview, not a second ASR request.
    // Wait for WS tail flush first, then read preview again.
    if is_doubao_provider && using_doubao_ws_realtime {
        let ws_idle = wait_for_doubao_ws_idle(std::time::Duration::from_millis(4500)).await;
        latest_preview = wait_for_nonempty_interim_text(
            latest_interim_text,
            std::time::Duration::from_millis(1500),
        )
        .await;
        if !ws_idle && latest_preview.is_empty() {
            tracing::warn!(
                "Doubao WS still active and preview empty after finalize wait; skip fallback ASR request"
            );
            bubble::hide_bubble(app);
            emit_status(app, "Ready", None);
            return;
        }
        if ws_idle && latest_preview.is_empty() {
            tracing::debug!(
                "Doubao WS settled with empty preview; wait briefly before fallback final ASR"
            );
            tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
            latest_preview = wait_for_nonempty_interim_text(
                latest_interim_text,
                std::time::Duration::from_millis(600),
            )
            .await;
        }
    }

    if is_doubao_provider && !latest_preview.is_empty() {
        tracing::info!(
            chars = latest_preview.chars().count(),
            "Finalize Doubao transcription from live preview"
        );
        emit_final_text(app, inputter, generation, gen_at_start, &latest_preview).await;
        return;
    }

    let guard = recognizer.read().await;
    let Some(rec) = guard.as_ref() else {
        tracing::warn!("No recognizer configured");
        bubble::set_error(app, "No recognizer credentials configured");
        emit_status(app, "Error", Some("No recognizer credentials configured"));
        auto_hide_bubble(app, generation, gen_at_start, 3).await;
        return;
    };

    if should_manage_gateway {
        if let Err(e) = ensure_local_doubao_gateway_running(
            gateway_process,
            base_url,
            credential_path,
            gateway_api_key,
        )
        .await
        {
            tracing::warn!("Failed to ensure local doubao-asr2api gateway before final recognition: {e}");
        }
    }

    // Use streaming to get SSE chunks, but display final result directly
    // (interim preview already shown during recording)
    let mut attempts = 0usize;
    let result = loop {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let run_result = if is_doubao_provider {
            let _serial_guard = doubao_asr_serial_mutex().lock().await;
            let out = rec
                .recognize_stream(audio.wav_bytes.clone(), "audio/wav".into(), system_prompt.clone(), tx)
                .await;
            drop(_serial_guard);
            out
        } else {
            rec.recognize_stream(audio.wav_bytes.clone(), "audio/wav".into(), system_prompt.clone(), tx)
                .await
        };

        if is_doubao_provider {
            if let Err(err) = &run_result {
                let message = err.to_string();
                if is_doubao_concurrency_quota_error(&message) && attempts < DOUBAO_QUOTA_RETRY_ATTEMPTS {
                    attempts += 1;
                    let delay = std::time::Duration::from_millis(
                        DOUBAO_QUOTA_RETRY_BASE_DELAY_MS * attempts as u64,
                    );
                    tracing::warn!(
                        attempt = attempts,
                        delay_ms = delay.as_millis(),
                        "Doubao final recognition hit ExceededConcurrentQuota, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }
        }

        break run_result;
    };

    match result {
        Ok(result) if result.text.is_empty() => {
            tracing::info!("Empty transcription (silence?)");
            bubble::hide_bubble(app);
            emit_status(app, "Ready", None);
        }
        Ok(result) => {
            tracing::info!(text = %result.text, "ASR transcription received");
            let final_text = result.text;

            if final_text.trim().is_empty() {
                tracing::info!("Empty final text after post-process");
                bubble::hide_bubble(app);
                emit_status(app, "Ready", None);
                return;
            }

            emit_final_text(app, inputter, generation, gen_at_start, &final_text).await;
        }
        Err(e) => {
            let error_msg = e.to_string();
            if is_doubao_concurrency_quota_error(&error_msg) {
                tracing::warn!("Suppressing Doubao concurrency quota error in finalize path");
                bubble::hide_bubble(app);
                emit_status(app, "Ready", None);
                return;
            }

            if is_doubao_provider {
                let mut fallback_text = read_cached_interim_text(latest_interim_text);
                if fallback_text.is_empty() {
                    fallback_text = wait_for_nonempty_interim_text(
                        latest_interim_text,
                        std::time::Duration::from_millis(1600),
                    )
                    .await;
                }

                if !fallback_text.is_empty() {
                    tracing::warn!(
                        error = %e,
                        chars = fallback_text.chars().count(),
                        "Final Doubao recognition failed, fallback to latest interim text"
                    );
                    if !fallback_text.trim().is_empty() {
                        emit_final_text(app, inputter, generation, gen_at_start, &fallback_text)
                            .await;
                        return;
                    }
                }
            }

            tracing::error!("Recognition failed: {e}");
            bubble::set_error(app, &e.to_string());
            emit_status(app, "Error", Some(&e.to_string()));
            auto_hide_bubble(app, generation, gen_at_start, 5).await;
        }
    }
}

/// Hide bubble after a delay, but only if no new recording has started since.
async fn auto_hide_bubble(
    app: &tauri::AppHandle,
    generation: &AtomicU64,
    expected_gen: u64,
    secs: u64,
) {
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    if generation.load(Ordering::SeqCst) == expected_gen {
        bubble::hide_bubble(app);
    }
}

// -- Helpers --

fn build_recognizer(config: &notype_config::AppConfig) -> Option<Box<dyn VoiceRecognizer>> {
    if !config.model.has_required_credentials() {
        tracing::warn!(
            provider = ?config.model.provider,
            model = %config.model.model_name,
            "Missing required credentials for current provider, recognizer disabled"
        );
        return None;
    }

    let provider = match config.model.provider {
        notype_config::Provider::Gemini => notype_llm::Provider::Gemini,
        notype_config::Provider::Qwen => notype_llm::Provider::Qwen,
        notype_config::Provider::Doubao => notype_llm::Provider::Doubao,
    };

    Some(notype_llm::create_recognizer(
        provider,
        config.model.active_api_key().to_string(),
        notype_llm::RecognizerOptions {
            model: Some(config.model.model_name.clone()),
            doubao_base_url: Some(config.model.doubao_base_url.clone()),
            doubao_official_app_key: if config.model.doubao_official_app_key.is_empty() {
                None
            } else {
                Some(config.model.doubao_official_app_key.clone())
            },
            doubao_official_access_key: if config.model.doubao_official_access_key.is_empty() {
                None
            } else {
                Some(config.model.doubao_official_access_key.clone())
            },
        },
    ))
}

#[cfg(desktop)]
fn reregister_shortcut(
    app: &tauri::AppHandle,
    hotkey_str: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let shortcut = parse_hotkey(hotkey_str)?;
    // Unregister all, then re-register the new one
    app.global_shortcut().unregister_all()?;
    app.global_shortcut().register(shortcut)?;
    Ok(())
}

#[cfg(not(desktop))]
fn reregister_shortcut(
    _app: &tauri::AppHandle,
    _hotkey_str: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

#[cfg(desktop)]
fn parse_hotkey(
    s: &str,
) -> std::result::Result<tauri_plugin_global_shortcut::Shortcut, Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::{Modifiers, Shortcut};

    let mut mods = Modifiers::empty();
    let mut code = None;

    for part in s.split('+') {
        let p = part.trim();
        match p.to_lowercase().as_str() {
            "ctrl" | "control" => mods |= Modifiers::CONTROL,
            "shift" => mods |= Modifiers::SHIFT,
            "alt" | "option" => mods |= Modifiers::ALT,
            "meta" | "cmd" | "command" | "super" => mods |= Modifiers::META,
            _ => {
                code = Some(key_to_code(p)?);
            }
        }
    }

    let c = code.ok_or("No key specified in hotkey")?;
    let m = if mods.is_empty() { None } else { Some(mods) };
    Ok(Shortcut::new(m, c))
}

#[cfg(desktop)]
fn key_to_code(key: &str) -> std::result::Result<tauri_plugin_global_shortcut::Code, String> {
    use tauri_plugin_global_shortcut::Code;

    match key.to_lowercase().as_str() {
        "." | "period" => Ok(Code::Period),
        "," | "comma" => Ok(Code::Comma),
        "/" | "slash" => Ok(Code::Slash),
        ";" | "semicolon" => Ok(Code::Semicolon),
        "'" | "quote" => Ok(Code::Quote),
        "[" | "bracketleft" => Ok(Code::BracketLeft),
        "]" | "bracketright" => Ok(Code::BracketRight),
        "`" | "backquote" => Ok(Code::Backquote),
        "-" | "minus" => Ok(Code::Minus),
        "=" | "equal" => Ok(Code::Equal),
        "\\" | "backslash" => Ok(Code::Backslash),
        "space" | " " => Ok(Code::Space),
        "enter" | "return" => Ok(Code::Enter),
        "tab" => Ok(Code::Tab),
        "escape" | "esc" => Ok(Code::Escape),
        "backspace" => Ok(Code::Backspace),
        "delete" => Ok(Code::Delete),
        "up" | "arrowup" => Ok(Code::ArrowUp),
        "down" | "arrowdown" => Ok(Code::ArrowDown),
        "left" | "arrowleft" => Ok(Code::ArrowLeft),
        "right" | "arrowright" => Ok(Code::ArrowRight),
        "f1" => Ok(Code::F1),
        "f2" => Ok(Code::F2),
        "f3" => Ok(Code::F3),
        "f4" => Ok(Code::F4),
        "f5" => Ok(Code::F5),
        "f6" => Ok(Code::F6),
        "f7" => Ok(Code::F7),
        "f8" => Ok(Code::F8),
        "f9" => Ok(Code::F9),
        "f10" => Ok(Code::F10),
        "f11" => Ok(Code::F11),
        "f12" => Ok(Code::F12),
        s if s.len() == 1 => {
            let ch = s.chars().next().unwrap();
            match ch {
                'a'..='z' => {
                    let variant = format!("Key{}", ch.to_uppercase());
                    match variant.as_str() {
                        "KeyA" => Ok(Code::KeyA),
                        "KeyB" => Ok(Code::KeyB),
                        "KeyC" => Ok(Code::KeyC),
                        "KeyD" => Ok(Code::KeyD),
                        "KeyE" => Ok(Code::KeyE),
                        "KeyF" => Ok(Code::KeyF),
                        "KeyG" => Ok(Code::KeyG),
                        "KeyH" => Ok(Code::KeyH),
                        "KeyI" => Ok(Code::KeyI),
                        "KeyJ" => Ok(Code::KeyJ),
                        "KeyK" => Ok(Code::KeyK),
                        "KeyL" => Ok(Code::KeyL),
                        "KeyM" => Ok(Code::KeyM),
                        "KeyN" => Ok(Code::KeyN),
                        "KeyO" => Ok(Code::KeyO),
                        "KeyP" => Ok(Code::KeyP),
                        "KeyQ" => Ok(Code::KeyQ),
                        "KeyR" => Ok(Code::KeyR),
                        "KeyS" => Ok(Code::KeyS),
                        "KeyT" => Ok(Code::KeyT),
                        "KeyU" => Ok(Code::KeyU),
                        "KeyV" => Ok(Code::KeyV),
                        "KeyW" => Ok(Code::KeyW),
                        "KeyX" => Ok(Code::KeyX),
                        "KeyY" => Ok(Code::KeyY),
                        "KeyZ" => Ok(Code::KeyZ),
                        _ => Err(format!("Unknown key: {key}")),
                    }
                }
                '0'..='9' => match ch {
                    '0' => Ok(Code::Digit0),
                    '1' => Ok(Code::Digit1),
                    '2' => Ok(Code::Digit2),
                    '3' => Ok(Code::Digit3),
                    '4' => Ok(Code::Digit4),
                    '5' => Ok(Code::Digit5),
                    '6' => Ok(Code::Digit6),
                    '7' => Ok(Code::Digit7),
                    '8' => Ok(Code::Digit8),
                    '9' => Ok(Code::Digit9),
                    _ => unreachable!(),
                },
                _ => Err(format!("Unknown key: {key}")),
            }
        }
        _ => Err(format!("Unknown key: {key}")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        combine_prefix_tail, compact_repeated_sentences, compose_live_display_text,
        merge_incremental_asr_text, split_live_completed_sentences, split_text_tail_chars,
        stable_live_prefix,
    };

    #[test]
    fn split_tail_chars_splits_expected_window() {
        let (prefix, tail) = split_text_tail_chars("abcdef", 3);
        assert_eq!(prefix, "abc");
        assert_eq!(tail, "def");
    }

    #[test]
    fn combine_prefix_tail_joins_text() {
        assert_eq!(combine_prefix_tail("hello ", "world"), "hello world");
        assert_eq!(combine_prefix_tail("", "  test"), "test");
    }

    #[test]
    fn combine_prefix_tail_avoids_duplicate_overlap() {
        assert_eq!(
            combine_prefix_tail("输入两个方括号，然后", "然后可以继续输入"),
            "输入两个方括号，然后可以继续输入"
        );
        assert_eq!(
            combine_prefix_tail("今天我们讨论接口", "今天我们讨论接口和缓存"),
            "今天我们讨论接口和缓存"
        );
    }

    #[test]
    fn combine_prefix_tail_does_not_insert_space_for_cjk() {
        assert_eq!(
            combine_prefix_tail("这是中文前缀", "继续输出"),
            "这是中文前缀继续输出"
        );
    }

    #[test]
    fn merge_incremental_text_handles_overlap() {
        let merged = merge_incremental_asr_text("今天我们讨论", "我们讨论一下接口");
        assert_eq!(merged, "今天我们讨论一下接口");
    }

    #[test]
    fn merge_incremental_text_handles_punctuation_drift() {
        let merged = merge_incremental_asr_text(
            "连接出来。用好这个功能，笔记之间的关系图谱",
            "连接出来, 用好这个功能，笔记之间的关系图谱会变得非常复杂",
        );
        assert_eq!(merged, "连接出来,用好这个功能，笔记之间的关系图谱会变得非常复杂");
    }

    #[test]
    fn merge_incremental_text_without_overlap_keeps_stable_prefix() {
        let merged = merge_incremental_asr_text(
            "这是第一句已经稳定。第二句还在讨论缓存和索引策略",
            "现在继续讨论数据库连接池和超时配置",
        );
        assert_eq!(merged, "这是第一句已经稳定。现在继续讨论数据库连接池和超时配置");
    }

    #[test]
    fn stable_live_prefix_stops_at_latest_clause_boundary() {
        assert_eq!(
            stable_live_prefix("粗转录第一句。第二句还没说完"),
            "粗转录第一句。"
        );
        assert_eq!(stable_live_prefix("前半句，后半句继续"), "");
    }

    #[test]
    fn compose_live_display_uses_corrected_prefix_and_raw_tail() {
        let displayed = compose_live_display_text(
            "这是粗转录第一句。第二句还没说完",
            &[String::from("这是修正后的第一句。")],
        );
        assert_eq!(displayed, "这是修正后的第一句。第二句还没说完");
    }

    #[test]
    fn split_live_completed_sentences_compacts_repeated_sentences() {
        let (completed, tail) = split_live_completed_sentences(
            "该怎么去形容你最贴切？该怎么去形容你最贴切？拿什么做比较才算特别",
        );
        assert_eq!(completed, vec!["该怎么去形容你最贴切？"]);
        assert_eq!(tail, "拿什么做比较才算特别");
    }

    #[test]
    fn compact_repeated_sentences_skips_duplicate_questions() {
        let compacted = compact_repeated_sentences(
            "该怎么去形容你最贴切？该怎么去形容你最贴切？拿什么做比较才算特别？拿什么做比较才算特别？",
        );
        assert_eq!(compacted, "该怎么去形容你最贴切？拿什么做比较才算特别？");
    }

    #[test]
    fn compact_repeated_sentences_keeps_longer_extension() {
        let compacted = compact_repeated_sentences(
            "拿什么做比较才算特别？拿什么做比较才算特别对你？",
        );
        assert_eq!(compacted, "拿什么做比较才算特别对你？");
    }
}
