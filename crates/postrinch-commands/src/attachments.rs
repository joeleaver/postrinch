//! Attachments: a message's, as files, and a draft's, kept in the store's
//! blob directory until the mail they belong to has gone.

use postrinch_model::{
    AttachmentOrigin, Draft, DraftAttachment, EmailAddress, MAX_ATTACHED, Message,
    ProviderMessageId,
};
use postrinch_render::Part;

use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// A downloaded message's `index`th attachment (its place in
    /// [`Message::attachments`]), decoded. `None` when the message is not
    /// downloaded or has no such attachment.
    pub fn attachment(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
        index: usize,
    ) -> CommandResult<Option<Part>> {
        self.check(account)?;
        let Some(raw) = self.store.raw_message(account, message)? else {
            return Ok(None);
        };
        Ok(postrinch_render::attachment(&raw, index))
    }

    /// Keep `bytes` as a file for a draft.
    pub fn attach_bytes(
        &self,
        name: &str,
        mime_type: Option<&str>,
        bytes: &[u8],
        origin: AttachmentOrigin,
    ) -> CommandResult<DraftAttachment> {
        let name = file_name(name);
        let mime_type = mime_type
            .map(str::to_owned)
            .unwrap_or_else(|| mime_for(&name).to_owned());
        Ok(DraftAttachment {
            blob: self.store.put_attachment(bytes)?,
            size: bytes.len() as u64,
            name,
            mime_type,
            origin,
        })
    }

    /// A message's `index`th attachment, kept as a file for a draft.
    pub fn attach_from_message(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
        index: usize,
        origin: AttachmentOrigin,
    ) -> CommandResult<Option<DraftAttachment>> {
        let Some(part) = self.attachment(account, message, index)? else {
            return Ok(None);
        };
        let name = part.filename.clone().unwrap_or_else(|| "attachment".into());
        self.attach_bytes(&name, Some(&part.mime_type), &part.bytes, origin)
            .map(Some)
    }

    /// The files a message carries (not the pictures its HTML shows
    /// inline), kept for a draft: what a forward or a kept draft brings.
    pub(crate) fn carried(
        &self,
        message: &Message,
        origin: AttachmentOrigin,
    ) -> CommandResult<Vec<DraftAttachment>> {
        let mut out = Vec::new();
        for (index, info) in message.attachments.iter().enumerate() {
            if info.inline {
                continue;
            }
            if let Some(a) =
                self.attach_from_message(&message.account, &message.id, index, origin.clone())?
            {
                out.push(a);
            }
        }
        Ok(out)
    }

    /// A draft attachment's bytes.
    pub fn draft_attachment_bytes(&self, attachment: &DraftAttachment) -> CommandResult<Vec<u8>> {
        self.store
            .attachment_bytes(&attachment.blob)?
            .ok_or_else(|| CommandError::Invalid(format!("{} is no longer here", attachment.name)))
    }

    /// Refuse a draft whose attachments the provider would refuse.
    pub(crate) fn check_attached(&self, draft: &Draft) -> CommandResult<()> {
        if draft.attached() > MAX_ATTACHED {
            return Err(CommandError::Invalid(format!(
                "{} attached: Gmail sends up to {}",
                size_text(draft.attached()),
                size_text(MAX_ATTACHED)
            )));
        }
        Ok(())
    }
}

/// A name for a file from a path or a sender's name: its last part, with
/// nothing that would make it a path again.
pub fn file_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name).trim();
    let clean: String = last
        .chars()
        .map(|c| if c.is_control() { '_' } else { c })
        .collect();
    match clean.trim_matches('.') {
        "" => "attachment".to_owned(),
        _ => clean,
    }
}

/// A size as people read one: 84 KB, 4.6 MB.
pub fn size_text(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} bytes")
    } else if b < KB * KB {
        format!("{:.0} KB", (b / KB).max(1.0))
    } else if b < KB * KB * KB {
        format!("{} MB", one_place(b / KB / KB))
    } else {
        format!("{} GB", one_place(b / KB / KB / KB))
    }
}

/// A number to one decimal place, without a trailing `.0`.
fn one_place(n: f64) -> String {
    let text = format!("{n:.1}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

/// The type a file's name suggests.
pub fn mime_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "txt" | "text" | "log" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "heic" => "image/heic",
        "tif" | "tiff" => "image/tiff",
        "zip" => "application/zip",
        "gz" => "application/gzip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// Whether a file is a program or script, which Postrinch never opens:
/// mail from strangers carries them to be run.
pub fn is_program(name: &str, mime_type: &str) -> bool {
    let ext = name
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    const EXTS: &[&str] = &[
        "exe", "msi", "bat", "cmd", "com", "scr", "pif", "vbs", "vbe", "js", "jse", "wsf", "wsh",
        "ps1", "psm1", "sh", "bash", "zsh", "csh", "ksh", "fish", "command", "run", "bin", "out",
        "appimage", "desktop", "jar", "py", "pyw", "pl", "rb", "php", "lnk", "reg", "deb", "rpm",
        "apk", "app", "dmg", "pkg", "elf", "so", "dll", "hta", "cpl", "msc", "url", "iso", "img",
        "docm", "xlsm", "pptm", "xlam",
    ];
    EXTS.contains(&ext.as_str())
        || ext.is_empty() && mime_type == "application/octet-stream"
        || mime_type.contains("executable")
        || mime_type.contains("x-sh")
        || mime_type.contains("x-msdownload")
        || mime_type.contains("javascript")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_stay_names() {
        assert_eq!(file_name("../../etc/passwd"), "passwd");
        assert_eq!(file_name("C:\\x\\a.pdf"), "a.pdf");
        assert_eq!(file_name(".."), "attachment");
        assert_eq!(file_name("a\nb.txt"), "a_b.txt");
    }

    #[test]
    fn programs_are_told_apart() {
        assert!(is_program("fonts-install.sh", "text/plain"));
        assert!(is_program("setup.EXE", "application/octet-stream"));
        assert!(is_program("blob", "application/octet-stream"));
        assert!(!is_program("proofs.pdf", "application/pdf"));
        assert!(!is_program("cover.jpg", "image/jpeg"));
    }

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(size_text(86_016), "84 KB");
        assert_eq!(size_text(4_823_449), "4.6 MB");
        assert_eq!(size_text(25 * 1024 * 1024), "25 MB");
    }
}
