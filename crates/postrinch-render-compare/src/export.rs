//! Choosing real mail from a store and writing each message out twice: as
//! its sender wrote it (`original.html`), and as Postrinch cleans it
//! (`cleaned.html`, plus `cleaned.json` for the probe), with `meta.json`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use postrinch_commands::Commands;
use postrinch_model::{EmailAddress, Message};
use postrinch_render::{HOST_CSS, Options, original_html, render};
use postrinch_store::Store;
use serde::{Deserialize, Serialize};

use crate::{Result, WIDTH};

/// One message chosen for comparing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meta {
    pub account: String,
    pub message: String,
    pub thread: String,
    pub from: String,
    pub subject: String,
    pub received: i64,
}

/// What the probe shows for a message: Postrinch's cleaned HTML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cleaned {
    pub html: String,
    pub body_style: String,
}

/// Write up to `count` messages matching `query` (Postrinch's search
/// language) into `out/NNN/`, one sender at most `per_sender` times.
pub fn export(
    data_dir: &Path,
    out: &Path,
    query: &str,
    count: usize,
    per_sender: usize,
) -> Result<Vec<PathBuf>> {
    let store = Store::open(data_dir)?;
    let commands = Commands::new(store.clone());
    // Ask for plenty: many matches are plain text or not downloaded.
    let results = commands.search(query, (count * 8).max(100))?;
    let mut senders: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut written = Vec::new();
    std::fs::create_dir_all(out)?;
    for row in results.threads {
        if written.len() >= count {
            break;
        }
        let account = row.summary.account.clone();
        let Some(thread) = commands.read_thread(&account, &row.summary.id)? else {
            continue;
        };
        // The latest downloaded message of the thread.
        let Some(message) = thread.messages.iter().rev().find(|m| m.downloaded) else {
            continue;
        };
        let sender = message
            .headers
            .from
            .first()
            .map(|m| m.address.key())
            .unwrap_or_default();
        if senders.iter().filter(|s| **s == sender).count() >= per_sender
            || !seen.insert(message.id.0.clone())
        {
            continue;
        }
        let Some(raw) = store.raw_message(&account, &message.id)? else {
            continue;
        };
        let Some(original) = original_html(&raw) else {
            continue;
        };
        let Some(cleaned) = render(&raw, Options { remote: true }) else {
            continue;
        };
        let dir = out.join(format!("{:03}", written.len()));
        write_message(&dir, &account, message, &original, &cleaned)?;
        senders.push(sender);
        written.push(dir);
    }
    Ok(written)
}

fn write_message(
    dir: &Path,
    account: &EmailAddress,
    message: &Message,
    original: &str,
    cleaned: &postrinch_render::Rendered,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let meta = Meta {
        account: account.to_string(),
        message: message.id.0.clone(),
        thread: message.thread.0.clone(),
        from: message
            .headers
            .from
            .first()
            .map(|m| m.display_name())
            .unwrap_or_default(),
        subject: message.subject().to_owned(),
        received: message.received.as_millis(),
    };
    std::fs::write(dir.join("meta.json"), serde_json::to_vec_pretty(&meta)?)?;
    std::fs::write(dir.join("original.html"), original)?;
    std::fs::write(dir.join("cleaned.html"), cleaned_page(cleaned))?;
    let probe = Cleaned {
        html: cleaned.html.clone(),
        body_style: cleaned.body_style.clone(),
    };
    std::fs::write(dir.join("cleaned.json"), serde_json::to_vec(&probe)?)?;
    Ok(())
}

/// Postrinch's cleaned HTML as a page for Chrome: in the window's card
/// (`HOST_CSS`), at the window's width, with the window's sans-serif
/// font, so Chrome and rinch get the same input and differ only in how
/// they lay it out.
fn cleaned_page(cleaned: &postrinch_render::Rendered) -> String {
    let fonts = crate::FONT_DIR;
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><style>
@font-face {{ font-family: 'Postrinch Sans'; src: url('file://{fonts}/IBMPlexSans-Regular.ttf'); font-weight: 400; }}
@font-face {{ font-family: 'Postrinch Sans'; src: url('file://{fonts}/IBMPlexSans-Medium.ttf'); font-weight: 500; }}
@font-face {{ font-family: 'Postrinch Sans'; src: url('file://{fonts}/IBMPlexSans-SemiBold.ttf'); font-weight: 600 900; }}
@font-face {{ font-family: 'Postrinch Sans'; src: url('file://{fonts}/IBMPlexSans-Italic.ttf'); font-style: italic; }}
html, body {{ margin: 0; background: #ffffff; }}
{HOST_CSS}
.mail-html {{ width: {WIDTH}px; box-sizing: border-box; border-radius: 0; font-family: 'Postrinch Sans', sans-serif; }}
</style></head><body><div class="mail-html"><div class="mail-body" style="{style}">{html}</div></div></body></html>"#,
        style = cleaned.body_style.replace('"', "&quot;"),
        html = cleaned.html,
    )
}
