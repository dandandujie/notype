//! Audio recorder with start/stop control.
//!
//! Uses a dedicated thread for the cpal stream since `cpal::Stream`
//! is not `Send + Sync`. Communication happens via channels.

use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    mpsc, Arc,
};

use cpal::traits::{DeviceTrait, StreamTrait};

use crate::{device, encoder, AudioData, AudioError, AudioPcmSlice, AudioSlice, Result};

enum Command {
    Start,
    SetDevice(Option<String>),
    Stop(mpsc::Sender<Result<AudioData>>),
    Snapshot(mpsc::Sender<Result<AudioData>>),
    SnapshotFrom {
        from_sample: usize,
        result_tx: mpsc::Sender<Result<AudioSlice>>,
    },
    SnapshotPcmFrom {
        from_sample: usize,
        result_tx: mpsc::Sender<Result<AudioPcmSlice>>,
    },
}

/// Controls microphone recording with start/stop semantics.
/// Safe to share across threads (Send + Sync).
pub struct Recorder {
    cmd_tx: mpsc::Sender<Command>,
    recording: Arc<AtomicBool>,
    /// Latest RMS input level (0.0..~1.0), f32 stored as bits.
    level: Arc<AtomicU32>,
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
        let level = Arc::new(AtomicU32::new(0));
        let level_clone = Arc::clone(&level);

        std::thread::Builder::new()
            .name("notype-audio".into())
            .spawn(move || {
                audio_thread(cmd_rx, recording_clone, level_clone, device_name);
            })
            .expect("failed to spawn audio thread");

        Self {
            cmd_tx,
            recording,
            level,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Relaxed)
    }

    /// Latest microphone RMS level (0.0..~1.0). 0.0 when not recording.
    pub fn input_level(&self) -> f32 {
        if !self.is_recording() {
            return 0.0;
        }
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Switch the target input device. Takes effect on the next `start()`;
    /// an in-flight recording keeps its current stream.
    pub fn set_device(&self, device_name: Option<String>) {
        let _ = self.cmd_tx.send(Command::SetDevice(device_name));
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

    /// Snapshot the audio recorded so far without stopping.
    /// Returns None if not currently recording.
    pub fn snapshot(&self) -> Option<Result<AudioData>> {
        if !self.is_recording() {
            return None;
        }
        let (result_tx, result_rx) = mpsc::channel();
        self.cmd_tx.send(Command::Snapshot(result_tx)).ok()?;
        result_rx.recv().ok()
    }

    /// Snapshot audio starting at `from_sample` (absolute sample index in current session).
    /// Returns None if not currently recording.
    pub fn snapshot_from(&self, from_sample: usize) -> Option<Result<AudioSlice>> {
        if !self.is_recording() {
            return None;
        }
        let (result_tx, result_rx) = mpsc::channel();
        self.cmd_tx
            .send(Command::SnapshotFrom {
                from_sample,
                result_tx,
            })
            .ok()?;
        result_rx.recv().ok()
    }

    /// Snapshot PCM audio (`s16le`) starting at `from_sample`.
    /// Returns None if not currently recording.
    pub fn snapshot_pcm_from(&self, from_sample: usize) -> Option<Result<AudioPcmSlice>> {
        if !self.is_recording() {
            return None;
        }
        let (result_tx, result_rx) = mpsc::channel();
        self.cmd_tx
            .send(Command::SnapshotPcmFrom {
                from_sample,
                result_tx,
            })
            .ok()?;
        result_rx.recv().ok()
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
    level: Arc<AtomicU32>,
    device_name: Option<String>,
) {
    let mut device_name = device_name;
    let mut stream: Option<cpal::Stream> = None;
    let buffer: Arc<std::sync::Mutex<Vec<f32>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut sample_rate: u32 = 16000;
    let mut channels: u16 = 1;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            Command::SetDevice(name) => {
                tracing::info!(device = ?name, "Audio input device updated");
                device_name = name;
            }
            Command::Start => {
                // GOTCHA: fall back to the default device if the named one unplugged,
                // instead of failing the whole recording session.
                let device = match device::get_device(device_name.as_deref())
                    .or_else(|e| {
                        if device_name.is_some() {
                            tracing::warn!("Named device unavailable ({e}), falling back to default");
                            device::get_device(None)
                        } else {
                            Err(e)
                        }
                    }) {
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
                let lvl = Arc::clone(&level);
                let stream_config: cpal::StreamConfig = config.into();

                match device.build_input_stream(
                    &stream_config,
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if rec.load(Ordering::Relaxed) {
                            buf.lock().unwrap().extend_from_slice(data);
                            // RMS level for live waveform feedback.
                            if !data.is_empty() {
                                let sum_sq: f32 = data.iter().map(|s| s * s).sum();
                                let rms = (sum_sq / data.len() as f32).sqrt();
                                lvl.store(rms.to_bits(), Ordering::Relaxed);
                            }
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
            Command::Snapshot(result_tx) => {
                let samples: Vec<f32> = buffer.lock().unwrap().clone();
                let total = samples.len() as f32;
                let duration_secs = total / (sample_rate as f32 * channels as f32);

                tracing::debug!(
                    samples = samples.len(),
                    duration_secs,
                    "Audio snapshot taken"
                );

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
            Command::SnapshotFrom {
                from_sample,
                result_tx,
            } => {
                let guard = buffer.lock().unwrap();
                let end_sample = guard.len();
                let start_sample = from_sample.min(end_sample);
                let samples = guard[start_sample..end_sample].to_vec();
                drop(guard);

                let total = samples.len() as f32;
                let duration_secs = total / (sample_rate as f32 * channels as f32);

                tracing::debug!(
                    start_sample,
                    end_sample,
                    samples = samples.len(),
                    duration_secs,
                    "Audio range snapshot taken"
                );

                let result =
                    encoder::encode_wav(&samples, sample_rate, channels).map(|wav_bytes| {
                        AudioSlice {
                            audio: AudioData {
                                wav_bytes,
                                sample_rate,
                                channels,
                                duration_secs,
                            },
                            start_sample,
                            end_sample,
                        }
                    });

                let _ = result_tx.send(result);
            }
            Command::SnapshotPcmFrom {
                from_sample,
                result_tx,
            } => {
                let guard = buffer.lock().unwrap();
                let end_sample = guard.len();
                let start_sample = from_sample.min(end_sample);
                let samples = &guard[start_sample..end_sample];
                let total = samples.len() as f32;
                let duration_secs = total / (sample_rate as f32 * channels as f32);

                let mut pcm_s16le = Vec::with_capacity(samples.len() * 2);
                for &sample in samples {
                    let s = sample.clamp(-1.0, 1.0);
                    let i = (s * i16::MAX as f32) as i16;
                    pcm_s16le.extend_from_slice(&i.to_le_bytes());
                }
                drop(guard);

                tracing::trace!(
                    start_sample,
                    end_sample,
                    pcm_bytes = pcm_s16le.len(),
                    duration_secs,
                    "Audio PCM range snapshot taken"
                );

                let _ = result_tx.send(Ok(AudioPcmSlice {
                    pcm_s16le,
                    sample_rate,
                    channels,
                    duration_secs,
                    start_sample,
                    end_sample,
                }));
            }
            Command::Stop(result_tx) => {
                // Drop stream first to flush any in-flight audio data,
                // THEN set recording=false. Otherwise the callback sees
                // recording=false and discards the last chunk.
                stream.take();
                // Brief yield to let any in-flight callbacks finish writing
                std::thread::sleep(std::time::Duration::from_millis(50));
                recording.store(false, Ordering::Relaxed);
                level.store(0f32.to_bits(), Ordering::Relaxed);

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
