// Global configuration, accessed like `git config`.
//
// A single TOML file lives under the user's home directory
// (`~/.config/r-ya-mapping/config.toml` on Linux/macOS, the equivalent
// `%APPDATA%`/`%USERPROFILE%` location on Windows via the `dirs` crate).
//
// Usage mirrors git config:
//   r-ya-mapping config mapper.name "Sora"
//   r-ya-mapping config mapper.name           # read
//   r-ya-mapping config --list
//   r-ya-mapping config --unset mapper.name
//
// The known keys surfaced to the user map directly onto the fields a Beat
// Saber map scaffold needs but that are tedious to retype every run.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// The whole config file: an ordered map of `section.key` -> value.
type Doc = BTreeMap<String, String>;

/// Actions accepted by the `config` subcommand.
pub struct ConfigAction {
    pub key: Option<String>,
    pub value: Option<String>,
    pub list: bool,
    pub unset: bool,
}

/// Strongly-typed view of the config expanded out into the mapping options.
/// This is what `start` consumes; it is materialized from the flat `Doc`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MappingConfig {
    /// `_levelAuthorName` - the mapper's credited name.
    pub mapper_name: Option<String>,
    /// Absolute path to the Beat Saber `CustomWIPLevels` folder where maps go.
    pub custom_wip_levels: Option<String>,
    /// Default characteristic (e.g. "Standard").
    pub characteristic: Option<String>,
    /// Default difficulty (e.g. "ExpertPlus").
    pub difficulty: Option<String>,
    /// Default difficulty rank (ExpertPlus=9, Expert=7, Hard=5, Normal=3, Easy=1).
    pub difficulty_rank: Option<i64>,
    /// Default Note Jump Movement Speed.
    pub njs: Option<f64>,
    /// Default Note Jump Start Beat Offset.
    pub note_jump_start_beat_offset: Option<f64>,
    /// Default environment name (e.g. "DefaultEnvironment").
    pub environment: Option<String>,
    /// Default `_songFilename` for the downloaded audio.
    pub song_filename: Option<String>,
}

impl MappingConfig {
    /// Materialize a typed config from the flat TOML document on disk,
    /// falling back to a default if no file exists yet.
    pub fn load() -> Result<Self> {
        let doc = read_doc()?;
        let get = |k: &str| doc.get(k).cloned();
        Ok(MappingConfig {
            mapper_name: get("mapper.name"),
            custom_wip_levels: get("customwiplevels.path"),
            characteristic: get("map.characteristic"),
            difficulty: get("map.difficulty"),
            difficulty_rank: doc.get("map.difficulty_rank").and_then(|s| s.parse().ok()),
            njs: doc.get("map.njs").and_then(|s| s.parse().ok()),
            note_jump_start_beat_offset: doc
                .get("map.note_jump_start_beat_offset")
                .and_then(|s| s.parse().ok()),
            environment: get("map.environment"),
            song_filename: get("map.song_filename"),
        })
    }

    /// Path to the configured CustomWIPLevels folder, erroring if unset.
    pub fn require_wip_levels(&self) -> Result<PathBuf> {
        match &self.custom_wip_levels {
            Some(p) => Ok(PathBuf::from(p)),
            None => bail!(
                "config key `customwiplevels.path` is not set.\n\
                 Set it with:\n  r-ya-mapping config customwiplevels.path \
                 \"C:/.../Beat Saber/CustomWIPLevels\""
            ),
        }
    }

    /// Mapper name, erroring if unset.
    pub fn require_mapper_name(&self) -> Result<String> {
        match &self.mapper_name {
            Some(n) => Ok(n.clone()),
            None => bail!(
                "config key `mapper.name` is not set.\n\
                 Set it with:\n  r-ya-mapping config mapper.name \"Your Name\""
            ),
        }
    }
}

/// Discussion table of every known key, its description and default.
pub const KNOWN_KEYS: &[(&str, &str)] = &[
    ("mapper.name", "Your name, written to `_levelAuthorName`."),
    (
        "customwiplevels.path",
        "Absolute path to the Beat Saber CustomWIPLevels folder.",
    ),
    (
        "map.characteristic",
        "Default beatmap characteristic (e.g. Standard).",
    ),
    (
        "map.difficulty",
        "Default difficulty (Easy/Normal/Hard/Expert/ExpertPlus).",
    ),
    (
        "map.difficulty_rank",
        "Difficulty rank number (Easy=1 .. ExpertPlus=9).",
    ),
    ("map.njs", "Default Note Jump Movement Speed."),
    (
        "map.note_jump_start_beat_offset",
        "Default Note Jump Start Beat Offset.",
    ),
    ("map.environment", "Default environment name."),
    (
        "map.song_filename",
        "Default `_songFilename` (defaults to song.ogg).",
    ),
];

pub fn run(action: ConfigAction) -> Result<()> {
    if action.list {
        let doc = read_doc()?;
        if doc.is_empty() {
            eprintln!("No configuration set yet. Known keys:");
        }
        for (k, desc) in KNOWN_KEYS {
            let v = doc.get(*k).map(|s| s.as_str()).unwrap_or("");
            println!("{k}={v}   ; {desc}");
        }
        return Ok(());
    }

    let Some(key) = action.key else {
        // No key and no --list: print help-ish listing.
        for (k, desc) in KNOWN_KEYS {
            println!("{k}   {desc}");
        }
        return Ok(());
    };

    validate_key(&key)?;

    if action.unset {
        let mut doc = read_doc()?;
        if doc.remove(&key).is_some() {
            write_doc(&doc)?;
            println!("Unset {key}");
        } else {
            println!("(no value for {key})");
        }
        return Ok(());
    }

    match action.value {
        Some(val) => {
            let mut doc = read_doc()?;
            doc.insert(key.clone(), val);
            write_doc(&doc)?;
        }
        None => {
            let doc = read_doc()?;
            match doc.get(&key) {
                Some(v) => println!("{v}"),
                None => bail!("key `{key}` is unset"),
            }
        }
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<()> {
    if KNOWN_KEYS.iter().any(|(k, _)| *k == key) {
        return Ok(());
    }
    // Allow arbitrary `section.name` keys rather than being overly strict,
    // but reject obviously malformed input.
    let mut parts = key.split('.');
    if parts.next().is_some() && parts.next().is_some() && parts.next().is_none() {
        if key
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            eprintln!("warning: `{key}` is not a known mapping key.");
            return Ok(());
        }
    }
    bail!("invalid config key `{key}`; expected `section.name`");
}

fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir()
        .or_else(dirs::home_dir)
        .context("could not locate the user home/config directory")?;
    Ok(base.join("r-ya-mapping").join("config.toml"))
}

fn read_doc() -> Result<Doc> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(Doc::new());
    }
    let text =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut doc = Doc::new();
    // Minimal flat TOML parser for `key = "value"` / `key = 1` lines, grouped
    // under `[section]` headers. Kept tiny rather than pulling a full TOML
    // emitter dependency: keys are flat strings here.
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(h) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            section = h.trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim();
        let v = strip_quotes(v);
        let full = if section.is_empty() {
            k.to_string()
        } else {
            format!("{section}.{k}")
        };
        doc.insert(full, v.to_string());
    }
    Ok(doc)
}

fn write_doc(doc: &Doc) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    // Group keys by section for a readable file.
    let mut sections: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (full, val) in doc {
        let (sec, key) = match full.split_once('.') {
            Some((s, k)) => (s.to_string(), k.to_string()),
            None => (String::new(), full.clone()),
        };
        sections.entry(sec).or_default().push((key, val.clone()));
    }
    let mut out = String::from(
        "# r-ya-mapping global config. Edit with `r-ya-mapping config <key> <value>`.\n",
    );
    for (sec, entries) in &sections {
        out.push_str(&format!("[{sec}]\n"));
        for (k, v) in entries {
            out.push_str(&format!("{k} = {}\n", quote(v)));
        }
        out.push('\n');
    }
    fs::write(&path, out).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn strip_quotes(v: &str) -> &str {
    let v = v.trim();
    if (v.starts_with('"') && v.ends_with('"') && v.len() >= 2)
        || (v.starts_with('\'') && v.ends_with('\'') && v.len() >= 2)
    {
        &v[1..v.len() - 1]
    } else {
        v
    }
}

fn quote(v: &str) -> String {
    // Basic TOML basic-string escaping.
    let escaped = v.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
