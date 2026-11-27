//! Postrinch: an agent-first desktop mail client.

// A window program on Windows: started from the Start menu or a shortcut it
// opens no console beside its window. Started from a terminal, it prints
// there (`attach_parent_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod actions;
mod add_account;
mod add_account_flow;
mod add_account_import;
mod add_account_run;
mod add_account_run_end;
mod add_account_runs;
mod agents;
mod appearance;
mod attachments;
mod calendar_drag;
mod calendar_edit;
mod calendar_layout;
mod calendar_panel;
mod calendar_proposals;
mod calendar_shares;
mod calendar_ui;
mod changes;
mod conversations;
mod dates;
mod imap_folders;
mod imap_settings_run;
mod import_thunderbird;
mod invitations;
mod links;
mod loader;
mod menus;
mod notices;
mod notify;
mod pictures;
mod reading;
mod recent;
mod remote_line;
mod remote_mail;
mod remote_search;
mod rule_draft;
mod rule_run_now;
mod rules;
mod rules_ui;
mod social_actions;
mod social_edit;
mod social_load;
mod social_shown;
mod social_ui;
mod spelling;
mod state;
mod sync;
mod sync_start;
mod ui;
mod update;
mod view_edit;
mod views;
mod writing;

use std::process::ExitCode;

use postrinch_store::Store;
use rinch::prelude::*;
use tracing_subscriber::prelude::*;

const FONTS: &[AppFont] = &[
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf")),
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf")),
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf")),
    AppFont::sans_serif(include_bytes!("../assets/fonts/IBMPlexSans-Italic.ttf")),
    AppFont::monospace(include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
];

fn main() -> ExitCode {
    attach_parent_console();
    // Before anything else: a window restarted into an update waits here
    // for the one it replaces to let go of the store.
    let waited = postrinch_update::at_start();
    // Notices name the app as the desktop shows it.
    rinch::notification::set_app_name("Postrinch");
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
        #[cfg(windows)]
        Some("--uninstall") => {
            let quiet = std::env::args().nth(2).as_deref() == Some("--quiet");
            return update::uninstall(quiet);
        }
        #[cfg(not(windows))]
        Some("--uninstall") => {
            eprintln!(
                "postrinch: --uninstall is what Windows' Apps & features runs; \
                 on this system Postrinch has no uninstaller yet"
            );
            return ExitCode::FAILURE;
        }
        Some("--version") => {
            println!("postrinch {}", postrinch_update::CURRENT);
            return ExitCode::SUCCESS;
        }
        _ => {}
    }
    // RUST_LOG or this default, never the mail protocols' raw lines
    // (postrinch-log says why).
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(postrinch_log::filter("postrinch=info,postrinch_sync=info")),
        )
        .init();
    if let Some(said) = waited {
        tracing::warn!("{said}");
    }
    let Some(data_dir) = Store::default_root() else {
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
    upkeep(&store);
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
        .spellcheck(spelling::config())
        .menu(menus::menus(state))
        .run();
    ExitCode::SUCCESS
}

/// Windows: borrow the console of the terminal this was started from, if it
/// was, so `--install`, `--version`, a store that cannot open and the log
/// print there. The shell does not wait for a window program, so its prompt
/// comes back at once and the log follows it. A window program has no console of its
/// own; started any other way there is none to borrow, and nothing prints.
/// Output already sent somewhere (`postrinch --version > file`, a pipe) has
/// handles of its own, which are kept.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    // SAFETY: no pointers in; a handle is only compared, never used.
    unsafe {
        let unset = |which| {
            let handle = GetStdHandle(which);
            handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE
        };
        if unset(STD_OUTPUT_HANDLE) && unset(STD_ERROR_HANDLE) {
            // Failing (no parent console) leaves things as they were.
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}

/// The store's upkeep after it opens, on a thread of its own: the latest
/// activity of threads stored before threads kept it filled in, and who
/// answered each last (lists and searches are right meanwhile, only
/// slower), then the one full compaction its files need
/// after an engine upgrade (postrinch-store's upkeep.rs).
fn upkeep(store: &Store) {
    let store = store.clone();
    let spawned = std::thread::Builder::new()
        .name("store-upkeep".into())
        .spawn(move || {
            match store.fill_thread_latest() {
                Ok(0) => {}
                Ok(n) => tracing::info!("kept the latest activity of {n} threads"),
                Err(e) => tracing::warn!("keeping threads' latest activity: {e}"),
            }
            let started = std::time::Instant::now();
            match store.fill_replied_last() {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    "kept who answered last for {n} threads in {:?}",
                    started.elapsed()
                ),
                Err(e) => tracing::warn!("keeping who answered threads last: {e}"),
            }
            let started = std::time::Instant::now();
            match store.compact_once() {
                Ok(false) => {}
                Ok(true) => tracing::info!("compacted the store in {:?}", started.elapsed()),
                Err(e) => tracing::warn!("compacting the store: {e}"),
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot look after the store: {e}");
    }
}
