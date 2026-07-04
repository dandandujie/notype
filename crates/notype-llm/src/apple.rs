//! Apple Speech (SFSpeechRecognizer) client — on-device recognition on macOS.
//!
//! Free, offline, no API key. Requires the Speech Recognition privacy
//! permission (`NSSpeechRecognitionUsageDescription` in Info.plist); macOS
//! prompts the user on first use.
//!
//! GOTCHA: SFSpeechRecognizer objects are not `Send`, so each request runs on
//! a dedicated blocking thread and communicates via channels.

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

pub struct AppleSpeechClient {
    /// BCP-47 locale like "zh-CN"; empty = system default.
    locale: String,
}

impl AppleSpeechClient {
    pub fn new(locale: Option<String>) -> Self {
        Self {
            locale: locale.unwrap_or_default().trim().to_string(),
        }
    }

    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        let locale = self.locale.clone();
        let text = tokio::task::spawn_blocking(move || recognize_blocking(&audio_data, &locale))
            .await
            .map_err(|e| LlmError::RequestFailed(format!("apple speech task: {e}")))??;

        if let Some(tx) = &tx {
            if !text.is_empty() {
                let _ = tx.send(text.clone());
            }
        }
        Ok(RecognitionResult { text })
    }
}

impl VoiceRecognizer for AppleSpeechClient {
    fn recognize(
        &self,
        audio_data: Vec<u8>,
        _mime_type: String,
        _system_prompt: String,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, None))
    }

    fn recognize_stream(
        &self,
        audio_data: Vec<u8>,
        _mime_type: String,
        _system_prompt: String,
        tx: mpsc::UnboundedSender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, Some(tx)))
    }
}

#[cfg(target_os = "macos")]
fn recognize_blocking(wav_bytes: &[u8], locale: &str) -> Result<String> {
    use block2::RcBlock;
    use objc2::AnyThread;
    use objc2_foundation::{NSLocale, NSString, NSURL};
    use objc2_speech::{
        SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus, SFSpeechURLRecognitionRequest,
    };

    // 1. Persist the audio to a temp file (SFSpeechURLRecognitionRequest is file-based).
    let path = std::env::temp_dir().join(format!("notype-asr-{}.wav", uuid::Uuid::new_v4()));
    std::fs::write(&path, wav_bytes)
        .map_err(|e| LlmError::RequestFailed(format!("temp wav write: {e}")))?;
    // Best-effort cleanup on all exits.
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());

    // 2. Ensure speech-recognition authorization.
    let (auth_tx, auth_rx) = std::sync::mpsc::channel::<isize>();
    unsafe {
        let block = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| {
            let _ = auth_tx.send(status.0);
        });
        SFSpeechRecognizer::requestAuthorization(&block);
    }
    let status = auth_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .map_err(|_| LlmError::RequestFailed("语音识别授权等待超时".into()))?;
    if status != SFSpeechRecognizerAuthorizationStatus::Authorized.0 {
        return Err(LlmError::RequestFailed(
            "未授权语音识别：请在 系统设置 → 隐私与安全性 → 语音识别 中允许 NoType".into(),
        ));
    }

    // 3. Build recognizer (+ optional locale) and file request.
    let (result_tx, result_rx) = std::sync::mpsc::channel::<std::result::Result<String, String>>();
    unsafe {
        let recognizer = if locale.is_empty() {
            Some(SFSpeechRecognizer::new())
        } else {
            let identifier = NSString::from_str(locale);
            let ns_locale = NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &identifier);
            SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &ns_locale)
        };
        let Some(recognizer) = recognizer else {
            return Err(LlmError::RequestFailed(
                "该语言不支持 Apple 语音识别".into(),
            ));
        };
        if !recognizer.isAvailable() {
            return Err(LlmError::RequestFailed(
                "Apple 语音识别当前不可用（检查网络或系统设置）".into(),
            ));
        }

        let ns_path = NSString::from_str(&path.to_string_lossy());
        let url = NSURL::fileURLWithPath(&ns_path);
        let request =
            SFSpeechURLRecognitionRequest::initWithURL(SFSpeechURLRecognitionRequest::alloc(), &url);

        let handler = RcBlock::new(
            move |result: *mut objc2_speech::SFSpeechRecognitionResult,
                  error: *mut objc2_foundation::NSError| {
                if !result.is_null() {
                    let result = &*result;
                    if result.isFinal() {
                        let text = result.bestTranscription().formattedString().to_string();
                        let _ = result_tx.send(Ok(text));
                        return;
                    }
                }
                if !error.is_null() {
                    let error = &*error;
                    let _ = result_tx.send(Err(error.localizedDescription().to_string()));
                }
            },
        );

        // Task object must stay alive until the handler fires; hold the retained pointer.
        let _task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler);

        match result_rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(Ok(text)) => {
                tracing::info!(chars = text.chars().count(), "Apple Speech transcription received");
                Ok(text.trim().to_string())
            }
            Ok(Err(msg)) => {
                // "No speech detected" style errors → treat as silence.
                if msg.contains("No speech") || msg.contains("1110") {
                    Ok(String::new())
                } else {
                    Err(LlmError::RequestFailed(format!("Apple 语音识别失败: {msg}")))
                }
            }
            Err(_) => Err(LlmError::RequestFailed("Apple 语音识别超时".into())),
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn recognize_blocking(_wav_bytes: &[u8], _locale: &str) -> Result<String> {
    Err(LlmError::RequestFailed(
        "Apple Speech 仅在 macOS 上可用".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_is_trimmed() {
        let c = AppleSpeechClient::new(Some("  zh-CN  ".into()));
        assert_eq!(c.locale, "zh-CN");
        let d = AppleSpeechClient::new(None);
        assert_eq!(d.locale, "");
    }
}
