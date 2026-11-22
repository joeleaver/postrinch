//! A message's attachments in the window, after the attachments mockup:
//! opened in the app the system has for them, saved where the person says,
//! and pictures shown small. A program or script is never opened from here,
//! only saved: mail from strangers carries them to be run.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use postrinch_commands::{Commands, Part, Thread, file_name, is_program};
use postrinch_model::{AttachmentInfo, Message, ProviderMessageId};

use crate::actions::commands;
use crate::state::State;

/// Pictures larger than this show as an icon, not a thumbnail.
const MAX_THUMBNAIL: u64 = 8 * 1024 * 1024;

/// The key of a message's attachment among the thumbnails.
pub fn key(message: &ProviderMessageId, index: usize) -> String {
    format!("{}#{index}", message.0)
}

/// Whether the window can draw a picture of this type.
pub fn is_picture(info: &AttachmentInfo) -> bool {
    matches!(
        info.mime_type.as_str(),
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp"
    )
}

/// The thread's attached pictures as `data:` URLs, by [`key`]. Run on the
/// loader thread, as decoding them takes a moment.
pub fn thumbnails(commands: &Commands, thread: &Thread) -> HashMap<String, String> {
    let account = &thread.summary.account;
    let mut out = HashMap::new();
    for message in thread.messages.iter().filter(|m| m.downloaded) {
        for (index, info) in message.attachments.iter().enumerate() {
            if info.inline || !is_picture(info) || info.size > MAX_THUMBNAIL {
                continue;
            }
            if let Ok(Some(part)) = commands.attachment(account, &message.id, index) {
                let url = format!(
                    "data:{};base64,{}",
                    info.mime_type,
                    STANDARD.encode(&part.bytes)
                );
                out.insert(key(&message.id, index), url);
            }
        }
    }
    out
}

/// The name an attachment is saved under.
pub fn name_of(info: &AttachmentInfo) -> String {
    file_name(info.filename.as_deref().unwrap_or("attachment"))
}

impl State {
    fn attachment_part(self, message: &Message, index: usize) -> Option<Part> {
        let commands = commands()?;
        match commands.attachment(&message.account, &message.id, index) {
            Ok(Some(part)) => Some(part),
            Ok(None) => {
                self.error
                    .set(Some("That attachment is not on this computer yet.".into()));
                None
            }
            Err(e) => {
                self.error.set(Some(e.to_string()));
                None
            }
        }
    }

    /// Open a message's attachment in the system's app for it, from a copy
    /// in Postrinch's cache. Programs are saved instead.
    pub fn open_attachment(self, message: &Message, index: usize) {
        let Some(info) = message.attachments.get(index) else {
            return;
        };
        let name = name_of(info);
        if is_program(&name, &info.mime_type) {
            self.save_attachment(message, index);
            return;
        }
        let Some(part) = self.attachment_part(message, index) else {
            return;
        };
        let opened = (|| -> io::Result<PathBuf> {
            let dir = opened_dir()?.join(folder_for(&message.id));
            fs::create_dir_all(&dir)?;
            let path = dir.join(&name);
            write_read_only(&path, &part.bytes)?;
            Ok(path)
        })();
        match opened {
            Ok(path) => {
                if let Err(e) = open::that_detached(&path) {
                    self.error.set(Some(format!("{name} would not open: {e}")));
                }
            }
            Err(e) => self.error.set(Some(format!("{name} would not open: {e}"))),
        }
    }

    /// Save a message's attachment where the person picks.
    pub fn save_attachment(self, message: &Message, index: usize) {
        let Some(info) = message.attachments.get(index) else {
            return;
        };
        let name = name_of(info);
        let Some(path) = rinch::dialogs::save_file().set_file_name(&name).save() else {
            return;
        };
        let Some(part) = self.attachment_part(message, index) else {
            return;
        };
        if let Err(e) = fs::write(&path, &part.bytes) {
            self.error.set(Some(format!("{name} was not saved: {e}")));
        }
    }

    /// Save every file a message carries into a folder the person picks,
    /// next to what is there rather than over it.
    pub fn save_attachments(self, message: &Message) {
        let Some(folder) = rinch::dialogs::pick_folder().pick() else {
            return;
        };
        for (index, info) in message.attachments.iter().enumerate() {
            if !info.is_file() {
                continue;
            }
            let Some(part) = self.attachment_part(message, index) else {
                return;
            };
            let path = free_path(&folder, &name_of(info));
            if let Err(e) = fs::write(&path, &part.bytes) {
                self.error
                    .set(Some(format!("{} was not saved: {e}", name_of(info))));
                return;
            }
        }
    }
}

/// Where opened attachments are copied: the cache, which the system may
/// clear.
fn opened_dir() -> io::Result<PathBuf> {
    dirs::cache_dir()
        .map(|d| d.join("postrinch").join("opened"))
        .ok_or_else(|| io::Error::other("this system has no cache folder"))
}

/// A folder name for a message's id: its letters and digits.
fn folder_for(message: &ProviderMessageId) -> String {
    let name: String = message
        .0
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(64)
        .collect();
    if name.is_empty() {
        "message".into()
    } else {
        name
    }
}

/// Write a copy the opening app cannot change: edits there would be lost
/// with the cache.
fn write_read_only(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if path.exists() {
        let mut perms = fs::metadata(path)?.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(path, perms)?;
    }
    fs::write(path, bytes)?;
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_readonly(true);
    fs::set_permissions(path, perms)
}

/// `name` in `folder`, numbered (`a (2).pdf`) when that is taken.
fn free_path(folder: &Path, name: &str) -> PathBuf {
    let first = folder.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    (2..)
        .map(|n| folder.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some number is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_next_to_a_file_numbers_the_new_one() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_path(dir.path(), "a.pdf"), dir.path().join("a.pdf"));
        fs::write(dir.path().join("a.pdf"), b"x").unwrap();
        assert_eq!(free_path(dir.path(), "a.pdf"), dir.path().join("a (2).pdf"));
        fs::write(dir.path().join("notes"), b"x").unwrap();
        assert_eq!(free_path(dir.path(), "notes"), dir.path().join("notes (2)"));
    }

    #[test]
    fn a_message_id_makes_a_plain_folder() {
        assert_eq!(
            folder_for(&ProviderMessageId("../18f3:ab".into())),
            "18f3ab"
        );
        assert_eq!(folder_for(&ProviderMessageId("..".into())), "message");
    }
}
