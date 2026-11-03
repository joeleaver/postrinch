//! Settings › About, after its mockup: the version, where it is installed,
//! and where keeping it current stands. Also the title bar's "Restart to
//! update" pill.

use rinch::prelude::*;

use crate::state::State;
use crate::update::{self, UpdateStatus};

pub const CSS: &str = r#"
.update-box {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 560px;
    padding: 16px 18px;
    border: 1px solid var(--line);
    border-radius: 8px;
}
.update-box.ready { border-color: var(--notice-line); background: var(--accent-tint); }
.update-box.trouble { border-color: var(--warn-line); background: var(--warn-bg); }
.update-head { display: flex; align-items: center; gap: 10px; font-weight: 600; }
.update-line { margin: 0; color: var(--muted); line-height: 1.5; }
.update-actions { display: flex; gap: 8px; }
.status-dot.ready { background: var(--accent); }
.status-dot.idle { background: var(--muted); }
.mono { font-family: 'IBM Plex Mono', monospace; font-size: 13px; }
.update-pill {
    display: flex;
    align-items: center;
    height: 24px;
    padding: 0 12px;
    border: 1px solid var(--notice-line);
    border-radius: 999px;
    background: var(--accent-tint);
    color: var(--accent);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
}
.update-pill:hover { border-color: var(--accent); }
"#;

/// What the box says for a status: its look, heading, line, and which
/// buttons it offers (restart, check).
struct Shown {
    class: &'static str,
    dot: &'static str,
    head: String,
    line: String,
    restart: bool,
    check: bool,
}

fn shown(status: UpdateStatus) -> Shown {
    let plain = |dot, head: &str, line: String, check| Shown {
        class: "update-box",
        dot,
        head: head.to_owned(),
        line,
        restart: false,
        check,
    };
    match status {
        UpdateStatus::NotInstalled => plain(
            "status-dot idle",
            "This copy does not update itself",
            "It runs from a build folder, not the installed app. Install it with postrinch --install to keep it current.".into(),
            false,
        ),
        UpdateStatus::Waiting => plain(
            "status-dot idle",
            "Checks for new versions",
            "The first check is a minute after Postrinch starts, then once a day.".into(),
            true,
        ),
        UpdateStatus::Checking => plain(
            "status-dot off",
            "Checking for a new version\u{2026}",
            "This runs in the background; keep reading.".into(),
            false,
        ),
        UpdateStatus::UpToDate { at } => {
            plain("status-dot", "Up to date", format!("Last checked {at}."), true)
        }
        UpdateStatus::Ready { version } => Shown {
            class: "update-box ready",
            dot: "status-dot ready",
            head: format!("Version {version} is ready"),
            line: "Downloaded and checked. Postrinch restarts into it when you say; your mail and settings stay as they are.".into(),
            restart: true,
            check: false,
        },
        UpdateStatus::Failed { reason, at } => Shown {
            class: "update-box trouble",
            dot: "status-dot off",
            head: "The last check did not work".into(),
            line: format!("{reason} ({at}). It tries again tomorrow."),
            restart: false,
            check: true,
        },
    }
}

#[component]
pub fn about(state: State) -> NodeHandle {
    let installed = postrinch_update::managed_dir()
        .map(|d| d.display().to_string())
        .unwrap_or_else(|| "Not installed: running from a build folder".into());
    rsx! {
        main { class: "settings-main",
            h1 { class: "settings-heading", "About" }
            div { class: "facts",
                span { class: "fact-name", "Version" }
                span { class: "mono", {postrinch_update::CURRENT} }
                span { class: "fact-name", "Installed in" }
                span { class: "mono", {installed} }
            }
            {status_box(__scope, state)}
            p { class: "settings-hint",
                "Postrinch checks for a new version a minute after it starts and then once a day. It downloads and checks the new version in the background, and changes nothing until you restart. The agents\u{2019} bridge and postrinch-cli are updated with it."
            }
        }
    }
}

/// The box follows the status as it changes, built once.
#[component]
fn status_box(state: State) -> NodeHandle {
    let now = move || shown(state.update.get());
    rsx! {
        section { class: {move || now().class},
            div { class: "update-head",
                span { class: {move || now().dot} }
                span { {move || now().head} }
            }
            p { class: "update-line", {move || now().line} }
            div { class: "update-actions",
                if now().restart {
                    Button { variant: "filled", size: "sm", onclick: move || update::restart(state), "Restart to update" }
                }
                if now().check {
                    Button { variant: "default", size: "sm", onclick: update::check_now, "Check now" }
                }
            }
        }
    }
}

/// The title bar's pill, while a new version waits for a restart.
#[component]
pub fn update_pill(state: State) -> NodeHandle {
    rsx! {
        for version in state.update_ready.get() {
            div {
                key: version.clone(),
                class: "update-pill",
                onclick: move || update::restart(state),
                {format!("Restart to update to {version}")}
            }
        }
    }
}
