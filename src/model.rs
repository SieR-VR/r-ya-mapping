// Beat Saber map data model.
//
// The generated map follows the **v3** schema per the BSMG wiki
// (https://bsmg.wiki/mapping/map-format.html):
//   * `Info.dat`     - v2-style layout (`_version: "2.1.0"`); v3 does not
//                      change the info file.
//   * Beatmap files  - v3 (`"version": "3.3.0"`): abbreviated, non-underscored
//                      fields (`b`, `x`, `y`, `c`, `d`, ...).
//   * `BPMInfo.dat`  - legacy `"2.0.0"` audio data (the 4.0.0 `AudioData.dat`
//                      layout is exclusive to the v4 schema).
//
// Field names mirror Beat Saber's JSON conventions (mixed underscore-prefixed
// and single-letter keys), so the snake_case lint is silenced for this module.

#![allow(non_snake_case)]

use serde::{Deserialize, Serialize};

/// v2.1.0-style `Info.dat` (unchanged by the v3 schema).
#[derive(Serialize, Deserialize)]
pub struct InfoDat {
    pub _version: String,
    pub _songName: String,
    pub _songSubName: String,
    pub _songAuthorName: String,
    pub _levelAuthorName: String,
    pub _beatsPerMinute: f64,
    pub _songTimeOffset: i64,
    pub _shuffle: f64,
    pub _shufflePeriod: f64,
    pub _previewStartTime: f64,
    pub _previewDuration: f64,
    pub _songFilename: String,
    pub _coverImageFilename: String,
    pub _environmentName: String,
    pub _allDirectionsEnvironmentName: String,
    pub _environmentNames: Vec<String>,
    pub _colorSchemes: Vec<serde_json::Value>,
    pub _difficultyBeatmapSets: Vec<DifficultyBeatmapSet>,
    pub _customData: serde_json::Value,
}

#[derive(Serialize, Deserialize)]
pub struct DifficultyBeatmapSet {
    pub _beatmapCharacteristicName: String,
    pub _difficultyBeatmaps: Vec<DifficultyBeatmap>,
}

#[derive(Serialize, Deserialize)]
pub struct DifficultyBeatmap {
    pub _difficulty: String,
    pub _difficultyRank: i64,
    pub _beatmapFilename: String,
    pub _noteJumpMovementSpeed: f64,
    pub _noteJumpStartBeatOffset: f64,
    pub _beatmapColorSchemeIdx: i64,
    pub _environmentNameIdx: i64,
}

/// Legacy 2.0.0 `BPMInfo.dat` (the audio-data format paired with v3 maps).
#[derive(Serialize, Deserialize)]
pub struct BpmInfoDat {
    pub _version: String,
    pub _songSampleCount: u64,
    pub _songFrequency: u32,
    pub _regions: Vec<BpmRegion>,
}

#[derive(Serialize, Deserialize)]
pub struct BpmRegion {
    pub _startSampleIndex: u64,
    pub _endSampleIndex: u64,
    pub _startBeat: f64,
    pub _endBeat: f64,
}

/// An empty (but valid) v3 difficulty beatmap file body.
///
/// `colorNotes`, `bombNotes`, and `obstacles` must be explicitly defined even
/// when empty, or the song-select screen cannot display NPS/note counts
/// (per the BSMG wiki "Defaulted Properties" warning). The remaining
/// collections are spelled out for editor friendliness.
pub fn empty_beatmap_json() -> serde_json::Value {
    serde_json::json!({
        "version": "3.3.0",
        "bpmEvents": [],
        "rotationEvents": [],
        "colorNotes": [],
        "bombNotes": [],
        "obstacles": [],
        "sliders": [],
        "burstSliders": [],
        "basicBeatmapEvents": [],
        "colorBoostBeatmapEvents": [],
        "waypoints": [],
        "basicEventTypesWithKeywords": {
            "d": []
        },
        "useNormalEventsAsCompatibleEvents": false
    })
}
