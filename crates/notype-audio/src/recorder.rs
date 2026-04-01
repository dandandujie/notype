//! Audio recorder with start/stop control.
//!
//! Uses a dedicated thread for the cpal stream since `cpal::Stream`
//! is not `Send + Sync`. Communication happens via channels.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};

use cpal::traits::{DeviceTrait, StreamTrait};

use crate::{device, encoder, AudioData, AudioError, Result};

enum Command {
    Start,
    Stop(mpsc::Sender<Result<AudioData>>),
}

/// Controls microphone recording with start/stop semantics.
/// Safe to share across threads (Send + Sync).
pub struct Recorder {
    cmd_tx: mpsc::Sender<Command>,
    recording: Arc<AtomicBool>,
}

// Recorder owns only Send types (mpsc::Sender + Arc<AtomicBool>)
unsafe impl Send for Recorder {}
unsafe impl Sync for Recorder {}

impl Recorder {
    /// Create a new recorder targeting the default or named input device.
    pub fn new(device_name: Option<String>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let recording = Arc::new(AtomicBool::new(false));
        let recording_clone = Arc::clone(&recording);

        std::thread::Builder::new()
            .name("notype-audio".into())
            .spawn(move || {
                audio_thread(cmd_rx, recording_clone, device_name);
            })
            .expect("failed to spawn audio thread");

        Self { cmd_tx, recording }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Start capturing audio from the microphone.
    pub fn start(&self) -> Result<()> {
        if self.is_recording() {
            tracing::warn!("Already recording, ignoring start");
            return Ok(());
        }
        self.cmd_tx
            .send(Command::Start)
            .map_err(|e| AudioError::StreamError(e.to_string()))
    }

    /// Stop recording and return the captured audio as WAV.
    pub fn stop(&self) -> Result<AudioData> {
        if !self.is_recording() {
            return Err(AudioError::NotRecording);
        }
        let (result_tx, result_rx) = mpsc::channel();
        self.cmd_tx
            .send(Command::Stop(result_tx))
            .map_err(|e| AudioError::StreamError(e.to_string()))?;

        result_rx
            .recv()
            .map_err(|e| AudioError::StreamError(e.to_string()))?
    }
}

fn audio_thread(
    cmd_rx: mpsc::Receiver<Command>,
    recording: Arc<AtomicBool>,
    device_name: Option<String>,
) {
    let mut stream: Option<cpal::Stream> = None;
    let buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut sample_rate: u32 = 16000;
    let mut channels: u16 = 1;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Command::Start => {
                let device = match device::get_device(device_name.as_deref()) {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("Failed to get audio device: {e}");
                        continue;
                    }
                };

                let config = match resolve_config(&device) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!("Failed to resolve audio config: {e}");
                        continue;
                    }
                };

                sample_rate = config.sample_rate().0;
                channels = config.channels();

                tracing::info!(sample_rate, channels, "Starting recording");
                buffer.lock().unwrap().clear();

                let buf = Arc::clone(&buffer);
                let rec = Arc::clone(&recording);
                let stream_config: cpal::StreamConfig = config.into();

                match device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if rec.load(Ordering::Relaxed) {
                            buf.lock().unwrap().extend_from_slice(data);
                        }
                    },
                    |err| tracing::error!("Audio stream error: {err}"),
                    None,
                ) {
                    Ok(s) => {
                        if let Err(e) = s.play() {
                            tracing::error!("Failed to play stream: {e}");
                            continue;
                        }
                        recording.store(true, Ordering::Relaxed);
                        stream = Some(s);
                    }
                    Err(e) => {
                        tracing::error!("Failed to build stream: {e}");
                    }
                }
            }
            Command::Stop(result_tx) => {
                recording.store(false, Ordering::Relaxed);
                stream.take(); // drop stream to release device

                let samples: Vec<f32> = std::mem::take(&mut *buffer.lock().unwrap());
                let total = samples.len() as f32;
                let duration_secs = total / (sample_rate as f32 * channels as f32);

                tracing::info!(samples = samples.len(), duration_secs, "Recording stopped");

                let result =
                    encoder::encode_wav(&samples, sample_rate, channels).map(|wav_bytes| {
                        AudioData {
                            wav_bytes,
                            sample_rate,
                            channels,
                            duration_secs,
                        }
                    });

                let _ = result_tx.send(result);
            }
        }
    }

    tracing::debug!("Audio thread exiting");
}

/// Find a supported config, preferring 16kHz mono for LLM APIs.
fn resolve_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig> {
    let supported = device
        .supported_input_configs()
        .map_err(|e| AudioError::DeviceError(e.to_string()))?;

    let target_rate = cpal::SampleRate(16000);
    for range in supported {
        if range.sample_format() == cpal::SampleFormat::F32
            && range.channels() == 1
            && range.min_sample_rate() <= target_rate
            && range.max_sample_rate() >= target_rate
        {
            return Ok(range.with_sample_rate(target_rate));
        }
    }

    tracing::debug!("16kHz mono not supported, using default config");
    device
        .default_input_config()
        .map_err(|e| AudioError::DeviceError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recorder_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Recorder>();
    }

    #[test]
    fn test_stop_without_start_returns_error() {
        let recorder = Recorder::new(None);
        assert!(!recorder.is_recording());
        assert!(recorder.stop().is_err());
    }
}
