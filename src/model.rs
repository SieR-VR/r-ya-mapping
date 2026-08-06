// Beat Saber map data model: `Info.dat` and `BPMInfo.dat` structured exactly
// the way the reference map (`refs/Beni Kanzashi`) lay them out, so the
// generated map plays nice in ChroMapper / MMA2 / Eek! without surprises.
//
// Field names mirror Beat Saber's underscore-prefixed JSON conventions, so we
// silence the snake_case lint for this module.

#![allow(non_snake_case)]

use serde::{Deserialize, Serialize};

/// v2.1.0 `Info.dat`.
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

/// v2.0.0 `BPMInfo.dat`.
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

/// An empty (but valid) v2.x difficulty beatmap file body.
pub fn empty_beatmap_json() -> serde_json::Value {
    serde_json::json!({
        "_version": "2.1.0",
        "_BPMChanges": [],
        "_events": [],
        "_notes": [],
        "_waypoints": [],
        "_customData": {
            "_time": 0.0,
            "_BPMChanges": []
        }
    })
}
