//! LLM API client module for NoType.
//!
//! Abstracts multimodal/ASR providers (Gemini, Qwen, Doubao) behind a unified
//! trait for voice-to-text recognition.

pub mod doubao;
pub mod gemini;
pub mod qwen;

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

/// Supported LLM providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum Provider {
    Gemini,
    Qwen,
    Doubao,
}

#[derive(Debug, Clone, Default)]
pub struct RecognizerOptions {
    pub model: Option<String>,
    pub doubao_base_url: Option<String>,
    pub doubao_official_app_key: Option<String>,
    pub doubao_official_access_key: Option<String>,
}

/// Create a recognizer from provider config.
pub fn create_recognizer(
    provider: Provider,
    api_key: String,
    options: RecognizerOptions,
) -> Box<dyn VoiceRecognizer> {
    match provider {
        Provider::Gemini => Box::new(gemini::GeminiClient::new(api_key, options.model)),
        Provider::Qwen => Box::new(qwen::QwenClient::new(api_key, options.model)),
        Provider::Doubao => Box::new(doubao::DoubaoClient::new(
            api_key,
            options.model,
            options.doubao_base_url,
            options.doubao_official_app_key,
            options.doubao_official_access_key,
        )),
    }
}

/// Stream post-processing for ASR raw text.
/// Currently supported providers: Gemini, Qwen.
pub async fn postprocess_text_stream(
    provider: Provider,
    api_key: String,
    model: Option<String>,
    system_prompt: String,
    raw_text: String,
    tx: mpsc::UnboundedSender<String>,
) -> Result<RecognitionResult> {
    match provider {
        Provider::Gemini => {
            let client = gemini::GeminiClient::new(api_key, model);
            client
                .postprocess_text_stream(system_prompt, raw_text, tx)
                .await
        }
        Provider::Qwen => {
            let client = qwen::QwenClient::new(api_key, model);
            client
                .postprocess_text_stream(system_prompt, raw_text, tx)
                .await
        }
        Provider::Doubao => Err(LlmError::ModelNotAvailable(
            "Doubao ASR is not a text post-process provider".into(),
        )),
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
