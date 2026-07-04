//! Volcengine (火山引擎) BigModel streaming ASR client.
//!
//! Implements the official WebSocket protocol documented at
//! <https://www.volcengine.com/docs/6561/1354869>:
//!
//! - Endpoint: `wss://openspeech.bytedance.com/api/v3/sauc/bigmodel`
//! - Auth via `X-Api-App-Key` / `X-Api-Access-Key` / `X-Api-Resource-Id` headers
//! - Binary frames: 4-byte header + (optional sequence) + payload size (u32 BE)
//!   + gzip payload
//! - First frame is a JSON "full client request"; audio follows as raw PCM
//!   (16 kHz mono s16le) gzip-compressed "audio only" frames; the last frame
//!   sets the last-package flag; the server pushes full-text results per frame.
//!
//! Two usage modes:
//! - [`VolcStreamSession`]: live session — push PCM while recording, receive
//!   incremental full-text updates, then `finish()` for the final text.
//! - [`VolcengineClient`] implements [`VoiceRecognizer`] for batch use
//!   (edit commands, fallback): the WAV is chunked and replayed over one session.

use std::future::Future;
use std::io::{Read, Write};
use std::pin::Pin;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message;

use crate::{LlmError, RecognitionResult, Result, VoiceRecognizer};

const WS_URL: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";
pub const DEFAULT_RESOURCE_ID: &str = "volc.bigasr.sauc.duration";

// -- Binary protocol constants --
const PROTOCOL_VERSION: u8 = 0b0001;
const HEADER_SIZE: u8 = 0b0001;
const MSG_FULL_CLIENT: u8 = 0b0001;
const MSG_AUDIO_ONLY: u8 = 0b0010;
const MSG_FULL_SERVER: u8 = 0b1001;
const MSG_ERROR: u8 = 0b1111;
const FLAG_NONE: u8 = 0b0000;
const FLAG_POS_SEQUENCE: u8 = 0b0001;
const FLAG_LAST_NO_SEQ: u8 = 0b0010;
const FLAG_LAST_NEG_SEQ: u8 = 0b0011;
const SER_NONE: u8 = 0b0000;
const SER_JSON: u8 = 0b0001;
const COMP_GZIP: u8 = 0b0001;

#[derive(Debug, Clone)]
pub struct VolcConfig {
    pub app_key: String,
    pub access_key: String,
    pub resource_id: String,
}

fn gzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder
        .write_all(data)
        .and_then(|_| encoder.finish())
        .map_err(|e| LlmError::RequestFailed(format!("gzip failed: {e}")))
}

fn gunzip(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoder = flate2::read::GzDecoder::new(data);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| LlmError::RequestFailed(format!("gunzip failed: {e}")))?;
    Ok(out)
}

fn frame(msg_type: u8, flags: u8, serialization: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.push((PROTOCOL_VERSION << 4) | HEADER_SIZE);
    out.push((msg_type << 4) | flags);
    out.push((serialization << 4) | COMP_GZIP);
    out.push(0x00);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Parsed server frame: either a result text or an error.
enum ServerFrame {
    Result { text: String, is_last: bool },
    Error { code: u32, message: String },
    Other,
}

fn parse_server_frame(data: &[u8]) -> Result<ServerFrame> {
    if data.len() < 4 {
        return Err(LlmError::RequestFailed("volc frame too short".into()));
    }
    let header_bytes = ((data[0] & 0x0F) as usize) * 4;
    let msg_type = data[1] >> 4;
    let flags = data[1] & 0x0F;
    let compression = data[2] & 0x0F;
    let mut offset = header_bytes;

    match msg_type {
        MSG_FULL_SERVER => {
            // Frames with the sequence flag bit carry a 4-byte sequence first.
            if flags & FLAG_POS_SEQUENCE != 0 || flags == FLAG_LAST_NEG_SEQ {
                offset += 4;
            }
            if data.len() < offset + 4 {
                return Ok(ServerFrame::Other);
            }
            let size =
                u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
                    as usize;
            offset += 4;
            if data.len() < offset + size {
                return Ok(ServerFrame::Other);
            }
            let raw = &data[offset..offset + size];
            let payload = if compression == COMP_GZIP {
                gunzip(raw)?
            } else {
                raw.to_vec()
            };
            let json: serde_json::Value = serde_json::from_slice(&payload)
                .map_err(|e| LlmError::RequestFailed(format!("volc bad json: {e}")))?;
            let text = json
                .pointer("/result/text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            // Last-package responses carry flag 0b0010 / 0b0011.
            let is_last = flags == FLAG_LAST_NO_SEQ || flags == FLAG_LAST_NEG_SEQ;
            Ok(ServerFrame::Result { text, is_last })
        }
        MSG_ERROR => {
            if data.len() < offset + 8 {
                return Err(LlmError::RequestFailed("volc error frame truncated".into()));
            }
            let code =
                u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]]);
            offset += 4;
            let size =
                u32::from_be_bytes([data[offset], data[offset + 1], data[offset + 2], data[offset + 3]])
                    as usize;
            offset += 4;
            let raw = data.get(offset..offset + size).unwrap_or_default();
            let message = gunzip(raw)
                .ok()
                .and_then(|d| String::from_utf8(d).ok())
                .unwrap_or_else(|| String::from_utf8_lossy(raw).to_string());
            Ok(ServerFrame::Error { code, message })
        }
        _ => Ok(ServerFrame::Other),
    }
}

fn full_client_request_payload() -> serde_json::Value {
    serde_json::json!({
        "user": { "uid": "notype" },
        "audio": {
            "format": "pcm",
            "codec": "raw",
            "rate": 16000,
            "bits": 16,
            "channel": 1
        },
        "request": {
            "model_name": "bigmodel",
            "enable_punc": true,
            "enable_itn": true,
            "result_type": "full"
        }
    })
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(config: &VolcConfig) -> Result<WsStream> {
    let mut request = WS_URL
        .into_client_request()
        .map_err(|e| LlmError::RequestFailed(format!("volc ws request: {e}")))?;
    let headers = request.headers_mut();
    let hv = |s: &str| {
        s.parse::<tokio_tungstenite::tungstenite::http::HeaderValue>()
            .map_err(|e| LlmError::RequestFailed(format!("volc header: {e}")))
    };
    headers.insert("X-Api-App-Key", hv(&config.app_key)?);
    headers.insert("X-Api-Access-Key", hv(&config.access_key)?);
    let resource = if config.resource_id.trim().is_empty() {
        DEFAULT_RESOURCE_ID
    } else {
        config.resource_id.trim()
    };
    headers.insert("X-Api-Resource-Id", hv(resource)?);
    headers.insert("X-Api-Connect-Id", hv(&uuid::Uuid::new_v4().to_string())?);

    let (mut ws, response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| LlmError::RequestFailed(format!("volc ws connect: {e}")))?;

    if let Some(logid) = response.headers().get("X-Tt-Logid") {
        tracing::info!(logid = ?logid, "Volcengine ASR connected");
    }

    // Full client request opens the session.
    let payload = gzip(full_client_request_payload().to_string().as_bytes())?;
    ws.send(Message::Binary(
        frame(MSG_FULL_CLIENT, FLAG_NONE, SER_JSON, &payload).into(),
    ))
    .await
    .map_err(|e| LlmError::RequestFailed(format!("volc ws send config: {e}")))?;

    // The server acknowledges with a full server response.
    match next_frame(&mut ws).await? {
        Some(ServerFrame::Error { code, message }) => Err(LlmError::RequestFailed(format!(
            "volc ASR error {code}: {message}"
        ))),
        Some(_) => Ok(ws),
        None => Err(LlmError::RequestFailed(
            "volc ws closed during handshake".into(),
        )),
    }
}

async fn next_frame(ws: &mut WsStream) -> Result<Option<ServerFrame>> {
    loop {
        match ws.next().await {
            Some(Ok(Message::Binary(data))) => return parse_server_frame(&data).map(Some),
            Some(Ok(Message::Close(_))) | None => return Ok(None),
            Some(Ok(_)) => continue, // ping/pong/text — ignore
            Some(Err(e)) => {
                return Err(LlmError::RequestFailed(format!("volc ws read: {e}")))
            }
        }
    }
}

/// Live streaming session: push PCM chunks while recording, watch text updates,
/// then `finish()` to flush and obtain the final transcript.
pub struct VolcStreamSession {
    pcm_tx: mpsc::UnboundedSender<SessionCmd>,
    final_rx: tokio::sync::oneshot::Receiver<Result<String>>,
}

enum SessionCmd {
    Pcm(Vec<u8>),
    Finish,
}

impl VolcStreamSession {
    /// Connect and spawn the pump task. `text_tx` receives every incremental
    /// full-text update (already cumulative — replace, don't append).
    pub async fn start(
        config: VolcConfig,
        text_tx: mpsc::UnboundedSender<String>,
    ) -> Result<Self> {
        let mut ws = connect(&config).await?;
        let (pcm_tx, mut cmd_rx) = mpsc::unbounded_channel::<SessionCmd>();
        let (final_tx, final_rx) = tokio::sync::oneshot::channel::<Result<String>>();

        tokio::spawn(async move {
            let mut latest_text = String::new();
            let mut finished = false;

            let outcome: Result<String> = loop {
                tokio::select! {
                    cmd = cmd_rx.recv(), if !finished => {
                        match cmd {
                            Some(SessionCmd::Pcm(pcm)) => {
                                if pcm.is_empty() { continue; }
                                let Ok(payload) = gzip(&pcm) else {
                                    break Err(LlmError::RequestFailed("gzip pcm failed".into()));
                                };
                                if let Err(e) = ws.send(Message::Binary(
                                    frame(MSG_AUDIO_ONLY, FLAG_NONE, SER_NONE, &payload).into(),
                                )).await {
                                    break Err(LlmError::RequestFailed(format!("volc ws send: {e}")));
                                }
                            }
                            Some(SessionCmd::Finish) | None => {
                                finished = true;
                                // Last package: empty audio with the last-package flag.
                                let Ok(payload) = gzip(&[]) else {
                                    break Err(LlmError::RequestFailed("gzip failed".into()));
                                };
                                if let Err(e) = ws.send(Message::Binary(
                                    frame(MSG_AUDIO_ONLY, FLAG_LAST_NO_SEQ, SER_NONE, &payload).into(),
                                )).await {
                                    break Err(LlmError::RequestFailed(format!("volc ws send last: {e}")));
                                }
                            }
                        }
                    }
                    frame_result = next_frame(&mut ws) => {
                        match frame_result {
                            Ok(Some(ServerFrame::Result { text, is_last })) => {
                                if !text.is_empty() && text != latest_text {
                                    latest_text = text.clone();
                                    let _ = text_tx.send(text);
                                }
                                if is_last {
                                    break Ok(latest_text);
                                }
                            }
                            Ok(Some(ServerFrame::Error { code, message })) => {
                                break Err(LlmError::RequestFailed(
                                    format!("volc ASR error {code}: {message}")));
                            }
                            Ok(Some(ServerFrame::Other)) => {}
                            Ok(None) => {
                                // Connection closed: whatever we have is the result.
                                break if finished || !latest_text.is_empty() {
                                    Ok(latest_text)
                                } else {
                                    Err(LlmError::RequestFailed("volc ws closed early".into()))
                                };
                            }
                            Err(e) => break Err(e),
                        }
                    }
                }
            };

            let _ = ws.close(None).await;
            let _ = final_tx.send(outcome);
        });

        Ok(Self { pcm_tx, final_rx })
    }

    /// Push a chunk of 16 kHz mono s16le PCM.
    pub fn push_pcm(&self, pcm: Vec<u8>) -> bool {
        self.pcm_tx.send(SessionCmd::Pcm(pcm)).is_ok()
    }

    /// Flush the session (sends the last package) and wait for the final text.
    pub async fn finish(self, max_wait: std::time::Duration) -> Result<String> {
        let _ = self.pcm_tx.send(SessionCmd::Finish);
        match tokio::time::timeout(max_wait, self.final_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(LlmError::RequestFailed("volc session dropped".into())),
            Err(_) => Err(LlmError::RequestFailed("volc finish timed out".into())),
        }
    }
}

// -- WAV helpers for batch mode --

/// Extract (sample_rate, channels, s16le PCM data) from a WAV container.
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
        let size = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
            as usize;
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
            b"data" => {
                data = Some(bytes[body_start..body_end].to_vec());
            }
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

/// Downmix + linear-resample s16le PCM to 16 kHz mono.
fn to_16k_mono(pcm: &[u8], sample_rate: u32, channels: u16) -> Vec<u8> {
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

    if sample_rate != 16_000 {
        let src_len = samples.len();
        if src_len == 0 {
            return Vec::new();
        }
        let dst_len = (src_len as u64 * 16_000).div_ceil(sample_rate as u64) as usize;
        let ratio = sample_rate as f64 / 16_000f64;
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

/// Batch client: replays a recorded WAV over one streaming session.
pub struct VolcengineClient {
    config: VolcConfig,
}

impl VolcengineClient {
    pub fn new(app_key: String, access_key: String, resource_id: Option<String>) -> Self {
        Self {
            config: VolcConfig {
                app_key,
                access_key,
                resource_id: resource_id.unwrap_or_else(|| DEFAULT_RESOURCE_ID.into()),
            },
        }
    }

    async fn do_recognize(
        &self,
        audio_data: Vec<u8>,
        tx: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<RecognitionResult> {
        let (rate, channels, pcm) = parse_wav(&audio_data)?;
        let pcm16k = to_16k_mono(&pcm, rate, channels);
        if pcm16k.is_empty() {
            return Ok(RecognitionResult { text: String::new() });
        }

        let (text_tx, mut text_rx) = mpsc::unbounded_channel::<String>();
        let session = VolcStreamSession::start(self.config.clone(), text_tx).await?;

        // 200 ms packets, as recommended by the protocol docs.
        const CHUNK: usize = 16_000 * 2 / 5;
        for chunk in pcm16k.chunks(CHUNK) {
            if !session.push_pcm(chunk.to_vec()) {
                break;
            }
        }

        // Forward incremental updates while waiting for the final text.
        let forward = async {
            while let Some(update) = text_rx.recv().await {
                if let Some(tx) = &tx {
                    let _ = tx.send(update);
                }
            }
        };

        let (final_text, ()) = tokio::join!(
            session.finish(std::time::Duration::from_secs(30)),
            forward
        );
        let text = final_text?;
        tracing::info!(chars = text.chars().count(), "Volcengine transcription received");
        Ok(RecognitionResult { text })
    }
}

impl VoiceRecognizer for VolcengineClient {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_layout_is_correct() {
        let f = frame(MSG_FULL_CLIENT, FLAG_NONE, SER_JSON, b"abc");
        assert_eq!(f[0], 0b0001_0001);
        assert_eq!(f[1], 0b0001_0000);
        assert_eq!(f[2], 0b0001_0001);
        assert_eq!(f[3], 0x00);
        assert_eq!(u32::from_be_bytes([f[4], f[5], f[6], f[7]]), 3);
        assert_eq!(&f[8..], b"abc");
    }

    #[test]
    fn gzip_roundtrip() {
        let data = b"hello volcengine";
        let zipped = gzip(data).unwrap();
        let unzipped = gunzip(&zipped).unwrap();
        assert_eq!(unzipped, data);
    }

    #[test]
    fn parse_wav_extracts_pcm() {
        // Minimal WAV: RIFF header + fmt + data with 4 samples
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&36u32.to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&16000u32.to_le_bytes());
        wav.extend_from_slice(&32000u32.to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&8u32.to_le_bytes());
        wav.extend_from_slice(&[1, 0, 2, 0, 3, 0, 4, 0]);

        let (rate, ch, pcm) = parse_wav(&wav).unwrap();
        assert_eq!(rate, 16000);
        assert_eq!(ch, 1);
        assert_eq!(pcm.len(), 8);
    }

    #[test]
    fn to_16k_mono_downmixes_stereo() {
        // Two stereo frames at 16k: (100, 200), (300, 400) → mono (150, 350)
        let pcm: Vec<u8> = [100i16, 200, 300, 400]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        let mono = to_16k_mono(&pcm, 16000, 2);
        let samples: Vec<i16> = mono
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(samples, vec![150, 350]);
    }

    #[test]
    fn server_result_frame_roundtrip() {
        let json = r#"{"result":{"text":"你好世界"}}"#.as_bytes();
        let payload = gzip(json).unwrap();
        let mut f = vec![
            (PROTOCOL_VERSION << 4) | HEADER_SIZE,
            (MSG_FULL_SERVER << 4) | FLAG_POS_SEQUENCE,
            (SER_JSON << 4) | COMP_GZIP,
            0,
        ];
        f.extend_from_slice(&1u32.to_be_bytes()); // sequence
        f.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        f.extend_from_slice(&payload);

        match parse_server_frame(&f).unwrap() {
            ServerFrame::Result { text, is_last } => {
                assert_eq!(text, "你好世界");
                assert!(!is_last);
            }
            _ => panic!("expected result frame"),
        }
    }
}
