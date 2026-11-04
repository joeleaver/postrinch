//! `render-compare`: real mail drawn three ways, side by side, to find what
//! renders badly and whose fault it is (DESIGN.md "Rendering").
//!
//! ```text
//! render-compare all --data-dir <store copy> --out <dir> [--query <search>] [--count N]
//! ```
//!
//! runs every step: `export` (choose mail, write it as sent and as
//! cleaned), `chrome` (screenshot both in headless Chrome), `capture`
//! (draw the cleaned HTML in a rinch probe window and screenshot it) and
//! `report` (`<out>/report.html`). Run it against a copy of the store: it
//! only reads, but the store takes one opener. Pictures from the web are
//! loaded, as "Show pictures" would.

mod capture;
mod chrome;
mod export;
mod probe;
mod report;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// How wide mail is drawn: about the reader's card in a 1440-wide window.
pub const WIDTH: u32 = 720;
/// The window's fonts, for Chrome to draw the cleaned HTML with.
pub const FONT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../postrinch/assets/fonts");

#[derive(Parser)]
#[command(about = "Real mail drawn by Chrome and by Postrinch, side by side")]
struct Cli {
    #[command(subcommand)]
    command: Step,
}

#[derive(Subcommand)]
enum Step {
    /// Every step, into `out`.
    All {
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Which mail, in Postrinch's search language.
        #[arg(long, default_value = "in:inbox")]
        query: String,
        #[arg(long, default_value_t = 40)]
        count: usize,
        /// At most this many messages from one sender.
        #[arg(long, default_value_t = 1)]
        per_sender: usize,
    },
    /// Choose mail and write it as sent and as cleaned.
    Export {
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value = "in:inbox")]
        query: String,
        #[arg(long, default_value_t = 40)]
        count: usize,
        #[arg(long, default_value_t = 1)]
        per_sender: usize,
    },
    /// Screenshot each message folder in `out` with Chrome.
    Chrome { out: PathBuf },
    /// Draw each message folder in `out` with rinch.
    Capture { out: PathBuf },
    /// Write `out/report.html`.
    Report { out: PathBuf },
    /// The rinch window `capture` drives (not for running by hand).
    Probe { dirs: Vec<PathBuf> },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli.command) {
        eprintln!("render-compare: {e}");
        std::process::exit(1);
    }
}

fn run(step: Step) -> Result<()> {
    match step {
        Step::All {
            data_dir,
            out,
            query,
            count,
            per_sender,
        } => {
            let dirs = export::export(&data_dir, &out, &query, count, per_sender)?;
            eprintln!("exported {} messages", dirs.len());
            screenshot(&out, &dirs)?;
            capture::capture(&dirs)?;
            let report = report::report(&out, &dirs)?;
            println!("{}", report.display());
        }
        Step::Export {
            data_dir,
            out,
            query,
            count,
            per_sender,
        } => {
            let dirs = export::export(&data_dir, &out, &query, count, per_sender)?;
            println!("exported {} messages into {}", dirs.len(), out.display());
        }
        Step::Chrome { out } => screenshot(&out, &message_dirs(&out)?)?,
        Step::Capture { out } => capture::capture(&message_dirs(&out)?)?,
        Step::Report { out } => {
            let report = report::report(&out, &message_dirs(&out)?)?;
            println!("{}", report.display());
        }
        Step::Probe { dirs } => probe::run(dirs),
    }
    Ok(())
}

fn screenshot(out: &Path, dirs: &[PathBuf]) -> Result<()> {
    let profile = out.join(".chrome-profile");
    for dir in dirs {
        chrome::message(dir, &profile)?;
        eprintln!("chrome: {}", dir.display());
    }
    Ok(())
}

/// The message folders in `out`, in order.
fn message_dirs(out: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(out)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("meta.json").exists())
        .collect();
    dirs.sort();
    Ok(dirs)
}
