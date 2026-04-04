//! Alibaba Qwen API client for voice recognition.
//!
//! Uses OpenAI-compatible chat/completions endpoint with audio content type.
//! API reference: dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

const DEFAULT_MODEL: &str = "qwen3.5-omni-flash";
const API_BASE: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
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

impl QwenClient {
    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        mime_type: String,
        system_prompt: String,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        let audio_base64 = BASE64.encode(&audio_data);
        let data_uri = format!("data:{mime_type};base64,{audio_base64}");
        let format = mime_type.strip_prefix("audio/").unwrap_or("wav");

        let body = serde_json::json!({
            "model": self.model,
            "messages": [
                { "role": "system", "content": system_prompt },
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_audio",
                            "input_audio": { "format": format, "data": data_uri }
                        },
                        { "type": "text", "text": "Transcribe this audio completely from start to end. Do not omit, summarize, or skip any part. Output every sentence exactly as spoken." }
                    ]
                }
            ],
            "modalities": ["text"],
            "stream": true,
            "temperature": 0.0,
            "max_tokens": 8192
        });

        let url = format!("{API_BASE}/chat/completions");
        tracing::debug!(model = %self.model, audio_bytes = audio_data.len(), "Sending to Qwen (streaming)");

        let text = self.send_stream(&url, &body, tx).await?;

        if text.is_empty() {
            tracing::warn!("Qwen returned empty transcription");
        } else {
            tracing::info!(chars = text.len(), "Qwen transcription received");
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

    /// Read SSE stream incrementally, sending each chunk through tx if provided.
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
            tracing::debug!(chunk_idx, bytes = bytes.len(), "SSE network chunk received");
            raw_buf.extend_from_slice(&bytes);

            // Only decode complete lines (newline-terminated) to avoid splitting UTF-8
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

                if line.starts_with("data: {") && line.contains("\"error\"") {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(error) = json.get("error") {
                                let msg = error
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("unknown error");
                                return Err(LlmError::RequestFailed(msg.into()));
                            }
                        }
                    }
                }

                if line == "data: [DONE]" {
                    return Ok(full_text.trim().to_string());
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(data) {
                        // Log finish_reason for diagnostics
                        if let Some(reason) = chunk
                            .pointer("/choices/0/finish_reason")
                            .and_then(|v| v.as_str())
                        {
                            tracing::info!(finish_reason = %reason, "Qwen stream finished");
                        }
                        if let Some(content) = chunk
                            .pointer("/choices/0/delta/content")
                            .and_then(|v| v.as_str())
                        {
                            if !content.is_empty() {
                                tracing::info!(chunk = %content, total_len = full_text.len() + content.len(), "Qwen stream delta");
                                full_text.push_str(content);
                                if let Some(tx) = tx {
                                    let _ = tx.send(content.to_string());
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
