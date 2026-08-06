// The `start` subcommand: glue everything together.
//
// Flow:
//   1. Load config from $HOME for mapper name + CustomWIPLevels path + defaults.
//   2. Fetch YouTube metadata (title/uploader) for the song-header fields.
//   3. Download the song as opus (`song.opus`) via yt-dlp into the map folder.
//   4. Probe the opus file with ffprobe for `_songSampleCount`/`_songFrequency`.
//   5. Decode mono 32-bit float PCM at 44.1 kHz with ffmpeg.
//   6. Run the ArrowVortex-ported BPM detector → top candidates.
//   7. Compute minimum leading silence so the first note lands on a whole beat
//      strictly more than 1.5 s in (phase-aligned; `_songTimeOffset` is
//      deprecated), then transcode opus → `song.ogg` (Vorbis) with the silence
//      baked in and re-probe the padded ogg for the final sample count.
//   8. Download the YouTube thumbnail into `cover.jpg`; warn the user if it's
//      not square (Beat Saber prefers a square cover).
//   9. Write out `Info.dat`, `BPMInfo.dat` and an empty difficulty beatmap.

use crate::beatmap::{write_map, MapParams};
use crate::bpm;
use crate::config::MappingConfig;
use crate::{audio, youtube};
use anyhow::{bail, Context, Result};
use std::path::PathBuf;
use std::time::Instant;

pub struct StartOpts {
    pub uri: String,
    pub bpm: Option<f64>,
    pub song_name: Option<String>,
    pub song_sub_name: Option<String>,
    pub song_author: Option<String>,
    pub map_name: Option<String>,
}

pub fn run_start(opts: StartOpts) -> Result<()> {
    let cfg = MappingConfig::load()?;
    let wip_levels = cfg.require_wip_levels()?;
    let level_author = cfg.require_mapper_name()?;

    // --- 1. metadata -----------------------------------------------------
    println!("→ fetching metadata for {}", opts.uri);
    let info = youtube::fetch_info(&opts.uri).context("failed to fetch YouTube metadata")?;

    let song_name = opts
        .song_name
        .or(info.track.clone())
        .unwrap_or_else(|| info.title.clone());
    let song_author = opts
        .song_author
        .or(info.artist.clone())
        .unwrap_or_else(|| info.uploader.clone());
    let song_sub_name = opts.song_sub_name.unwrap_or_default();

    let folder_name = opts
        .map_name
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| (!song_name.is_empty()).then(|| song_name.clone()))
        .unwrap_or_else(|| format!("yt_{}", info.id));
    let map_dir = sanitize_path(wip_levels.join(&folder_name));

    // Make sure it doesn't already exist (don't clobber anything).
    if map_dir.exists() {
        bail!(
            "map folder already exists: {}\n\
             Move or delete it first, or pass --map-name <name>.",
            map_dir.display()
        );
    }

    // --- 2. download as opus --------------------------------------------
    println!("→ downloading audio (opus) → {}", map_dir.display());
    let opus_path =
        youtube::download_opus(&opts.uri, &map_dir).context("failed to download song")?;

    // --- 3. probe original sample info for BPMInfo.dat -------------------
    let (sample_count_orig, freq_orig) =
        audio::original_sample_info(&opus_path).unwrap_or((0, 48000));

    // --- 4. decode mono PCM at 44.1 kHz for analysis ---------------------
    const ANALYSIS_RATE: u32 = 44100;
    println!("→ decoding for analysis @ {ANALYSIS_RATE} Hz");
    let pcm = audio::decode_mono(&opus_path, ANALYSIS_RATE).context("failed to decode audio")?;

    // --- 5. detect BPM ---------------------------------------------------
    println!("→ detecting BPM (onsets: rolling FFT)…");
    let t0 = Instant::now();
    let tempo = bpm::detect(&pcm.data, ANALYSIS_RATE);
    let chosen = match opts.bpm {
        Some(b) if b > 0.0 => b,
        _ => match tempo.first() {
            Some(t) => t.bpm,
            None => bail!("could not detect BPM; pass --bpm <value>"),
        },
    };
    let candidates: Vec<String> = tempo.iter().map(|t| format!("{:.2}", t.bpm)).collect();
    let extra = if candidates.is_empty() {
        String::new()
    } else {
        format!("; top candidates: {}", candidates.join(", "))
    };
    println!(
        "→ detected BPM = {:.2} (took {:.2?}){}",
        chosen,
        t0.elapsed(),
        extra
    );

    // --- 6. leading-silence intro ---------------------------------------
    // `_songTimeOffset` is deprecated, so the beat phase is fixed by
    // physically prepending silence to the song. The amount is the minimum
    // that (a) realigns music beat 0 to a whole map beat and (b) makes the
    // intro strictly longer than 1.5 s, kept small so intros aren't bloated.
    let offset_sec = tempo
        .iter()
        .find(|t| (t.bpm - chosen).abs() < 0.5)
        .map(|t| t.offset)
        .unwrap_or(0.0);
    const MIN_INTRO: f64 = 1.5;
    let (pad_sec, first_note_beat) = bpm::intro_padding(chosen, offset_sec, MIN_INTRO);
    let beat_sec = 60.0 / chosen;
    println!(
        "→ beat-0 phase = {:.4} s; prepending {pad_sec:.3} s of silence \
         (first note @ beat {first_note_beat} = {:.3} s into audio)",
        offset_sec,
        first_note_beat as f64 * beat_sec,
    );
    println!("→ finalizing audio as song.ogg (opus → vorbis, baked-in intro)");
    audio::finalize_audio(&opus_path, pad_sec).context("failed to finalize song.ogg")?;
    let ogg_path = map_dir.join("song.ogg");

    // Re-probe the (now padded, ogg) file so `_songSampleCount` reflects the
    // real decoded length including the prepended silence.
    let (song_sample_count, song_frequency) = audio::original_sample_info(&ogg_path).unwrap_or((
        (sample_count_orig as f64 + pad_sec * freq_orig as f64).round() as u64,
        freq_orig,
    ));

    // --- thumbnail ----------------------------------------------------
    // Beat Saber wants a square cover. Try yt-dlp's best thumbnail; if it's
    // not 1:1 we keep it (so the map is still valid) but warn the mapper so
    // they can crop it by hand before publishing.
    let cover_path = map_dir.join("cover.jpg");
    let mut cover_filename: Option<String> = None;
    match youtube::download_thumbnail(&opts.uri, &cover_path) {
        Ok(p) => match audio::image_dimensions(&p).unwrap_or(None) {
            Some((w, h)) => {
                if w == h {
                    println!("→ cover: {w}x{h} (square, good to go)");
                } else {
                    eprintln!(
                        "⚠ cover thumbnail is {w}x{h} (NOT square). Beat Saber\n\
                         ⚠  prefers a square cover. The map is still written, but\n\
                         ⚠  please crop `cover.jpg` to 1:1 before publishing."
                    );
                }
                cover_filename = Some("cover.jpg".to_string());
            }
            None => println!("→ cover downloaded (dimensions unreadable, kept as-is)"),
        },
        Err(e) => eprintln!("⚠ could not download thumbnail: {e}"),
    }

    // --- 7. write the map -----------------------------------------------
    let characteristic = cfg
        .characteristic
        .clone()
        .unwrap_or_else(|| "Standard".into());
    let difficulty = cfg
        .difficulty
        .clone()
        .unwrap_or_else(|| "ExpertPlus".into());
    let difficulty_rank = cfg.difficulty_rank.unwrap_or(9);
    let njs = cfg.njs.unwrap_or(18.0);
    let njbo = cfg.note_jump_start_beat_offset.unwrap_or(-1.902);
    let environment = cfg
        .environment
        .clone()
        .unwrap_or_else(|| "DefaultEnvironment".into());
    let song_filename = cfg
        .song_filename
        .clone()
        .unwrap_or_else(|| "song.ogg".into());

    let map = MapParams {
        song_name,
        song_sub_name,
        song_author,
        level_author,
        bpm: chosen,
        song_filename,
        cover_filename,
        environment,
        characteristic,
        difficulty,
        difficulty_rank,
        njs,
        note_jump_start_beat_offset: njbo,
        song_sample_count: song_sample_count.max(1),
        song_frequency: song_frequency,
        first_note_beat: first_note_beat as i64,
    };

    let beatmap_path = write_map(&map_dir, &map)?;
    println!("→ wrote {}", map_dir.join("Info.dat").display());
    println!("→ wrote {}", map_dir.join("BPMInfo.dat").display());
    println!("→ wrote {} (empty beatmap)", beatmap_path.display());
    println!("✓ map scaffolded: {}", map_dir.display());
    Ok(())
}

fn sanitize_path(p: PathBuf) -> PathBuf {
    // Strip the few filesystem-illegal Windows chars from each component.
    let mut out = PathBuf::new();
    for comp in p.components() {
        use std::path::Component;
        match comp {
            Component::Normal(s) => {
                let cleaned = s
                    .to_string_lossy()
                    .replace(['<', '>', ':', '"', '/', '\\', '|', '?', '*'], "_");
                out.push(cleaned);
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}
