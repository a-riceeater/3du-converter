//! Minimal RIFF/WAVE reader: PCM 8/16/24/32-bit, IEEE float 32/64, WAVE_FORMAT_EXTENSIBLE.
//!
//! Recorders that stream to disk often leave the `data` size as 0 or 0xFFFFFFFF; the reader
//! then uses the bytes actually present.

use crate::AudioError;

/// Decoded audio, downmixed to mono.
#[derive(Debug, Clone)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames in the file (per channel).
    pub frames: u64,
    pub mono: Vec<f32>,
}

impl Audio {
    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }
}

/// Upper bound on decoded length (2 hours at 192 kHz would be far beyond any show).
const MAX_FRAMES: u64 = 2 * 3600 * 192_000;

fn le16(b: &[u8], p: usize) -> u16 {
    u16::from_le_bytes([b[p], b[p + 1]])
}
fn le32(b: &[u8], p: usize) -> u32 {
    u32::from_le_bytes([b[p], b[p + 1], b[p + 2], b[p + 3]])
}

pub fn read(bytes: &[u8]) -> Result<Audio, AudioError> {
    let bad = |m: &str| AudioError::Unsupported(m.to_string());
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad("not a RIFF/WAVE file (only .wav is supported)"));
    }
    let mut p = 12;
    let mut fmt: Option<(u16, u16, u32, u16)> = None;
    while p + 8 <= bytes.len() {
        let id = &bytes[p..p + 4];
        let size = le32(bytes, p + 4) as usize;
        let body = p + 8;
        if id == b"fmt " {
            if body + 16 > bytes.len() {
                return Err(bad("truncated fmt chunk"));
            }
            let mut format = le16(bytes, body);
            let channels = le16(bytes, body + 2);
            let rate = le32(bytes, body + 4);
            let bits = le16(bytes, body + 14);
            if format == 0xFFFE && size >= 40 && body + 26 <= bytes.len() {
                format = le16(bytes, body + 24); // sub-format GUID starts with the format tag
            }
            fmt = Some((format, channels, rate, bits));
        } else if id == b"data" {
            let (format, channels, rate, bits) = fmt.ok_or_else(|| bad("data chunk before fmt chunk"))?;
            if channels == 0 || rate == 0 {
                return Err(bad("zero channels or sample rate"));
            }
            let available = bytes.len() - body;
            let len = if size == 0 || size == u32::MAX as usize || size > available { available } else { size };
            return decode(&bytes[body..body + len], format, channels, rate, bits);
        }
        p = body + size + (size & 1);
    }
    Err(bad("no data chunk"))
}

fn decode(data: &[u8], format: u16, channels: u16, rate: u32, bits: u16) -> Result<Audio, AudioError> {
    let width = (bits as usize).div_ceil(8);
    let frame = width * channels as usize;
    if frame == 0 {
        return Err(AudioError::Unsupported("zero-width samples".into()));
    }
    let frames = (data.len() / frame) as u64;
    if frames > MAX_FRAMES {
        return Err(AudioError::Unsupported("audio is longer than the supported limit".into()));
    }
    let sample: Box<dyn Fn(&[u8]) -> f32> = match (format, bits) {
        (1, 8) => Box::new(|s: &[u8]| (s[0] as f32 - 128.0) / 128.0),
        (1, 16) => Box::new(|s: &[u8]| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0),
        (1, 24) => Box::new(|s: &[u8]| (i32::from_le_bytes([0, s[0], s[1], s[2]]) >> 8) as f32 / 8_388_608.0),
        (1, 32) => Box::new(|s: &[u8]| i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / 2_147_483_648.0),
        (3, 32) => Box::new(|s: &[u8]| f32::from_le_bytes([s[0], s[1], s[2], s[3]])),
        (3, 64) => Box::new(|s: &[u8]| f64::from_le_bytes(s[..8].try_into().unwrap()) as f32),
        _ => return Err(AudioError::Unsupported(format!("WAV format {format} with {bits}-bit samples"))),
    };
    let ch = channels as usize;
    let mono = data
        .chunks_exact(frame)
        .map(|f| f.chunks_exact(width).map(&sample).sum::<f32>() / ch as f32)
        .collect();
    Ok(Audio { sample_rate: rate, channels, frames, mono })
}

/// Encodes mono samples as 16-bit PCM WAV (used by tests to build synthetic audio).
pub fn write_pcm16(samples: &[f32], rate: u32) -> Vec<u8> {
    let mut o = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&(36 + data_len).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&rate.to_le_bytes());
    o.extend_from_slice(&(rate * 2).to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        o.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_streaming_size() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 / 50.0).sin() * 0.5).collect();
        let mut wav = write_pcm16(&samples, 8000);
        let a = read(&wav).unwrap();
        assert_eq!((a.sample_rate, a.channels, a.frames), (8000, 1, 1000));
        assert!((a.mono[10] - samples[10]).abs() < 1e-4);
        // A streaming writer that never patched the data size.
        wav[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read(&wav).unwrap().frames, 1000);
        assert!(read(b"OggS....").is_err());
    }
}
