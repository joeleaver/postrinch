//! A message's attachments as files: their names, types and bytes.

use mail_parser::MessageParser;

/// One attachment, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub filename: Option<String>,
    pub mime_type: String,
    pub bytes: Vec<u8>,
}

/// The message's `index`th attachment, counted as `postrinch-sync` lists
/// a message's attachments (mail-parser's order, inline parts included).
/// `None` when the message does not parse or has fewer.
pub fn attachment(raw: &[u8], index: usize) -> Option<Part> {
    let parsed = MessageParser::default().parse(raw)?;
    let part = parsed.attachments().nth(index)?;
    Some(Part {
        filename: part.attachment_name().map(str::to_owned),
        mime_type: part
            .content_type()
            .map(|ct| match ct.subtype() {
                Some(sub) => format!("{}/{sub}", ct.ctype()),
                None => ct.ctype().to_owned(),
            })
            .unwrap_or_else(|| "application/octet-stream".to_owned())
            .to_ascii_lowercase(),
        bytes: part.decoded().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gives_an_attachment_s_bytes_by_its_place() {
        let raw = concat!(
            "From: a@example.org\r\nSubject: menu\r\nMIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=\"b\"\r\n\r\n",
            "--b\r\nContent-Type: text/plain\r\n\r\nSee attached.\r\n",
            "--b\r\nContent-Type: text/csv; name=\"a.csv\"\r\n",
            "Content-Disposition: attachment; filename=\"a.csv\"\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\nb25lLHR3bwo=\r\n",
            "--b--\r\n"
        );
        let part = attachment(raw.as_bytes(), 0).unwrap();
        assert_eq!(part.filename.as_deref(), Some("a.csv"));
        assert_eq!(part.mime_type, "text/csv");
        assert_eq!(part.bytes, b"one,two\n");
        assert!(attachment(raw.as_bytes(), 1).is_none());
    }
}
