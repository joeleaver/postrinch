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
            // Beside the cases: `<out>/.pictures`, shared by every redraw.
            let cache = dirs
                .first()
                .and_then(|d| d.parent())
                .map_or_else(|| PathBuf::from(".pictures"), |out| out.join(".pictures"));
            probe::fetch_web_pictures(&cache);
            html_compare::probe::run(dirs, setup(width));
        }
    }
    Ok(())
}

/// A fontconfig file for Chrome alone: the system's, with the window's sans
/// (IBM Plex) first for the generic families. The window gives `sans-serif`,
/// `system-ui` and `ui-sans-serif` to IBM Plex (rinch's `AppFont::sans_serif`),
/// so a mail that names one itself must get IBM Plex in Chrome too, or the
/// comparison measures the font and not the layout (issue #1). A CSS rule
/// cannot remap a generic family, and `system-ui` follows fontconfig. Only
/// the window's own IBM Plex faces are seen (a system copy is set aside), so
/// bold text in Chrome takes SemiBold as the window does, not a Bold the window
/// does not have.
fn chrome_fonts(out: &Path) -> Result<PathBuf> {
    let dir = out.join(".chrome-fonts");
    std::fs::create_dir_all(&dir)?;
    let fonts = std::fs::canonicalize(FONT_DIR)?;
    // Each other folder the system's IBM Plex Sans faces are in, set aside by
    // path (this fontconfig lets no `acceptfont` win over a `rejectfont`).
    let listed = Command::new("fc-list")
        .args(["IBM Plex Sans", "file"])
        .output()?;
    let mut others: Vec<String> = String::from_utf8_lossy(&listed.stdout)
        .lines()
        .filter_map(|line| {
            Path::new(line.trim().trim_end_matches(':'))
                .parent()
                .map(Path::to_path_buf)
        })
        .filter(|parent| *parent != fonts)
        .map(|parent| format!("<glob>{}/IBMPlexSans-*</glob>", parent.display()))
        .collect();
    others.sort();
    others.dedup();
    let reject = if others.is_empty() {
        String::new()
    } else {
        format!(
            "  <selectfont><rejectfont>{}</rejectfont></selectfont>\n",
            others.concat()
        )
    };
    let fonts = fonts.display();
    let generic = |family: &str| {
        format!(
            "  <match target=\"pattern\"><test name=\"family\"><string>{family}</string></test>\
             <edit name=\"family\" mode=\"prepend\" binding=\"strong\"><string>IBM Plex Sans</string></edit></match>\n"
        )
    };
    let conf = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n<fontconfig>\n  \
         <include ignore_missing=\"yes\">/etc/fonts/fonts.conf</include>\n  <dir>{fonts}</dir>\n{reject}{}</fontconfig>\n",
        ["sans-serif", "sans", "system-ui", "ui-sans-serif"]
            .map(generic)
            .concat()
    );
    let path = dir.join("fonts.conf");
    std::fs::write(&path, conf)?;
    Ok(path)
}

/// Chrome's own font preferences, in its profile: Chrome takes the generic
/// `sans-serif` from these (Arial on Linux) and never asks fontconfig, so
/// `chrome_fonts` alone reaches only `system-ui` (rinch#2423). The profile
/// is the tool's own, so its preferences are written whole.
fn chrome_preferences(profile: &Path) -> Result<()> {
    let dir = profile.join("Default");
    std::fs::create_dir_all(&dir)?;
    let prefs = r#"{"webkit":{"webprefs":{"fonts":{"sansserif":{"Zyyy":"IBM Plex Sans"}}}}}"#;
    std::fs::write(dir.join("Preferences"), prefs)?;
    Ok(())
}

fn screenshot(out: &Path, dirs: &[PathBuf], width: u32) -> Result<()> {
    let fonts = chrome_fonts(out)?;
    // SAFETY: set before Chrome is started, while this process runs no other
    // thread; Chrome inherits it, nothing here reads it.
    unsafe { std::env::set_var("FONTCONFIG_FILE", &fonts) };
    let profile = out.join(".chrome-profile");
    chrome_preferences(&profile)?;
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
