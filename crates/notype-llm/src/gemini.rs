//! Google Gemini API client for voice recognition.
//!
//! Sends audio as base64 inline_data to the generateContent endpoint.
//! API reference: generativelanguage.googleapis.com/v1beta/models/{model}:generateContent

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

const DEFAULT_MODEL: &str = "gemini-3-flash-preview";
const API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
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
        Box::pin(self.do_recognize(audio_data, mime_type, system_prompt, None))
    }

    fn recognize_stream(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
        tx: mpsc::UnboundedSender<String>,
    ) -> Pin<Box<dyn Future<Output = Result<RecognitionResult>> + Send + '_>> {
        Box::pin(self.do_recognize(audio_data, mime_type, system_prompt, Some(tx)))
    }
}

impl GeminiClient {
    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        let audio_base64 = BASE64.encode(&audio_data);

        let body = serde_json::json!({
            "systemInstruction": {
                "parts": [{ "text": system_prompt }]
            },
            "contents": [{
                "parts": [
                    {
                        "inline_data": {
                            "mime_type": &mime_type,
                            "data": audio_base64
                        }
                    },
                    { "text": "Transcribe this audio completely from start to end. Do not omit, summarize, or skip any part. Output every sentence exactly as spoken." }
                ]
            }],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 8192
            }
        });

        // Use streamGenerateContent with alt=sse for streaming
        let url = format!(
            "{API_BASE}/models/{}:streamGenerateContent?alt=sse&key={}",
            self.model, self.api_key
        );

        tracing::debug!(model = %self.model, audio_bytes = audio_data.len(), "Sending to Gemini (streaming)");

        let text = self.send_stream(&url, &body, tx).await?;

        if text.is_empty() {
            tracing::warn!("Gemini returned empty transcription");
        } else {
            tracing::info!(chars = text.len(), "Gemini transcription received");
        }

        Ok(RecognitionResult { text })
    }

    async fn send_stream(
        &self,
        url: &str,
        body: &serde_json::Value,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
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
                    if !status.is_success() {
                        let body_text = response.text().await.unwrap_or_default();
                        return Err(LlmError::RequestFailed(body_text));
                    }
                    return self.read_sse_stream(response, &tx).await;
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

    /// Parse Gemini SSE stream. Each event has `data: {JSON}` with
    /// candidates[0].content.parts[0].text containing the text chunk.
    /// Buffers raw bytes to avoid corrupting multibyte UTF-8 at chunk boundaries.
    async fn read_sse_stream(
        &self,
        response: reqwest::Response,
        tx: &Option<mpsc::UnboundedSender<String>>,
    ) -> Result<String> {
        use futures_util::StreamExt;

        let mut full_text = String::new();
        let mut raw_buf: Vec<u8> = Vec::new();
        let mut stream = response.bytes_stream();

        let mut chunk_idx: u32 = 0;
        while let Some(chunk_result) = stream.next().await {
            let bytes = chunk_result.map_err(|e| LlmError::RequestFailed(e.to_string()))?;
            chunk_idx += 1;
            tracing::debug!(chunk_idx, bytes = bytes.len(), "Gemini SSE network chunk received");
            raw_buf.extend_from_slice(&bytes);

            while let Some(newline_pos) = raw_buf.iter().position(|&b| b == b'\n') {
                let line_bytes = raw_buf[..newline_pos].to_vec();
                raw_buf = raw_buf[newline_pos + 1..].to_vec();

                let line = String::from_utf8(line_bytes)
                    .unwrap_or_default()
                    .trim()
                    .to_string();

                if line.is_empty() {
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                        // Check for API error
                        if let Some(error) = json.get("error") {
                            let msg = error
                                .get("message")
                                .and_then(|m| m.as_str())
                                .unwrap_or("unknown error");
                            return Err(LlmError::RequestFailed(msg.into()));
                        }
                        // Extract text from candidates[0].content.parts[*].text
                        if let Some(parts) = json
                            .pointer("/candidates/0/content/parts")
                            .and_then(|v| v.as_array())
                        {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                                    if !text.is_empty() {
                                        tracing::info!(chunk = %text, total_len = full_text.len() + text.len(), "Gemini stream delta");
                                        full_text.push_str(text);
                                        if let Some(tx) = tx {
                                            let _ = tx.send(text.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(full_text.trim().to_string())
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
