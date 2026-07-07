//! ASR + LLM client module for NoType.
//!
//! Two kinds of engines behind one trait:
//! - Multimodal LLMs that transcribe + polish in one call (Gemini, Qwen-Omni, MiMo)
//! - Dedicated ASR engines whose raw text is polished by a separate LLM pass
//!   (Volcengine streaming, Whisper-compatible batch, Apple Speech, Qwen-ASR)
//!
//! Post-processing (`postprocess_text_stream`) accepts any OpenAI-compatible
//! chat endpoint, so the polish step works with arbitrary LLM vendors.

pub mod apple;
pub mod gemini;
pub mod gpt_realtime;
pub mod mimo;
pub mod qwen;
pub mod volcengine;
pub mod whisper;

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("API request failed: {0}")]
    RequestFailed(String),
    #[error("invalid API key")]
    InvalidApiKey,
    #[error("model not available: {0}")]
    ModelNotAvailable(String),
    #[error("empty response from model")]
    EmptyResponse,
    #[error("HTTP error: {0}")]
    HttpError(#[from] reqwest::Error),
}

pub type Result<T> = std::result::Result<T, LlmError>;

/// Result of a voice recognition request.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RecognitionResult {
    pub text: String,
}

/// Unified trait for voice recognition providers (object-safe).
pub trait VoiceRecognizer: Send + Sync {
    /// Recognize speech from audio data with a custom system prompt.
    /// ASR-only engines ignore the prompt.
    fn recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>>;

    /// Streaming recognition: sends text chunks through the channel as they arrive.
    /// Returns the full concatenated text when done.
    fn recognize_stream(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
        tx: mpsc::UnboundedSender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>>;
}

/// Supported recognition providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Provider {
    Gemini,
    Qwen,
    Mimo,
    Volcengine,
    Whisper,
    Apple,
    GptRealtime,
}

#[derive(Debug, Clone, Default)]
pub struct RecognizerOptions {
    pub model: Option<String>,
    /// Custom OpenAI-compatible endpoint for Qwen (cloud default: DashScope).
    pub qwen_base_url: Option<String>,
    pub mimo_base_url: Option<String>,
    /// Volcengine streaming ASR credentials.
    pub volc_app_key: Option<String>,
    pub volc_access_key: Option<String>,
    pub volc_resource_id: Option<String>,
    /// Whisper-compatible endpoint root (e.g. https://api.openai.com/v1).
    pub whisper_base_url: Option<String>,
    /// Apple Speech locale (empty = system default).
    pub apple_locale: Option<String>,
    /// OpenAI API key for the Realtime transcription engine.
    pub openai_api_key: Option<String>,
}

/// Create a recognizer from provider config. `api_key` is the key of the
/// selected provider (Volcengine uses the dedicated volc_* options instead).
pub fn create_recognizer(
    provider: Provider,
    api_key: String,
    options: RecognizerOptions,
) -> Box<dyn VoiceRecognizer> {
    match provider {
        Provider::Gemini => Box::new(gemini::GeminiClient::new(api_key, options.model)),
        Provider::Qwen => Box::new(qwen::QwenClient::with_base_url(
            api_key,
            options.model,
            options.qwen_base_url,
        )),
        Provider::Mimo => Box::new(mimo::MimoClient::new(
            api_key,
            options.model,
            options.mimo_base_url,
        )),
        Provider::Volcengine => Box::new(volcengine::VolcengineClient::new(
            options.volc_app_key.unwrap_or_default(),
            options.volc_access_key.unwrap_or_default(),
            options.volc_resource_id,
        )),
        Provider::Whisper => Box::new(whisper::WhisperClient::new(
            api_key,
            options.model,
            options.whisper_base_url,
        )),
        Provider::Apple => Box::new(apple::AppleSpeechClient::new(options.apple_locale)),
        Provider::GptRealtime => Box::new(gpt_realtime::GptRealtimeClient::new(
            options.openai_api_key.unwrap_or(api_key),
            options.model,
        )),
    }
}

/// A text-capable LLM endpoint for the polish/post-process pass.
/// `base_url = None` selects each provider's official endpoint; any
/// OpenAI-compatible service works by setting `base_url`.
#[derive(Debug, Clone)]
pub struct TextLlmTarget {
    pub kind: TextLlmKind,
    pub api_key: String,
    pub model: String,
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextLlmKind {
    /// Any OpenAI-compatible chat/completions endpoint (incl. DashScope/Qwen).
    OpenAiCompatible,
    Gemini,
    Mimo,
}

/// Stream post-processing for ASR raw text through the chosen text LLM.
pub async fn postprocess_text_stream_to(
    target: &TextLlmTarget,
    system_prompt: String,
    raw_text: String,
    tx: mpsc::UnboundedSender<String>,
) -> Result<RecognitionResult> {
    match target.kind {
        TextLlmKind::OpenAiCompatible => {
            let client = qwen::QwenClient::with_base_url(
                target.api_key.clone(),
                Some(target.model.clone()),
                target.base_url.clone(),
            );
            client
                .postprocess_text_stream(system_prompt, raw_text, tx)
                .await
        }
        TextLlmKind::Gemini => {
            let client =
                gemini::GeminiClient::new(target.api_key.clone(), Some(target.model.clone()));
            client
                .postprocess_text_stream(system_prompt, raw_text, tx)
                .await
        }
        TextLlmKind::Mimo => {
            let client = mimo::MimoClient::new(
                target.api_key.clone(),
                Some(target.model.clone()),
                target.base_url.clone(),
            );
            client
                .postprocess_text_stream(system_prompt, raw_text, tx)
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recognition_result_serialization() {
        let result = RecognitionResult {
            text: "hello world".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("hello world"));
    }
}
