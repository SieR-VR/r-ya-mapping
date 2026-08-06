// Beatmap folder scaffolding helpers.
//
// Writes out an empty, but valid, Beat Saber v2.1.0 CustomWIPLevels folder
// (`Info.dat`, `BPMInfo.dat`, and the difficulty beatmap file named in
// `Info.dat`). Style matches the reference map: pretty JSON with the familiar
// 2-space indentation and CRLF line endings, matching the editor's output.

use crate::model::{
    empty_beatmap_json, BpmInfoDat, BpmRegion, DifficultyBeatmap, DifficultyBeatmapSet, InfoDat,
};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Parameters for the generated map.
pub struct MapParams {
    pub song_name: String,
    pub song_sub_name: String,
    pub song_author: String,
    pub level_author: String,
    pub bpm: f64,
    pub song_filename: String,
    pub cover_filename: Option<String>,
    pub environment: String,
    pub characteristic: String,
    pub difficulty: String,
    pub difficulty_rank: i64,
    pub njs: f64,
    pub note_jump_start_beat_offset: f64,
    pub song_sample_count: u64,
    pub song_frequency: u32,
    /// Beat at which the first note should be placed (after the leading
    /// silence that was prepended to the song file). Stored in `_customData`
    /// for the mapper's convenience.
    pub first_note_beat: i64,
}

/// Write a map into `dir`, creating it. Returns the difficulty file name.
pub fn write_map(dir: &Path, p: &MapParams) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;

    let beatmap_file = format!("{difficulty}Standard.dat", difficulty = p.difficulty);
    let cover = p.cover_filename.clone().unwrap_or_default();

    let info = InfoDat {
        _version: "2.1.0".into(),
        _songName: p.song_name.clone(),
        _songSubName: p.song_sub_name.clone(),
        _songAuthorName: p.song_author.clone(),
        _levelAuthorName: p.level_author.clone(),
        _beatsPerMinute: p.bpm,
        _songTimeOffset: 0,
        _shuffle: 0.0,
        _shufflePeriod: 0.0,
        _previewStartTime: (p.bpm / 2.0).round() * 0.5,
        _previewDuration: 28.0,
        _songFilename: p.song_filename.clone(),
        _coverImageFilename: cover,
        _environmentName: p.environment.clone(),
        _allDirectionsEnvironmentName: "GlassDesertEnvironment".into(),
        _environmentNames: vec![p.environment.clone()],
        _colorSchemes: Vec::new(),
        _difficultyBeatmapSets: vec![DifficultyBeatmapSet {
            _beatmapCharacteristicName: p.characteristic.clone(),
            _difficultyBeatmaps: vec![DifficultyBeatmap {
                _difficulty: p.difficulty.clone(),
                _difficultyRank: p.difficulty_rank,
                _beatmapFilename: beatmap_file.clone(),
                _noteJumpMovementSpeed: p.njs,
                _noteJumpStartBeatOffset: p.note_jump_start_beat_offset,
                _beatmapColorSchemeIdx: 0,
                _environmentNameIdx: 0,
            }],
        }],
        _customData: serde_json::json!({
            "_editors": {
                "_lastEditedBy": "r-ya-mapping",
            },
            "_firstNoteBeat": p.first_note_beat,
            "_introPadSeconds": p.first_note_beat as f64 * 60.0 / p.bpm,
        }),
    };

    let beats_per_sec = p.bpm / 60.0;
    let end_beat = beats_per_sec * p.song_sample_count as f64 / p.song_frequency as f64;
    let bpm_info = BpmInfoDat {
        _version: "2.0.0".into(),
        _songSampleCount: p.song_sample_count,
        _songFrequency: p.song_frequency,
        _regions: vec![BpmRegion {
            _startSampleIndex: 0,
            _endSampleIndex: p.song_sample_count,
            _startBeat: 0.0,
            _endBeat: end_beat,
        }],
    };

    write_pretty(dir.join("Info.dat"), &info)?;
    write_pretty(dir.join("BPMInfo.dat"), &bpm_info)?;
    write_pretty(dir.join(&beatmap_file), &empty_beatmap_json())?;

    Ok(dir.join(beatmap_file))
}

/// Pretty-print JSON with the same conventions the editor uses:
/// 2-space indent and CRLF line endings (so diffs against the reference map
/// stay minimal).
fn write_pretty<T: serde::Serialize>(path: PathBuf, value: &T) -> Result<()> {
    let pretty = serde_json::to_string_pretty(value)
        .with_context(|| format!("failed to serialize {}", path.display()))?;
    let crlf = pretty.replace('\n', "\r\n");
    std::fs::write(&path, crlf).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}
