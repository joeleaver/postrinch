//! Choosing real mail from a store and writing each message as a case for
//! rinch's `html_compare`: as its sender wrote it, and as Postrinch cleans
//! it (with pictures from the web allowed, as "Show pictures" would).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use postrinch_commands::Commands;
use postrinch_render::{Options, original_html, render};
use postrinch_store::Store;
use rinch_html::Cleaned;
use rinch_visual_test::html_compare::case::{self, Meta};

use crate::Result;

/// Write up to `count` messages matching `query` (Postrinch's search
/// language) into `out/NNN/`, one sender at most `per_sender` times.
pub fn export(
    data_dir: &Path,
    out: &Path,
    query: &str,
    count: usize,
    per_sender: usize,
    width: u32,
) -> Result<Vec<PathBuf>> {
    let store = Store::open(data_dir)?;
    let commands = Commands::new(store.clone());
    let setup = crate::setup(width);
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
        let (Some(original), Some(rendered)) = (
            original_html(&raw),
            render(
                &raw,
                Options {
                    remote: true,
                    width: Some(setup.width as f32),
                },
            ),
        ) else {
            continue;
        };
        let from = message
            .headers
            .from
            .first()
            .map(|m| m.display_name())
            .unwrap_or_default();
        let meta = Meta {
            title: format!("{from} · {}", message.subject()),
            detail: format!("{account} · message {}", message.id.0),
        };
        let cleaned = Cleaned {
            html: rendered.html,
            body_style: rendered.body_style,
            ..Cleaned::default()
        };
        let dir = out.join(format!("{:03}", written.len()));
        case::write(&dir, &meta, &original, &cleaned, &setup)?;
        senders.push(sender);
        written.push(dir);
    }
    Ok(written)
}
