mod bubble;
mod tray;

use std::sync::Arc;

use notype_audio::Recorder;
use notype_input::TextInputter;
use notype_llm::VoiceRecognizer;
use tauri::{Emitter, Manager};

struct AppState {
    recorder: Recorder,
    inputter: Arc<TextInputter>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    runtime: tokio::runtime::Handle,
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
        _ => notype_config::Provider::Gemini,
    };
    if !dto.gemini_api_key.is_empty() {
        config.model.gemini_api_key = dto.gemini_api_key;
    }
    if !dto.qwen_api_key.is_empty() {
        config.model.qwen_api_key = dto.qwen_api_key;
    }
    config.model.model_name = dto.model_name;
    config.general.hotkey = dto.hotkey.clone();

    notype_config::save(&config).map_err(|e| e.to_string())?;

    // Rebuild recognizer
    let new_recognizer = build_recognizer(&config);
    drop(config);

    let mut rec = state.recognizer.write().await;
    *rec = new_recognizer;

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
    model_name: String,
    hotkey: String,
    has_gemini_key: bool,
    has_qwen_key: bool,
}

impl ConfigDto {
    fn from(config: &notype_config::AppConfig) -> Self {
        let provider = match config.model.provider {
            notype_config::Provider::Gemini => "gemini",
            notype_config::Provider::Qwen => "qwen",
        };
        Self {
            provider: provider.into(),
            gemini_api_key: String::new(),
            qwen_api_key: String::new(),
            model_name: config.model.model_name.clone(),
            hotkey: config.general.hotkey.clone(),
            has_gemini_key: !config.model.gemini_api_key.is_empty(),
            has_qwen_key: !config.model.qwen_api_key.is_empty(),
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct SaveResult {
    restart_needed: bool,
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
        "Config loaded"
    );

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            recorder: Recorder::new(None),
            inputter: Arc::new(TextInputter::new()),
            recognizer: Arc::new(tokio::sync::RwLock::new(recognizer)),
            config: Arc::new(tokio::sync::RwLock::new(config)),
            runtime: runtime.handle().clone(),
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

            // Show settings on first launch if no API key
            let show_on_start = {
                let config = app.state::<AppState>();
                let rt = config.runtime.clone();
                let cfg = Arc::clone(&config.config);
                rt.block_on(async { cfg.read().await.model.active_api_key().is_empty() })
            };
            if show_on_start {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }

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

                match event.state() {
                    ShortcutState::Pressed => {
                        if !recorder.is_recording() {
                            if let Err(e) = recorder.start() {
                                tracing::error!("Failed to start recording: {e}");
                                emit_status(app, "Error", Some(&e.to_string()));
                            } else {
                                bubble::show_bubble(app);
                                bubble::set_recording(app);
                                emit_status(app, "Recording", None);
                            }
                        }
                    }
                    ShortcutState::Released => {
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
                                    let rt = state.runtime.clone();
                                    let handle = app.clone();
                                    rt.spawn(async move {
                                        process_audio(&handle, &recognizer, &cfg, &inputter, audio)
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

// -- Audio Processing Pipeline --

async fn process_audio(
    app: &tauri::AppHandle,
    recognizer: &tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>,
    config: &tokio::sync::RwLock<notype_config::AppConfig>,
    inputter: &TextInputter,
    audio: notype_audio::AudioData,
) {
    let guard = recognizer.read().await;
    let Some(rec) = guard.as_ref() else {
        tracing::warn!("No recognizer configured");
        bubble::set_error(app, "No API key configured");
        emit_status(app, "Error", Some("No API key configured"));
        auto_hide_bubble(app, 3).await;
        return;
    };

    let system_prompt = config.read().await.prompts.compose();

    match rec
        .recognize(audio.wav_bytes, "audio/wav".into(), system_prompt)
        .await
    {
        Ok(result) if result.text.is_empty() => {
            tracing::info!("Empty transcription (silence?)");
            bubble::hide_bubble(app);
            emit_status(app, "Ready", None);
        }
        Ok(result) => {
            tracing::info!(text = %result.text, "Transcription received");
            bubble::set_result(app, &result.text);
            emit_status(app, "Done", Some(&result.text));
            if let Err(e) = inputter.type_text(&result.text) {
                tracing::error!("Failed to type text: {e}");
                emit_status(app, "Error", Some(&e.to_string()));
            }
            auto_hide_bubble(app, 3).await;
            emit_status(app, "Ready", None);
        }
        Err(e) => {
            tracing::error!("LLM recognition failed: {e}");
            bubble::set_error(app, &e.to_string());
            emit_status(app, "Error", Some(&e.to_string()));
            auto_hide_bubble(app, 3).await;
        }
    }
}

async fn auto_hide_bubble(app: &tauri::AppHandle, secs: u64) {
    tokio::time::sleep(std::time::Duration::from_secs(secs)).await;
    bubble::hide_bubble(app);
}

// -- Helpers --

fn build_recognizer(config: &notype_config::AppConfig) -> Option<Box<dyn VoiceRecognizer>> {
    let api_key = config.model.active_api_key();
    if api_key.is_empty() {
        tracing::warn!(
            provider = ?config.model.provider,
            "No API key for current provider, recognizer disabled"
        );
        return None;
    }

    let provider = match config.model.provider {
        notype_config::Provider::Gemini => notype_llm::Provider::Gemini,
        notype_config::Provider::Qwen => notype_llm::Provider::Qwen,
    };

    Some(notype_llm::create_recognizer(
        provider,
        api_key.to_string(),
        Some(config.model.model_name.clone()),
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
