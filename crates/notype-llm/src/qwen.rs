//! Alibaba Qwen API client for voice recognition.
//!
//! Uses OpenAI-compatible chat/completions endpoint with audio content type.
//! API reference: dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use std::future::Future;
use std::pin::Pin;

use crate::{prompt, LlmError, RecognitionResult, Result, VoiceRecognizer};

const DEFAULT_MODEL: &str = "qwen3.5-omni-flash";
const API_BASE: &str = "https://dashscope-intl.aliyuncs.com/compatible-mode/v1";

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_RETRIES: u32 = 2;

pub struct QwenClient {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl QwenClient {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self {
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
            client,
        }
    }
}

impl VoiceRecognizer for QwenClient {
    fn recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, mime_type))
    }
}

impl QwenClient {
    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
    ) -> Result<RecognitionResult> {
        let audio_base64 = BASE64.encode(&audio_data);

        // Extract format from mime_type: "audio/wav" -> "wav"
        let format = mime_type.strip_prefix("audio/").unwrap_or("wav");

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                {
                    "role": "system",
                    "content": prompt::TRANSCRIPTION_PROMPT
                },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "audio",
                            "audio": {
                                "format": format,
                                "data": audio_base64
                            }
                        },
                        {
                            "type": "text",
                            "text": "Transcribe this audio."
                        }
                    ]
                }
            ],
            "modalities": ["text"],
            "temperature": 0.0,
            "max_tokens": 4096
        });

        let url = format!("{API_BASE}/chat/completions");

        tracing::debug!(model = %self.model, audio_bytes = audio_data.len(), "Sending to Qwen");

        let resp_body = self.send_with_retry(&url, &body).await?;

        // Check for API error
        if let Some(error) = resp_body.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(LlmError::RequestFailed(msg.into()));
        }

        // Extract: choices[0].message.content
        let text = resp_body
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if text.is_empty() {
            tracing::warn!("Qwen returned empty transcription");
        } else {
            tracing::info!(chars = text.len(), "Qwen transcription received");
        }

        Ok(RecognitionResult { text })
    }

    async fn send_with_retry(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut last_err = LlmError::RequestFailed("no attempts".into());

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * u64::from(attempt));
                tracing::warn!(attempt, "Retrying Qwen request after {delay:?}");
                tokio::time::sleep(delay).await;
            }

            let result = self
                .client
                .post(url)
                .bearer_auth(&self.api_key)
                .json(body)
                .send()
                .await;

            match result {
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
                    return response
                        .json()
                        .await
                        .map_err(|e| LlmError::RequestFailed(e.to_string()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_model() {
        let client = QwenClient::new("test-key".into(), None);
        assert_eq!(client.model, "qwen3.5-omni-flash");
    }

    #[test]
    fn test_custom_model() {
        let client = QwenClient::new("test-key".into(), Some("qwen3.5-omni-plus".into()));
        assert_eq!(client.model, "qwen3.5-omni-plus");
    }

    #[test]
    fn test_mime_type_parsing() {
        let format = "audio/wav".strip_prefix("audio/").unwrap_or("wav");
        assert_eq!(format, "wav");

        let format = "audio/mp3".strip_prefix("audio/").unwrap_or("wav");
        assert_eq!(format, "mp3");
    }
}
