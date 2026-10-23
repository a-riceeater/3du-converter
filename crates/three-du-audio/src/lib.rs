//! Reading show audio and aligning it with a drill's count clock.

pub mod align;
pub mod wav;

use std::path::Path;

pub use align::Alignment;
use three_du_core::DrillProject;
pub use wav::Audio;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn read_file(path: impl AsRef<Path>) -> Result<Audio, AudioError> {
    wav::read(&std::fs::read(path)?)
}

/// The musical beats of a drill: the time each count lands, for counts 1..=drill end.
pub fn drill_beats(p: &DrillProject) -> Vec<f64> {
    let end = p.timeline.drill_end_count as usize;
    p.timeline.count_times_seconds[1..=end.min(p.timeline.count_len().saturating_sub(1))].to_vec()
}

/// How far (s) the beat tracker may move each beat from the drill clock: a little when the
/// offset is known (a confident alignment or one the user gave), so that the tracker absorbs
/// drift without hopping between the beat and the band's off-beats, and a lot when it is not.
pub fn tracking_window(offset_known: bool) -> f64 {
    if offset_known {
        0.25
    } else {
        2.5
    }
}

/// Aligns the drill with a recording. The offset is searched from 2 s before the audio
/// starts up to the point where the drill would no longer fit in the recording.
pub fn align_project(p: &DrillProject, audio: &Audio) -> Alignment {
    let beats = drill_beats(p);
    let last = beats.last().copied().unwrap_or(0.0);
    let hi = (audio.duration_seconds() - last).max(0.0).min(120.0);
    let sets: Vec<usize> = p.sets.iter().skip(1).map(|s| s.start_count as usize).filter(|&c| c >= 1).map(|c| c - 1).collect();
    align::align(audio, &beats, (-2.0, hi), &sets)
}

#[cfg(test)]
mod tests {
    use super::*;
    use three_du_core::synth::{encode, sample_show};
    use three_du_core::{read_bytes, ReadOptions};

    /// Synthesizes a click track for the drill's beats, starting `offset` seconds into the audio.
    fn clicks(beats: &[f64], offset: f64, rate: u32, len_s: f64, seed: u32) -> Audio {
        let n = (len_s * rate as f64) as usize;
        let mut x = vec![0f32; n];
        let mut s = seed;
        for v in x.iter_mut() {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            *v = (s as f32 / u32::MAX as f32 - 0.5) * 0.05;
        }
        for b in beats {
            let start = ((b + offset) * rate as f64) as usize;
            for i in 0..(0.03 * rate as f64) as usize {
                if start + i < n {
                    let t = i as f32 / rate as f32;
                    x[start + i] += (t * 2.0 * std::f32::consts::PI * 1500.0).sin() * (-t * 120.0).exp();
                }
            }
        }
        Audio { sample_rate: rate, channels: 1, frames: n as u64, mono: x }
    }

    #[test]
    fn recovers_a_known_offset() {
        let p = read_bytes(&encode(&sample_show()), ReadOptions::default()).unwrap();
        let beats = drill_beats(&p);
        for (offset, rate) in [(0.0, 22050), (0.437, 22050), (1.9, 48000), (-0.25, 48000)] {
            let audio = clicks(&beats, offset, rate, 12.0, 7);
            let a = align_project(&p, &audio);
            assert!((a.offset_seconds - offset).abs() < 0.003, "expected {offset}, got {}", a.offset_seconds);
            assert!(a.confident, "{a:?}");
            assert!(a.max_drift_seconds < 0.005);
        }
    }

    #[test]
    fn follows_a_performance_that_drifts() {
        let p = read_bytes(&encode(&sample_show()), ReadOptions::default()).unwrap();
        let nominal = drill_beats(&p);
        // The "band" plays 4% slow and then rushes the last beats; audio starts 0.3 s early.
        let mut performed = vec![0.3 + nominal[0]];
        for k in 1..nominal.len() {
            let stretch = if k < 12 { 1.04 } else { 0.95 };
            performed.push(performed[k - 1] + (nominal[k] - nominal[k - 1]) * stretch);
        }
        let audio = clicks(&performed, 0.0, 22050, 12.0, 11);
        let t = align::track_beats(&audio, &nominal, 0.3, 1.0);
        let worst = t.times.iter().zip(&performed).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
        assert!(worst < 0.006, "worst beat error {worst}");
        assert!(t.confident && t.onset_rate > t.constant_offset_onset_rate);
    }

    #[test]
    fn noise_is_not_confident() {
        let p = read_bytes(&encode(&sample_show()), ReadOptions::default()).unwrap();
        let audio = clicks(&[], 0.0, 22050, 12.0, 3);
        let a = align_project(&p, &audio);
        assert!(!a.confident, "{a:?}");
        let t = align::track_beats(&audio, &drill_beats(&p), 0.0, 1.0);
        assert!(!t.confident, "onset {} chance {}", t.onset_rate, t.chance_onset_rate);
    }
}
