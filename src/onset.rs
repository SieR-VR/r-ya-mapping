// Onset detection feeding the BPM detector.
//
// ArrowVortex delegated this stage to `aubio`'s complex-domain onset
// detector (`Editor/FindOnsets.cpp`, aubio method "complex"). aubio is a C
// library, so we re-implement an equivalent here in pure Rust: the
// complex-domain onset detector (Duxbury et al.) which predicts each frame's
// magnitude/phase from the previous frame and accumulates the deviation as a
// detection function, followed by adaptive-threshold peak picking. This is the
// same family of technique ArrowVortex used to produce the `Onset { pos,
// strength }` values that `FindTempo.cpp` consumes.
//
// The public surface is intentionally identical in spirit to the C
// `FindOnsets`: in go mono float samples + sample rate, out come onsets with
// sample positions and strengths.

#![allow(dead_code)]

use rustfft::{num_complex::Complex, FftPlanner};

/// A detected note onset: sample position and how strong it is (0..~1).
#[derive(Clone, Copy, Debug)]
pub struct Onset {
    pub pos: usize,
    pub strength: f64,
}

const HOP: usize = 256; // aubio used windowlen=harp; reproduce hop=256
const WINSIZE: usize = 1024; // 4x hop, matching aubio's bufsize=windowlen*4

/// Detect onsets in the mono sample stream.
pub fn find_onsets(samples: &[f32], sample_rate: u32) -> Vec<Onset> {
    if samples.len() < WINSIZE {
        return Vec::new();
    }
    let df = complex_domain_detection_function(samples);
    let positions = pick_peaks(&df, HOP, sample_rate);

    // ArrowVortex's `FindOnsets` (via aubio) emitted onsets with a fixed
    // strength of 1.0; the beat-offset stage (`exec` in FindTempo.cpp) then
    // refined strength to the local energy around each onset. We perform the
    // same refinement here, searching a window wide enough to survive
    // hop-aligned peak jitter.
    const STRENGTH_HALF: usize = 256; // ~= one hop, ~5.8 ms @ 44.1k
    let mut onsets = Vec::with_capacity(positions.len());
    for pos in positions {
        let strength = local_energy(samples, pos as usize, STRENGTH_HALF).clamp(0.0, 1.0);
        onsets.push(Onset {
            pos: pos as usize,
            strength: strength as f64,
        });
    }
    onsets
}

/// Complex-domain detection function (Duxbury, Davies, Sandler).
/// df[i] = sum_k | |X[i,k]| - |R[i,k]| |, where R predicts magnitude from
/// the expected phase change combined with last magnitude.
fn complex_domain_detection_function(samples: &[f32]) -> Vec<f64> {
    let window = hann_window(WINSIZE);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(WINSIZE);
    let mut frame: Vec<Complex<f32>> = vec![Complex::new(0.0, 0.0); WINSIZE];
    let mut prev_phase = vec![0.0f32; WINSIZE];
    let mut prev_mag = vec![0.0f32; WINSIZE];
    let mut df = Vec::new();

    let omega = std::f32::consts::TAU / WINSIZE as f32;
    let expected_delta = omega * HOP as f32; // phase advance per bin

    let mut start = 0usize;
    while start + WINSIZE <= samples.len() {
        for (i, w) in window.iter().enumerate() {
            frame[i] = Complex::new(samples[start + i] * w, 0.0);
        }
        fft.process(&mut frame);
        let mut total = 0.0f64;
        for k in 0..WINSIZE {
            let mag = frame[k].norm();
            let phase = frame[k].arg();
            // Predicted phase from the previous frame.
            let mut pred_phase = prev_phase[k] + expected_delta;
            pred_phase = wrap_phase(pred_phase);
            let dphase = wrap_phase(phase - pred_phase);
            // Predicted magnitude = previous magnitude.
            let dmag = mag - prev_mag[k];
            // Complex-domain deviation: combine magnitude and phase error.
            let dev = f64::from(
                (Complex::new(mag * dphase.cos(), mag * dphase.sin())
                    - Complex::new(prev_mag[k] * 0.0, 0.0))
                .norm(),
            );
            // Used form: |mag - prev_mag| is the magnitude term; phase term
            // weighted. Following Duxbury, deviation = |X - R| where R uses
            // predicted phase and prev magnitude:
            let r = Complex::new(prev_mag[k] * dphase.cos(), prev_mag[k] * dphase.sin());
            let x = Complex::new(mag * phase.cos(), mag * phase.sin());
            total += f64::from((x - r).norm()) + dev * 0.0 + f64::from(dmag.abs());
        }
        df.push(total / WINSIZE as f64);
        prev_phase.copy_from_slice(&frame.iter().map(|c| c.arg()).collect::<Vec<_>>());
        prev_mag.copy_from_slice(&frame.iter().map(|c| c.norm()).collect::<Vec<_>>());
        start += HOP;
    }
    df
}

fn wrap_phase(mut p: f32) -> f32 {
    while p > std::f32::consts::PI {
        p -= std::f32::consts::TAU;
    }
    while p < -std::f32::consts::PI {
        p += std::f32::consts::TAU;
    }
    p
}

fn hann_window(n: usize) -> Vec<f32> {
    let mut w = Vec::with_capacity(n);
    for i in 0..n {
        let t = 2.0 * std::f32::consts::PI * i as f32 / n as f32;
        w.push(0.5 * (1.0 - t.cos()));
    }
    w
}

/// Local average energy around `pos` within +/- `half`, normalized.
fn local_energy(samples: &[f32], pos: usize, half: usize) -> f32 {
    let a = pos.saturating_sub(half);
    let b = (pos + half).min(samples.len());
    if b <= a {
        return 0.0;
    }
    let mut s = 0.0f64;
    for i in a..b {
        s += (samples[i].abs() as f64) * (samples[i].abs() as f64);
    }
    let rms = (s / (b - a) as f64).sqrt();
    (rms as f32) * 4.0 // scale into a 0..~1 strength range
}

/// Peak picking with a moving median threshold, mirroring aubio's approach:
/// a frame is an onset only if it's a local max and above the adaptive
/// threshold, with a refractory period to avoid double-triggers.
fn pick_peaks(df: &[f64], hop: usize, _sample_rate: u32) -> Vec<usize> {
    let n = df.len();
    if n == 0 {
        return Vec::new();
    }
    let win = 20usize; // frames for the median window (~0.1s at hop=256/44.1k)
    let min_separation = 4usize; // frames between successive onsets

    let threshold = adaptive_threshold(df, win);
    let mut peaks = Vec::new();
    for i in 1..n.saturating_sub(1) {
        let v = df[i];
        if v > threshold[i] && v > df[i - 1] && v >= df[i + 1] {
            let pos = i * hop;
            if let Some(&last) = peaks.last() {
                if pos - last < min_separation * hop {
                    let prev_index = last / hop;
                    let prev_val: f64 = df.get(prev_index).copied().unwrap_or(0.0);
                    if v > prev_val.max(df[prev_index]) {
                        *peaks.last_mut().unwrap() = pos;
                    }
                    continue;
                }
            }
            peaks.push(pos);
        }
    }
    peaks
}

fn adaptive_threshold(df: &[f64], win: usize) -> Vec<f64> {
    let n = df.len();
    let mut out = vec![0.0f64; n];
    let half = win / 2;
    for i in 0..n {
        let a = i.saturating_sub(half);
        let b = (i + half).min(n);
        let slice = &df[a..b];
        let mut s = slice.to_vec();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let med = if s.is_empty() { 0.0 } else { s[s.len() / 2] };
        // Threshold above the median by a margin (aubio uses peak-picking
        // thresholds; a 20% margin over the local median works well here).
        out[i] = med * 1.2 + 1e-4;
    }
    out
}
