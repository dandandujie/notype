mod bubble;
mod platform;
mod tray;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use notype_audio::Recorder;
use notype_input::TextInputter;
use notype_llm::VoiceRecognizer;
use tauri::{Emitter, Manager};

struct AppState {
    recorder: Arc<Recorder>,
    inputter: Arc<TextInputter>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    runtime: tokio::runtime::Handle,
    bubble_generation: Arc<AtomicU64>,
    hotkey_down: Arc<AtomicBool>,
    /// True when the active capture session was started from the main window
    /// (dictation mode: result goes to clipboard + window, not typed).
    ui_capture: Arc<AtomicBool>,
    /// True when the active capture is a voice-edit session (selection rewrite).
    edit_session: Arc<AtomicBool>,
    /// Instant of the last hotkey press (for tap-to-latch detection).
    hotkey_pressed_at: Arc<std::sync::Mutex<Option<Instant>>>,
    /// Recording latched on by a quick tap; the next press stops it.
    capture_latched: Arc<AtomicBool>,
    /// Swallow the release event that follows a latch-stopping press.
    swallow_release: Arc<AtomicBool>,
    /// Frontmost app at capture start — powers per-app tone adaptation.
    active_app: Arc<std::sync::Mutex<Option<String>>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    /// Generation whose streaming ASR session has flushed its final text
    /// into `latest_interim_text` (Volcengine live pipeline).
    stream_final_gen: Arc<AtomicU64>,
}

const DEFAULT_POSTPROCESS_QWEN_MODEL: &str = "qwen3.5-omni-flash";
const DEFAULT_POSTPROCESS_GEMINI_MODEL: &str = "gemini-3-flash-preview";
const DEFAULT_POSTPROCESS_MIMO_MODEL: &str = "mimo-v2.5";
const POSTPROCESS_SYSTEM_PROMPT: &str = r#"你是实时语音转写后处理器。
输入是 ASR 粗转写文本，请在不改变原意的前提下做语义和表达修正：
- 修正同音/近音误识别，尤其是技术词汇、人名、品牌、代码术语
- 补全标点与断句，优化段落与可读性
- 清理明显口头语、重复词、改口残留（保持语义）
- 说话人中途改口时（「不对」「我是说」「算了重说」），只保留最终意图
- 保持原语言，不翻译
- 只输出处理后的最终文本，不要解释"#;

const POSTPROCESS_TRANSLATE_PROMPT: &str = r#"你是实时语音转写翻译器。
输入是 ASR 粗转写文本，请把它翻译成地道、自然的英文：
- 去除口水词、重复和改口残留，只翻译最终想表达的内容
- 专有名词、品牌、代码术语保留原写法
- 只输出英文译文，不要任何解释"#;

/// Compose the system prompt for the current session: output style preset +
/// user prompts + (if enabled) the frontmost-app tone context.
fn compose_session_prompt(app: &tauri::AppHandle, cfg: &notype_config::AppConfig) -> String {
    let state = app.state::<AppState>();
    let active = state.active_app.lock().ok().and_then(|g| g.clone());
    let app_ctx: Option<(String, String)> = if cfg.general.enable_app_context {
        active.map(|name| {
            let tone = notype_config::resolve_app_tone(&name, &cfg.general.app_rules);
            (name, tone)
        })
    } else {
        None
    };
    cfg.prompts.compose_for(
        &cfg.general.output_style,
        app_ctx.as_ref().map(|(a, t)| (a.as_str(), t.as_str())),
        cfg.general.structured_output,
    )
}

fn read_cached_interim_text(latest_interim_text: &std::sync::Mutex<String>) -> String {
    latest_interim_text
        .lock()
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

#[derive(Clone)]
struct PostprocessSpec {
    target: notype_llm::TextLlmTarget,
    /// Style-dependent postprocess instruction (polish / translate / …).
    system_prompt: String,
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
        // Main window mirrors the live transcript when dictating from the UI.
        let _ = app.emit("notype://interim", text.to_string());
    }
}

// -- Tauri Commands --

#[tauri::command]
fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[derive(Clone, serde::Serialize)]
struct AudioDeviceDto {
    name: String,
    is_default: bool,
}

#[tauri::command]
fn list_audio_devices() -> Vec<AudioDeviceDto> {
    notype_audio::list_input_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| AudioDeviceDto {
            name: d.name,
            is_default: d.is_default,
        })
        .collect()
}

#[tauri::command]
fn is_recording(state: tauri::State<'_, AppState>) -> bool {
    state.recorder.is_recording()
}

// -- Capture control (shared by hotkey handler and UI button) --

/// Start a capture session. `from_ui = true` means dictation mode started
/// from the main window: the window stays visible, no bubble is shown, and
/// the final text is copied instead of typed. `edit = true` means voice-edit:
/// the user has text selected and is speaking an instruction.
fn start_capture(app: &tauri::AppHandle, from_ui: bool, edit: bool) -> Result<(), String> {
    let state = app.state::<AppState>();
    if state.recorder.is_recording() {
        return Err("已在录音中".into());
    }

    state.bubble_generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut guard) = state.latest_interim_text.lock() {
        guard.clear();
    }
    state.ui_capture.store(from_ui, Ordering::SeqCst);
    state.edit_session.store(edit, Ordering::SeqCst);

    // Capture the target app BEFORE hiding our window — this powers the
    // per-app tone adaptation. UI dictation goes to the clipboard, so the
    // frontmost app (NoType itself) carries no context there.
    let frontmost = if from_ui {
        None
    } else {
        platform::frontmost_app_name().filter(|name| name != "NoType" && name != "notype")
    };
    if let Some(name) = frontmost.as_deref() {
        tracing::info!(app = %name, "Capture context: frontmost app");
    }
    if let Ok(mut guard) = state.active_app.lock() {
        *guard = frontmost;
    }

    if !from_ui {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
    }

    state.recorder.start().map_err(|e| e.to_string())?;

    if !from_ui {
        bubble::hide_bubble(app);
        bubble::show_bubble(app);
        bubble::set_recording(app);
    }
    emit_status(app, "Recording", None);

    // Voice-edit commands are short — no interim preview loop for them.
    let gen_val = state.bubble_generation.load(Ordering::SeqCst);
    if !edit {
        let rec_clone = Arc::clone(&state.recorder);
        let recognizer = Arc::clone(&state.recognizer);
        let cfg = Arc::clone(&state.config);
        let handle = app.clone();
        let gen = Arc::clone(&state.bubble_generation);
        let latest_interim_text = Arc::clone(&state.latest_interim_text);
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

    // Live input-level meter: drives the real waveform in bubble + window.
    let rec_level = Arc::clone(&state.recorder);
    let gen_level = Arc::clone(&state.bubble_generation);
    let handle_level = app.clone();
    state.runtime.spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(80));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if gen_level.load(Ordering::SeqCst) != gen_val || !rec_level.is_recording() {
                break;
            }
            let level = rec_level.input_level();
            let _ = handle_level.emit("notype://level", level);
            bubble::set_level(&handle_level, level);
        }
        let _ = handle_level.emit("notype://level", 0.0f32);
        bubble::set_level(&handle_level, 0.0);
    });

    Ok(())
}

/// Stop the current capture and hand the audio to the recognition pipeline.
fn stop_capture(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    if !state.recorder.is_recording() {
        return Err("当前没有录音".into());
    }

    let audio = state.recorder.stop().map_err(|e| e.to_string())?;
    tracing::info!(
        duration = audio.duration_secs,
        bytes = audio.wav_bytes.len(),
        "Audio captured"
    );

    state.capture_latched.store(false, Ordering::SeqCst);
    let from_ui = state.ui_capture.load(Ordering::SeqCst);
    let edit = state.edit_session.load(Ordering::SeqCst);
    if !from_ui {
        bubble::set_recognizing(app);
    }
    emit_status(app, "Recognizing", None);

    let recognizer = Arc::clone(&state.recognizer);
    let cfg = Arc::clone(&state.config);
    let inputter = Arc::clone(&state.inputter);
    let gen = Arc::clone(&state.bubble_generation);
    let latest_interim_text = Arc::clone(&state.latest_interim_text);
    let stream_final_gen = Arc::clone(&state.stream_final_gen);
    let handle = app.clone();
    state.runtime.spawn(async move {
        if edit {
            process_edit_audio(&handle, &recognizer, &cfg, &inputter, &gen, audio).await;
        } else {
            process_audio(
                &handle,
                &recognizer,
                &cfg,
                &inputter,
                latest_interim_text.as_ref(),
                stream_final_gen.as_ref(),
                &gen,
                audio,
                from_ui,
            )
            .await;
        }
    });

    Ok(())
}

/// Abort the current capture, discarding the audio.
fn cancel_capture(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    // Bump generation first so interim loops and pending finalize tasks bail out.
    state.bubble_generation.fetch_add(1, Ordering::SeqCst);
    if state.recorder.is_recording() {
        let _ = state.recorder.stop();
    }
    if let Ok(mut guard) = state.latest_interim_text.lock() {
        guard.clear();
    }
    state.hotkey_down.store(false, Ordering::SeqCst);
    state.edit_session.store(false, Ordering::SeqCst);
    state.capture_latched.store(false, Ordering::SeqCst);
    bubble::hide_bubble(app);
    emit_status(app, "Ready", None);
    tracing::info!("Capture cancelled by user");
}

// -- Voice edit (Typeless "speak to edit selected text") --

fn edit_system_prompt(selection: &str) -> String {
    format!(
        "你是一个文本编辑引擎。用户选中了一段文本，并通过语音说出编辑指令。\n\
         请严格按照指令处理选中的文本：\n\
         - 修改类指令（改写、缩短、扩写、换语气、翻译、改格式等）：输出修改后的完整文本\n\
         - 提问类指令（总结、解释这段话等）：输出针对该文本的回答\n\
         - 只输出结果文本本身，不要任何解释、前缀或引号包裹\n\
         - 除非指令要求翻译，否则保持选中文本的原语言\n\n\
         ## 选中的文本\n{selection}"
    )
}

/// Pull the user's current selection via a synthetic copy chord.
/// Called after the hotkey is released so held modifiers can't corrupt the chord.
async fn capture_selection(inputter: &TextInputter) -> String {
    if inputter.send_copy_shortcut().is_err() {
        return String::new();
    }
    // Give the target app a beat to service the copy.
    tokio::time::sleep(std::time::Duration::from_millis(260)).await;
    inputter.read_text().unwrap_or_default()
}

/// Pick the text LLM used for polish/edit passes, honoring the configured
/// preference. `Custom` accepts any OpenAI-compatible vendor.
fn choose_text_llm(cfg: &notype_config::AppConfig) -> Option<notype_llm::TextLlmTarget> {
    use notype_llm::{TextLlmKind, TextLlmTarget};

    let custom = || -> Option<TextLlmTarget> {
        let base_url = cfg.model.custom_llm_base_url.trim();
        let model = cfg.model.custom_llm_model.trim();
        if base_url.is_empty() || model.is_empty() {
            return None;
        }
        Some(TextLlmTarget {
            kind: TextLlmKind::OpenAiCompatible,
            api_key: cfg.model.custom_llm_api_key.clone(),
            model: model.to_string(),
            base_url: Some(base_url.to_string()),
        })
    };

    let qwen = || -> Option<TextLlmTarget> {
        // Local Qwen endpoints may be keyless.
        if cfg.model.qwen_api_key.is_empty() && !cfg.model.qwen_is_custom_endpoint() {
            return None;
        }
        let model =
            if cfg.model.model_name.starts_with("qwen") && !cfg.model.model_name.contains("asr") {
                cfg.model.model_name.clone()
            } else {
                DEFAULT_POSTPROCESS_QWEN_MODEL.to_string()
            };
        Some(TextLlmTarget {
            kind: TextLlmKind::OpenAiCompatible,
            api_key: cfg.model.qwen_api_key.clone(),
            model,
            base_url: Some(cfg.model.qwen_base_url.clone()),
        })
    };

    let gemini = || -> Option<TextLlmTarget> {
        if cfg.model.gemini_api_key.is_empty() {
            return None;
        }
        let model = if cfg.model.model_name.starts_with("gemini") {
            cfg.model.model_name.clone()
        } else {
            DEFAULT_POSTPROCESS_GEMINI_MODEL.to_string()
        };
        Some(TextLlmTarget {
            kind: TextLlmKind::Gemini,
            api_key: cfg.model.gemini_api_key.clone(),
            model,
            base_url: None,
        })
    };

    let mimo = || -> Option<TextLlmTarget> {
        if cfg.model.mimo_api_key.is_empty() {
            return None;
        }
        Some(TextLlmTarget {
            kind: TextLlmKind::Mimo,
            api_key: cfg.model.mimo_api_key.clone(),
            model: DEFAULT_POSTPROCESS_MIMO_MODEL.to_string(),
            base_url: Some(cfg.model.mimo_base_url.clone()),
        })
    };

    match cfg.model.postprocess_provider {
        notype_config::PostprocessProvider::Custom => custom(),
        notype_config::PostprocessProvider::Qwen => qwen(),
        notype_config::PostprocessProvider::Gemini => gemini(),
        notype_config::PostprocessProvider::Mimo => mimo(),
        notype_config::PostprocessProvider::Auto => {
            custom().or_else(qwen).or_else(gemini).or_else(mimo)
        }
    }
}

async fn process_edit_audio(
    app: &tauri::AppHandle,
    recognizer: &tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>,
    config: &tokio::sync::RwLock<notype_config::AppConfig>,
    inputter: &TextInputter,
    generation: &AtomicU64,
    audio: notype_audio::AudioData,
) {
    let gen_at_start = generation.load(Ordering::SeqCst);

    let fail = |app: &tauri::AppHandle, msg: &str| {
        bubble::set_error(app, msg);
        emit_status(app, "Error", Some(msg));
    };

    let selection = capture_selection(inputter).await;
    if selection.trim().is_empty() {
        tracing::warn!("Voice edit: no selection captured");
        fail(app, "未检测到选中文本");
        auto_hide_bubble(app, generation, gen_at_start, 3).await;
        emit_status(app, "Ready", None);
        return;
    }
    tracing::info!(
        chars = selection.chars().count(),
        "Voice edit: selection captured"
    );

    let (is_asr, model_name, text_target) = {
        let cfg = config.read().await;
        (
            cfg.model.is_asr_pipeline(),
            cfg.model.model_name.clone(),
            choose_text_llm(&cfg),
        )
    };
    let system_prompt = edit_system_prompt(&selection);

    let edited: Result<String, String> = {
        let guard = recognizer.read().await;
        let Some(rec) = guard.as_ref() else {
            fail(app, "未配置识别引擎");
            auto_hide_bubble(app, generation, gen_at_start, 3).await;
            emit_status(app, "Ready", None);
            return;
        };

        if is_asr {
            // ASR engines can't follow instructions: transcribe the spoken
            // command first, then apply it with a text-capable LLM.
            let asr_result = rec
                .recognize(
                    audio.wav_bytes.clone(),
                    "audio/wav".into(),
                    "转录这段语音指令，只输出文字。".into(),
                )
                .await;
            drop(guard);

            match asr_result {
                Ok(r) if !r.text.trim().is_empty() => {
                    if let Some(target) = text_target {
                        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                        notype_llm::postprocess_text_stream_to(&target, system_prompt, r.text, tx)
                            .await
                            .map(|out| out.text)
                            .map_err(|e| e.to_string())
                    } else {
                        Err("语音编辑需要配置润色 LLM（自定义 / Qwen / Gemini / MiMo）".into())
                    }
                }
                Ok(_) => Err("没有听到编辑指令".into()),
                Err(e) => Err(e.to_string()),
            }
        } else {
            rec.recognize(audio.wav_bytes.clone(), "audio/wav".into(), system_prompt)
                .await
                .map(|r| r.text)
                .map_err(|e| e.to_string())
        }
    };

    match edited {
        Ok(text) if !text.trim().is_empty() => {
            let ctx = FinalizeCtx {
                from_ui: false,
                // Paste atomically replaces the still-active selection.
                input_mode: notype_config::InputMode::Clipboard,
                auto_copy: false,
                auto_enter: false,
                replace_rules: String::new(),
                provider_label: "语音编辑".to_string(),
                model_name,
                duration_secs: audio.duration_secs,
            };
            emit_final_text(
                app,
                inputter,
                generation,
                gen_at_start,
                text.trim(),
                "",
                &ctx,
            )
            .await;
        }
        Ok(_) => {
            tracing::info!("Voice edit produced empty result");
            bubble::hide_bubble(app);
            emit_status(app, "Ready", None);
        }
        Err(e) => {
            tracing::error!("Voice edit failed: {e}");
            fail(app, &e);
            auto_hide_bubble(app, generation, gen_at_start, 5).await;
            emit_status(app, "Ready", None);
        }
    }
}

/// Toggle dictation from the main window. Returns `true` if now recording.
#[tauri::command]
fn toggle_recording(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<bool, String> {
    if state.recorder.is_recording() {
        stop_capture(&app)?;
        Ok(false)
    } else {
        start_capture(&app, true, false)?;
        Ok(true)
    }
}

#[tauri::command]
fn cancel_recording(app: tauri::AppHandle) {
    cancel_capture(&app);
}

// -- Stats --

#[derive(Clone, serde::Serialize)]
struct StatsDto {
    total_chars: u64,
    total_duration_secs: f64,
    total_sessions: u64,
    streak_days: u32,
    learned_pairs: u64,
}

impl StatsDto {
    fn from(stats: &notype_config::stats::Stats) -> Self {
        Self {
            total_chars: stats.total_chars,
            total_duration_secs: stats.total_duration_secs,
            total_sessions: stats.total_sessions,
            streak_days: notype_config::stats::effective_streak(stats),
            learned_pairs: stats.learned_pairs,
        }
    }
}

#[tauri::command]
fn get_stats() -> StatsDto {
    StatsDto::from(&notype_config::stats::load())
}

/// Quick style switch from the home view — persists without a full save.
#[tauri::command]
async fn set_output_style(state: tauri::State<'_, AppState>, style: String) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.general.output_style = match style.as_str() {
        "verbatim" => notype_config::OutputStyle::Verbatim,
        "translate_en" => notype_config::OutputStyle::TranslateEn,
        _ => notype_config::OutputStyle::Polish,
    };
    notype_config::save(&config).map_err(|e| e.to_string())
}

/// Onboarding: set provider (+key) with one call and rebuild the recognizer.
#[tauri::command]
async fn quick_setup(
    state: tauri::State<'_, AppState>,
    provider: String,
    api_key: String,
) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.model.provider = match provider.to_lowercase().as_str() {
        "qwen" => notype_config::Provider::Qwen,
        "gemini" => notype_config::Provider::Gemini,
        "mimo" => notype_config::Provider::Mimo,
        "volcengine" => notype_config::Provider::Volcengine,
        "whisper" => notype_config::Provider::Whisper,
        "apple" => notype_config::Provider::Apple,
        other => return Err(format!("未知引擎: {other}")),
    };
    // Sensible default model per provider.
    match config.model.provider {
        notype_config::Provider::Qwen => config.model.model_name = "qwen3.5-omni-flash".into(),
        notype_config::Provider::Gemini => {
            config.model.model_name = "gemini-3-flash-preview".into()
        }
        notype_config::Provider::Mimo => config.model.model_name = "mimo-v2.5-asr".into(),
        _ => {}
    }
    let key = api_key.trim();
    if !key.is_empty() {
        match config.model.provider {
            notype_config::Provider::Qwen => config.model.qwen_api_key = key.to_string(),
            notype_config::Provider::Gemini => config.model.gemini_api_key = key.to_string(),
            notype_config::Provider::Mimo => config.model.mimo_api_key = key.to_string(),
            notype_config::Provider::Whisper => config.model.whisper_api_key = key.to_string(),
            _ => {}
        }
    }
    notype_config::save(&config).map_err(|e| e.to_string())?;

    let new_recognizer = build_recognizer(&config);
    drop(config);
    let mut rec = state.recognizer.write().await;
    *rec = new_recognizer;
    Ok(())
}

#[tauri::command]
async fn mark_onboarded(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.general.onboarded = true;
    notype_config::save(&config).map_err(|e| e.to_string())
}

/// Export the transcription history as Markdown into ~/Downloads.
/// Returns the written file path.
#[tauri::command]
fn export_history() -> Result<String, String> {
    let entries = notype_config::history::load();
    if entries.is_empty() {
        return Err("没有可导出的历史记录".into());
    }

    let mut out = String::from("# NoType 转写历史\n");
    let mut last_day = String::new();
    for entry in &entries {
        let secs = entry.id / 1000;
        let datetime = chrono::DateTime::from_timestamp(secs as i64, 0)
            .map(|dt| dt.with_timezone(&chrono::Local));
        let (day, time) = match datetime {
            Some(dt) => (
                dt.format("%Y-%m-%d").to_string(),
                dt.format("%H:%M").to_string(),
            ),
            None => (String::from("未知日期"), String::new()),
        };
        if day != last_day {
            out.push_str(&format!("\n## {day}\n\n"));
            last_day = day;
        }
        out.push_str(&format!(
            "- **{time}**（{}）{}\n",
            entry.provider,
            entry.text.replace('\n', " ")
        ));
    }

    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| "找不到下载目录".to_string())?;
    let filename = format!(
        "notype-history-{}.md",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let path = dir.join(filename);
    std::fs::write(&path, out).map_err(|e| e.to_string())?;
    Ok(path.to_string_lossy().to_string())
}

// -- Permissions --

#[derive(Clone, serde::Serialize)]
struct PermissionsDto {
    accessibility: bool,
}

#[tauri::command]
fn check_permissions() -> PermissionsDto {
    PermissionsDto {
        accessibility: platform::accessibility_trusted(),
    }
}

#[tauri::command]
fn open_accessibility_settings(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if platform::ACCESSIBILITY_SETTINGS_URL.is_empty() {
        return Ok(());
    }
    app.opener()
        .open_url(platform::ACCESSIBILITY_SETTINGS_URL, None::<&str>)
        .map_err(|e| e.to_string())
}

// -- History --

#[tauri::command]
fn get_history() -> Vec<notype_config::history::HistoryEntry> {
    notype_config::history::load()
}

#[tauri::command]
fn delete_history_entry(id: u64) -> Result<Vec<notype_config::history::HistoryEntry>, String> {
    notype_config::history::delete(id).map_err(|e| e.to_string())
}

#[tauri::command]
fn update_history_entry(
    id: u64,
    text: String,
) -> Result<Vec<notype_config::history::HistoryEntry>, String> {
    notype_config::history::update_text(id, &text).map_err(|e| e.to_string())
}

#[derive(Clone, serde::Deserialize)]
struct VocabPair {
    wrong: String,
    right: String,
}

/// Dictionary auto-learning: append correction pairs extracted from a user
/// edit to the vocabulary prompt. Returns how many were actually new.
#[tauri::command]
async fn learn_vocabulary(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    pairs: Vec<VocabPair>,
) -> Result<usize, String> {
    if pairs.is_empty() {
        return Ok(0);
    }

    let mut config = state.config.write().await;
    // Materialize the effective text so builtin defaults survive the append.
    let mut vocab = config.prompts.vocabulary_text().to_string();

    let mut added = 0usize;
    for pair in &pairs {
        let (wrong, right) = (pair.wrong.trim(), pair.right.trim());
        if wrong.is_empty() || right.is_empty() || wrong == right {
            continue;
        }
        let line = format!("- {wrong} → {right}");
        if vocab.lines().any(|l| l.trim() == line) {
            continue;
        }
        vocab = format!("{}\n{line}", vocab.trim_end());
        added += 1;
    }

    if added == 0 {
        return Ok(0);
    }

    config.prompts.vocabulary = vocab;
    notype_config::save(&config).map_err(|e| e.to_string())?;
    drop(config);

    match notype_config::stats::record_learned(added) {
        Ok(stats) => {
            let _ = app.emit("notype://stats", StatsDto::from(&stats));
        }
        Err(e) => tracing::warn!("Failed to record learned pairs: {e}"),
    }
    tracing::info!(added, "Vocabulary auto-learned from user edit");
    Ok(added)
}

#[tauri::command]
fn clear_history() -> Result<(), String> {
    notype_config::history::clear().map_err(|e| e.to_string())
}

#[tauri::command]
fn copy_text_to_clipboard(state: tauri::State<'_, AppState>, text: String) -> Result<(), String> {
    state.inputter.copy_text(&text).map_err(|e| e.to_string())
}

// -- Autostart --

#[cfg(desktop)]
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[cfg(desktop)]
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let autolaunch = app.autolaunch();
    if enabled {
        autolaunch.enable().map_err(|e| e.to_string())
    } else {
        autolaunch.disable().map_err(|e| e.to_string())
    }
}

#[cfg(not(desktop))]
#[tauri::command]
fn get_autostart(_app: tauri::AppHandle) -> Result<bool, String> {
    Ok(false)
}

#[cfg(not(desktop))]
#[tauri::command]
fn set_autostart(_app: tauri::AppHandle, _enabled: bool) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
fn open_config_dir(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let dir = notype_config::config_dir();
    let _ = std::fs::create_dir_all(&dir);
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn get_prompts(state: tauri::State<'_, AppState>) -> Result<PromptsDto, String> {
    let config = state.config.read().await;
    Ok(PromptsDto {
        agent: config.prompts.agent_text().to_string(),
        rules: config.prompts.rules_text().to_string(),
        vocabulary: config.prompts.vocabulary_text().to_string(),
        replace_rules: config.prompts.replace_rules.clone(),
    })
}

#[tauri::command]
async fn save_prompts(state: tauri::State<'_, AppState>, dto: PromptsDto) -> Result<(), String> {
    let mut config = state.config.write().await;
    config.prompts.agent = dto.agent;
    config.prompts.rules = dto.rules;
    config.prompts.vocabulary = dto.vocabulary;
    config.prompts.replace_rules = dto.replace_rules;
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
        replace_rules: String::new(),
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

    let hotkey_changed =
        config.general.hotkey != dto.hotkey || config.general.edit_hotkey != dto.edit_hotkey;

    config.model.provider = match dto.provider.to_lowercase().as_str() {
        "qwen" => notype_config::Provider::Qwen,
        "mimo" | "xiaomi" => notype_config::Provider::Mimo,
        "volcengine" | "volc" | "doubao" => notype_config::Provider::Volcengine,
        "whisper" | "openai" => notype_config::Provider::Whisper,
        "apple" => notype_config::Provider::Apple,
        _ => notype_config::Provider::Gemini,
    };
    if !dto.gemini_api_key.is_empty() {
        config.model.gemini_api_key = dto.gemini_api_key;
    }
    if !dto.qwen_api_key.is_empty() {
        config.model.qwen_api_key = dto.qwen_api_key;
    }
    if !dto.qwen_base_url.trim().is_empty() {
        config.model.qwen_base_url = dto.qwen_base_url.trim().to_string();
    }
    if !dto.mimo_api_key.is_empty() {
        config.model.mimo_api_key = dto.mimo_api_key;
    }
    if !dto.mimo_base_url.trim().is_empty() {
        config.model.mimo_base_url = dto.mimo_base_url;
    }
    if !dto.volc_app_key.is_empty() {
        config.model.volc_app_key = dto.volc_app_key;
    }
    if !dto.volc_access_key.is_empty() {
        config.model.volc_access_key = dto.volc_access_key;
    }
    if !dto.volc_resource_id.trim().is_empty() {
        config.model.volc_resource_id = dto.volc_resource_id.trim().to_string();
    }
    if !dto.whisper_base_url.trim().is_empty() {
        config.model.whisper_base_url = dto.whisper_base_url.trim().to_string();
    }
    if !dto.whisper_api_key.is_empty() {
        config.model.whisper_api_key = dto.whisper_api_key;
    }
    if !dto.whisper_model.trim().is_empty() {
        config.model.whisper_model = dto.whisper_model.trim().to_string();
    }
    config.model.apple_locale = dto.apple_locale.trim().to_string();
    config.model.enable_postprocess = dto.enable_postprocess;
    config.model.postprocess_provider = match dto.postprocess_provider.as_str() {
        "custom" => notype_config::PostprocessProvider::Custom,
        "qwen" => notype_config::PostprocessProvider::Qwen,
        "gemini" => notype_config::PostprocessProvider::Gemini,
        "mimo" => notype_config::PostprocessProvider::Mimo,
        _ => notype_config::PostprocessProvider::Auto,
    };
    config.model.custom_llm_base_url = dto.custom_llm_base_url.trim().to_string();
    if !dto.custom_llm_api_key.is_empty() {
        config.model.custom_llm_api_key = dto.custom_llm_api_key;
    }
    config.model.custom_llm_model = dto.custom_llm_model.trim().to_string();
    config.model.model_name = dto.model_name;
    config.general.hotkey = dto.hotkey.clone();
    config.general.edit_hotkey = dto.edit_hotkey.trim().to_string();
    config.general.audio_device = dto.audio_device.trim().to_string();
    config.general.input_mode = match dto.input_mode.to_lowercase().as_str() {
        "clipboard" => notype_config::InputMode::Clipboard,
        _ => notype_config::InputMode::Keyboard,
    };
    config.general.auto_copy = dto.auto_copy;
    config.general.output_style = match dto.output_style.as_str() {
        "verbatim" => notype_config::OutputStyle::Verbatim,
        "translate_en" => notype_config::OutputStyle::TranslateEn,
        _ => notype_config::OutputStyle::Polish,
    };
    config.general.enable_app_context = dto.enable_app_context;
    config.general.structured_output = dto.structured_output;
    config.general.stream_typing = dto.stream_typing;
    config.general.sound_feedback = dto.sound_feedback;
    config.general.auto_enter = dto.auto_enter;
    config.general.app_rules = dto.app_rules;

    notype_config::save(&config).map_err(|e| e.to_string())?;

    // Apply microphone selection (effective on next recording).
    let device = config.general.audio_device.clone();
    state.recorder.set_device(if device.is_empty() {
        None
    } else {
        Some(device)
    });

    // Rebuild recognizer
    let new_recognizer = build_recognizer(&config);
    drop(config);

    let mut rec = state.recognizer.write().await;
    *rec = new_recognizer;
    drop(rec);

    // Re-register shortcut if changed
    if hotkey_changed {
        if let Err(e) = reregister_shortcut(&app, &dto.hotkey, &dto.edit_hotkey) {
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
    #[serde(default)]
    replace_rules: String,
}

/// DTO for frontend config exchange. Keys are never sent back — only has_* flags.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ConfigDto {
    provider: String,
    gemini_api_key: String,
    qwen_api_key: String,
    #[serde(default)]
    qwen_base_url: String,
    mimo_api_key: String,
    mimo_base_url: String,
    #[serde(default)]
    volc_app_key: String,
    #[serde(default)]
    volc_access_key: String,
    #[serde(default)]
    volc_resource_id: String,
    #[serde(default)]
    whisper_base_url: String,
    #[serde(default)]
    whisper_api_key: String,
    #[serde(default)]
    whisper_model: String,
    #[serde(default)]
    apple_locale: String,
    #[serde(default)]
    enable_postprocess: bool,
    #[serde(default)]
    postprocess_provider: String,
    #[serde(default)]
    custom_llm_base_url: String,
    #[serde(default)]
    custom_llm_api_key: String,
    #[serde(default)]
    custom_llm_model: String,
    model_name: String,
    hotkey: String,
    #[serde(default)]
    edit_hotkey: String,
    #[serde(default)]
    audio_device: String,
    #[serde(default)]
    input_mode: String,
    #[serde(default)]
    auto_copy: bool,
    #[serde(default)]
    output_style: String,
    #[serde(default)]
    enable_app_context: bool,
    #[serde(default)]
    structured_output: bool,
    #[serde(default)]
    stream_typing: bool,
    #[serde(default)]
    sound_feedback: bool,
    #[serde(default)]
    auto_enter: bool,
    #[serde(default)]
    app_rules: String,
    #[serde(default)]
    onboarded: bool,
    has_gemini_key: bool,
    has_qwen_key: bool,
    has_mimo_key: bool,
    #[serde(default)]
    has_volc_keys: bool,
    #[serde(default)]
    has_whisper_key: bool,
    #[serde(default)]
    has_custom_llm_key: bool,
}

impl ConfigDto {
    fn from(config: &notype_config::AppConfig) -> Self {
        let provider = match config.model.provider {
            notype_config::Provider::Gemini => "gemini",
            notype_config::Provider::Qwen => "qwen",
            notype_config::Provider::Mimo => "mimo",
            notype_config::Provider::Volcengine => "volcengine",
            notype_config::Provider::Whisper => "whisper",
            notype_config::Provider::Apple => "apple",
        };
        Self {
            provider: provider.into(),
            gemini_api_key: String::new(),
            qwen_api_key: String::new(),
            qwen_base_url: config.model.qwen_base_url.clone(),
            mimo_api_key: String::new(),
            mimo_base_url: config.model.mimo_base_url.clone(),
            volc_app_key: String::new(),
            volc_access_key: String::new(),
            volc_resource_id: config.model.volc_resource_id.clone(),
            whisper_base_url: config.model.whisper_base_url.clone(),
            whisper_api_key: String::new(),
            whisper_model: config.model.whisper_model.clone(),
            apple_locale: config.model.apple_locale.clone(),
            enable_postprocess: config.model.enable_postprocess,
            postprocess_provider: match config.model.postprocess_provider {
                notype_config::PostprocessProvider::Auto => "auto",
                notype_config::PostprocessProvider::Custom => "custom",
                notype_config::PostprocessProvider::Qwen => "qwen",
                notype_config::PostprocessProvider::Gemini => "gemini",
                notype_config::PostprocessProvider::Mimo => "mimo",
            }
            .to_string(),
            custom_llm_base_url: config.model.custom_llm_base_url.clone(),
            custom_llm_api_key: String::new(),
            custom_llm_model: config.model.custom_llm_model.clone(),
            model_name: config.model.model_name.clone(),
            hotkey: config.general.hotkey.clone(),
            edit_hotkey: config.general.edit_hotkey.clone(),
            audio_device: config.general.audio_device.clone(),
            input_mode: match config.general.input_mode {
                notype_config::InputMode::Keyboard => "keyboard",
                notype_config::InputMode::Clipboard => "clipboard",
            }
            .to_string(),
            auto_copy: config.general.auto_copy,
            output_style: match config.general.output_style {
                notype_config::OutputStyle::Polish => "polish",
                notype_config::OutputStyle::Verbatim => "verbatim",
                notype_config::OutputStyle::TranslateEn => "translate_en",
            }
            .to_string(),
            enable_app_context: config.general.enable_app_context,
            structured_output: config.general.structured_output,
            stream_typing: config.general.stream_typing,
            sound_feedback: config.general.sound_feedback,
            auto_enter: config.general.auto_enter,
            app_rules: config.general.app_rules.clone(),
            onboarded: config.general.onboarded,
            has_gemini_key: !config.model.gemini_api_key.is_empty(),
            has_qwen_key: !config.model.qwen_api_key.is_empty(),
            has_mimo_key: !config.model.mimo_api_key.is_empty(),
            has_volc_keys: !config.model.volc_app_key.is_empty()
                && !config.model.volc_access_key.is_empty(),
            has_whisper_key: !config.model.whisper_api_key.is_empty(),
            has_custom_llm_key: !config.model.custom_llm_api_key.is_empty(),
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct SaveResult {
    restart_needed: bool,
}

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
    let audio_device = {
        let name = config.general.audio_device.trim();
        if name.is_empty() {
            None
        } else {
            Some(name.to_string())
        }
    };

    tracing::info!(
        provider = ?config.model.provider,
        model = %config.model.model_name,
        has_gemini_key = !config.model.gemini_api_key.is_empty(),
        has_qwen_key = !config.model.qwen_api_key.is_empty(),
        has_mimo_key = !config.model.mimo_api_key.is_empty(),
        has_volc_keys = !config.model.volc_app_key.is_empty(),
        has_whisper_key = !config.model.whisper_api_key.is_empty(),
        "Config loaded"
    );

    let mut builder = tauri::Builder::default().plugin(tauri_plugin_opener::init());

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ));
    }

    builder
        .manage(AppState {
            recorder: Arc::new(Recorder::new(audio_device)),
            inputter: Arc::new(TextInputter::new()),
            recognizer: Arc::new(tokio::sync::RwLock::new(recognizer)),
            config: Arc::new(tokio::sync::RwLock::new(config)),
            runtime: runtime.handle().clone(),
            bubble_generation: Arc::new(AtomicU64::new(0)),
            hotkey_down: Arc::new(AtomicBool::new(false)),
            ui_capture: Arc::new(AtomicBool::new(false)),
            edit_session: Arc::new(AtomicBool::new(false)),
            hotkey_pressed_at: Arc::new(std::sync::Mutex::new(None)),
            capture_latched: Arc::new(AtomicBool::new(false)),
            swallow_release: Arc::new(AtomicBool::new(false)),
            active_app: Arc::new(std::sync::Mutex::new(None)),
            latest_interim_text: Arc::new(std::sync::Mutex::new(String::new())),
            stream_final_gen: Arc::new(AtomicU64::new(0)),
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

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_version,
            list_audio_devices,
            get_prompts,
            save_prompts,
            get_builtin_prompts,
            is_recording,
            toggle_recording,
            cancel_recording,
            get_config,
            save_config,
            get_history,
            delete_history_entry,
            update_history_entry,
            learn_vocabulary,
            clear_history,
            copy_text_to_clipboard,
            get_stats,
            set_output_style,
            quick_setup,
            mark_onboarded,
            export_history,
            check_permissions,
            open_accessibility_settings,
            get_autostart,
            set_autostart,
            open_config_dir,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// -- Global Shortcut --

#[cfg(desktop)]
fn setup_global_shortcut(app: &tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

    // Read hotkeys from config
    let (hotkey_str, edit_hotkey_str) = {
        let state = app.state::<AppState>();
        let rt = state.runtime.clone();
        let cfg = Arc::clone(&state.config);
        rt.block_on(async {
            let c = cfg.read().await;
            (c.general.hotkey.clone(), c.general.edit_hotkey.clone())
        })
    };

    let shortcut =
        parse_hotkey(&hotkey_str).map_err(|e| format!("Invalid hotkey '{hotkey_str}': {e}"))?;
    let edit_shortcut = resolve_edit_shortcut(&edit_hotkey_str, &shortcut);
    let edit_id = edit_shortcut.as_ref().map(|s| s.id());

    app.handle().plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |app, sc, event| {
                let state = app.state::<AppState>();
                let hotkey_down = &state.hotkey_down;
                let is_edit = edit_id == Some(sc.id());

                match event.state() {
                    ShortcutState::Pressed => {
                        // A latched (tap-locked) session stops on the next press.
                        if state.capture_latched.load(Ordering::SeqCst)
                            && state.recorder.is_recording()
                        {
                            state.capture_latched.store(false, Ordering::SeqCst);
                            state.swallow_release.store(true, Ordering::SeqCst);
                            if let Err(e) = stop_capture(app) {
                                tracing::error!("Failed to stop latched recording: {e}");
                                emit_status(app, "Error", Some(&e));
                            }
                            return;
                        }

                        if hotkey_down
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                            .is_err()
                        {
                            return;
                        }

                        if state.recorder.is_recording() {
                            // A UI-initiated dictation is running; the hotkey must not
                            // steal or restart that session.
                            hotkey_down.store(false, Ordering::SeqCst);
                            return;
                        }

                        if let Err(e) = start_capture(app, false, is_edit) {
                            hotkey_down.store(false, Ordering::SeqCst);
                            tracing::error!("Failed to start recording: {e}");
                            emit_status(app, "Error", Some(&e));
                        } else if let Ok(mut guard) = state.hotkey_pressed_at.lock() {
                            *guard = Some(Instant::now());
                        }
                    }
                    ShortcutState::Released => {
                        if state.swallow_release.swap(false, Ordering::SeqCst) {
                            hotkey_down.store(false, Ordering::SeqCst);
                            return;
                        }
                        if !hotkey_down.swap(false, Ordering::SeqCst) {
                            return;
                        }

                        if state.recorder.is_recording() {
                            // Quick tap → latch: keep recording hands-free
                            // until the hotkey is pressed again.
                            let quick_tap = state
                                .hotkey_pressed_at
                                .lock()
                                .ok()
                                .and_then(|g| *g)
                                .is_some_and(|t| t.elapsed().as_millis() < 450);
                            if quick_tap {
                                state.capture_latched.store(true, Ordering::SeqCst);
                                tracing::info!("Recording latched by quick tap");
                                return;
                            }
                            if let Err(e) = stop_capture(app) {
                                tracing::error!("Failed to stop recording: {e}");
                                emit_status(app, "Error", Some(&e));
                            }
                        }
                    }
                }
            })
            .build(),
    )?;

    app.global_shortcut().register(shortcut)?;
    if let Some(edit_sc) = edit_shortcut {
        if let Err(e) = app.global_shortcut().register(edit_sc) {
            tracing::warn!("Failed to register voice-edit hotkey '{edit_hotkey_str}': {e}");
        } else {
            tracing::info!(hotkey = %edit_hotkey_str, "Voice-edit shortcut registered");
        }
    }
    tracing::info!(hotkey = %hotkey_str, "Global shortcut registered");
    Ok(())
}

/// Parse the voice-edit hotkey; disabled when empty, invalid, or identical
/// to the dictation hotkey.
#[cfg(desktop)]
fn resolve_edit_shortcut(
    edit_hotkey: &str,
    main: &tauri_plugin_global_shortcut::Shortcut,
) -> Option<tauri_plugin_global_shortcut::Shortcut> {
    let trimmed = edit_hotkey.trim();
    if trimmed.is_empty() {
        return None;
    }
    match parse_hotkey(trimmed) {
        Ok(sc) if sc.id() == main.id() => {
            tracing::warn!("Voice-edit hotkey equals dictation hotkey; edit disabled");
            None
        }
        Ok(sc) => Some(sc),
        Err(e) => {
            tracing::warn!("Invalid voice-edit hotkey '{trimmed}': {e}");
            None
        }
    }
}

#[cfg(not(desktop))]
fn setup_global_shortcut(_app: &tauri::App) -> std::result::Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

// -- Interim Transcription (live preview while recording) --

const INTERIM_INITIAL_DELAY: std::time::Duration = std::time::Duration::from_millis(2000);
const INTERIM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(2500);
/// Dedicated ASR engines are fast + cheap; poll them more eagerly.
const INTERIM_INITIAL_DELAY_ASR: std::time::Duration = std::time::Duration::from_millis(900);
const INTERIM_INTERVAL_ASR: std::time::Duration = std::time::Duration::from_millis(1300);
const INTERIM_MIN_DURATION: f32 = 1.0;
const VOLC_PCM_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

async fn interim_loop(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
) {
    let (provider, is_asr) = {
        let cfg = config.read().await;
        (cfg.model.provider.clone(), cfg.model.is_asr_pipeline())
    };

    if matches!(provider, notype_config::Provider::Volcengine) {
        interim_loop_volcengine(
            app,
            recorder,
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
            is_asr,
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn interim_loop_default(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    recognizer: Arc<tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
    is_asr: bool,
) {
    let prompt = {
        let cfg = config.read().await;
        compose_session_prompt(&app, &cfg)
    };
    let (initial_delay, interval) = if is_asr {
        (INTERIM_INITIAL_DELAY_ASR, INTERIM_INTERVAL_ASR)
    } else {
        (INTERIM_INITIAL_DELAY, INTERIM_INTERVAL)
    };
    let mut last_asr_text = String::new();

    tokio::time::sleep(initial_delay).await;

    loop {
        if generation.load(Ordering::SeqCst) != gen_val || !recorder.is_recording() {
            break;
        }

        let snapshot = recorder.snapshot();
        let Some(Ok(audio)) = snapshot else {
            break;
        };

        if audio.duration_secs < INTERIM_MIN_DURATION {
            tokio::time::sleep(interval).await;
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

        tokio::time::sleep(interval).await;
    }
}

/// Volcengine live pipeline: stream mic PCM over one WebSocket session while
/// recording; incremental full-text updates drive the preview. When the
/// recorder stops we flush the session and publish the final text for
/// `process_audio` to pick up.
async fn interim_loop_volcengine(
    app: tauri::AppHandle,
    recorder: Arc<Recorder>,
    config: Arc<tokio::sync::RwLock<notype_config::AppConfig>>,
    latest_interim_text: Arc<std::sync::Mutex<String>>,
    generation: Arc<AtomicU64>,
    gen_val: u64,
) {
    let volc_config = {
        let cfg = config.read().await;
        notype_llm::volcengine::VolcConfig {
            app_key: cfg.model.volc_app_key.clone(),
            access_key: cfg.model.volc_access_key.clone(),
            resource_id: cfg.model.volc_resource_id.clone(),
        }
    };
    if volc_config.app_key.trim().is_empty() || volc_config.access_key.trim().is_empty() {
        return;
    }

    let (text_tx, mut text_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let session = match notype_llm::volcengine::VolcStreamSession::start(volc_config, text_tx).await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("Volcengine live session failed to start: {e}");
            return;
        }
    };

    let mut last_end_sample = 0usize;
    let mut pcm_interval = tokio::time::interval(VOLC_PCM_INTERVAL);
    pcm_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let is_valid = || generation.load(Ordering::SeqCst) == gen_val;

    loop {
        if !is_valid() {
            // Session cancelled — drop everything.
            return;
        }
        if !recorder.is_recording() {
            break;
        }

        tokio::select! {
            _ = pcm_interval.tick() => {
                let Some(snapshot) = recorder.snapshot_pcm_from(last_end_sample) else {
                    break;
                };
                let Ok(pcm) = snapshot else {
                    continue;
                };
                if pcm.end_sample <= last_end_sample || pcm.pcm_s16le.is_empty() {
                    continue;
                }
                last_end_sample = pcm.end_sample;
                let chunk = convert_pcm_chunk_to_16k_mono_s16le(
                    &pcm.pcm_s16le,
                    pcm.sample_rate,
                    pcm.channels,
                );
                if !chunk.is_empty() && !session.push_pcm(chunk) {
                    tracing::warn!("Volcengine session closed while pushing PCM");
                    break;
                }
            }
            update = text_rx.recv() => {
                match update {
                    Some(text) if is_valid() => {
                        set_interim_with_cache(&app, &latest_interim_text, &text);
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        }
    }

    // Recorder stopped: flush the final tail (any samples captured after the
    // last tick), then close the session for the final transcript.
    if let Some(Ok(pcm)) = recorder.snapshot_pcm_from(last_end_sample) {
        if pcm.end_sample > last_end_sample && !pcm.pcm_s16le.is_empty() {
            let chunk =
                convert_pcm_chunk_to_16k_mono_s16le(&pcm.pcm_s16le, pcm.sample_rate, pcm.channels);
            if !chunk.is_empty() {
                let _ = session.push_pcm(chunk);
            }
        }
    }

    // Keep draining incremental updates while waiting for the final result.
    let drain = async {
        while let Some(text) = text_rx.recv().await {
            if is_valid() {
                set_interim_with_cache(&app, &latest_interim_text, &text);
            }
        }
    };
    let (final_result, ()) = tokio::join!(session.finish(std::time::Duration::from_secs(6)), drain);

    match final_result {
        Ok(text) if !text.trim().is_empty() => {
            if let Ok(mut guard) = latest_interim_text.lock() {
                *guard = text;
            }
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("Volcengine session finish failed: {e}"),
    }

    if is_valid() {
        let state = app.state::<AppState>();
        state.stream_final_gen.store(gen_val, Ordering::SeqCst);
    }
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
        let dst_len = (src_len as u64 * 16_000).div_ceil(sample_rate as u64) as usize;
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

struct FinalizeCtx {
    from_ui: bool,
    input_mode: notype_config::InputMode,
    auto_copy: bool,
    /// Press Enter after successful injection (auto-send).
    auto_enter: bool,
    /// Deterministic post-replacement rules (applied when nothing was
    /// stream-typed yet — otherwise the typed prefix would diverge).
    replace_rules: String,
    provider_label: String,
    model_name: String,
    duration_secs: f32,
}

/// Finalize a session: record history/stats, inject the text, settle the UI.
///
/// `typed_prefix` is what stream-typing already put at the cursor. The
/// remainder is typed; if typing fails midway the leftover is delivered as a
/// one-shot paste (the automatic fallback).
async fn emit_final_text(
    app: &tauri::AppHandle,
    inputter: &TextInputter,
    generation: &AtomicU64,
    gen_at_start: u64,
    text: &str,
    typed_prefix: &str,
    ctx: &FinalizeCtx,
) {
    // Deterministic hot-rule replacement — only safe when stream typing
    // hasn't already committed characters to the target app.
    let replaced;
    let text = if typed_prefix.is_empty() && !ctx.replace_rules.trim().is_empty() {
        replaced = notype_config::apply_replace_rules(&ctx.replace_rules, text);
        replaced.as_str()
    } else {
        text
    };

    bubble::set_result(app, text);
    emit_status(app, "Done", Some(text));

    // Persist to history and notify the main window.
    match notype_config::history::append(
        text,
        &ctx.provider_label,
        &ctx.model_name,
        ctx.duration_secs,
    ) {
        Ok(entry) => {
            let _ = app.emit("notype://result", entry);
        }
        Err(e) => tracing::warn!("Failed to persist history entry: {e}"),
    }

    // Lifetime stats: total chars / speaking time / streak.
    match notype_config::stats::record(text.chars().count(), ctx.duration_secs) {
        Ok(stats) => {
            let _ = app.emit("notype://stats", StatsDto::from(&stats));
        }
        Err(e) => tracing::warn!("Failed to persist stats: {e}"),
    }

    if ctx.from_ui {
        // Dictation mode: the main window has focus, so typing would land in
        // NoType itself. Copy instead; the window shows the text.
        if let Err(e) = inputter.copy_text(text) {
            tracing::warn!("Failed to copy dictation result: {e}");
        }
    } else {
        let inject = match ctx.input_mode {
            notype_config::InputMode::Keyboard => {
                // Deliver only what stream-typing hasn't already typed.
                let remaining = if typed_prefix.is_empty() {
                    Some(text)
                } else if let Some(rest) = text.strip_prefix(typed_prefix) {
                    Some(rest)
                } else {
                    // Streamed text diverged from the final text (rare) —
                    // we can't unsay what was typed; leave it as-is.
                    tracing::warn!(
                        "Stream-typed text diverged from final text; skipping remainder"
                    );
                    None
                };
                match remaining {
                    Some(rest) if !rest.is_empty() => {
                        inputter.type_text(rest).or_else(|type_err| {
                            // Automatic one-shot paste fallback.
                            tracing::warn!("Typing failed ({type_err}), falling back to paste");
                            inputter.paste_text(rest)
                        })
                    }
                    _ => Ok(()),
                }
            }
            notype_config::InputMode::Clipboard => inputter.paste_text(text),
        };
        match inject {
            Err(e) => {
                tracing::error!("Failed to inject text: {e}");
                emit_status(app, "Error", Some(&e.to_string()));
            }
            Ok(()) if ctx.auto_enter => {
                // Auto-send: press Enter after the text lands (chat apps).
                if let Err(e) = inputter.press_enter() {
                    tracing::warn!("Auto-enter failed: {e}");
                }
            }
            Ok(()) => {}
        }
        if ctx.auto_copy && ctx.input_mode == notype_config::InputMode::Keyboard {
            if let Err(e) = inputter.copy_text(text) {
                tracing::warn!("Failed to auto-copy result: {e}");
            }
        }
    }

    bubble::enable_result_interaction(app);
    let display_secs = (5 + text.len() as u64 / 50).min(15);
    auto_hide_bubble(app, generation, gen_at_start, display_secs).await;
    emit_status(app, "Ready", None);
}

/// Build the polish pass spec: which text LLM + which instruction.
/// Returns `None` when polishing is off, style is verbatim, or no LLM is
/// configured — the raw ASR text is delivered as-is in that case.
fn build_postprocess_spec(
    cfg: &notype_config::AppConfig,
    app_context: Option<&str>,
) -> Option<PostprocessSpec> {
    if !cfg.model.enable_postprocess {
        return None;
    }

    let system_prompt = match cfg.general.output_style {
        // Verbatim: raw ASR text IS the desired output.
        notype_config::OutputStyle::Verbatim => return None,
        notype_config::OutputStyle::TranslateEn => POSTPROCESS_TRANSLATE_PROMPT.to_string(),
        notype_config::OutputStyle::Polish => {
            // Reuse the user's rules + vocabulary so both pipelines share
            // one set of formatting/correction knowledge.
            let mut parts = vec![
                POSTPROCESS_SYSTEM_PROMPT.to_string(),
                cfg.prompts.rules_text().to_string(),
                cfg.prompts.vocabulary_text().to_string(),
            ];
            if cfg.general.enable_app_context {
                if let Some(target) = app_context.map(str::trim).filter(|s| !s.is_empty()) {
                    parts.push(format!(
                        "## 当前输入场景\n用户正在「{}」中输入文字。{}",
                        target,
                        notype_config::resolve_app_tone(target, &cfg.general.app_rules)
                    ));
                }
            }
            if !cfg.general.structured_output {
                parts.push(notype_config::UNSTRUCTURED_DIRECTIVE.to_string());
            }
            parts.retain(|s| !s.trim().is_empty());
            parts.join("\n\n")
        }
    };

    choose_text_llm(cfg).map(|target| PostprocessSpec {
        target,
        system_prompt,
    })
}

/// Drive a streaming text future to completion while mirroring chunks to the
/// bubble preview and (optionally) typing them at the cursor as they arrive.
/// Returns `(final_text, typed_prefix)`.
async fn consume_text_stream<F>(
    app: &tauri::AppHandle,
    inputter: &TextInputter,
    fut: F,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<String>,
    stream_type: bool,
) -> (notype_llm::Result<String>, String)
where
    F: std::future::Future<Output = notype_llm::Result<notype_llm::RecognitionResult>>,
{
    tokio::pin!(fut);

    let mut streamed = String::new();
    let mut typed = String::new();
    let mut type_failed = false;

    let outcome = loop {
        tokio::select! {
            result = &mut fut => {
                // Drain whatever is still queued in the channel.
                while let Ok(chunk) = rx.try_recv() {
                    streamed.push_str(&chunk);
                    if stream_type && !type_failed {
                        if inputter.type_text(&chunk).is_ok() {
                            typed.push_str(&chunk);
                        } else {
                            type_failed = true;
                        }
                    }
                }
                if !streamed.is_empty() {
                    bubble::set_interim(app, &streamed);
                }
                break result.map(|r| {
                    if r.text.trim().is_empty() && !streamed.trim().is_empty() {
                        streamed.clone()
                    } else {
                        r.text
                    }
                });
            }
            maybe_chunk = rx.recv() => {
                match maybe_chunk {
                    Some(chunk) => {
                        streamed.push_str(&chunk);
                        bubble::set_interim(app, &streamed);
                        if stream_type && !type_failed {
                            if inputter.type_text(&chunk).is_ok() {
                                typed.push_str(&chunk);
                            } else {
                                tracing::warn!("Stream typing failed; remainder will be pasted");
                                type_failed = true;
                            }
                        }
                    }
                    None => tokio::task::yield_now().await,
                }
            }
        }
    };

    (outcome, typed)
}

#[allow(clippy::too_many_arguments)]
async fn process_audio(
    app: &tauri::AppHandle,
    recognizer: &tokio::sync::RwLock<Option<Box<dyn VoiceRecognizer>>>,
    config: &tokio::sync::RwLock<notype_config::AppConfig>,
    inputter: &TextInputter,
    latest_interim_text: &std::sync::Mutex<String>,
    stream_final_gen: &AtomicU64,
    generation: &AtomicU64,
    audio: notype_audio::AudioData,
    from_ui: bool,
) {
    let gen_at_start = generation.load(Ordering::SeqCst);
    let (system_prompt, active_provider, is_asr, postprocess_spec, stream_typing, finalize_ctx) = {
        let cfg = config.read().await;
        let provider_label = match cfg.model.provider {
            notype_config::Provider::Gemini => "gemini",
            notype_config::Provider::Qwen => "qwen",
            notype_config::Provider::Mimo => "mimo",
            notype_config::Provider::Volcengine => "volcengine",
            notype_config::Provider::Whisper => "whisper",
            notype_config::Provider::Apple => "apple",
        };
        let active_app = {
            let state = app.state::<AppState>();
            let value = state.active_app.lock().ok().and_then(|g| g.clone());
            value
        };
        (
            compose_session_prompt(app, &cfg),
            cfg.model.provider.clone(),
            cfg.model.is_asr_pipeline(),
            build_postprocess_spec(&cfg, active_app.as_deref()),
            cfg.general.stream_typing,
            FinalizeCtx {
                from_ui,
                input_mode: cfg.general.input_mode.clone(),
                auto_copy: cfg.general.auto_copy,
                auto_enter: cfg.general.auto_enter,
                replace_rules: cfg.prompts.replace_rules.clone(),
                provider_label: provider_label.to_string(),
                model_name: cfg.model.model_name.clone(),
                duration_secs: audio.duration_secs,
            },
        )
    };
    // Stream typing only makes sense when we're typing at a foreign cursor.
    let can_stream_type =
        stream_typing && !from_ui && finalize_ctx.input_mode == notype_config::InputMode::Keyboard;

    let settle_empty = |app: &tauri::AppHandle| {
        tracing::info!("Empty transcription (silence?)");
        bubble::hide_bubble(app);
        emit_status(app, "Ready", None);
    };

    // -- Stage 1: obtain raw text --
    let is_volc = matches!(active_provider, notype_config::Provider::Volcengine);
    let raw_text: Result<String, String> = if is_volc {
        // The live session already has the audio; wait for its final flush.
        let deadline = Instant::now() + std::time::Duration::from_secs(8);
        while Instant::now() < deadline
            && stream_final_gen.load(Ordering::SeqCst) != gen_at_start
            && generation.load(Ordering::SeqCst) == gen_at_start
        {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        }
        if generation.load(Ordering::SeqCst) != gen_at_start {
            return; // superseded by a newer session
        }

        let preview = read_cached_interim_text(latest_interim_text);
        if stream_final_gen.load(Ordering::SeqCst) == gen_at_start && !preview.is_empty() {
            Ok(preview)
        } else {
            // Live session produced nothing — replay the recording once.
            tracing::warn!("Volcengine live session yielded no text; batch fallback");
            let guard = recognizer.read().await;
            match guard.as_ref() {
                Some(rec) => rec
                    .recognize(audio.wav_bytes.clone(), "audio/wav".into(), String::new())
                    .await
                    .map(|r| r.text)
                    .map_err(|e| e.to_string()),
                None => Err("未配置识别引擎".into()),
            }
        }
    } else if is_asr {
        // Batch ASR engines (Whisper-compatible / Apple / Qwen-ASR).
        let guard = recognizer.read().await;
        match guard.as_ref() {
            Some(rec) => rec
                .recognize(audio.wav_bytes.clone(), "audio/wav".into(), String::new())
                .await
                .map(|r| r.text)
                .map_err(|e| e.to_string()),
            None => Err("未配置识别引擎".into()),
        }
    } else {
        // -- Multimodal single-pass: recognize + polish in one streaming call --
        let guard = recognizer.read().await;
        let Some(rec) = guard.as_ref() else {
            bubble::set_error(app, "未配置识别引擎凭证");
            emit_status(app, "Error", Some("未配置识别引擎凭证"));
            auto_hide_bubble(app, generation, gen_at_start, 3).await;
            return;
        };

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let fut = rec.recognize_stream(
            audio.wav_bytes.clone(),
            "audio/wav".into(),
            system_prompt.clone(),
            tx,
        );
        let (outcome, typed) = consume_text_stream(app, inputter, fut, rx, can_stream_type).await;
        drop(guard);

        match outcome {
            Ok(text) if text.trim().is_empty() => {
                settle_empty(app);
            }
            Ok(text) => {
                emit_final_text(
                    app,
                    inputter,
                    generation,
                    gen_at_start,
                    text.trim(),
                    &typed,
                    &finalize_ctx,
                )
                .await;
            }
            Err(e) => {
                tracing::error!("Recognition failed: {e}");
                bubble::set_error(app, &e.to_string());
                emit_status(app, "Error", Some(&e.to_string()));
                auto_hide_bubble(app, generation, gen_at_start, 5).await;
            }
        }
        return;
    };

    // -- Stage 2 (ASR pipelines): polish the raw text, then deliver --
    let raw = match raw_text {
        Ok(text) if text.trim().is_empty() => {
            settle_empty(app);
            return;
        }
        // Hot-rules run on the raw ASR text so corrections reach the LLM too.
        Ok(text) => notype_config::apply_replace_rules(&finalize_ctx.replace_rules, text.trim()),
        Err(e) => {
            // Last resort: whatever the live preview captured.
            let fallback = read_cached_interim_text(latest_interim_text);
            if !fallback.trim().is_empty() {
                tracing::warn!(error = %e, "ASR failed; falling back to live preview text");
                fallback
            } else {
                tracing::error!("Recognition failed: {e}");
                bubble::set_error(app, &e);
                emit_status(app, "Error", Some(&e));
                auto_hide_bubble(app, generation, gen_at_start, 5).await;
                return;
            }
        }
    };

    match postprocess_spec {
        Some(spec) => {
            let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
            let fut = notype_llm::postprocess_text_stream_to(
                &spec.target,
                spec.system_prompt.clone(),
                raw.clone(),
                tx,
            );
            let (outcome, typed) =
                consume_text_stream(app, inputter, fut, rx, can_stream_type).await;

            match outcome {
                Ok(text) if !text.trim().is_empty() => {
                    emit_final_text(
                        app,
                        inputter,
                        generation,
                        gen_at_start,
                        text.trim(),
                        &typed,
                        &finalize_ctx,
                    )
                    .await;
                }
                Ok(_) => {
                    // Polish produced nothing — deliver the raw ASR text,
                    // accounting for anything stream-typing already typed.
                    emit_final_text(
                        app,
                        inputter,
                        generation,
                        gen_at_start,
                        &raw,
                        &typed,
                        &finalize_ctx,
                    )
                    .await;
                }
                Err(e) => {
                    tracing::warn!("Polish pass failed ({e}); delivering raw ASR text");
                    emit_final_text(
                        app,
                        inputter,
                        generation,
                        gen_at_start,
                        &raw,
                        &typed,
                        &finalize_ctx,
                    )
                    .await;
                }
            }
        }
        None => {
            emit_final_text(
                app,
                inputter,
                generation,
                gen_at_start,
                &raw,
                "",
                &finalize_ctx,
            )
            .await;
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
        notype_config::Provider::Mimo => notype_llm::Provider::Mimo,
        notype_config::Provider::Volcengine => notype_llm::Provider::Volcengine,
        notype_config::Provider::Whisper => notype_llm::Provider::Whisper,
        notype_config::Provider::Apple => notype_llm::Provider::Apple,
    };

    // Whisper engines have a dedicated model field; others share model_name.
    let model = match config.model.provider {
        notype_config::Provider::Whisper => Some(config.model.whisper_model.clone()),
        _ => Some(config.model.model_name.clone()),
    };

    Some(notype_llm::create_recognizer(
        provider,
        config.model.active_api_key().to_string(),
        notype_llm::RecognizerOptions {
            model,
            qwen_base_url: Some(config.model.qwen_base_url.clone()),
            mimo_base_url: Some(config.model.mimo_base_url.clone()),
            volc_app_key: Some(config.model.volc_app_key.clone()),
            volc_access_key: Some(config.model.volc_access_key.clone()),
            volc_resource_id: Some(config.model.volc_resource_id.clone()),
            whisper_base_url: Some(config.model.whisper_base_url.clone()),
            apple_locale: Some(config.model.apple_locale.clone()),
        },
    ))
}

#[cfg(desktop)]
fn reregister_shortcut(
    app: &tauri::AppHandle,
    hotkey_str: &str,
    edit_hotkey_str: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;

    let shortcut = parse_hotkey(hotkey_str)?;
    let edit_shortcut = resolve_edit_shortcut(edit_hotkey_str, &shortcut);
    // Unregister all, then re-register the new set
    app.global_shortcut().unregister_all()?;
    app.global_shortcut().register(shortcut)?;
    if let Some(edit_sc) = edit_shortcut {
        if let Err(e) = app.global_shortcut().register(edit_sc) {
            tracing::warn!("Failed to re-register voice-edit hotkey: {e}");
        }
    }
    Ok(())
}

#[cfg(not(desktop))]
fn reregister_shortcut(
    _app: &tauri::AppHandle,
    _hotkey_str: &str,
    _edit_hotkey_str: &str,
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
