//! WAV encoding for audio data.

use crate::{AudioError, Result};

/// Encode PCM f32 samples to WAV bytes in memory.
pub fn encode_wav(samples: &[f32], sample_rate: u32, channels: u16) -> Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)
            .map_err(|e| AudioError::EncodingError(e.to_string()))?;

        for &sample in samples {
            let amplitude = (sample * i16::MAX as f32) as i16;
            writer
                .write_sample(amplitude)
                .map_err(|e| AudioError::EncodingError(e.to_string()))?;
        }

        writer
            .finalize()
            .map_err(|e| AudioError::EncodingError(e.to_string()))?;
    }

    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_wav_produces_valid_header() {
        let samples = vec![0.0f32; 16000]; // 1 second of silence at 16kHz
        let wav = encode_wav(&samples, 16000, 1).unwrap();

        // WAV files start with "RIFF"
        assert_eq!(&wav[0..4], b"RIFF");
        // Format should be "WAVE"
        assert_eq!(&wav[8..12], b"WAVE");
        // Should have reasonable size
        assert!(wav.len() > 44); // WAV header is 44 bytes
    }

    #[test]
    fn test_encode_wav_stereo() {
        let samples = vec![0.0f32; 32000]; // 1 second stereo at 16kHz
        let wav = encode_wav(&samples, 16000, 2).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
    }
}
