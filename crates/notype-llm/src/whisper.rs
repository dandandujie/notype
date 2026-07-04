//! OpenAI Whisper-compatible batch ASR client.
//!
//! Talks to any `/v1/audio/transcriptions` endpoint: OpenAI, Groq,
//! whisper.cpp server, faster-whisper-server, vLLM deployments, etc.
//! `base_url` should point at the API root (e.g. `https://api.openai.com/v1`).

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

pub const DEFAULT_BASE_URL: &str = "https://api.openai.com/v1";
pub const DEFAULT_MODEL: &str = "whisper-1";

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
const MAX_RETRIES: u32 = 2;

pub struct WhisperClient {
    api_key: String,
    model: String,
    base_url: String,
    client: reqwest::Client,
}

impl WhisperClient {
    pub fn new(api_key: String, model: Option<String>, base_url: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        let base_url = base_url
            .map(|u| u.trim().trim_end_matches('/').to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.into());
        Self {
            api_key,
            model: model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
            base_url,
            client,
        }
    }

    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        let url = format!("{}/audio/transcriptions", self.base_url);
        let ext = mime_type.strip_prefix("audio/").unwrap_or("wav");
        let mut last_err = LlmError::RequestFailed("no attempts".into());

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(500 * u64::from(attempt)))
                    .await;
                tracing::warn!(attempt, "Retrying Whisper-compatible request");
            }

            let part = reqwest::multipart::Part::bytes(audio_data.clone())
                .file_name(format!("audio.{ext}"))
                .mime_str(&mime_type)
                .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
            let form = reqwest::multipart::Form::new()
                .part("file", part)
                .text("model", self.model.clone())
                .text("response_format", "json");

            let mut request = self.client.post(&url).multipart(form);
            if !self.api_key.trim().is_empty() {
                request = request.bearer_auth(self.api_key.trim());
            }

            match request.send().await {
                Ok(response) => {
                    let status = response.status();
                    if status == reqwest::StatusCode::UNAUTHORIZED
                        || status == reqwest::StatusCode::FORBIDDEN
                    {
                        return Err(LlmError::InvalidApiKey);
                    }
                    if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
                    {
                        last_err = LlmError::RequestFailed(format!("HTTP {status}"));
                        continue;
                    }
                    if !status.is_success() {
                        let body = response.text().await.unwrap_or_default();
                        return Err(LlmError::RequestFailed(format!("HTTP {status}: {body}")));
                    }

                    let json: serde_json::Value = response
                        .json()
                        .await
                        .map_err(|e| LlmError::RequestFailed(e.to_string()))?;
                    let text = json
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .trim()
                        .to_string();
                    tracing::info!(
                        chars = text.chars().count(),
                        model = %self.model,
                        "Whisper-compatible transcription received"
                    );
                    if let Some(tx) = &tx {
                        if !text.is_empty() {
                            let _ = tx.send(text.clone());
                        }
                    }
                    return Ok(RecognitionResult { text });
                }
                Err(e) if e.is_timeout() || e.is_connect() => {
                    last_err = LlmError::RequestFailed(e.to_string());
                    continue;
                }
                Err(e) => return Err(LlmError::HttpError(e)),
            }
        }

        Err(last_err)
    }
}

impl VoiceRecognizer for WhisperClient {
    fn recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        _system_prompt: String,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, mime_type, None))
    }

    fn recognize_stream(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        _system_prompt: String,
        tx: mpsc::UnboundedSender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, mime_type, Some(tx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_is_normalized() {
        let c = WhisperClient::new("k".into(), None, Some("http://127.0.0.1:8080/v1/".into()));
        assert_eq!(c.base_url, "http://127.0.0.1:8080/v1");
        assert_eq!(c.model, "whisper-1");
    }

    #[test]
    fn defaults_apply() {
        let c = WhisperClient::new("k".into(), Some("  ".into()), None);
        assert_eq!(c.base_url, DEFAULT_BASE_URL);
        assert_eq!(c.model, DEFAULT_MODEL);
    }
}
