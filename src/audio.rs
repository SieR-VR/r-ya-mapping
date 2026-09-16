// Audio decoding for analysis.
//
// Symphonia has no pure-Rust opus decoder, and an opus-in-ogg file is what
// `start` downloads, so we lean on `ffmpeg` (already required for yt-dlp's
// opus transcoding) to decode the opus file into mono 32-bit float PCM at a
// fixed 44100 Hz. That gives the flat mono sample stream the BPM detector
// expects, and matches Beat Saber's conventional `_songFrequency` of 44100.

#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

/// Mono float samples decoded from any audio file supported by ffmpeg.
pub struct Samples {
    pub data: Vec<f32>,
    pub sample_rate: u32,
}

impl Samples {
    /// Number of mono frames (i.e. one frame == one sample here).
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

/// Decode `path` to mono 32-bit float PCM at `sample_rate` Hz.
pub fn decode_mono(path: &Path, sample_rate: u32) -> Result<Samples> {
    require_ffmpeg()?;
    let mut child = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &path.to_string_lossy(),
            "-vn", // no video
            "-ac",
            "1", // mono
            "-ar",
            &sample_rate.to_string(),
            "-f",
            "f32le", // 32-bit float little-endian
            "-",     // to stdout
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn ffmpeg")?;

    let mut bytes = Vec::with_capacity(1 << 24);
    child
        .stdout
        .take()
        .context("ffmpeg stdout pipe missing")?
        .read_to_end(&mut bytes)
        .context("failed to read ffmpeg stdout")?;

    let mut stderr = String::new();
    child
        .stderr
        .take()
        .map(|mut s| s.read_to_string(&mut stderr).ok());

    let status = child.wait().context("failed to wait on ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg decode failed:\n{stderr}");
    }

    if bytes.len() % 4 != 0 {
        bail!("ffmpeg produced a non-multiple-of-4 float stream");
    }
    let mut data = vec![0.0f32; bytes.len() / 4];
    for (i, chunk) in bytes.chunks_exact(4).enumerate() {
        data[i] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
    }
    if data.is_empty() {
        bail!("ffmpeg produced no audio samples");
    }
    Ok(Samples { data, sample_rate })
}

/// Report the decoded `_songSampleCount` and `_songFrequency` for `BPMInfo.dat`.
/// Beats Saber stores the *original* sample count, so we trust ffprobe rather
/// than assuming 44100. On failure we fall back to the decoded length.
pub fn original_sample_info(path: &Path) -> Result<(u64, u32)> {
    let _ = require_ffprobe();
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,duration",
            "-of",
            "default=nw=1:nk=1",
            &path.to_string_lossy(),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let mut lines = s.lines();
            let sr: u32 = lines
                .next()
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(48000);
            let dur: f64 = lines
                .next()
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0.0);
            let count = (dur * sr as f64).round() as u64;
            Ok((count.max(1), sr))
        }
        _ => bail!("ffprobe unavailable; sample count will be approximate"),
    }
}

fn require_ffmpeg() -> Result<()> {
    if crate::youtube::has("ffmpeg") {
        return Ok(());
    }
    bail!("`ffmpeg` was not found on PATH; it is required to decode opus audio.");
}

fn require_ffprobe() -> Result<()> {
    if crate::youtube::has("ffprobe") {
        return Ok(());
    }
    bail!("`ffprobe` was not found on PATH.");
}

/// Finalize the map audio: convert the intermediate opus source into the
/// `.ogg` (Vorbis) file Beat Saber expects, baking in the leading silence
/// needed for the beat-phase-aligned intro. The intermediate `.opus` source
/// is removed on success. This replaces the deprecated `_songTimeOffset`
/// with a physically repositioned song.
///
/// Even when `pad_sec == 0` we still transcode opus → ogg so the map's
/// `_songFilename` is always a single, consistent format.
pub fn finalize_audio(opus_path: &Path, pad_sec: f64) -> Result<()> {
    require_ffmpeg()?;
    let pad_ms = if pad_sec > 0.0 {
        (pad_sec * 1000.0).round() as i64
    } else {
        0
    };

    let ogg_path = opus_path.with_file_name("song.ogg");
    let _ = std::fs::remove_file(&ogg_path);

    // Skip the filtergraph entirely when there's no padding to insert.
    let af = if pad_ms > 0 {
        format!("adelay=delays={pad_ms}:all=1")
    } else {
        "anull".to_string()
    };

    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &opus_path.to_string_lossy(),
            "-af",
            &af,
            // Beat Saber's standard audio container is Ogg Vorbis.
            "-c:a",
            "libvorbis",
            "-q:a",
            "6", // ~170-210 kbps VBR; high quality for this size.
            &ogg_path.to_string_lossy(),
        ])
        .status()
        .context("failed to spawn ffmpeg")?;
    if !status.success() {
        bail!(
            "ffmpeg failed to transcode {} -> {}",
            opus_path.display(),
            ogg_path.display()
        );
    }
    if !ogg_path.exists() {
        bail!(
            "expected ogg output at {} but it was not created",
            ogg_path.display()
        );
    }
    // Drop the intermediate opus source now that the final ogg exists.
    std::fs::remove_file(opus_path).ok();
    Ok(())
}

/// Probe an image's `(width, height)`. Returns `None` if ffprobe can't read it.
pub fn image_dimensions(path: &Path) -> Result<Option<(u32, u32)>> {
    require_ffprobe()?;
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "default=nw=1:nk=1",
            &path.to_string_lossy(),
        ])
        .output();
    match out {
        Ok(o) if o.status.success() && !o.stdout.is_empty() => {
            let s = String::from_utf8_lossy(&o.stdout);
            let mut lines = s.lines();
            let w: u32 = lines
                .next()
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0);
            let h: u32 = lines
                .next()
                .and_then(|l| l.trim().parse().ok())
                .unwrap_or(0);
            if w > 0 && h > 0 {
                Ok(Some((w, h)))
            } else {
                Ok(None)
            }
        }
        _ => Ok(None),
    }
}

/// Center-crop `src` to a square (the smaller of its dimensions) and write
/// the result to `dst` as JPEG. Used to produce a Beat Saber-ready square
/// cover from a YouTube Music thumbnail, whose album art is usually the
/// full video frame; cropping to 1:1 yields the canonical cover square.
pub fn center_crop_square(src: &Path, dst: &Path) -> Result<()> {
    require_ffmpeg()?;
    let _ = std::fs::remove_file(dst);
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-i",
            &src.to_string_lossy(),
            // crop=w=h=min(iw,ih), centered. Commas are the ffmpeg filter
            // separator; the colons inside are nested args.
            "-vf",
            "crop=min(iw\\,ih):min(iw\\,ih):(iw-min(iw\\,ih))/2:(ih-min(iw\\,ih))/2",
            "-q:v",
            "2", // high-quality jpeg
            &dst.to_string_lossy(),
        ])
        .status()
        .context("failed to spawn ffmpeg")?;
    if !status.success() {
        bail!(
            "ffmpeg failed to center-crop {} -> {}",
            src.display(),
            dst.display()
        );
    }
    if !dst.exists() {
        bail!(
            "expected cropped cover at {} but it was not created",
            dst.display()
        );
    }
    Ok(())
}
