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
async fn get_config(state: tauri::State<'_, AppState>) -> Result<ConfigDto, String> {
    let config = state.config.read().await;
    Ok(ConfigDto::from(&config))
}

#[tauri::command]
async fn save_config(state: tauri::State<'_, AppState>, dto: ConfigDto) -> Result<(), String> {
    let mut config = state.config.write().await;

    config.model.provider = match dto.provider.to_lowercase().as_str() {
        "qwen" => notype_config::Provider::Qwen,
        _ => notype_config::Provider::Gemini,
    };
    config.model.api_key = dto.api_key;
    config.model.model_name = dto.model_name;
    config.general.hotkey = dto.hotkey;

    notype_config::save(&config).map_err(|e| e.to_string())?;

    // Rebuild recognizer with new config
    let new_recognizer = build_recognizer(&config);
    drop(config);

    let mut rec = state.recognizer.write().await;
    *rec = new_recognizer;

    tracing::info!("Config saved and recognizer rebuilt");
    Ok(())
}

/// DTO for frontend config exchange (no secrets leak — api_key is masked on read).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ConfigDto {
    provider: String,
    api_key: String,
    model_name: String,
    hotkey: String,
    has_api_key: bool,
}

impl ConfigDto {
    fn from(config: &notype_config::AppConfig) -> Self {
        let provider = match config.model.provider {
            notype_config::Provider::Gemini => "gemini",
            notype_config::Provider::Qwen => "qwen",
        };
        Self {
            provider: provider.into(),
            api_key: String::new(), // Never send key to frontend
            model_name: config.model.model_name.clone(),
            hotkey: config.general.hotkey.clone(),
            has_api_key: !config.model.api_key.is_empty(),
        }
    }
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
        has_key = !config.model.api_key.is_empty(),
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
            // Hide window instead of closing (keep tray alive)
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
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
                rt.block_on(async { cfg.read().await.model.api_key.is_empty() })
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
    use tauri_plugin_global_shortcut::{
        Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState,
    };

    let shortcut = Shortcut::new(Some(Modifiers::CONTROL), Code::Period);

    app.handle().plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, sc, event| {
                if sc != &shortcut {
                    return;
                }
                let state = app.state::<AppState>();
                let recorder = &state.recorder;

                match event.state() {
                    ShortcutState::Pressed => {
                        if !recorder.is_recording() {
                            if let Err(e) = recorder.start() {
                                tracing::error!("Failed to start recording: {e}");
                                emit_status(app, "Error", Some(&e.to_string()));
                            } else {
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
                                    emit_status(app, "Recognizing", None);
                                    let recognizer = Arc::clone(&state.recognizer);
                                    let inputter = Arc::clone(&state.inputter);
                                    let rt = state.runtime.clone();
                                    let handle = app.clone();
                                    rt.spawn(async move {
                                        process_audio(&handle, &recognizer, &inputter, audio).await;
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
    inputter: &TextInputter,
    audio: notype_audio::AudioData,
) {
    let guard = recognizer.read().await;
    let Some(rec) = guard.as_ref() else {
        tracing::warn!("No recognizer configured");
        emit_status(app, "Error", Some("No API key configured"));
        return;
    };

    match rec.recognize(audio.wav_bytes, "audio/wav".into()).await {
        Ok(result) if result.text.is_empty() => {
            tracing::info!("Empty transcription (silence?)");
            emit_status(app, "Ready", None);
        }
        Ok(result) => {
            tracing::info!(text = %result.text, "Transcription received");
            emit_status(app, "Done", Some(&result.text));
            if let Err(e) = inputter.type_text(&result.text) {
                tracing::error!("Failed to type text: {e}");
                emit_status(app, "Error", Some(&e.to_string()));
                return;
            }
            // Brief delay then back to ready
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            emit_status(app, "Ready", None);
        }
        Err(e) => {
            tracing::error!("LLM recognition failed: {e}");
            emit_status(app, "Error", Some(&e.to_string()));
        }
    }
}

// -- Helpers --

fn build_recognizer(config: &notype_config::AppConfig) -> Option<Box<dyn VoiceRecognizer>> {
    if config.model.api_key.is_empty() {
        tracing::warn!("No API key configured, recognizer disabled");
        return None;
    }

    let provider = match config.model.provider {
        notype_config::Provider::Gemini => notype_llm::Provider::Gemini,
        notype_config::Provider::Qwen => notype_llm::Provider::Qwen,
    };

    Some(notype_llm::create_recognizer(
        provider,
        config.model.api_key.clone(),
        Some(config.model.model_name.clone()),
    ))
}
