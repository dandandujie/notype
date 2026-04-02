//! Google Gemini API client for voice recognition.
//!
//! Sends audio as base64 inline_data to the generateContent endpoint.
//! API reference: generativelanguage.googleapis.com/v1beta/models/{model}:generateContent

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use std::future::Future;
use std::pin::Pin;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

const DEFAULT_MODEL: &str = "gemini-3-flash-preview";
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MAX_RETRIES: u32 = 2;

pub struct GeminiClient {
    api_key: String,
    model: String,
    client: reqwest::Client,
}

impl GeminiClient {
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

impl VoiceRecognizer for GeminiClient {
    fn recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, mime_type, system_prompt))
    }
}

impl GeminiClient {
    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
    ) -> Result<RecognitionResult> {
        let audio_base64 = BASE64.encode(&audio_data);

        let body = serde_json::json!({
            "contents": [{
                "parts": [
                    { "text": system_prompt },
                    {
                        "inline_data": {
                            "mime_type": &mime_type,
                            "data": audio_base64
                        }
                    }
                ]
            }],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 4096
            }
        });

        let url = format!(
            "{API_BASE}/models/{}:generateContent?key={}",
            self.model, self.api_key
        );

        tracing::debug!(model = %self.model, audio_bytes = audio_data.len(), "Sending to Gemini");

        let resp_body = self.send_with_retry(&url, &body).await?;

        // Check for API error
        if let Some(error) = resp_body.get("error") {
            let msg = error
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error");
            return Err(LlmError::RequestFailed(msg.into()));
        }

        // Extract text: candidates[0].content.parts[0].text
        let text = resp_body
            .pointer("/candidates/0/content/parts/0/text")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        if text.is_empty() {
            tracing::warn!("Gemini returned empty transcription");
        } else {
            tracing::info!(chars = text.len(), "Gemini transcription received");
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
                tracing::warn!(attempt, "Retrying Gemini request after {delay:?}");
                tokio::time::sleep(delay).await;
            }

            let result = self.client.post(url).json(body).send().await;
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
        let client = GeminiClient::new("test-key".into(), None);
        assert_eq!(client.model, "gemini-3-flash-preview");
    }

    #[test]
    fn test_custom_model() {
        let client = GeminiClient::new("test-key".into(), Some("gemini-3.1-flash-lite".into()));
        assert_eq!(client.model, "gemini-3.1-flash-lite");
    }
}
