//! Finds where the drill's count clock sits in a recording.
//!
//! 1. An onset-strength envelope is computed from the audio (log-magnitude spectral flux,
//!    1024-sample Hann window, 128-sample hop).
//! 2. Every candidate offset is scored by summing the envelope at the drill's beat times
//!    (each count's time plus the offset). Because the drill's tempo changes, only the true
//!    offset lines up every section, so the best score stands out from its rivals.
//! 3. The peak is refined to sub-hop precision, and each set's neighborhood is re-aligned
//!    locally to measure drift.

use serde::Serialize;

use crate::wav::Audio;

const WINDOW: usize = 1024;
const HOP: usize = 128;
/// Where in the window an attack sits when the flux peaks (calibrated with synthetic clicks:
/// rising energy is detected as the attack enters the high-weight part of the Hann window).
const ATTACK_POSITION: f64 = 0.785 * WINDOW as f64;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Alignment {
    /// Audio time (s) of the drill's count 0. Audio time of count c = count_time[c] + offset.
    pub offset_seconds: f64,
    /// Best score divided by the best score at least one beat away. Above ~1.3 is unambiguous.
    pub peak_ratio: f64,
    /// Fraction of beats that land within 35 ms of a detected onset.
    pub beat_hit_rate: f64,
    /// The same rate at deliberately wrong offsets: what chance alone gives.
    pub chance_hit_rate: f64,
    /// Largest local disagreement with the global offset across sets (s).
    pub max_drift_seconds: f64,
    pub beats_used: usize,
    pub confident: bool,
}

/// Radix-2 in-place FFT on interleaved (re, im) buffers; `n` must be a power of two.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f64).sin_cos();
                let (wr, wi) = (c as f32, s as f32);
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

/// Onset strength per hop, normalized to zero mean and unit variance.
pub fn onset_envelope(audio: &Audio) -> Vec<f32> {
    let x = &audio.mono;
    if x.len() < WINDOW {
        return vec![];
    }
    let window: Vec<f32> = (0..WINDOW).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WINDOW as f32).cos()).collect();
    let frames = (x.len() - WINDOW) / HOP + 1;
    let mut prev = vec![0f32; WINDOW / 2];
    let mut env = Vec::with_capacity(frames);
    let (mut re, mut im) = (vec![0f32; WINDOW], vec![0f32; WINDOW]);
    for f in 0..frames {
        let s = f * HOP;
        for i in 0..WINDOW {
            re[i] = x[s + i] * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        let mut flux = 0f32;
        for k in 1..WINDOW / 2 {
            let mag = (1.0 + 100.0 * (re[k] * re[k] + im[k] * im[k]).sqrt()).ln();
            flux += (mag - prev[k]).max(0.0);
            prev[k] = mag;
        }
        // The first frame has nothing to compare with; it only primes `prev`.
        env.push(if f == 0 { 0.0 } else { flux });
    }
    // Remove the slowly varying loudness (0.5 s moving average) and keep rising edges.
    let w = (0.5 * audio.sample_rate as f64 / HOP as f64) as usize;
    let mut prefix = vec![0f64; env.len() + 1];
    for (i, v) in env.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *v as f64;
    }
    let mut out: Vec<f32> = (0..env.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(env.len()));
            (env[i] as f64 - (prefix[b] - prefix[a]) / (b - a) as f64).max(0.0) as f32
        })
        .collect();
    let mean = out.iter().map(|v| *v as f64).sum::<f64>() / out.len() as f64;
    let var = out.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / out.len() as f64;
    let sd = var.sqrt().max(1e-9);
    for v in &mut out {
        *v = ((*v as f64 - mean) / sd) as f32;
    }
    out
}

fn frame_rate(audio: &Audio) -> f64 {
    audio.sample_rate as f64 / HOP as f64
}

/// Envelope value at time `t` (s), linearly interpolated.
fn sample(env: &[f32], fr: f64, t: f64) -> f64 {
    let pos = t * fr - ATTACK_POSITION / HOP as f64;
    if pos < 0.0 || pos >= (env.len() - 1) as f64 {
        return 0.0;
    }
    let i = pos.floor() as usize;
    let f = pos - i as f64;
    env[i] as f64 * (1.0 - f) + env[i + 1] as f64 * f
}

fn score(env: &[f32], fr: f64, beats: &[f64], offset: f64) -> f64 {
    beats.iter().map(|b| sample(env, fr, b + offset)).sum::<f64>() / beats.len().max(1) as f64
}

/// Best offset in `[lo, hi]`, searched at one-hop steps then refined to 0.25 ms.
fn search(env: &[f32], fr: f64, beats: &[f64], lo: f64, hi: f64) -> (f64, f64) {
    let step = 1.0 / fr;
    let mut best = (lo, f64::MIN);
    let mut o = lo;
    while o <= hi {
        let s = score(env, fr, beats, o);
        if s > best.1 {
            best = (o, s);
        }
        o += step;
    }
    let mut fine = best;
    let mut o = best.0 - step;
    while o <= best.0 + step {
        let s = score(env, fr, beats, o);
        if s > fine.1 {
            fine = (o, s);
        }
        o += 0.00025;
    }
    fine
}

/// Beat-by-beat alignment of a performance that does not keep the drill's exact tempo.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BeatTrack {
    /// Audio time (s) of each drill beat, in order.
    pub times: Vec<f64>,
    /// Fraction of tracked beats on a clear onset (envelope above one standard deviation).
    pub onset_rate: f64,
    /// Same measure for the best single offset, for comparison.
    pub constant_offset_onset_rate: f64,
    /// Onset rate the tracker reaches on the time-reversed recording (chance level).
    pub chance_onset_rate: f64,
    /// Largest and median ratio of performed to notated beat length.
    pub max_tempo_deviation: f64,
    pub median_tempo_deviation: f64,
    pub confident: bool,
}

/// Dynamic-programming beat tracker (after Ellis 2007) that knows how many beats there are
/// and how long each should be. Each beat may land anywhere within `±window` s of its
/// nominal time (shifted by `offset`), and each interval may stretch between 0.75× and
/// 1.33× its notated length; intervals far from notated length are penalized.
pub fn track_beats(audio: &Audio, beats: &[f64], offset: f64, window: f64) -> BeatTrack {
    let env = onset_envelope(audio);
    let fr = frame_rate(audio);
    let n = beats.len();
    if n < 2 || env.is_empty() {
        return BeatTrack {
            times: beats.iter().map(|b| b + offset).collect(),
            onset_rate: 0.0,
            constant_offset_onset_rate: 0.0,
            chance_onset_rate: 0.0,
            max_tempo_deviation: 1.0,
            median_tempo_deviation: 1.0,
            confident: false,
        };
    }
    let times = dp_track(&env, fr, beats, offset, window);
    let strong = |e: &[f32], t: f64| sample(e, fr, t) > 1.0;
    let rate = |e: &[f32], ts: &[f64]| ts.iter().filter(|&&t| strong(e, t)).count() as f64 / n as f64;
    let onset_rate = rate(&env, &times);
    let nominal: Vec<f64> = beats.iter().map(|b| b + offset).collect();
    let constant_offset_onset_rate = rate(&env, &nominal);
    let reversed: Vec<f32> = env.iter().rev().copied().collect();
    let chance_onset_rate = rate(&reversed, &dp_track(&reversed, fr, beats, offset, window));
    let mut ratios: Vec<f64> = (1..n).map(|k| (times[k] - times[k - 1]) / (beats[k] - beats[k - 1])).collect();
    ratios.sort_by(f64::total_cmp);
    let dev = |r: f64| if r >= 1.0 { r } else { 1.0 / r };
    BeatTrack {
        max_tempo_deviation: ratios.iter().map(|r| dev(*r)).fold(1.0, f64::max),
        median_tempo_deviation: dev(ratios[ratios.len() / 2]),
        confident: onset_rate >= 0.6 && onset_rate - chance_onset_rate >= 0.2,
        times,
        onset_rate,
        constant_offset_onset_rate,
        chance_onset_rate,
    }
}

fn dp_track(env: &[f32], fr: f64, beats: &[f64], offset: f64, window: f64) -> Vec<f64> {
    const TIGHTNESS: f64 = 40.0;
    let n = beats.len();
    let lag = ATTACK_POSITION / HOP as f64;
    let to_frame = |t: f64| t * fr - lag;
    let to_time = |f: f64| (f + lag) / fr;
    let half = (window * fr).round() as i64;

    // Candidate frames per beat and their running best scores.
    let mut starts: Vec<i64> = Vec::with_capacity(n);
    let mut scores: Vec<Vec<f64>> = Vec::with_capacity(n);
    let mut back: Vec<Vec<i64>> = Vec::with_capacity(n);
    for (k, b) in beats.iter().enumerate() {
        let center = to_frame(b + offset).round() as i64;
        let lo = center - half;
        let hi = center + half;
        let local = |f: i64| if f >= 0 && (f as usize) < env.len() { env[f as usize] as f64 } else { -1.0 };
        let mut sc = Vec::with_capacity((hi - lo + 1) as usize);
        let mut bk = Vec::with_capacity((hi - lo + 1) as usize);
        for f in lo..=hi {
            if k == 0 {
                sc.push(local(f));
                bk.push(-1);
                continue;
            }
            let nominal = (beats[k] - beats[k - 1]) * fr;
            let (pmin, pmax) = ((nominal * 0.75).floor() as i64, (nominal * 1.33).ceil() as i64);
            let (plo, pscores) = (starts[k - 1], &scores[k - 1]);
            let mut best = (f64::MIN, -1i64);
            for d in pmin.max(1)..=pmax {
                let prev = f - d;
                let idx = prev - plo;
                if idx < 0 || idx as usize >= pscores.len() {
                    continue;
                }
                let pen = (d as f64 / nominal).ln();
                let s = pscores[idx as usize] - TIGHTNESS * pen * pen;
                if s > best.0 {
                    best = (s, prev);
                }
            }
            sc.push(if best.1 < 0 { f64::MIN / 4.0 } else { best.0 + local(f) });
            bk.push(best.1);
        }
        starts.push(lo);
        scores.push(sc);
        back.push(bk);
    }
    // Trace back from the best final frame.
    let last = &scores[n - 1];
    let mut f = starts[n - 1] + last.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i as i64).unwrap_or(0);
    let mut frames = vec![0i64; n];
    for k in (0..n).rev() {
        frames[k] = f;
        if k > 0 {
            f = back[k][(f - starts[k]) as usize];
        }
    }
    frames.iter().map(|&f| to_time(f as f64)).collect()
}

/// The recording with a click on every beat (accented at `accents`), for checking by ear.
pub fn click_check(audio: &Audio, times: &[f64], accents: &[usize]) -> Vec<f32> {
    let rate = audio.sample_rate as f64;
    let mut out: Vec<f32> = audio.mono.iter().map(|s| s * 0.6).collect();
    for (k, t) in times.iter().enumerate() {
        let (freq, gain) = if accents.contains(&k) { (2000.0, 0.5) } else { (1200.0, 0.35) };
        let start = (t * rate).round() as i64;
        for i in 0..(0.04 * rate) as i64 {
            let j = start + i;
            if j >= 0 && (j as usize) < out.len() {
                let x = i as f64 / rate;
                out[j as usize] += (gain * (x * 2.0 * std::f64::consts::PI * freq).sin() * (-x * 90.0).exp()) as f32;
            }
        }
    }
    out
}

/// Best offset for each consecutive window of `chunk` beats, searched within `±spread` s of
/// `center`. Returns (first beat index, offset, peak ratio against the best offset ≥ 150 ms away).
pub fn local_offsets(audio: &Audio, beats: &[f64], center: f64, spread: f64, chunk: usize) -> Vec<(usize, f64, f64)> {
    let env = onset_envelope(audio);
    let fr = frame_rate(audio);
    let mut out = Vec::new();
    for start in (0..beats.len()).step_by(chunk.max(1)) {
        let w = &beats[start..(start + chunk).min(beats.len())];
        let (o, best) = search(&env, fr, w, center - spread, center + spread);
        let mut rival = f64::MIN;
        let mut x = center - spread;
        while x <= center + spread {
            if (x - o).abs() >= 0.15 {
                rival = rival.max(score(&env, fr, w, x));
            }
            x += 1.0 / fr;
        }
        out.push((start, o, if rival > 0.0 { best / rival } else { f64::INFINITY }));
    }
    out
}

/// Aligns beat times `beats` (seconds on the drill clock, one per count) to the audio.
/// `search_range` bounds the offset; `sets` are beat indices used for the drift check.
pub fn align(audio: &Audio, beats: &[f64], search_range: (f64, f64), sets: &[usize]) -> Alignment {
    let env = onset_envelope(audio);
    let fr = frame_rate(audio);
    let (offset, best) = search(&env, fr, beats, search_range.0, search_range.1);

    // Strongest competing offset at least ~one beat (≥ 150 ms) away.
    let step = 1.0 / fr;
    let mut rival = f64::MIN;
    let mut o = search_range.0;
    while o <= search_range.1 {
        if (o - offset).abs() >= 0.15 {
            rival = rival.max(score(&env, fr, beats, o));
        }
        o += step;
    }
    let peak_ratio = if rival > 0.0 { best / rival } else { f64::INFINITY };

    // A beat "hits" when the envelope peaks above the mean within ±35 ms of it.
    let hit_rate = |o: f64| {
        let hits = beats.iter().filter(|b| (-35..=35).step_by(5).any(|ms| sample(&env, fr, *b + o + ms as f64 / 1000.0) > 1.0)).count();
        hits as f64 / beats.len().max(1) as f64
    };
    let beat_hit_rate = hit_rate(offset);
    let chance_hit_rate = (hit_rate(offset + 0.173) + hit_rate(offset - 0.211) + hit_rate(offset + 0.097)) / 3.0;

    // Local re-alignment over the 16 beats before each set, within ±80 ms of the global offset.
    let mut max_drift = 0f64;
    for &end in sets {
        let start = end.saturating_sub(16);
        if end <= start + 4 || end > beats.len() {
            continue;
        }
        let local = &beats[start..end];
        let (lo, _) = search(&env, fr, local, offset - 0.08, offset + 0.08);
        max_drift = max_drift.max((lo - offset).abs());
    }

    Alignment {
        offset_seconds: offset,
        peak_ratio,
        beat_hit_rate,
        chance_hit_rate,
        max_drift_seconds: max_drift,
        beats_used: beats.len(),
        confident: peak_ratio >= 1.15 && beat_hit_rate - chance_hit_rate >= 0.2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_dft() {
        let mut re: Vec<f32> = (0..8).map(|i| (i as f32 * 0.7).sin()).collect();
        let mut im = vec![0f32; 8];
        let orig = re.clone();
        fft(&mut re, &mut im);
        for k in 0..8 {
            let (mut sr, mut si) = (0f64, 0f64);
            for (n, v) in orig.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * n) as f64 / 8.0;
                sr += *v as f64 * a.cos();
                si += *v as f64 * a.sin();
            }
            assert!((re[k] as f64 - sr).abs() < 1e-4 && (im[k] as f64 - si).abs() < 1e-4);
        }
    }
}
