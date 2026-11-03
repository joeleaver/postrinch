//! Postrinch: an agent-first desktop mail client.

mod dates;
mod state;
mod sync;
mod ui;

use std::process::ExitCode;

use postrinch_store::Store;
use rinch::prelude::*;

const FONTS: &[AppFont] = &[
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")),
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")),
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")),
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-Italic.ttf")),
    AppFont::monospace(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
];

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "postrinch=info,postrinch_sync=info".into()),
        )
        .init();
    let Some(data_dir) = dirs::data_dir().map(|d| d.join("postrinch")) else {
        eprintln!("postrinch: this system has no data directory");
        return ExitCode::FAILURE;
    };
    let store = match Store::open(&data_dir) {
        Ok(store) => store,
        Err(e) => {
            eprintln!(
                "postrinch: cannot open the store at {}: {e}",
                data_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };
    let theme = ThemeProviderProps {
        primary_color: Some("blue".into()),
        default_radius: Some("md".into()),
        font_family: Some("'IBM Plex Sans', sans-serif".into()),
        font_family_monospace: Some("'IBM Plex Mono', monospace".into()),
        ..Default::default()
    };
    App::new(move |scope| ui::app(scope, store))
        .title("Postrinch")
        .size(1440, 900)
        .fonts(FONTS)
        .theme(theme)
        .run();
    ExitCode::SUCCESS
}
