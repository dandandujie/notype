//! Doubao ASR client.
//!
//! Supports two backend modes:
//! - asr2api: OpenAI-compatible `/v1/audio/transcriptions` gateway
//! - official: direct ByteDance OpenSpeech file ASR API (flash/standard)

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

const DEFAULT_MODEL: &str = "doubao-asr";
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8000";

const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

const OFFICIAL_STANDARD_SUBMIT_ENDPOINT: &str =
    "https://openspeech.bytedance.com/api/v3/auc/bigmodel/submit";
const OFFICIAL_STANDARD_QUERY_ENDPOINT: &str =
    "https://openspeech.bytedance.com/api/v3/auc/bigmodel/query";
const OFFICIAL_FLASH_ENDPOINT: &str =
    "https://openspeech.bytedance.com/api/v3/auc/bigmodel/recognize/flash";
const OFFICIAL_STANDARD_RESOURCE_ID: &str = "volc.seedasr.auc";
const OFFICIAL_FLASH_RESOURCE_ID: &str = "volc.bigasr.auc_turbo";
const OFFICIAL_MODEL_NAME: &str = "bigmodel";
const OFFICIAL_QUERY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
const OFFICIAL_QUERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

#[derive(Debug, Clone, Copy)]
enum BackendMode {
    Asr2Api,
    Official(OfficialMode),
}

#[derive(Debug, Clone, Copy)]
enum OfficialMode {
    Standard,
    Flash,
}

pub struct DoubaoClient {
    api_key: String,
    model: String,
    base_url: String,
    official_app_key: Option<String>,
    official_access_key: Option<String>,
    client: reqwest::Client,
}

impl DoubaoClient {
    pub fn new(
        api_key: String,
        model: Option<String>,
        base_url: Option<String>,
        official_app_key: Option<String>,
        official_access_key: Option<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .no_proxy()
            .build()
            .unwrap_or_default();

        Self {
            api_key,
            model: model.unwrap_or_else(|| DEFAULT_MODEL.into()),
            base_url: base_url.unwrap_or_else(|| DEFAULT_BASE_URL.into()),
            official_app_key,
            official_access_key,
            client,
        }
    }

    fn backend_mode(&self) -> BackendMode {
        let model = self.model.trim().to_lowercase();
        if matches!(
            model.as_str(),
            "doubao-asr-official-standard" | "official-standard" | "standard"
        ) {
            return BackendMode::Official(OfficialMode::Standard);
        }

        if matches!(
            model.as_str(),
            "doubao-asr-official"
                | "doubao-asr-official-flash"
                | "official"
                | "official-flash"
                | "flash"
        ) {
            return BackendMode::Official(OfficialMode::Flash);
        }

        BackendMode::Asr2Api
    }
}

impl VoiceRecognizer for DoubaoClient {
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

impl DoubaoClient {
    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        let text = match self.backend_mode() {
            BackendMode::Asr2Api => self.transcribe_via_asr2api(audio_data).await?,
            BackendMode::Official(mode) => self.transcribe_via_official(audio_data, mode).await?,
        };

        if let Some(tx) = tx {
            if !text.is_empty() {
                let _ = tx.send(text.clone());
            }
        }

        Ok(RecognitionResult { text })
    }

    async fn transcribe_via_asr2api(&self, audio_data: Vec<u8>) -> Result<String> {
        let base = self.base_url.trim_end_matches('/');
        let url = format!("{base}/v1/audio/transcriptions");

        let part = reqwest::multipart::Part::bytes(audio_data)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| LlmError::RequestFailed(e.to_string()))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", self.model.clone())
            .text("response_format", "json");

        let mut request = self.client.post(url).multipart(form);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }

        let response = request.send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(LlmError::InvalidApiKey);
        }

        let headers = response.headers().clone();
        let body = response.text().await.unwrap_or_default();
        let body_lower = body.to_lowercase();
        if !status.is_success() {
            if body_lower.contains("exceededconcurrentquota")
                || body_lower.contains("concurrentquota")
                || (body_lower.contains("concurrent") && body_lower.contains("quota"))
            {
                return Ok(String::new());
            }
            if status == reqwest::StatusCode::BAD_GATEWAY
                && self.base_url.contains("127.0.0.1")
                && body.trim().is_empty()
            {
                return Err(LlmError::RequestFailed(
                    "HTTP 502 Bad Gateway: local asr2api unavailable (check 127.0.0.1:8000) or proxy intercepted localhost. Try starting asr2api service, or switch model to doubao-asr-official-*.".into(),
                ));
            }
            return Err(LlmError::RequestFailed(format!("HTTP {status}: {body}")));
        }

        let is_json = headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.contains("application/json"))
            .unwrap_or(true);

        if !is_json {
            return Ok(body.trim().to_string());
        }

        let payload = serde_json::from_str::<serde_json::Value>(&body)
            .map_err(|e| LlmError::RequestFailed(format!("Invalid JSON response: {e}")))?;

        Ok(Self::extract_transcription_text(&payload))
    }

    async fn transcribe_via_official(
        &self,
        audio_data: Vec<u8>,
        mode: OfficialMode,
    ) -> Result<String> {
        let app_key = self
            .official_app_key
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| LlmError::RequestFailed("DOUBAO official app key is missing".into()))?;
        let access_key = self
            .official_access_key
            .as_ref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                LlmError::RequestFailed("DOUBAO official access key is missing".into())
            })?;

        match mode {
            OfficialMode::Flash => self.official_flash(audio_data, app_key, access_key).await,
            OfficialMode::Standard => {
                self.official_standard(audio_data, app_key, access_key)
                    .await
            }
        }
    }

    async fn official_flash(
        &self,
        audio_data: Vec<u8>,
        app_key: &str,
        access_key: &str,
    ) -> Result<String> {
        let request_id = Uuid::new_v4().to_string();
        let body = serde_json::json!({
            "user": { "uid": app_key },
            "audio": { "data": BASE64.encode(audio_data) },
            "request": { "model_name": OFFICIAL_MODEL_NAME }
        });

        let response = self
            .client
            .post(OFFICIAL_FLASH_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("X-Api-App-Key", app_key)
            .header("X-Api-Access-Key", access_key)
            .header("X-Api-Resource-Id", OFFICIAL_FLASH_RESOURCE_ID)
            .header("X-Api-Request-Id", &request_id)
            .header("X-Api-Sequence", "-1")
            .json(&body)
            .send()
            .await?;

        Self::parse_official_response(response).await
    }

    async fn official_standard(
        &self,
        audio_data: Vec<u8>,
        app_key: &str,
        access_key: &str,
    ) -> Result<String> {
        let request_id = Uuid::new_v4().to_string();
        let submit_body = serde_json::json!({
            "user": { "uid": app_key },
            "audio": { "data": BASE64.encode(audio_data) },
            "request": { "model_name": OFFICIAL_MODEL_NAME }
        });

        let submit_resp = self
            .client
            .post(OFFICIAL_STANDARD_SUBMIT_ENDPOINT)
            .header("Content-Type", "application/json")
            .header("X-Api-App-Key", app_key)
            .header("X-Api-Access-Key", access_key)
            .header("X-Api-Resource-Id", OFFICIAL_STANDARD_RESOURCE_ID)
            .header("X-Api-Request-Id", &request_id)
            .header("X-Api-Sequence", "-1")
            .json(&submit_body)
            .send()
            .await?;

        let submit_headers = Self::header_map_to_lower(submit_resp.headers());
        let submit_payload = Self::json_body_or_error(submit_resp).await?;
        if let Some(code) = Self::official_status_code(&submit_payload, &submit_headers) {
            if code != "20000000" {
                return Err(LlmError::RequestFailed(format!(
                    "Official ASR standard submit failed: status={code}, message={}",
                    Self::official_status_message(&submit_payload, &submit_headers)
                )));
            }
        }

        let task_id = submit_headers
            .get("x-api-request-id")
            .cloned()
            .unwrap_or(request_id);

        let deadline = std::time::Instant::now() + OFFICIAL_QUERY_TIMEOUT;
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(LlmError::RequestFailed(
                    "Official ASR standard query timeout".into(),
                ));
            }

            let query_resp = self
                .client
                .post(OFFICIAL_STANDARD_QUERY_ENDPOINT)
                .header("Content-Type", "application/json")
                .header("X-Api-App-Key", app_key)
                .header("X-Api-Access-Key", access_key)
                .header("X-Api-Resource-Id", OFFICIAL_STANDARD_RESOURCE_ID)
                .header("X-Api-Request-Id", &task_id)
                .header("X-Api-Sequence", "-1")
                .json(&serde_json::json!({}))
                .send()
                .await?;

            let query_headers = Self::header_map_to_lower(query_resp.headers());
            let query_payload = Self::json_body_or_error(query_resp).await?;
            let query_status = Self::official_status_code(&query_payload, &query_headers);

            match query_status.as_deref() {
                Some("20000000") => return Ok(Self::extract_official_text(&query_payload)),
                Some("20000003") => return Ok(String::new()),
                Some("20000001") | Some("20000002") => {
                    tokio::time::sleep(OFFICIAL_QUERY_INTERVAL).await;
                }
                Some(other) => {
                    return Err(LlmError::RequestFailed(format!(
                        "Official ASR standard query failed: status={other}, message={}",
                        Self::official_status_message(&query_payload, &query_headers)
                    )));
                }
                None => {
                    return Err(LlmError::RequestFailed(format!(
                        "Official ASR standard query failed: message={}",
                        Self::official_status_message(&query_payload, &query_headers)
                    )));
                }
            }
        }
    }

    async fn parse_official_response(response: reqwest::Response) -> Result<String> {
        let headers = Self::header_map_to_lower(response.headers());
        let payload = Self::json_body_or_error(response).await?;
        let code = Self::official_status_code(&payload, &headers);

        if code.as_deref() == Some("20000003") {
            return Ok(String::new());
        }

        if let Some(code) = code {
            if code != "20000000" {
                return Err(LlmError::RequestFailed(format!(
                    "Official ASR failed: status={code}, message={}",
                    Self::official_status_message(&payload, &headers)
                )));
            }
        }

        Ok(Self::extract_official_text(&payload))
    }

    fn header_map_to_lower(headers: &reqwest::header::HeaderMap) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for (k, v) in headers {
            if let Ok(value) = v.to_str() {
                map.insert(k.as_str().to_lowercase(), value.to_string());
            }
        }
        map
    }

    async fn json_body_or_error(response: reqwest::Response) -> Result<serde_json::Value> {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(LlmError::RequestFailed(format!("HTTP {status}: {text}")));
        }
        if text.trim().is_empty() {
            return Ok(serde_json::json!({}));
        }
        serde_json::from_str::<serde_json::Value>(&text)
            .map_err(|e| LlmError::RequestFailed(format!("Invalid JSON response: {e}")))
    }

    fn extract_transcription_text(payload: &serde_json::Value) -> String {
        if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }

        if let Some(text) = payload
            .pointer("/result/text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
        {
            return text;
        }

        if let Some(items) = payload.get("result").and_then(|v| v.as_array()) {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
            return out;
        }

        String::new()
    }

    fn extract_official_text(payload: &serde_json::Value) -> String {
        if let Some(text) = payload.pointer("/result/text").and_then(|v| v.as_str()) {
            return text.to_string();
        }

        if let Some(items) = payload.get("result").and_then(|v| v.as_array()) {
            let mut out = String::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
            return out;
        }

        if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
            return text.to_string();
        }

        String::new()
    }

    fn official_status_code(
        payload: &serde_json::Value,
        headers: &HashMap<String, String>,
    ) -> Option<String> {
        if let Some(value) = headers.get("x-api-status-code") {
            return Some(value.to_string());
        }
        payload.get("code").map(|v| {
            v.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| v.to_string())
        })
    }

    fn official_status_message(
        payload: &serde_json::Value,
        headers: &HashMap<String, String>,
    ) -> String {
        payload
            .get("message")
            .and_then(|v| v.as_str())
            .or_else(|| payload.get("msg").and_then(|v| v.as_str()))
            .or_else(|| headers.get("x-api-message").map(String::as_str))
            .unwrap_or("unknown error")
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_mode_asr2api() {
        let client = DoubaoClient::new("".into(), Some("doubao-asr".into()), None, None, None);
        assert!(matches!(client.backend_mode(), BackendMode::Asr2Api));
    }

    #[test]
    fn test_backend_mode_official() {
        let client = DoubaoClient::new(
            "".into(),
            Some("doubao-asr-official-standard".into()),
            None,
            None,
            None,
        );
        assert!(matches!(
            client.backend_mode(),
            BackendMode::Official(OfficialMode::Standard)
        ));
    }

    #[test]
    fn test_extract_text_shapes() {
        let v = serde_json::json!({ "text": "hello" });
        assert_eq!(DoubaoClient::extract_transcription_text(&v), "hello");

        let v = serde_json::json!({ "result": { "text": "world" } });
        assert_eq!(DoubaoClient::extract_transcription_text(&v), "world");
    }
}
