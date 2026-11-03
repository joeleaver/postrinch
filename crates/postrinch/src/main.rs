//! Postrinch: an agent-first desktop mail client.

mod actions;
mod agents;
mod appearance;
mod changes;
mod dates;
mod loader;
mod menus;
mod reading;
mod state;
mod sync;
mod ui;
mod update;

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
    // Before anything else: a window restarted into an update waits here
    // for the one it replaces to let go of the store.
    postrinch_update::at_start();
    match std::env::args().nth(1).as_deref() {
        Some("--install") => {
            return match update::install() {
                Ok(said) => {
                    println!("{said}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("postrinch: cannot install: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        Some("--version") => {
            println!("postrinch {}", postrinch_update::CURRENT);
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
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
    fill_thread_latest(&store);
    let theme = ThemeProviderProps {
        primary_color: Some("blue".into()),
        default_radius: Some("md".into()),
        font_family: Some("'IBM Plex Sans', sans-serif".into()),
        font_family_monospace: Some("'IBM Plex Mono', monospace".into()),
        ..Default::default()
    };
    let state = state::State::new();
    let window = WindowProps {
        title: "Postrinch".into(),
        width: 1440,
        height: 900,
        borderless: true,
        // Transparent, so the window's rounded corners show the desktop.
        transparent: true,
        menu_in_titlebar: true,
        app_id: Some("postrinch".into()),
        icon: Some(include_bytes!("../assets/icons/postrinch-256.png")),
        ..Default::default()
    };
    App::new(move |scope| ui::app(scope, state, store))
        .window_props(window)
        .fonts(FONTS)
        .theme(theme)
        .menu(menus::menus(state))
        .run();
    ExitCode::SUCCESS
}

/// Keep the latest activity of threads stored before threads kept it, in
/// the background: lists are right meanwhile, only slower.
fn fill_thread_latest(store: &Store) {
    let store = store.clone();
    let spawned = std::thread::Builder::new()
        .name("fill-threads".into())
        .spawn(move || match store.fill_thread_latest() {
            Ok(0) => {}
            Ok(n) => tracing::info!("kept the latest activity of {n} threads"),
            Err(e) => tracing::warn!("keeping threads' latest activity: {e}"),
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot keep threads' latest activity: {e}");
    }
}
