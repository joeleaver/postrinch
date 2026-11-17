//! A message's attachments under it, after the attachments mockup: a card
//! per file with its name, kind and size, pictures shown small. A click
//! opens the file in its app; the card under the pointer offers Open and
//! Save as…; Save all… saves every file into a folder. A program or script
//! says so, and is only saved.

use postrinch_commands::{is_program, size_text};
use postrinch_model::{AttachmentInfo, Message};
use rinch::prelude::*;

use crate::attachments::{is_picture, key, name_of};
use crate::state::State;
use crate::ui::icons::{Icon, icon};

/// What a file is, for its card: a short badge and a few words.
pub fn kind(name: &str, mime_type: &str) -> (&'static str, String, &'static str) {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if is_program(name, mime_type) {
        return ("", "A program: save only".into(), "program");
    }
    match (ext.as_str(), mime_type) {
        ("pdf", _) | (_, "application/pdf") => ("PDF", "PDF".into(), "pdf"),
        (_, m) if m.starts_with("image/") => {
            let sub = m.trim_start_matches("image/").to_ascii_uppercase();
            ("IMG", format!("{sub} picture"), "picture")
        }
        ("xls" | "xlsx" | "ods" | "csv" | "numbers", _) => ("XLS", "Spreadsheet".into(), "sheet"),
        ("doc" | "docx" | "odt" | "rtf" | "pages", _) => ("DOC", "Document".into(), "doc"),
        ("ppt" | "pptx" | "odp" | "key", _) => ("PPT", "Slides".into(), "doc"),
        ("zip" | "gz" | "tgz" | "7z" | "rar" | "xz" | "bz2" | "tar", _) => {
            ("ZIP", "Archive".into(), "archive")
        }
        ("ics", _) | (_, "text/calendar") => ("ICS", "Invitation".into(), "doc"),
        ("eml", _) | (_, "message/rfc822") => ("EML", "A message".into(), "doc"),
        (_, m) if m.starts_with("text/") => ("TXT", "Text".into(), "doc"),
        (_, m) if m.starts_with("audio/") => ("AUD", "Sound".into(), "archive"),
        (_, m) if m.starts_with("video/") => ("VID", "Video".into(), "archive"),
        _ => ("FILE", "File".into(), "archive"),
    }
}

#[component]
pub fn attachment_cards(state: State, message: Message) -> NodeHandle {
    let files: Vec<(usize, AttachmentInfo)> = message
        .attachments
        .iter()
        .cloned()
        .enumerate()
        .filter(|(_, a)| !a.inline)
        .collect();
    if files.is_empty() {
        return rsx! { span {} };
    }
    let total: u64 = files.iter().map(|(_, a)| a.size).sum();
    let count = match files.len() {
        1 => "1 attachment".to_owned(),
        n => format!("{n} attachments"),
    };
    let summary = format!("{count} · {}", size_text(total));
    let many = files.len() > 1;
    let all = message.clone();
    let cards: Vec<NodeHandle> = files
        .into_iter()
        .map(|(index, info)| card(__scope, state, message.clone(), index, info))
        .collect();
    rsx! {
        div { class: "attachments",
            div { class: "attachments-head",
                span { {summary} }
                if many {
                    button {
                        class: "link-button",
                        onclick: move || state.save_attachments(&all),
                        "Save all…"
                    }
                }
            }
            div { class: "attachment-cards", {cards} }
        }
    }
}

#[component]
fn card(state: State, message: Message, index: usize, info: AttachmentInfo) -> NodeHandle {
    let name = name_of(&info);
    let (badge, words, family) = kind(&name, &info.mime_type);
    let program = family == "program";
    let picture = is_picture(&info);
    let thumb_key = key(&message.id, index);
    let detail = if program {
        words
    } else {
        format!("{words} · {}", size_text(info.size))
    };
    let (m1, m2, m3) = (message.clone(), message.clone(), message.clone());
    let title = if program {
        format!("{name}: a program, so Postrinch only saves it")
    } else {
        format!("Open {name}")
    };
    let badge_node = (!picture).then(|| {
        if program {
            rsx! { span { class: "attachment-badge program", {icon(__scope, Icon::Warning)} } }
        } else {
            let class = format!("attachment-badge {family}");
            rsx! { span { class: {class.clone()}, {badge} } }
        }
    });
    let picture_node = picture.then(|| thumbnail(__scope, state, thumb_key.clone()));
    let open_node = (!program).then(|| {
        rsx! {
            button {
                class: "attachment-open",
                onclick: move || state.open_attachment(&m2, index),
                "Open"
            }
        }
    });
    let card_class = if picture {
        "attachment-card with-picture"
    } else {
        "attachment-card"
    };
    let detail_class = if program {
        "attachment-detail program"
    } else {
        "attachment-detail"
    };
    rsx! {
        div {
            class: {card_class},
            title: {title.clone()},
            onclick: move || state.open_attachment(&m1, index),
            {picture_node}
            div { class: "attachment-line",
                {badge_node}
                div { class: "attachment-text",
                    span { class: "attachment-name", {name} }
                    span { class: {detail_class}, {detail} }
                }
            }
            div { class: "attachment-actions",
                {open_node}
                button {
                    class: "attachment-save",
                    title: "Save as…",
                    aria-label: "Save as…",
                    onclick: move || state.save_attachment(&m3, index),
                    {icon(__scope, Icon::Save)}
                }
            }
        }
    }
}

/// A picture's thumbnail, once the loader has it; an icon until then.
#[component]
fn thumbnail(state: State, key: String) -> NodeHandle {
    let waiting = key.clone();
    let drawn = key.clone();
    rsx! {
        div {
            // Pictures are drawn on white, as mail is: a logo on a
            // transparent ground is often dark.
            class: {move || if state.thumbnails.get().contains_key(&drawn) { "attachment-picture drawn" } else { "attachment-picture" }},
            for url in state.thumbnails.get().get(&key).cloned() {
                img { key: url.len().to_string(), src: {url.clone()}, alt: "" }
            }
            if !state.thumbnails.get().contains_key(&waiting) {
                {icon(__scope, Icon::Picture)}
            }
        }
    }
}

/// The cards' styles.
pub const CSS: &str = r#"
.attachments {
    display: flex; flex-direction: column; gap: 8px;
    border-top: 1px solid var(--line-soft);
    padding-top: 12px;
}
.attachments-head { display: flex; align-items: center; gap: 10px; font-size: 13px; color: var(--muted); }
.link-button {
    border: 0; padding: 0; background: transparent;
    color: var(--accent); font: inherit; font-size: 13px; cursor: pointer;
}
.link-button:hover { color: var(--accent-ink); text-decoration: underline; }
.attachment-cards { display: flex; flex-wrap: wrap; align-items: flex-start; gap: 10px; }
.attachment-card {
    width: 216px;
    display: flex; flex-direction: column;
    border: 1px solid var(--line);
    border-radius: 8px;
    background: var(--panel);
    overflow: hidden;
    cursor: pointer;
}
.attachment-card:hover { border-color: var(--accent); background: var(--row-hover); }
.attachment-picture {
    height: 92px;
    display: flex; align-items: center; justify-content: center;
    background: var(--accent-tint); color: var(--accent);
    overflow: hidden;
}
.attachment-picture.drawn { background: #ffffff; }
.attachment-picture img { width: 100%; height: 100%; object-fit: contain; }
.attachment-line { display: flex; align-items: center; gap: 10px; padding: 10px; }
.with-picture .attachment-line { padding: 8px 10px; }
.attachment-badge {
    width: 34px; height: 34px; flex-shrink: 0;
    display: flex; align-items: center; justify-content: center;
    border-radius: 6px;
    font-size: 10px; font-weight: 700; letter-spacing: 0.04em;
    background: var(--hover); color: var(--ink-2);
}
.attachment-badge.pdf { background: var(--red-tint); color: var(--red-ink); }
.attachment-badge.sheet { background: var(--green-tint); color: var(--green-ink); }
.attachment-badge.doc { background: var(--accent-tint); color: var(--accent-ink); }
.attachment-badge.program { background: var(--warn-bg); color: var(--trouble); }
.attachment-text { display: flex; flex-direction: column; gap: 2px; min-width: 0; flex: 1; }
.attachment-name { font-size: 13px; color: var(--ink); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.attachment-detail { font-size: 12px; color: var(--muted); }
.attachment-detail.program { color: var(--trouble); }
.attachment-actions { display: none; gap: 6px; padding: 0 10px 10px; }
.attachment-card:hover .attachment-actions { display: flex; }
.attachment-open {
    flex: 1; height: 28px;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--field); border-radius: 6px;
    background: var(--surface); color: var(--ink);
    font: inherit; font-size: 12px; cursor: pointer;
}
.attachment-save {
    width: 28px; height: 28px; margin-left: auto;
    display: flex; align-items: center; justify-content: center;
    border: 1px solid var(--field); border-radius: 6px;
    background: var(--surface); color: var(--ink); padding: 0; cursor: pointer;
}
.attachment-open:hover, .attachment-save:hover { background: var(--hover); }
"#;
