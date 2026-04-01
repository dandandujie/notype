//! Audio capture and encoding module for NoType.
//!
//! Provides microphone recording via cpal and WAV encoding for
//! multimodal LLM API consumption.

mod device;
mod encoder;
mod recorder;

pub use device::{list_input_devices, AudioDeviceInfo};
pub use encoder::encode_wav;
pub use recorder::Recorder;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("no input device available")]
    NoInputDevice,
    #[error("device error: {0}")]
    DeviceError(String),
    #[error("stream error: {0}")]
    StreamError(String),
    #[error("encoding error: {0}")]
    EncodingError(String),
    #[error("recorder not recording")]
    NotRecording,
}

pub type Result<T> = std::result::Result<T, AudioError>;

/// Recorded audio data ready for LLM API consumption.
#[derive(Debug, Clone)]
pub struct AudioData {
    pub wav_bytes: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration_secs: f32,
}
