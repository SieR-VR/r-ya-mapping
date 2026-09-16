// Downloading and metadata extraction for a YouTube URI via `yt-dlp`.
//
// The song is always fetched as **opus** for sound quality. We let yt-dlp
// pick the best audio and, if needed, transcode to opus with ffmpeg, writing
// into a fresh output path. yt-dlp is also asked (via `--dump-json`) for the
// track metadata we need to fill in the map's `_songName`/`_songAuthorName`:
// the human-easy fields a mapper can adjust by hand later.

#![allow(dead_code)]

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Metadata pulled from yt-dlp's JSON that's useful for scaffolding a map.
#[derive(Debug, Clone, Default)]
pub struct YoutubeInfo {
    /// Best-effort song title.
    pub title: String,
    /// Best-effort artist/uploader.
    pub uploader: String,
    /// Track title if the uploader/upload provides one.
    pub track: Option<String>,
    /// Track artist if provided.
    pub artist: Option<String>,
    /// Video id (used to derive a stable folder name when title is empty).
    pub id: String,
    /// Duration in seconds, if known.
    pub duration: Option<f64>,
}

#[derive(Deserialize)]
struct DumpJson {
    title: Option<String>,
    uploader: Option<String>,
    channel: Option<String>,
    track: Option<String>,
    artist: Option<String>,
    id: Option<String>,
    duration: Option<f64>,
}

/// Whether the given URI points at YouTube Music (rather than vanilla
/// YouTube). YouTube Music pages present a different "now-playing" cover -
/// typically the square album art - and we use that cue to fetch a
/// square-cropped cover instead of the 16:9 video thumbnail.
pub fn is_youtube_music(uri: &str) -> bool {
    uri.contains("music.youtube.com") || uri.contains("music.aa.youtube.com")
}

/// Extract the YouTube video id from any common URI form.
pub fn extract_id(uri: &str) -> Result<String> {
    // watch?v=ID, youtu.be/ID, /shorts/ID, /embed/ID, or a bare id.
    if let Some((_, q)) = uri.split_once("v=") {
        let id = q.split('&').next().unwrap_or("");
        if !id.is_empty() {
            return Ok(id.to_string());
        }
    }
    for sep in ["youtu.be/", "/shorts/", "/embed/", "/live/"] {
        if let Some((_, rest)) = uri.split_once(sep) {
            let id = rest.split(&['?', '&', '/'][..]).next().unwrap_or("");
            if !id.is_empty() {
                return Ok(id.to_string());
            }
        }
    }
    // Maybe it's a bare 11-char id already.
    let trimmed = uri.trim();
    if trimmed.len() == 11
        && trimmed
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
    {
        return Ok(trimmed.to_string());
    }
    bail!("could not find a video id in `{uri}`");
}

/// Fetch metadata for a URI without downloading.
pub fn fetch_info(uri: &str) -> Result<YoutubeInfo> {
    require_yt_dlp()?;
    let out = Command::new("yt-dlp")
        .args(["--no-warnings", "--no-playlist", "--dump-json", uri])
        .output()
        .context("failed to spawn yt-dlp")?;
    if !out.status.success() {
        bail!(
            "yt-dlp metadata failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: Value =
        serde_json::from_slice(&out.stdout).context("yt-dlp returned non-JSON metadata")?;
    // Prefer explicit track fields; fall back to title/uploader.
    let title = json
        .get("track")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| json.get("title").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let uploader = json
        .get("artist")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| json.get("uploader").and_then(|v| v.as_str()))
        .or_else(|| json.get("channel").and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let duration = json.get("duration").and_then(|v| v.as_f64());
    let _d: DumpJson = serde_json::from_value(json).unwrap_or(DumpJson {
        title: None,
        uploader: None,
        channel: None,
        track: None,
        artist: None,
        id: None,
        duration: None,
    });
    Ok(YoutubeInfo {
        title,
        uploader,
        track: None,
        artist: None,
        id,
        duration,
    })
}

/// Download the audio as opus into `out_dir` as `song.opus`. This is an
/// *intermediate* file: it's the best-quality source for analysis and for
/// later transcoding to the final `.ogg` map audio (the format Beat Saber
/// expects).
pub fn download_opus(uri: &str, out_dir: &Path) -> Result<PathBuf> {
    require_yt_dlp()?;
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("failed to create {}", out_dir.display()))?;
    let out_template = out_dir.join("song.opus");

    // Remove a stale file so yt-dlp's `--no-overwrites` doesn't silently skip.
    let _ = std::fs::remove_file(&out_template);

    let status = Command::new("yt-dlp")
        .args([
            "--no-warnings",
            "--no-playlist",
            "-x", // extract audio
            "--audio-format",
            "opus", // always opus, for best source quality
            "--audio-quality",
            "0",  // best
            "-o", // output template
            &out_template.to_string_lossy(),
            "--no-part",
            uri,
        ])
        .status()
        .context("failed to spawn yt-dlp")?;
    if !status.success() {
        bail!("yt-dlp failed to download `{uri}`");
    }
    if !out_template.exists() {
        bail!(
            "expected opus output at {} but it was not created",
            out_template.display()
        );
    }
    Ok(out_template)
}

/// Download the video's best still thumbnail into `out_path` (a `.jpg`).
/// YouTube's standard thumbnails are 16:9 (maxresdefault) or 4:3 (hqdefault),
/// so this is often *not* square - callers must check before using it as
/// the Beat Saber cover.
pub fn download_thumbnail(uri: &str, out_path: &Path) -> Result<PathBuf> {
    require_yt_dlp()?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let _ = std::fs::remove_file(out_path);

    // yt-dlp appends the source image's real extension to the output template
    // (e.g. `cover.jpg` -> `cover.jpg.webp`), so don't rely on a fixed one.
    // Use a unique "thumb.<ext>" sentinel and sweep it up afterwards.
    let stem_prefix = out_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cover");
    let tmp_template = out_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem_prefix}.%(ext)s"));

    let status = Command::new("yt-dlp")
        .args([
            "--no-warnings",
            "--no-playlist",
            "--write-thumbnail",
            "--convert-thumbnails",
            "jpg",
            "--skip-download",
            "-o",
            &tmp_template.to_string_lossy(),
            "--no-part",
            uri,
        ])
        .status()
        .context("failed to spawn yt-dlp")?;
    if !status.success() {
        bail!("yt-dlp failed to download the thumbnail for `{uri}`");
    }

    // Find whatever yt-dlp actually wrote under our prefix and rename to out_path.
    let parent = tmp_template.parent().unwrap_or_else(|| Path::new("."));
    let probe = format!("{stem_prefix}.");
    let mut picked: Option<PathBuf> = None;
    if let Ok(entries) = std::fs::read_dir(parent) {
        for e in entries.flatten() {
            let p = e.path();
            if p == out_path {
                continue;
            }
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.starts_with(&probe) {
                    picked = Some(p);
                    break;
                }
            }
        }
    }
    // yt-dlp's --convert-thumbnails jpg should have produced "<prefix>.jpg";
    // prefer that exact name, else the swept-up one.
    let jpg_candidate = parent.join(format!("{stem_prefix}.jpg"));
    let want = if jpg_candidate.exists() {
        jpg_candidate
    } else {
        picked.ok_or_else(|| anyhow::anyhow!("thumbnail was not written by yt-dlp"))?
    };
    std::fs::rename(&want, out_path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            want.display(),
            out_path.display()
        )
    })?;
    Ok(out_path.to_path_buf())
}

/// Whether a given executable is on PATH (shared with other modules).
pub fn has(bin: &str) -> bool {
    which(bin).is_ok()
}

fn require_yt_dlp() -> Result<()> {
    if has("yt-dlp") {
        return Ok(());
    }
    bail!("`yt-dlp` was not found on PATH. Install it to download songs.");
}

/// Cheap `which` without pulling another dependency.
fn which(bin: &str) -> Result<PathBuf, ()> {
    let ext = if cfg!(windows) { ".exe" } else { "" };
    for dir in std::env::split_paths(&std::env::var_os("PATH").ok_or(())?) {
        let candidate = dir.join(format!("{bin}{ext}"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(())
}
