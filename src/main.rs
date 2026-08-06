// Command-line interface and subcommand dispatch.
//
// Two subcommands are exposed, mirroring the spec:
//   * `config`  - git-config style get/set/list of global options in $HOME.
//   * `start`   - given a YouTube URI, download the song (opus source), detect
//                 BPM, finalize the map audio as `.ogg`, and scaffold a Beat
//                 Saber map folder (with a square-cover thumbnail if present).
//
// `start` is also reachable directly via a bare positional, making it the
// default subcommand: `r-ya-mapping <youtube-uri>` is equivalent to
// `r-ya-mapping start <youtube-uri>`.

use anyhow::Result;
use clap::{Parser, Subcommand};

mod audio;
mod beatmap;
mod bpm;
mod config;
mod model;
mod onset;
mod start;
mod youtube;

#[derive(Parser)]
#[command(
    name = "r-ya-mapping",
    version,
    about = "Beat Saber quick-start mapping helper",
    long_about = "Download a song from YouTube (opus source), detect its BPM, \n\
                  finalize the audio as .ogg, and scaffold a Beat Saber \n\
                  CustomWIPLevels folder.",
    args_conflicts_with_subcommands = true,
    subcommand_negates_reqs = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Option<Cmd>,

    /// YouTube URI for the implicit (default) `start` invocation.
    /// Equivalent to `r-ya-mapping start <uri>` (with default options).
    uri: Option<String>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Read or write global mapping options (like `git config`).
    Config {
        /// Key in `section.name` form (e.g. `mapper.name`). Omit when --list.
        key: Option<String>,
        /// Value to set. Omit to read the current value; supply to write.
        value: Option<String>,
        /// List all configured key/value pairs.
        #[arg(short, long)]
        list: bool,
        /// Unset a key.
        #[arg(short = 'u', long)]
        unset: bool,
    },
    /// Download a YouTube song, detect BPM and scaffold a map (default).
    #[command(alias = "default")]
    Start {
        /// YouTube URI (https://www.youtube.com/watch?v=... or youtu.be/...).
        uri: String,
        /// Override the automatically detected BPM.
        #[arg(long)]
        bpm: Option<f64>,
        /// Override the song name (`_songName`).
        #[arg(long)]
        song_name: Option<String>,
        /// Override the song sub name (`_songSubName`). Pass "" to clear it.
        #[arg(long)]
        song_sub_name: Option<String>,
        /// Override the song author (`_songAuthorName`).
        #[arg(long)]
        song_author: Option<String>,
        /// Override the map folder name (defaults to the song name).
        #[arg(long)]
        map_name: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.cmd {
        Some(Cmd::Config {
            key,
            value,
            list,
            unset,
        }) => config::run(config::ConfigAction {
            key,
            value,
            list,
            unset,
        }),
        Some(Cmd::Start {
            uri,
            bpm,
            song_name,
            song_sub_name,
            song_author,
            map_name,
        }) => start::run_start(start::StartOpts {
            uri,
            bpm,
            song_name,
            song_sub_name,
            song_author,
            map_name,
        }),
        None => match cli.uri {
            Some(uri) => start::run_start(start::StartOpts {
                uri,
                bpm: None,
                song_name: None,
                song_sub_name: None,
                song_author: None,
                map_name: None,
            }),
            None => {
                use clap::CommandFactory;
                Cli::command().print_help()?;
                println!();
                Ok(())
            }
        },
    }
}
