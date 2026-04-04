//! LLM API client module for NoType.
//!
//! Abstracts multimodal LLM providers (Gemini, Qwen) behind a unified
//! trait for voice-to-text recognition.

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
}

/// Create a recognizer from provider config.
pub fn create_recognizer(
    provider: Provider,
    api_key: String,
    model: Option<String>,
) -> Box<dyn VoiceRecognizer> {
    match provider {
        Provider::Gemini => Box::new(gemini::GeminiClient::new(api_key, model)),
        Provider::Qwen => Box::new(qwen::QwenClient::new(api_key, model)),
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
