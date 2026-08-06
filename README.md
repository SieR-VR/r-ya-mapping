# r-ya-mapping

Beat Saber map scaffolder. You give it a YouTube link, it gives you a
CustomWIPLevels folder ready to open in ChroMapper.

It downloads the song, detects the BPM, pads the audio so the first beat
lands where you want it, grabs the thumbnail, and writes `Info.dat`,
`BPMInfo.dat`, and an empty difficulty file. Then you map.

The BPM detector is a port of ArrowVortex's `FindTempo` algorithm
(`src/bpm.rs`). ArrowVortex is GPLv3, so this is too.

## Requirements

External tools on `PATH`:

- `yt-dlp` - download + metadata
- `ffmpeg` - decode, transcode, pad
- `ffprobe` - sample count, image dimensions

A working Rust toolchain to build.

On Windows, `yt-dlp`, `ffmpeg`, and `ffprobe` (ffmpeg ships with ffprobe) are
all available from `winget`, so the easiest setup is a one-liner:

    winget install yt-dlp.yt-dlp Gyan.FFmpeg

## Install

    cargo install --path .

Or, if you want to run it from source without installing:

    cargo build --release
    target/release/r-ya-mapping --help

## Setup

Tell it who you are and where Beat Saber keeps WIP maps:

    r-ya-mapping config mapper.name "Your Name"
    r-ya-mapping config customwiplevels.path "C:/Steam/steamapps/common/Beat Saber/CustomWIPLevels"

That's the minimum. `config --list` shows the rest:

    map.characteristic              Standard
    map.difficulty                  ExpertPlus
    map.difficulty_rank             9
    map.njs                         18
    map.note_jump_start_beat_offset -1.902
    map.environment                 DefaultEnvironment
    map.song_filename               song.ogg

Set any of them the same way: `r-ya-mapping config map.njs 20`.

## Usage

    r-ya-mapping <youtube-uri>

That's it. The `start` subcommand is the default; the bare form above is
the same as:

    r-ya-mapping start <youtube-uri>

What it does, in order:

1. fetches metadata (title, uploader)
2. downloads the song (opus source, best quality)
3. probes it (sample count, frequency)
4. decodes to mono PCM (44.1 kHz, for analysis only)
5. detects BPM (ArrowVortex algorithm; top-3 candidates)
6. pads the audio (minimum silence so beat 0 is on a whole beat
   and the intro is > 1.5 s; `_songTimeOffset`
   is deprecated, so the audio is physically
   repositioned instead)
7. finalizes as song.ogg (opus -> Vorbis, padding baked in)
8. grabs the thumbnail (cover.jpg; warns if it's not square)
9. writes Info.dat, BPMInfo.dat, and an empty beatmap

Overrides, if the auto-detected values are wrong:

    r-ya-mapping start <uri> --bpm 220
    r-ya-mapping start <uri> --song-name "Red Line" --song-author "Camellia"
    r-ya-mapping start <uri> --map-name "My Custom Folder Name"

BPM detection is the hard part. The algorithm keeps three candidates
because of the octave ambiguity (a 265 BPM song looks like 132.5 to a
periodicity detector). If it picks the half or double, pass `--bpm`.
Song name and author come from YouTube metadata and are often messy;
fix them with the flags above or by editing `Info.dat` by hand.

## Config

Like `git config`. Stored under `$HOME` (or `%APPDATA%` on Windows) at
`r-ya-mapping/config.toml`.

    r-ya-mapping config mapper.name "Your Name"     # set
    r-ya-mapping config mapper.name                  # read
    r-ya-mapping config --list                       # list all
    r-ya-mapping config --unset map.njs             # unset

## Tests

    cargo test

Includes synthetic click-track tests for the BPM detector and unit
tests for the intro-padding math.

## License

GPL-3.0-or-later, same as ArrowVortex. See `Cargo.toml`.
