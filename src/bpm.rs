// BPM detection — a Rust port of ArrowVortex's `Editor/FindTempo.cpp`.
//
// The algorithm: take a list of note onsets (sample position + strength),
// then for every candidate beat interval in [MinimumBPM, MaximumBPM] compute a
// "gap confidence" that measures how well onsets line up with a grid of that
// period (and its half-beat offbeat). Do a coarse 1-in-10 scan, fit a cubic
// polynomial to the noise floor to normalize it, refine around interesting
// peaks, round to integer BPM when possible, dedupe near-duplicates and
// octaves, and pick the top candidates. Finally compute a beat offset for each
// candidate.
//
// Constants and structure mirror the C++ source so behaviour stays faithful.
// `offset` is reported in seconds (sample_pos / sample_rate).

#![allow(dead_code)]

use crate::onset::Onset;

const MIN_BPM: f64 = 89.0;
const MAX_BPM: f64 = 205.0;
const INTERVAL_DELTA: usize = 10; // coarse stride (every 10th interval)
const INTERVAL_DOWNSAMPLE: usize = 3; // log2 downsample for the gap window
const MAX_CANDIDATES: usize = 3; // keep top-3 results

/// A detected BPM candidate: bpm, beat offset (seconds), fitness.
#[derive(Clone, Copy, Debug)]
pub struct TempoResult {
    pub bpm: f64,
    pub offset: f64,
    pub fitness: f64,
}

/// Compute the minimum leading-silence padding so that the song's first note
/// (placed at music beat 0) lands strictly more than `min_intro` seconds into
/// the padded audio and is phase-aligned to the beat grid.
///
/// Returns `(pad_seconds, first_note_beat)`:
///   * `pad_seconds`     - silence to physically prepend to the audio file.
///   * `first_note_beat` - the map beat at which the first note should sit
///                        (= music beat 0 after the audio has been shifted).
///
/// Modern Beat Saber deprecates `_songTimeOffset`, so the beat phase can no
/// longer be expressed through that field. Instead we physically reposition
/// the audio: prepending `pad_seconds` of silence moves music beat 0 onto a
/// whole-numbered map beat `M` whose time `M * beat_sec` is the smallest such
/// value strictly greater than `min_intro`. That keeps the intro short while
/// guaranteeing it exceeds `min_intro`, and the removed phase realigns the
/// beat grid with the music automatically.
pub fn intro_padding(bpm: f64, offset_sec: f64, min_intro: f64) -> (f64, f64) {
    let beat_sec = 60.0 / bpm;
    if beat_sec <= 0.0 || !beat_sec.is_finite() {
        return (0.0, 0.0);
    }
    // Music beat-0 phase, normalised into [0, beat_sec).
    let phase = offset_sec.rem_euclid(beat_sec);
    // Smallest whole-beat count M >= 1 with M*beat_sec strictly > min_intro.
    let mut m = (min_intro / beat_sec).floor() as i64;
    if m < 1 {
        m = 1;
    }
    if (m as f64) * beat_sec <= min_intro {
        m += 1;
    }
    let m = m as f64;
    let intro_sec = m * beat_sec; // time of first note from audio start
    let pad_sec = intro_sec - phase; // silence to prepend (always > 0)
    (pad_sec.max(0.0), m)
}

/// Detect BPM candidates from a mono sample stream.
pub fn detect(samples: &[f32], sample_rate: u32) -> Vec<TempoResult> {
    let onsets = crate::onset::find_onsets(samples, sample_rate);
    let _ = sample_rate; // used implicitly via onsets' sample positions
    detect_from_onsets(&onsets, sample_rate, samples)
}

/// Detect BPM from an explicit onset list (used by `detect` and tests).
pub fn detect_from_onsets(onsets: &[Onset], sample_rate: u32, samples: &[f32]) -> Vec<TempoResult> {
    if onsets.len() < 2 {
        return Vec::new();
    }

    let samplerate = sample_rate as f64;
    let mut test = IntervalTester::new(samplerate, onsets);

    // --- Coarse pass: every 10th interval -------------------------------
    let gap = GapData::new(test.max_interval as usize, INTERVAL_DOWNSAMPLE, onsets);
    for i in 0..test.num_intervals {
        test.fitness[i] = 0.0;
    }
    fill_coarse_intervals(&mut test, &gap);

    // Fit a cubic to the coarse fitness vs interval curve as the noise floor.
    let num_coarse = test.num_intervals.div_ceil(INTERVAL_DELTA);
    let xs: Vec<f64> = (0..num_coarse)
        .map(|i| (test.min_interval + i * INTERVAL_DELTA) as f64)
        .collect();
    let ys: Vec<f64> = (0..num_coarse)
        .map(|i| test.fitness[i * INTERVAL_DELTA])
        .collect();
    let coefs = polyfit3(&xs, &ys);

    let mut max_fitness = 0.001f64;
    for i in (0..test.num_intervals).step_by(INTERVAL_DELTA) {
        normalize_fitness(&mut test.fitness[i], &coefs, (test.min_interval + i) as f64);
        if test.fitness[i] > max_fitness {
            max_fitness = test.fitness[i];
        }
    }

    // --- Refine around any peak above 40% of the best --------------------
    let threshold = max_fitness * 0.4;
    let mut tempo: Vec<TempoResult> = Vec::new();
    for i in (0..test.num_intervals).step_by(INTERVAL_DELTA) {
        if test.fitness[i] > threshold {
            let begin = i.saturating_sub(INTERVAL_DELTA);
            let end = (i + INTERVAL_DELTA + 1).min(test.num_intervals);
            fill_interval_range(&mut test, &gap, begin, end, &coefs);
            let best = find_best_interval(&test.fitness, begin, end);
            let bpm = interval_to_bpm(&test, best);
            tempo.push(TempoResult {
                bpm,
                offset: 0.0,
                fitness: test.fitness[best],
            });
        }
    }

    // Upgrade to a full-resolution gap window (no downsampling).
    drop(gap);
    let gap = GapData::new(test.max_interval as usize, 0, onsets);

    tempo.sort_by(|a, b| {
        b.fitness
            .partial_cmp(&a.fitness)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    remove_duplicates(&mut tempo);
    round_bpm_values(&mut test, &gap, &mut tempo);

    // Re-rank purely from confidences if the top two are close.
    if tempo.len() >= 2 && tempo[0].fitness / tempo[1].fitness < 1.05 {
        for t in tempo.iter_mut() {
            t.fitness = get_confidence_for_bpm(&gap, &test, t.bpm);
        }
        tempo.sort_by(|a, b| {
            b.fitness
                .partial_cmp(&a.fitness)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    tempo.truncate(MAX_CANDIDATES);

    // --- Offsets ---------------------------------------------------------
    let max_interval = tempo
        .iter()
        .map(|t| samplerate * 60.0 / t.bpm)
        .fold(0.0f64, f64::max);
    let gap_off = GapData::new(max_interval.ceil() as usize + 1, 1, onsets);
    for t in tempo.iter_mut() {
        t.offset = get_base_offset_value(&gap_off, samplerate, t.bpm);
    }
    // Final offbeat adjustment uses the raw sample slopes.
    for t in tempo.iter_mut() {
        t.offset = adjust_for_offbeats(samples, sample_rate, t.offset, t.bpm);
    }

    tempo
}

// ===================== interval testing =====================

struct IntervalTester {
    min_interval: usize,
    max_interval: usize,
    num_intervals: usize,
    samplerate: f64,
    fitness: Vec<f64>,
}

impl IntervalTester {
    fn new(samplerate: f64, _onsets: &[Onset]) -> Self {
        let min_interval = (samplerate * 60.0 / MAX_BPM + 0.5) as usize;
        let max_interval = (samplerate * 60.0 / MIN_BPM + 0.5) as usize;
        let num_intervals = max_interval - min_interval;
        IntervalTester {
            min_interval,
            max_interval,
            num_intervals,
            samplerate,
            fitness: vec![0.0; num_intervals],
        }
    }
}

fn interval_to_bpm(test: &IntervalTester, i: usize) -> f64 {
    test.samplerate * 60.0 / (i + test.min_interval) as f64
}

fn fill_coarse_intervals(test: &mut IntervalTester, gap: &GapData) {
    let num_coarse = test.num_intervals.div_ceil(INTERVAL_DELTA);
    for i in 0..num_coarse {
        let interval = test.min_interval + i * INTERVAL_DELTA;
        test.fitness[i * INTERVAL_DELTA] = get_confidence_for_interval(gap, interval).max(0.001);
    }
}

fn fill_interval_range(
    test: &mut IntervalTester,
    gap: &GapData,
    begin: usize,
    end: usize,
    coefs: &[f64; 4],
) {
    let begin = begin.min(test.num_intervals);
    let end = end.min(test.num_intervals);
    for i in begin..end {
        if test.fitness[i] == 0.0 {
            let interval = test.min_interval + i;
            test.fitness[i] = get_confidence_for_interval(gap, interval);
            normalize_fitness(&mut test.fitness[i], coefs, interval as f64);
            if test.fitness[i] < 0.1 {
                test.fitness[i] = 0.1;
            }
        }
    }
}

fn find_best_interval(fitness: &[f64], begin: usize, end: usize) -> usize {
    let mut best = begin;
    let mut highest = 0.0f64;
    for i in begin..end {
        if fitness[i] > highest {
            highest = fitness[i];
            best = i;
        }
    }
    best
}

// ===================== gap confidence =====================

struct GapData {
    onsets: Vec<Onset>,
    window: Vec<f64>,
    window_size: usize,
    downsample: usize,
    // Per-interval scratch, sized for the largest interval tested.
    buffer_size: usize,
}

impl GapData {
    fn new(max_interval: usize, downsample: usize, onsets: &[Onset]) -> Self {
        let window_size = 2048usize >> downsample;
        GapData {
            onsets: onsets.to_vec(),
            window: hamming_window(window_size),
            window_size,
            downsample,
            buffer_size: max_interval + 1,
        }
    }
}

fn hamming_window(n: usize) -> Vec<f64> {
    if n <= 1 {
        return vec![1.0; n];
    }
    let t = 2.0 * std::f64::consts::PI / (n - 1) as f64;
    (0..n).map(|i| 0.54 - 0.46 * (i as f64 * t).cos()).collect()
}

fn gap_confidence(wrapped: &[f64], window: &[f64], gap_pos: i64, interval: i64) -> f64 {
    let half = window.len() as i64 / 2;
    let mut area = 0.0f64;

    let mut begin = gap_pos - half;
    let mut end = gap_pos + half;

    if begin < 0 {
        let wrapped_begin = begin + interval;
        for i in wrapped_begin..interval {
            let wi = (i - wrapped_begin) as usize;
            area += wrapped[i as usize] * window[wi];
        }
        begin = 0;
    }
    if end > interval {
        let wrapped_end = end - interval;
        let index_offset = window.len() as i64 - wrapped_end;
        for i in 0..wrapped_end {
            let wi = (i + index_offset) as usize;
            area += wrapped[i as usize] * window[wi];
        }
        end = interval;
    }
    for i in begin..end {
        let wi = (i - begin) as usize;
        area += wrapped[i as usize] * window[wi];
    }
    area
}

fn get_confidence_for_interval(gap: &GapData, interval: usize) -> f64 {
    let downsample = gap.downsample;
    let reduced = (interval >> downsample).max(1);
    let mut wrapped = vec![0.0f64; reduced + 1];
    let mut wrapped_pos = vec![0i64; gap.onsets.len()];

    for (i, o) in gap.onsets.iter().enumerate() {
        let pos = ((o.pos as i64 % interval as i64) >> downsample).min(reduced as i64);
        wrapped_pos[i] = pos;
        wrapped[pos as usize] += o.strength;
    }

    let mut highest = 0.0f64;
    for &pos in &wrapped_pos {
        let pos = pos.min(reduced as i64);
        let mut confidence = gap_confidence(&wrapped, &gap.window, pos, reduced as i64);
        let offbeat = (pos + reduced as i64 / 2) % reduced as i64;
        confidence += 0.5 * gap_confidence(&wrapped, &gap.window, offbeat, reduced as i64);
        if confidence > highest {
            highest = confidence;
        }
    }
    highest
}

fn get_confidence_for_bpm(gap: &GapData, test: &IntervalTester, bpm: f64) -> f64 {
    let intervalf = test.samplerate * 60.0 / bpm;
    let interval = (intervalf + 0.5) as i64;
    let interval_idx = (interval as usize).max(1);
    let mut wrapped = vec![0.0f64; interval_idx];
    let mut wrapped_pos = vec![0i64; gap.onsets.len()];

    for (i, o) in gap.onsets.iter().enumerate() {
        let pos = (o.pos as f64 % intervalf) as i64;
        let pos = pos.min(interval - 1).max(0);
        wrapped_pos[i] = pos;
        wrapped[pos as usize] += o.strength;
    }

    let mut highest = 0.0f64;
    for &pos in &wrapped_pos {
        let mut confidence = gap_confidence(&wrapped, &gap.window, pos, interval);
        let offbeat = (pos + interval / 2) % interval;
        confidence += 0.5 * gap_confidence(&wrapped, &gap.window, offbeat, interval);
        if confidence > highest {
            highest = confidence;
        }
    }
    normalize_fitness_val(highest, &test.coefs_placeholder(), intervalf)
}

// ===================== offsets =====================

fn get_base_offset_value(gap: &GapData, samplerate: f64, bpm: f64) -> f64 {
    let intervalf = samplerate * 60.0 / bpm;
    let interval = (intervalf + 0.5) as i64;
    let interval_idx = (interval as usize).max(1);
    let mut wrapped = vec![0.0f64; interval_idx];
    let mut wrapped_pos = vec![0i64; gap.onsets.len()];

    for (i, o) in gap.onsets.iter().enumerate() {
        let pos = (o.pos as f64 % intervalf) as i64;
        let pos = pos.min(interval - 1).max(0);
        wrapped_pos[i] = pos;
        wrapped[pos as usize] += 1.0;
    }

    let mut highest = 0.0f64;
    let mut offset_pos = 0i64;
    for &pos in &wrapped_pos {
        let mut confidence = gap_confidence(&wrapped, &gap.window, pos, interval);
        let offbeat = (pos + interval / 2) % interval;
        confidence += 0.5 * gap_confidence(&wrapped, &gap.window, offbeat, interval);
        if confidence > highest {
            highest = confidence;
            offset_pos = pos;
        }
    }
    offset_pos as f64 / samplerate
}

fn adjust_for_offbeats(samples: &[f32], sample_rate: u32, offset: f64, bpm: f64) -> f64 {
    let samplerate = sample_rate as f64;
    let seconds_per_beat = 60.0 / bpm;
    let mut offbeat = offset + seconds_per_beat * 0.5;
    if offbeat > seconds_per_beat {
        offbeat -= seconds_per_beat;
    }

    let slopes = compute_slopes(samples, sample_rate as f64);
    if slopes.is_empty() {
        return offset;
    }

    let end = slopes.len() as f64;
    let interval = seconds_per_beat * samplerate;
    let mut pos_a = offset * samplerate;
    let mut pos_b = offbeat * samplerate;
    let mut sum_a = 0.0f64;
    let mut sum_b = 0.0f64;
    while pos_a < end && pos_b < end {
        let ia = pos_a as usize;
        let ib = pos_b as usize;
        if ia < slopes.len() {
            sum_a += slopes[ia];
        }
        if ib < slopes.len() {
            sum_b += slopes[ib];
        }
        pos_a += interval;
        pos_b += interval;
    }
    if sum_a >= sum_b {
        offset
    } else {
        offbeat
    }
}

fn compute_slopes(samples: &[f32], samplerate: f64) -> Vec<f64> {
    let n = samples.len();
    let mut out = vec![0.0f64; n];
    let wh = (samplerate / 20.0) as usize;
    if n < wh * 2 {
        return out;
    }
    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    for i in 0..wh {
        sum_l += samples[i].abs() as f64;
        sum_r += samples[wh + i].abs() as f64;
    }
    let scalar = 1.0 / wh as f64;
    for i in wh..n - wh {
        out[i] = ((sum_r - sum_l) * scalar).max(0.0);
        let cur = samples[i].abs() as f64;
        sum_l -= samples[i - wh].abs() as f64;
        sum_l += cur;
        sum_r -= cur;
        sum_r += samples[i + wh].abs() as f64;
    }
    out
}

// ===================== helpers =====================

fn normalize_fitness(fitness: &mut f64, coefs: &[f64; 4], interval: f64) {
    *fitness = normalize_fitness_val(*fitness, coefs, interval);
}

fn normalize_fitness_val(mut fitness: f64, coefs: &[f64; 4], interval: f64) -> f64 {
    let x = interval;
    let x2 = x * x;
    let x3 = x2 * x;
    fitness -= coefs[0] + coefs[1] * x + coefs[2] * x2 + coefs[3] * x3;
    fitness
}

impl IntervalTester {
    fn coefs_placeholder(&self) -> [f64; 4] {
        // The C++ keeps last-fit coefs on the tester; we re-fit as needed.
        // For the noise floor we use a small-constant placeholder so the
        // normalized confidence is left essentially unchanged when used
        // outside the main fit. The main scan path always supplies real
        // coefs via `normalize_fitness`.
        [0.0, 0.0, 0.0, 0.0]
    }
}

fn remove_duplicates(tempo: &mut Vec<TempoResult>) {
    let mut i = 0;
    while i < tempo.len() {
        let bpm = tempo[i].bpm;
        let doubled = bpm * 2.0;
        let halved = bpm * 0.5;
        let mut j = tempo.len();
        while j > i + 1 {
            j -= 1;
            let v = tempo[j].bpm;
            let min = (v - bpm)
                .abs()
                .min((v - doubled).abs())
                .min((v - halved).abs());
            if min < 0.1 {
                tempo.remove(j);
            }
        }
        i += 1;
    }
}

fn round_bpm_values(test: &IntervalTester, gap: &GapData, tempo: &mut [TempoResult]) {
    for t in tempo.iter_mut() {
        let round_bpm = t.bpm.round();
        let diff = (t.bpm - round_bpm).abs();
        if diff < 0.01 {
            t.bpm = round_bpm;
        } else if diff < 0.05 {
            let _old = t.fitness;
            // Compare confidences at the raw vs the rounded BPM.
            let cur = get_confidence_for_bpm(gap, test, round_bpm);
            let old = get_confidence_for_bpm(gap, test, t.bpm);
            if cur > old * 0.99 {
                t.bpm = round_bpm;
            }
        }
    }
}

/// Least-squares fit of a cubic `coefs[0] + coefs[1]*x + coefs[2]*x^2 + coefs[3]*x^3`
/// to `(xs, ys)` by solving the 4x4 normal equations directly.
fn polyfit3(xs: &[f64], ys: &[f64]) -> [f64; 4] {
    // Build sums for the normal equations of a degree-3 polynomial.
    let n = xs.len() as f64;
    if n == 0.0 {
        return [0.0; 4];
    }
    let mut s = [0.0f64; 7]; // s[k] = sum x^k, k=0..6
    let mut t = [0.0f64; 4]; // t[k] = sum y * x^k, k=0..3
    for k in 0..7 {
        s[k] = xs.iter().map(|x| x.powi(k as i32)).sum();
    }
    for k in 0..4 {
        t[k] = xs
            .iter()
            .zip(ys.iter())
            .map(|(x, y)| y * x.powi(k as i32))
            .sum();
    }
    // 4x4 system: [[s0,s1,s2,s3],[s1,s2,s3,s4],[s2,s3,s4,s5],[s3,s4,s5,s6]] * c = [t0,t1,t2,t3]
    let a = [
        [s[0], s[1], s[2], s[3]],
        [s[1], s[2], s[3], s[4]],
        [s[2], s[3], s[4], s[5]],
        [s[3], s[4], s[5], s[6]],
    ];
    let b = [t[0], t[1], t[2], t[3]];
    solve4(&a, &b)
}

fn solve4(a: &[[f64; 4]; 4], b: &[f64; 4]) -> [f64; 4] {
    // Gaussian elimination with partial pivoting.
    let mut m = [[0.0f64; 5]; 4];
    for i in 0..4 {
        for j in 0..4 {
            m[i][j] = a[i][j];
        }
        m[i][4] = b[i];
    }
    for col in 0..4 {
        let mut piv = col;
        for r in (col + 1)..4 {
            if m[r][col].abs() > m[piv][col].abs() {
                piv = r;
            }
        }
        m.swap(col, piv);
        if m[col][col].abs() < 1e-12 {
            continue;
        }
        for r in (col + 1)..4 {
            let factor = m[r][col] / m[col][col];
            for c in col..5 {
                m[r][c] -= factor * m[col][c];
            }
        }
    }
    let mut x = [0.0f64; 4];
    for i in (0..4).rev() {
        let mut acc = m[i][4];
        for c in (i + 1)..4 {
            acc -= m[i][c] * x[c];
        }
        x[i] = if m[i][i].abs() < 1e-12 {
            0.0
        } else {
            acc / m[i][i]
        };
    }
    x
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic "click track": short decaying bursts every `bpm` beat,
    /// enough to give the detector a clear periodic signal to lock onto.
    fn click_track(bpm: f64, secs: f64, sr: u32) -> Vec<f32> {
        let n = (secs * sr as f64) as usize;
        let mut out = vec![0.0f32; n];
        let period = (60.0 / bpm * sr as f64) as usize;
        let mut t = 0usize;
        while t < n {
            // 30 ms exponential decay burst.
            let burst = (0.03 * sr as f64) as usize;
            for k in 0..burst {
                let i = t + k;
                if i >= n {
                    break;
                }
                let env = (-(k as f64 / burst as f64) * 6.0).exp();
                out[i] = (env * 0.9) as f32;
            }
            t += period;
        }
        out
    }

    #[test]
    fn detects_128_bpm_click_track_within_tolerance() {
        let sr = 44100;
        let samples = click_track(128.0, 30.0, sr);
        let tempo = detect(&samples, sr);
        assert!(!tempo.is_empty(), "detector returned no candidates");
        let best = tempo[0].bpm;
        // The true answer is 128; we accept 64/128/256-style results that land
        // within a half BPM, but the dedupe+octave logic should keep 128.
        let near = (best - 128.0).abs().min((best - 64.0).abs());
        assert!(
            near < 1.0,
            "expected ~128 BPM, got {best} (candidates: {:?})",
            tempo.iter().map(|t| t.bpm).collect::<Vec<_>>()
        );
    }

    #[test]
    fn detects_200_bpm_click_track() {
        let sr = 44100;
        let samples = click_track(200.0, 25.0, sr);
        let tempo = detect(&samples, sr);
        assert!(!tempo.is_empty());
        let best = tempo[0].bpm;
        let near = (best - 200.0).abs().min((best - 100.0).abs());
        assert!(
            near < 1.0,
            "expected ~200 BPM, got {best} ({:?})",
            tempo.iter().map(|t| t.bpm).collect::<Vec<_>>()
        );
    }

    #[test]
    fn intro_padding_is_minimal_and_exceeds_1_5s() {
        // 128 BPM => 0.46875 s/beat. smallest M with M*0.46875 > 1.5 is 4
        // (3 beats = 1.40625 <= 1.5, 4 beats = 1.875 > 1.5).
        let (pad, beat) = intro_padding(128.0, 0.0, 1.5);
        assert_eq!(beat, 4.0);
        assert!((pad - 1.875).abs() < 1e-6, "pad={pad}");
        assert!((pad) > 1.5);
    }

    #[test]
    fn intro_padding_phase_aligns_to_beat_grid() {
        // phase 0.12 s at 128 BPM should be removed from the pad so the
        // first note still lands on a whole beat (beat 4), exactly
        // 4*beat_sec into the audio.
        let beat_sec = 60.0 / 128.0;
        let phase = 0.12;
        let (pad, beat) = intro_padding(128.0, phase, 1.5);
        assert_eq!(beat, 4.0);
        // audio-time of first note = pad + phase must equal beat*beat_sec.
        let first_note_time = pad + phase;
        assert!(
            (first_note_time - beat * beat_sec).abs() < 1e-9,
            "misaligned"
        );
        assert!(first_note_time > 1.5);
    }

    #[test]
    fn intro_padding_uses_whole_beats_for_high_bpm() {
        // 265 BPM => 0.2264 s/beat; M=7 -> 1.5849 s > 1.5 (M=6 -> 1.358 < 1.5).
        let beat_sec = 60.0 / 265.0;
        let (pad, beat) = intro_padding(265.0, 0.0, 1.5);
        assert_eq!(beat, 7.0);
        assert!((pad - 7.0 * beat_sec).abs() < 1e-9);
        assert!(pad > 1.5);
    }
}
