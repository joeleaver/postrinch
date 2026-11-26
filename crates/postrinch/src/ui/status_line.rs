//! The status line, after design/mockups/status-line.html: one small line
//! at the foot of the left pane in every mode, summing up every account's
//! sync. A click opens each account's own line above it, with "Sign in
//! again…" on one that needs it.

use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::state::{SettingsSection, State, View};
use crate::sync::{AccountSync, SyncHow};

/// How the line looks: quiet, busy, or calling for the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    Quiet,
    Working,
    Trouble,
}

/// What the line says, and how much mail is downloading across accounts.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Summary {
    text: String,
    tone: Tone,
    progress: Option<(usize, usize)>,
}

/// "1 account" or "3 accounts".
fn accounts(n: usize) -> String {
    if n == 1 {
        "1 account".to_owned()
    } else {
        format!("{n} accounts")
    }
}

/// Sum up the accounts' sync, worst first: sign-in the person must give,
/// other trouble, sync being off, accounts at work, then all is well.
/// `off` is why sync (or Google's part of it) cannot run.
fn summary(all: &[AccountSync], off: Option<&str>) -> Summary {
    let count = |how: SyncHow| all.iter().filter(|s| s.how == how).count();
    let trouble = |text: String| Summary {
        text,
        tone: Tone::Trouble,
        progress: None,
    };
    let signing = count(SyncHow::NeedsSignIn);
    if signing > 0 {
        let verb = if signing == 1 { "needs" } else { "need" };
        return trouble(format!("{} {verb} sign-in", accounts(signing)));
    }
    let failing = count(SyncHow::Trouble);
    if failing > 0 {
        return trouble(format!("Trouble with {}", accounts(failing)));
    }
    if let Some(off) = off {
        return trouble(off.to_owned());
    }
    let working = count(SyncHow::Working);
    if working > 0 {
        let text = if all.len() == 1 {
            "Syncing\u{2026}".to_owned()
        } else {
            format!("Syncing {working} of {}\u{2026}", accounts(all.len()))
        };
        let downloads: Vec<_> = all.iter().filter_map(|s| s.progress).collect();
        let progress = (!downloads.is_empty()).then(|| {
            downloads
                .iter()
                .fold((0, 0), |(d, t), (done, total)| (d + done, t + total))
        });
        return Summary {
            text,
            tone: Tone::Working,
            progress,
        };
    }
    let text = if all.is_empty() {
        "No accounts syncing"
    } else {
        "Up to date"
    };
    Summary {
        text: text.to_owned(),
        tone: Tone::Quiet,
        progress: None,
    }
}

/// `done` of `total` as a whole percentage, 100 when there is nothing.
fn percent((done, total): (usize, usize)) -> usize {
    (done * 100).checked_div(total).unwrap_or(100).min(100)
}

#[component]
pub fn status_line(state: State) -> NodeHandle {
    let now = Memo::new(move || {
        let off = state.sync_off.get();
        state.sync.with(|all| summary(all, off.as_deref()))
    });
    rsx! {
        div { class: "status-foot",
            Popover {
                opened_fn: move || state.status_open.get(),
                onclose: move || state.status_open.set(false),
                // At the foot of the pane: the detail opens upward.
                position: "top-start",
                width: "300px",
                PopoverTarget {
                    button {
                        class: {move || match now.get().tone {
                            Tone::Quiet => "status-line",
                            Tone::Working => "status-line working",
                            Tone::Trouble => "status-line trouble",
                        }},
                        title: "Sync, account by account",
                        aria-haspopup: "dialog",
                        aria-expanded: {move || if state.status_open.get() { "true" } else { "false" }},
                        onclick: move || state.status_open.update(|open| *open = !*open),
                        span { class: "status-mark" }
                        span { class: "status-text", {move || now.get().text} }
                        span { class: "status-caret", {move || if state.status_open.get() { "\u{25be}" } else { "\u{25b4}" }} }
                        span {
                            class: "status-bar",
                            style: {move || if now.get().progress.is_some() { "" } else { "display: none" }},
                            span {
                                class: "status-bar-fill",
                                style: {move || format!("width: {}%", now.get().progress.map_or(0, percent))},
                            }
                        }
                    }
                }
                PopoverDropdown {
                    div { class: "status-detail", role: "dialog", aria-label: "Sync",
                        for why in state.sync_off.get() {
                            div { key: why.clone(), class: "status-off", {why.clone()} }
                        }
                        // Keyed by account alone: a row is built once and its
                        // line and bar follow the account's sync in place.
                        for account in synced(state) {
                            div { key: account.key(), {account_row(__scope, state, account.clone())} }
                        }
                        if state.sync.with(|all| all.is_empty()) && state.sync_off.with(|o| o.is_none()) {
                            div { class: "status-none", "No account has synced yet." }
                        }
                    }
                }
            }
        }
    }
}

fn synced(state: State) -> Vec<EmailAddress> {
    state
        .sync
        .with(|all| all.iter().map(|s| s.account.clone()).collect())
}

/// One account's own line: its address, what it is doing, a bar while
/// downloading, and "Sign in again…" when it needs it.
#[component]
fn account_row(state: State, account: EmailAddress) -> NodeHandle {
    let address = account.to_string();
    let again = account.clone();
    let now = Memo::new(move || {
        state
            .sync
            .with(|all| all.iter().find(|s| s.account == account).cloned())
    });
    let progress = move || now.get().and_then(|s| s.progress);
    let how = move || now.get().map(|s| s.how);
    rsx! {
        div { class: "status-account",
            div { class: "status-address", {address} }
            div {
                class: {move || if how().is_some_and(SyncHow::trouble) { "sync-line trouble" } else { "sync-line" }},
                {move || now.get().map(|s| s.line).unwrap_or_default()}
            }
            div {
                class: "progress",
                style: {move || if progress().is_some() { "" } else { "display: none" }},
                div {
                    class: "progress-fill",
                    style: {move || format!("width: {}%", progress().map_or(0, percent))},
                }
            }
            if how() == Some(SyncHow::NeedsSignIn) {
                {sign_in_button(__scope, state, again.clone())}
            }
        }
    }
}

#[component]
fn sign_in_button(state: State, account: EmailAddress) -> NodeHandle {
    rsx! {
        Button {
            variant: "default",
            size: "xs",
            onclick: move || sign_in_again(state, account.clone()),
            "Sign in again\u{2026}"
        }
    }
}

/// Sign the account in again, on its page in Settings › Accounts, where
/// what the sign-in says (and any trouble with it) shows.
fn sign_in_again(state: State, account: EmailAddress) {
    state.status_open.set(false);
    state.view.set(View::Settings);
    state.settings_section.set(SettingsSection::Accounts);
    state.account_shown.set(Some(account.clone()));
    state.sign_in(Some(account));
}

pub const CSS: &str = r#"
.status-foot { flex-shrink: 0; margin-top: auto; }
/* The popover and its target are inline boxes as rinch draws them: here
   they span the pane, so the line is the pane's width and long words end
   in an ellipsis rather than widening it. */
.status-foot .rinch-popover, .status-foot .rinch-popover__target { display: block; }
.status-line {
    position: relative; width: 100%; height: 30px; box-sizing: border-box; overflow: hidden;
    display: flex; align-items: center; gap: 8px; padding: 0 10px;
    border: 1px solid var(--line); border-radius: 6px; background: var(--surface);
    color: var(--muted); font: inherit; font-size: 12.5px; text-align: left; cursor: pointer;
}
.status-line:hover { background: var(--hover); }
.status-line:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.status-mark { width: 8px; height: 8px; border-radius: 4px; flex-shrink: 0; background: var(--faint); }
.status-line.working .status-mark { background: var(--accent); }
.status-line.trouble { background: var(--warn-bg); border-color: var(--warn-line); color: var(--trouble); font-weight: 600; }
.status-line.trouble .status-mark { background: var(--trouble); }
.status-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.status-caret { color: var(--faint); font-size: 10px; }
.status-bar { position: absolute; left: 0; right: 0; bottom: 0; height: 2px; background: var(--line); }
.status-bar-fill { display: block; height: 100%; background: var(--accent); }
.status-detail { display: flex; flex-direction: column; gap: 10px; }
.status-off { font-size: 12.5px; color: var(--trouble); }
.status-none { font-size: 12.5px; color: var(--muted); }
.status-account { display: flex; flex-direction: column; gap: 4px; align-items: flex-start; }
.status-account .progress { align-self: stretch; }
.status-address { font-size: 13px; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
"#;

#[cfg(test)]
#[path = "status_line_tests.rs"]
mod tests;
