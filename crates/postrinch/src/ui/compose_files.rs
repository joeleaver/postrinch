//! The files attached to the mail being written, after the attachments
//! mockup: one chip each with its size and a button to take it out, where
//! an agent's file came from, and a warning (with Send off) past Gmail's
//! 25 MB. Files come from Attach… in the footer or are dropped on the form.

use postrinch_commands::size_text;
use postrinch_model::{AttachmentOrigin, DraftAttachment, MAX_ATTACHED};
use rinch::file::{FilePicker, LocalFile};
use rinch::prelude::*;

use crate::actions::commands;
use crate::state::State;
use crate::ui::icons::{Icon, icon};

/// Whether the files are more than Gmail sends.
pub fn too_large(files: &[DraftAttachment]) -> bool {
    files.iter().map(|a| a.size).sum::<u64>() > MAX_ATTACHED
}

/// Ask for files and attach them.
pub fn pick(state: State, files: Signal<Vec<DraftAttachment>>) {
    FilePicker::new()
        .multiple(true)
        .set_title("Attach files")
        .open(move |picked| attach(state, files, picked));
}

/// Attach files the person picked or dropped, as their bytes arrive.
pub fn attach(state: State, files: Signal<Vec<DraftAttachment>>, picked: Vec<LocalFile>) {
    for file in picked {
        let name = file.name().to_owned();
        file.read(move |bytes| {
            let Some(commands) = commands() else {
                return;
            };
            let attached = bytes.map_err(|e| e.to_string()).and_then(|bytes| {
                commands
                    .attach_bytes(&name, None, &bytes, AttachmentOrigin::Person)
                    .map_err(|e| e.to_string())
            });
            match attached {
                Ok(a) => files.update(|f| f.push(a)),
                Err(e) => state
                    .compose_error
                    .set(Some(format!("{name} could not be attached: {e}"))),
            }
        });
    }
}

#[component]
pub fn attached_files(files: Signal<Vec<DraftAttachment>>) -> NodeHandle {
    rsx! {
        div { class: "compose-files",
            style: {move || if files.get().is_empty() { "display: none" } else { "" }},
            div { class: "compose-files-head",
                {move || {
                    let list = files.get();
                    let total: u64 = list.iter().map(|a| a.size).sum();
                    match list.len() {
                        1 => format!("1 attachment · {}", size_text(total)),
                        n => format!("{n} attachments · {}", size_text(total)),
                    }
                }}
            }
            div { class: "compose-chips",
                for (at, file) in files.get().into_iter().enumerate() {
                    div { key: format!("{at}|{}", file.blob),
                        {chip(__scope, files, at, file.clone())}
                    }
                }
            }
            div { class: "compose-warn",
                style: {move || if too_large(&files.get()) { "" } else { "display: none" }},
                {move || {
                    let total: u64 = files.get().iter().map(|a| a.size).sum();
                    format!(
                        "{} attached: Gmail sends up to {}. Take something out to send.",
                        size_text(total),
                        size_text(MAX_ATTACHED)
                    )
                }}
            }
        }
    }
}

#[component]
fn chip(files: Signal<Vec<DraftAttachment>>, at: usize, file: DraftAttachment) -> NodeHandle {
    let take_out = format!("Take out {}", file.name);
    let label = take_out.clone();
    let kind = if file.mime_type.starts_with("image/") {
        Icon::Picture
    } else {
        Icon::File
    };
    let size = format!(" · {}", size_text(file.size));
    let origin = origin_line(&file.origin).map(|o| {
        rsx! { span { class: "compose-chip-origin", {o} } }
    });
    rsx! {
        div { class: "compose-chip",
            {icon(__scope, kind)}
            div { class: "compose-chip-text",
                span { class: "compose-chip-name",
                    {file.name.clone()}
                    span { class: "compose-chip-size", {size} }
                }
                {origin}
            }
            button {
                class: "compose-chip-remove",
                title: {take_out.clone()},
                aria-label: {label.clone()},
                onclick: move || files.update(|f| {
                    if at < f.len() {
                        f.remove(at);
                    }
                }),
                {icon(__scope, Icon::Close)}
            }
        }
    }
}

/// Where an agent's file came from, for the person checking it.
fn origin_line(origin: &AttachmentOrigin) -> Option<String> {
    match origin {
        AttachmentOrigin::Person | AttachmentOrigin::Mail => None,
        AttachmentOrigin::File { path } => {
            Some(format!("from this computer: {}", home_short(path)))
        }
        AttachmentOrigin::Agent => Some("made by the agent".into()),
    }
}

/// A path with the home folder as `~`.
fn home_short(path: &str) -> String {
    match dirs::home_dir().and_then(|h| h.to_str().map(str::to_owned)) {
        Some(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

pub const CSS: &str = r#"
.compose-files { display: flex; flex-direction: column; gap: 6px; }
.compose-files-head { font-size: 12px; color: var(--muted); }
.compose-chips { display: flex; flex-wrap: wrap; gap: 6px; }
.compose-chip {
    display: flex; align-items: center; gap: 8px;
    max-width: 100%;
    padding: 5px 4px 5px 10px;
    border: 1px solid var(--line);
    border-radius: 7px;
    background: var(--panel);
    color: var(--muted);
}
.compose-chip-text { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
.compose-chip-name { font-size: 13px; color: var(--ink); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.compose-chip-size { color: var(--muted); }
.compose-chip-origin { font-size: 12px; color: var(--muted); font-family: 'IBM Plex Mono', monospace; overflow-wrap: anywhere; }
.compose-chip-remove {
    width: 24px; height: 24px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    border: 0; border-radius: 5px;
    background: transparent; color: var(--muted);
    padding: 0; cursor: pointer;
}
.compose-chip-remove:hover { background: var(--hover); color: var(--ink); }
.compose-attach {
    height: 30px;
    display: flex; align-items: center; gap: 6px;
    padding: 0 12px;
    border: 1px solid var(--field); border-radius: 6px;
    background: transparent; color: var(--ink);
    font: inherit; font-size: 13px; cursor: pointer;
}
.compose-attach:hover { background: var(--hover); }
"#;
