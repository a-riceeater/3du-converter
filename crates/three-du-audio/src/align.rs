//! Finds where the drill's count clock sits in a recording.
//!
//! 1. Onset-strength envelopes are computed from the audio in ten frequency bands
//!    (log-magnitude spectral flux, 1024-sample Hann window, 128-sample hop), so that a
//!    metronome in the background is not buried under the band's attacks.
//! 2. Every candidate offset is scored by summing the envelopes at the drill's beat times
//!    (each count's time plus the offset), window by window, with each band and window
//!    normalized on its own. Because the drill's tempo changes, only the true offset lines
//!    up every section, so the best score stands out from its rivals.
//! 3. Each window's best offset near the peak is refined to sub-hop precision; their median
//!    is the result, and each set's neighborhood is re-aligned locally to measure drift.

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

/// Upper edges (Hz) of the onset bands. A metronome or click track usually has a band to
/// itself (often far above the music's fundamentals), where a broadband envelope would bury
/// it under the band's attacks. Bands above the Nyquist frequency are left out.
const BAND_EDGES: [f64; 10] = [200.0, 400.0, 800.0, 1600.0, 3200.0, 6400.0, 9000.0, 12000.0, 16000.0, f64::INFINITY];

/// Moving average of `v` over `±half` entries.
fn moving_average(v: &[f32], half: usize) -> Vec<f64> {
    let mut prefix = vec![0f64; v.len() + 1];
    for (i, x) in v.iter().enumerate() {
        prefix[i + 1] = prefix[i] + *x as f64;
    }
    (0..v.len())
        .map(|i| {
            let (a, b) = (i.saturating_sub(half), (i + half + 1).min(v.len()));
            (prefix[b] - prefix[a]) / (b - a) as f64
        })
        .collect()
}

/// Onset strength per hop in each frequency band of `BAND_EDGES` (log-magnitude spectral
/// flux), with the slowly varying loudness removed and each band scaled to unit RMS.
pub fn band_envelopes(audio: &Audio) -> Vec<Vec<f32>> {
    let x = &audio.mono;
    if x.len() < WINDOW {
        return vec![];
    }
    let bin_hz = audio.sample_rate as f64 / WINDOW as f64;
    let band_of: Vec<usize> = (0..WINDOW / 2).map(|k| BAND_EDGES.iter().position(|&e| k as f64 * bin_hz < e).unwrap_or(BAND_EDGES.len() - 1)).collect();
    let bands = band_of[WINDOW / 2 - 1] + 1;
    let window: Vec<f32> = (0..WINDOW).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WINDOW as f32).cos()).collect();
    let frames = (x.len() - WINDOW) / HOP + 1;
    let mut prev = vec![0f32; WINDOW / 2];
    let mut env = vec![Vec::with_capacity(frames); bands];
    let (mut re, mut im) = (vec![0f32; WINDOW], vec![0f32; WINDOW]);
    let mut flux = vec![0f32; bands];
    for f in 0..frames {
        let s = f * HOP;
        for i in 0..WINDOW {
            re[i] = x[s + i] * window[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        flux.fill(0.0);
        for k in 1..WINDOW / 2 {
            let mag = (1.0 + 100.0 * (re[k] * re[k] + im[k] * im[k]).sqrt()).ln();
            flux[band_of[k]] += (mag - prev[k]).max(0.0);
            prev[k] = mag;
        }
        // The first frame has nothing to compare with; it only primes `prev`.
        for (e, v) in env.iter_mut().zip(&flux) {
            e.push(if f == 0 { 0.0 } else { *v });
        }
    }
    // Remove the slowly varying loudness (0.5 s moving average) and keep rising edges.
    let w = (0.5 * audio.sample_rate as f64 / HOP as f64) as usize;
    env.into_iter()
        .map(|e| {
            let avg = moving_average(&e, w);
            let mut out: Vec<f32> = e.iter().zip(&avg).map(|(v, a)| (*v as f64 - a).max(0.0) as f32).collect();
            let rms = (out.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / out.len() as f64).sqrt();
            if rms > 1e-12 {
                out.iter_mut().for_each(|v| *v = (*v as f64 / rms) as f32);
            }
            out
        })
        .collect()
}

/// The bands' mean onset strength (never negative).
fn band_mean(bands: &[Vec<f32>]) -> Vec<f32> {
    let n = bands.first().map_or(0, Vec::len);
    (0..n).map(|i| bands.iter().map(|b| b[i]).sum::<f32>() / bands.len() as f32).collect()
}

/// An onset this many times its local average strength is a clear one.
const STRONG_ONSET: f32 = 3.0;
/// Cap on relative onset strength in the beat tracker, so that one loud attack is worth
/// little more than a clear click.
const RELATIVE_CAP: f32 = 2.0 * STRONG_ONSET;

/// Weighted mean of the bands' onset strength, relative to its own average over the
/// surrounding 3 s, so that quiet passages count as much as loud ones.
fn relative_onsets(bands: &[Vec<f32>], weights: &[f64], fr: f64) -> Vec<f32> {
    let total: f64 = weights.iter().sum();
    let n = bands.first().map_or(0, Vec::len);
    let mix: Vec<f32> = (0..n).map(|i| (bands.iter().zip(weights).map(|(b, w)| b[i] as f64 * w).sum::<f64>() / total) as f32).collect();
    let avg = moving_average(&mix, (1.5 * fr) as usize);
    mix.iter().zip(&avg).map(|(v, a)| if *a > 1e-9 { (*v as f64 / a) as f32 } else { 0.0 }).collect()
}

/// How much each band follows the beats at `offset`: the square of its mean z-score over
/// windows of beats (each window at its own best offset nearby), or 0 when it does not.
/// Every band weighs the same when none follows the beats.
fn band_weights(bands: &[Vec<f32>], fr: f64, beats: &[f64], offset: f64) -> Vec<f64> {
    let step = 1.0 / fr;
    let grid: Vec<f64> = (0..).map(|i| offset - 1.0 + i as f64 * step).take_while(|o| *o <= offset + 1.0).collect();
    let mut z = vec![0f64; bands.len()];
    let windows = beats.chunks(WINDOW_BEATS.min(beats.len()).max(1));
    let count = windows.len() as f64;
    for w in windows {
        let ws = WindowScore::new(bands, fr, w, &grid);
        let (o, _) = ws.local_best(&grid, offset, LOCAL_REACH);
        z.iter_mut().enumerate().for_each(|(b, v)| *v += ws.band_z(b, o) / count);
    }
    let weights: Vec<f64> = z.iter().map(|v| v.max(0.0).powi(2)).collect();
    if weights.iter().sum::<f64>() > 0.0 {
        weights
    } else {
        vec![1.0; bands.len()]
    }
}

/// Onset strength per hop over all bands, normalized to zero mean and unit variance.
pub fn onset_envelope(audio: &Audio) -> Vec<f32> {
    let mut out = band_mean(&band_envelopes(audio));
    if out.is_empty() {
        return out;
    }
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
    /// Fraction of tracked beats on a clear onset (three times the local average strength).
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
/// nominal time (shifted by `offset`), and is pulled toward it the more the farther it goes
/// (a band's off-beat pattern can be as strong as the click it plays to; only the drill's
/// clock says which is the beat). Each interval may stretch between 0.75× and 1.33× its
/// notated length; intervals far from notated length are penalized. Pass a narrow `window`
/// (a few tenths of a second) when `offset` is known to be right.
///
/// The tracker listens mostly to the bands that lock onto the beats at `offset` (a metronome
/// in the background, when there is one), and to onsets relative to their surroundings, so
/// that one loud attack cannot pull a beat away from a steady click.
pub fn track_beats(audio: &Audio, beats: &[f64], offset: f64, window: f64) -> BeatTrack {
    let bands = band_envelopes(audio);
    let fr = frame_rate(audio);
    let relative = relative_onsets(&bands, &band_weights(&bands, fr, beats, offset), fr);
    let env: Vec<f32> = relative.iter().map(|v| v.min(RELATIVE_CAP)).collect();
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
    // The cap flattens the tops of clear onsets, so each beat moves to its onset's peak in all
    // bands together (the envelope `ATTACK_POSITION` was calibrated on).
    let all = relative_onsets(&bands, &vec![1.0; bands.len()], fr);
    let times: Vec<f64> = dp_track(&env, fr, beats, offset, window).iter().map(|&t| peak_near(&all, fr, t, 0.015)).collect();
    let strong =|e: &[f32], t: f64| sample(e, fr, t) > STRONG_ONSET as f64;
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

/// The time of `env`'s highest point within `±reach` s of `t`, interpolated between hops.
fn peak_near(env: &[f32], fr: f64, t: f64, reach: f64) -> f64 {
    let lag = ATTACK_POSITION / HOP as f64;
    let center = (t * fr - lag).round() as i64;
    let r = (reach * fr).round() as i64;
    let at = |f: i64| if f >= 0 && (f as usize) < env.len() { env[f as usize] as f64 } else { f64::MIN };
    let Some(f) = (center - r..=center + r).filter(|&f| at(f) > f64::MIN).max_by(|a, b| at(*a).total_cmp(&at(*b))) else { return t };
    let (a, b, c) = (at(f - 1), at(f), at(f + 1));
    let denom = a - 2.0 * b + c;
    let shift = if a > f64::MIN && c > f64::MIN && denom < 0.0 { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) } else { 0.0 };
    (f as f64 + shift + lag) / fr
}

fn dp_track(env: &[f32], fr: f64, beats: &[f64], offset: f64, window: f64) -> Vec<f64> {
    const TIGHTNESS: f64 = 200.0;
    // A beat at the edge of the window costs as much as the clearest onset gains.
    const PULL: f64 = RELATIVE_CAP as f64;
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
        let local = |f: i64| {
            let dev = (f - center) as f64 / half.max(1) as f64;
            (if f >= 0 && (f as usize) < env.len() { env[f as usize] as f64 } else { -1.0 }) - PULL * dev * dev
        };
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

/// Beats per scoring window.
const WINDOW_BEATS: usize = 16;
/// How far each window may sit from the global offset while still supporting it (s). A
/// performance or recording that drifts by a few tens of milliseconds over minutes still has
/// one clear offset.
const DRIFT_TOLERANCE: f64 = 0.015;
/// How far a window's own best offset is searched from the global one (s).
const LOCAL_REACH: f64 = 0.06;

/// Mean and standard deviation of `v`; the deviation is 0 when `v` is flat.
fn mean_sd(v: &[f64]) -> (f64, f64) {
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
    (mean, if sd > 1e-9 { sd } else { 0.0 })
}

/// Scores of one stretch of beats over a grid of offsets. Each band's score (its mean
/// envelope at the beats) is a z-score against the same band at the other offsets,
/// so a quiet passage counts as much as a loud one and a band that only carries the click
/// counts as much as the band's whole sound. The window's score is the bands' mean, normalized
/// the same way (`at` leaves out that last step, which never moves a window's best offset).
struct WindowScore<'a> {
    bands: &'a [Vec<f32>],
    fr: f64,
    beats: &'a [f64],
    /// Mean and standard deviation per band (0: the band is flat here and is ignored).
    stats: Vec<(f64, f64)>,
    curve: Vec<f64>,
}

impl<'a> WindowScore<'a> {
    fn new(bands: &'a [Vec<f32>], fr: f64, beats: &'a [f64], grid: &[f64]) -> Self {
        let mut stats = Vec::with_capacity(bands.len());
        let mut curve = vec![0f64; grid.len()];
        for env in bands {
            let raw: Vec<f64> = grid.iter().map(|&o| score(env, fr, beats, o)).collect();
            let (med, scale) = mean_sd(&raw);
            if scale > 0.0 {
                curve.iter_mut().zip(&raw).for_each(|(c, v)| *c += (v - med) / scale / bands.len() as f64);
            }
            stats.push((med, scale));
        }
        // Every window gets the same say, however many bands carry its beats.
        let (med, scale) = mean_sd(&curve);
        if scale > 0.0 {
            curve.iter_mut().for_each(|c| *c = (*c - med) / scale);
        }
        WindowScore { bands, fr, beats, stats, curve }
    }

    /// Band `b`'s z-score at any offset (off the grid too).
    fn band_z(&self, b: usize, o: f64) -> f64 {
        let (med, scale) = self.stats[b];
        if scale > 0.0 {
            (score(&self.bands[b], self.fr, self.beats, o) - med) / scale
        } else {
            0.0
        }
    }

    /// The score at any offset (off the grid too).
    fn at(&self, o: f64) -> f64 {
        (0..self.bands.len()).map(|b| self.band_z(b, o)).sum::<f64>() / self.bands.len() as f64
    }

    /// Best offset within `±reach` of `center`, refined to 0.25 ms, with its score.
    fn local_best(&self, grid: &[f64], center: f64, reach: f64) -> (f64, f64) {
        let step = 1.0 / self.fr;
        let coarse = grid.iter().zip(&self.curve).filter(|(o, _)| (*o - center).abs() <= reach).max_by(|a, b| a.1.total_cmp(b.1)).map_or(center, |(o, _)| *o);
        let mut best = (coarse, self.at(coarse));
        let mut o = coarse - step;
        while o <= coarse + step {
            let s = self.at(o);
            if s > best.1 && (o - center).abs() <= reach {
                best = (o, s);
            }
            o += 0.00025;
        }
        best
    }
}

/// Aligns beat times `beats` (seconds on the drill clock, one per count) to the audio.
/// `search_range` bounds the offset; `sets` are beat indices used for the drift check.
///
/// The beats are scored in windows of `WINDOW_BEATS`. A window at one tempo fits every offset
/// that is a whole number of its beats from the true one; summed over windows at different
/// tempos, only the true offset is supported everywhere. Each window may sit within
/// `DRIFT_TOLERANCE` of the global offset, and the result is the median of the windows' own
/// best offsets near it.
pub fn align(audio: &Audio, beats: &[f64], search_range: (f64, f64), sets: &[usize]) -> Alignment {
    let bands = band_envelopes(audio);
    let fr = frame_rate(audio);
    let step = 1.0 / fr;
    let grid: Vec<f64> = (0..).map(|i| search_range.0 + i as f64 * step).take_while(|o| *o <= search_range.1 + 1e-9).collect();
    if bands.is_empty() || beats.is_empty() || grid.is_empty() {
        let offset = search_range.0.max(0.0).min(search_range.1);
        return Alignment { offset_seconds: offset, peak_ratio: 1.0, beat_hit_rate: 0.0, chance_hit_rate: 0.0, max_drift_seconds: 0.0, beats_used: beats.len(), confident: false };
    }

    let size = beats.len().div_ceil(beats.len().div_ceil(WINDOW_BEATS));
    let windows: Vec<WindowScore> = beats.chunks(size).map(|w| WindowScore::new(&bands, fr, w, &grid)).collect();
    let reach = (DRIFT_TOLERANCE * fr).round() as usize;
    let mut total = vec![0f64; grid.len()];
    for w in &windows {
        for (i, t) in total.iter_mut().enumerate() {
            let (a, b) = (i.saturating_sub(reach), (i + reach + 1).min(grid.len()));
            *t += w.curve[a..b].iter().copied().fold(f64::MIN, f64::max) / windows.len() as f64;
        }
    }
    let (peak, best) = total.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, s)| (grid[i], *s)).unwrap();

    // Strongest competing offset at least ~one beat (≥ 150 ms) away.
    let rival = grid.iter().zip(&total).filter(|(o, _)| (*o - peak).abs() >= 0.15).map(|(_, s)| *s).fold(f64::MIN, f64::max);
    let peak_ratio = if rival > 0.0 { best / rival } else { f64::INFINITY };

    let mut locals: Vec<f64> = windows.iter().filter(|w| w.stats.iter().any(|s| s.1 > 0.0)).map(|w| w.local_best(&grid, peak, LOCAL_REACH).0).collect();
    locals.sort_by(f64::total_cmp);
    let offset = match locals.len() {
        0 => peak,
        n if n % 2 == 1 => locals[n / 2],
        n => (locals[n / 2 - 1] + locals[n / 2]) / 2.0,
    };

    // A beat "hits" when, within ±35 ms of it, the onset strength reaches three times its
    // average over the surrounding 3 s.
    let relative = relative_onsets(&bands, &vec![1.0; bands.len()], fr);
    let hit_rate = |o: f64| {
        let hits = beats.iter().filter(|b| (-35..=35).step_by(5).any(|ms| sample(&relative, fr, *b + o + ms as f64 / 1000.0) > STRONG_ONSET as f64)).count();
        hits as f64 / beats.len() as f64
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
        let local = WindowScore::new(&bands, fr, &beats[start..end], &grid);
        let (lo, _) = local.local_best(&grid, offset, 0.08);
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
