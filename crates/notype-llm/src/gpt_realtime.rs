//! OpenAI Realtime transcription client (gpt-realtime series).
//!
//! Uses the Realtime WebSocket transcription intent
//! (<https://developers.openai.com/api/docs/guides/realtime-transcription>):
//!
//! - Endpoint: `wss://api.openai.com/v1/realtime?intent=transcription`
//! - Auth: `Authorization: Bearer <key>` + `OpenAI-Beta: realtime=v1`
//! - After connect, send `transcription_session.update` to pick the model and
//!   disable server turn detection (we commit manually).
//! - Audio: `input_audio_buffer.append` with base64 PCM16 24 kHz mono, then
//!   `input_audio_buffer.commit` to trigger transcription.
//! - Server streams `conversation.item.input_audio_transcription.delta` and a
//!   final `...transcription.completed` carrying the full transcript.
//!
//! Implemented as a batch [`VoiceRecognizer`]: the recorded WAV is replayed
//! over one session. Its raw text then flows through the shared LLM polish
//! pass, like the other dedicated ASR engines.

use std::future::Future;
use std::pin::Pin;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

const WS_URL: &str = "wss://api.openai.com/v1/realtime?intent=transcription";
pub const DEFAULT_MODEL: &str = "gpt-4o-transcribe";
/// Realtime PCM sample rate expected by the `pcm16` format.
const TARGET_RATE: u32 = 24_000;

pub struct GptRealtimeClient {
    api_key: String,
    model: String,
}

impl GptRealtimeClient {
    pub fn new(api_key: String, model: Option<String>) -> Self {
        Self {
            api_key,
            model: model
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_MODEL.into()),
        }
    }

    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        if self.api_key.trim().is_empty() {
            return Err(LlmError::InvalidApiKey);
        }

        let (rate, channels, pcm) = parse_wav(&audio_data)?;
        let pcm24 = to_target_mono(&pcm, rate, channels);
        if pcm24.is_empty() {
            return Ok(RecognitionResult {
                text: String::new(),
            });
        }

        let mut ws = self.connect().await?;

        // Configure a transcription-only session and disable server VAD so we
        // control when the buffer is committed.
        let session_update = serde_json::json!({
            "type": "transcription_session.update",
            "session": {
                "input_audio_format": "pcm16",
                "input_audio_transcription": { "model": self.model },
                "turn_detection": serde_json::Value::Null
            }
        });
        ws.send(Message::Text(session_update.to_string().into()))
            .await
            .map_err(|e| LlmError::RequestFailed(format!("realtime ws send config: {e}")))?;

        // Append audio in ~200 ms base64 chunks.
        const CHUNK: usize = (TARGET_RATE as usize * 2) / 5;
        for chunk in pcm24.chunks(CHUNK) {
            let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, chunk);
            let append = serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": b64,
            });
            ws.send(Message::Text(append.to_string().into()))
                .await
                .map_err(|e| LlmError::RequestFailed(format!("realtime ws send audio: {e}")))?;
        }

        ws.send(Message::Text(
            serde_json::json!({ "type": "input_audio_buffer.commit" })
                .to_string()
                .into(),
        ))
        .await
        .map_err(|e| LlmError::RequestFailed(format!("realtime ws commit: {e}")))?;

        // Collect deltas + completed transcripts until the buffer drains.
        let mut streamed = String::new();
        let mut completed = String::new();
        let mut got_completed = false;
        let overall_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);

        loop {
            // After a completed event, drain briefly for any further items.
            let per_msg = if got_completed {
                std::time::Duration::from_millis(500)
            } else {
                std::time::Duration::from_secs(20)
            };
            let next = tokio::time::timeout(per_msg, ws.next()).await;
            let msg = match next {
                Ok(Some(Ok(Message::Text(t)))) => t,
                Ok(Some(Ok(Message::Close(_)))) | Ok(None) => break,
                Ok(Some(Ok(_))) => continue, // ping/binary — ignore
                Ok(Some(Err(e))) => {
                    return Err(LlmError::RequestFailed(format!("realtime ws read: {e}")))
                }
                Err(_) => break, // per-message timeout → done
            };

            let Ok(json) = serde_json::from_str::<serde_json::Value>(&msg) else {
                continue;
            };
            match json.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                "conversation.item.input_audio_transcription.delta" => {
                    if let Some(delta) = json.get("delta").and_then(|v| v.as_str()) {
                        if !delta.is_empty() {
                            streamed.push_str(delta);
                            if let Some(tx) = &tx {
                                let _ = tx.send(delta.to_string());
                            }
                        }
                    }
                }
                "conversation.item.input_audio_transcription.completed" => {
                    if let Some(t) = json.get("transcript").and_then(|v| v.as_str()) {
                        if !completed.is_empty() && !t.is_empty() {
                            completed.push(' ');
                        }
                        completed.push_str(t);
                    }
                    got_completed = true;
                }
                "error" => {
                    let message = json
                        .pointer("/error/message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown realtime error");
                    // Auth errors surface distinctly for a better hint.
                    if message.to_lowercase().contains("api key")
                        || message.to_lowercase().contains("unauthorized")
                    {
                        let _ = ws.close(None).await;
                        return Err(LlmError::InvalidApiKey);
                    }
                    let _ = ws.close(None).await;
                    return Err(LlmError::RequestFailed(format!(
                        "OpenAI Realtime error: {message}"
                    )));
                }
                _ => {}
            }

            if tokio::time::Instant::now() >= overall_deadline {
                break;
            }
        }

        let _ = ws.close(None).await;

        let text = if !completed.trim().is_empty() {
            completed.trim().to_string()
        } else {
            streamed.trim().to_string()
        };
        tracing::info!(
            chars = text.chars().count(),
            model = %self.model,
            "OpenAI Realtime transcription received"
        );
        Ok(RecognitionResult { text })
    }

    async fn connect(
        &self,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let mut request = WS_URL
            .into_client_request()
            .map_err(|e| LlmError::RequestFailed(format!("realtime ws request: {e}")))?;
        let headers = request.headers_mut();
        let bearer = format!("Bearer {}", self.api_key.trim());
        headers.insert(
            "Authorization",
            bearer
                .parse()
                .map_err(|e| LlmError::RequestFailed(format!("realtime auth header: {e}")))?,
        );
        headers.insert(
            "OpenAI-Beta",
            "realtime=v1"
                .parse()
                .map_err(|e| LlmError::RequestFailed(format!("realtime beta header: {e}")))?,
        );

        let (ws, _resp) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| LlmError::RequestFailed(format!("realtime ws connect: {e}")))?;
        Ok(ws)
    }
}

impl VoiceRecognizer for GptRealtimeClient {
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

// -- WAV / PCM helpers --

/// Extract (sample_rate, channels, s16le PCM) from a WAV container.
fn parse_wav(bytes: &[u8]) -> Result<(u32, u16, Vec<u8>)> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(LlmError::RequestFailed("not a WAV file".into()));
    }
    let mut pos = 12usize;
    let mut sample_rate = 16_000u32;
    let mut channels = 1u16;
    let mut bits = 16u16;
    let mut data: Option<Vec<u8>> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        let body_start = pos + 8;
        let body_end = (body_start + size).min(bytes.len());
        match id {
            b"fmt " if size >= 16 => {
                channels = u16::from_le_bytes([bytes[body_start + 2], bytes[body_start + 3]]);
                sample_rate = u32::from_le_bytes([
                    bytes[body_start + 4],
                    bytes[body_start + 5],
                    bytes[body_start + 6],
                    bytes[body_start + 7],
                ]);
                bits = u16::from_le_bytes([bytes[body_start + 14], bytes[body_start + 15]]);
            }
            b"data" => data = Some(bytes[body_start..body_end].to_vec()),
            _ => {}
        }
        pos = body_start + size + (size & 1);
    }

    if bits != 16 {
        return Err(LlmError::RequestFailed(format!(
            "unsupported WAV bit depth: {bits}"
        )));
    }
    let data = data.ok_or_else(|| LlmError::RequestFailed("WAV has no data chunk".into()))?;
    Ok((sample_rate, channels, data))
}

/// Downmix + linear-resample s16le PCM to [`TARGET_RATE`] mono.
fn to_target_mono(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
    if pcm.len() < 2 || sample_rate == 0 || channels == 0 {
        return Vec::new();
    }
    let mut samples: Vec<i16> = pcm
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();

    let ch = channels as usize;
    if ch > 1 {
        samples = samples
            .chunks_exact(ch)
            .map(|f| (f.iter().map(|&s| s as i32).sum::<i32>() / ch as i32) as i16)
            .collect();
    }

    if sample_rate != TARGET_RATE {
        let src_len = samples.len();
        if src_len == 0 {
            return Vec::new();
        }
        let dst_len = (src_len as u64 * TARGET_RATE as u64).div_ceil(sample_rate as u64) as usize;
        let ratio = sample_rate as f64 / TARGET_RATE as f64;
        let mut resampled = Vec::with_capacity(dst_len);
        for i in 0..dst_len {
            let pos = i as f64 * ratio;
            let idx = pos.floor() as usize;
            let frac = pos - idx as f64;
            let a = samples[idx.min(src_len - 1)] as f64;
            let b = samples[(idx + 1).min(src_len - 1)] as f64;
            resampled.push((a + (b - a) * frac).clamp(i16::MIN as f64, i16::MAX as f64) as i16);
        }
        samples = resampled;
    }

    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_model_applies() {
        let c = GptRealtimeClient::new("k".into(), None);
        assert_eq!(c.model, DEFAULT_MODEL);
        let d = GptRealtimeClient::new("k".into(), Some("  ".into()));
        assert_eq!(d.model, DEFAULT_MODEL);
        let e = GptRealtimeClient::new("k".into(), Some("gpt-4o-mini-transcribe".into()));
        assert_eq!(e.model, "gpt-4o-mini-transcribe");
    }

    #[test]
    fn resample_16k_to_24k_upsamples() {
        // 4 samples @16k → ceil(4*24000/16000) = 6 samples @24k
        let pcm: Vec<u8> = [100i16, 200, 300, 400]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let out = to_target_mono(&pcm, 16_000, 1);
        assert_eq!(out.len() / 2, 6);
    }
}
