//! `render-compare`: real mail drawn three ways, side by side, to find what
//! renders badly and whose fault it is (DESIGN.md "Rendering"). The
//! comparison itself is rinch's (`rinch_visual_test::html_compare`); this
//! chooses the mail and draws it as the window does.
//!
//! ```text
//! render-compare all --data-dir <store copy> --out <dir> [--query <search>] [--count N] [--widths 480,720,1080]
//! ```
//!
//! runs every step: `export` (choose mail, write it as sent and as
//! cleaned), `chrome` (screenshot both in headless Chrome), `capture` (draw
//! the cleaned HTML in a rinch probe window as the reader does, and
//! screenshot it) and `report` (`<out>/report.html`). Run it against a copy
//! of the store: it only reads, but the store takes one opener. Pictures from
//! the web are loaded, as "Show pictures" would. For hand-written HTML files,
//! rinch's own `html-compare` does the same without a store.

mod export;
mod probe;

use std::path::{Path, PathBuf};
use std::process::Command;

use clap::{Parser, Subcommand};
use rinch_visual_test::html_compare::{self, Setup, capture, chrome, report};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// The window's fonts, for Chrome to draw the cleaned HTML with.
const FONT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../postrinch/assets/fonts");

/// Mail as the reader draws it, `width` wide (720 is about the reader's
/// card in a 1440-wide window): the card's CSS, the window's fonts.
pub fn setup(width: u32) -> Setup {
    let face = |file: &str, weight: &str, style: &str| {
        format!(
            "@font-face {{ font-family: 'Postrinch Sans'; src: url('file://{FONT_DIR}/{file}'); font-weight: {weight}; font-style: {style}; }}"
        )
    };
    let chrome_css = [
        face("IBMPlexSans-Regular.ttf", "400", "normal"),
        face("IBMPlexSans-Medium.ttf", "500", "normal"),
        face("IBMPlexSans-SemiBold.ttf", "600 900", "normal"),
        face("IBMPlexSans-Italic.ttf", "400", "italic"),
        // The window's sans-serif is IBM Plex; Chrome's is a system font.
        ".html-compare-host { font-family: 'Postrinch Sans', sans-serif; }".to_owned(),
    ]
    .join("\n");
    Setup {
        width,
        host_css: postrinch_render::HOST_CSS.replace(".mail-html", ".html-compare-host")
            + "\n.html-compare-host { border-radius: 0; }",
        chrome_css,
        fonts: probe::FONTS,
    }
}

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
        /// The widths to draw at, each in `out/w<width>` when more than one.
        #[arg(long, value_delimiter = ',', default_value = "720")]
        widths: Vec<u32>,
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
    /// Screenshot each case in `out` with Chrome.
    Chrome { out: PathBuf },
    /// Draw each case in `out` with rinch.
    Capture { out: PathBuf },
    /// Write `out/report.html`.
    Report { out: PathBuf },
    /// The rinch window `capture` drives (not for running by hand).
    Probe { width: u32, dirs: Vec<PathBuf> },
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
            widths,
        } => {
            for &width in &widths {
                let out = if widths.len() == 1 {
                    out.clone()
                } else {
                    out.join(format!("w{width}"))
                };
                let dirs = export::export(&data_dir, &out, &query, count, per_sender, width)?;
                eprintln!("exported {} messages at {width}px", dirs.len());
                screenshot(&out, &dirs, width)?;
                draw(&dirs, width)?;
                println!("{}", report::report(&out, &dirs)?.display());
            }
            if widths.len() > 1 {
                println!("{}", report::index(&out, &widths)?.display());
            }
        }
        Step::Export {
            data_dir,
            out,
            query,
            count,
            per_sender,
        } => {
            let dirs = export::export(&data_dir, &out, &query, count, per_sender, 720)?;
            println!("exported {} messages into {}", dirs.len(), out.display());
        }
        Step::Chrome { out } => screenshot(&out, &html_compare::case_dirs(&out)?, 720)?,
        Step::Capture { out } => draw(&html_compare::case_dirs(&out)?, 720)?,
        Step::Report { out } => {
            let dirs = html_compare::case_dirs(&out)?;
            println!("{}", report::report(&out, &dirs)?.display());
        }
        Step::Probe { width, dirs } => {
            probe::fetch_web_pictures();
            html_compare::probe::run(dirs, setup(width));
        }
    }
    Ok(())
}

fn screenshot(out: &Path, dirs: &[PathBuf], width: u32) -> Result<()> {
    let profile = out.join(".chrome-profile");
    for dir in dirs {
        // Pictures from the web, as "Show pictures" loads them: the exported
        // cases allow them too. (Scripts never run in Chrome's copy.)
        chrome::case(dir, width, &profile, true)?;
        eprintln!("chrome: {}", dir.display());
    }
    Ok(())
}

fn draw(dirs: &[PathBuf], width: u32) -> Result<()> {
    let mut probe = Command::new(std::env::current_exe()?);
    probe.arg("probe").arg(width.to_string()).args(dirs);
    capture::capture(dirs, probe, width)
}
