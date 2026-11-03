//! From a raw RFC 5322 message to the model's [`Message`].
//!
//! The provider knows some things the message does not (its id, thread,
//! labels, when it arrived); those come in an [`Envelope`]. The rest is read
//! from the message itself. The raw bytes stay with the store for the
//! renderer; this only extracts what lists, threads and search need.

use mail_parser::{AddressList, MessageParser};
use postrinch_model::{
    AttachmentInfo, EmailAddress, Flags, Mailbox, Message, MessageHeaders, ProviderMessageId,
    ProviderThreadId, Timestamp,
};

use crate::{SyncError, SyncResult};

/// How long a snippet made from the body may be, in characters.
const SNIPPET_CHARS: usize = 200;

/// What the provider says about a message, beside the message itself.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub account: EmailAddress,
    pub id: ProviderMessageId,
    pub thread: ProviderThreadId,
    pub received: Timestamp,
    pub labels: Vec<String>,
    pub flags: Flags,
    /// The provider's preview, if it gives one.
    pub snippet: Option<String>,
}

/// Build a [`Message`] from its envelope and raw bytes.
pub fn parse_message(envelope: Envelope, raw: &[u8]) -> SyncResult<Message> {
    let parsed = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| SyncError::Mime(format!("message {} has no headers", envelope.id.0)))?;

    let body_text = parsed
        .text_bodies()
        .map(|b| one_kind_of_newline(b.trim()))
        .filter(|b| !b.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let attachments = parsed
        .attachments()
        .map(|part| AttachmentInfo {
            filename: part.attachment_name().map(str::to_owned),
            mime_type: part
                .content_type()
                .map(|ct| match ct.subtype() {
                    Some(sub) => format!("{}/{sub}", ct.ctype()),
                    None => ct.ctype().to_owned(),
                })
                .unwrap_or_else(|| "application/octet-stream".to_owned())
                .to_ascii_lowercase(),
            size: part.decoded_len() as u64,
            content_id: part
                .content_id()
                .map(|id| id.trim_matches(['<', '>']).to_owned()),
            inline: part.is_inline(),
        })
        .collect();

    let snippet = match &envelope.snippet {
        Some(s) if !s.trim().is_empty() => s.clone(),
        _ => snippet_from(&body_text),
    };

    Ok(Message {
        headers: headers(&parsed),
        snippet,
        body_text,
        attachments,
        size: raw.len() as u64,
        downloaded: true,
        ..from_envelope(envelope)
    })
}

/// Build a [`Message`] that is not downloaded yet, from its envelope and
/// its headers alone (`Name: value` lines ending in a blank line). `size`
/// is the provider's estimate of the whole message's.
pub fn parse_headers(envelope: Envelope, header_block: &[u8], size: u64) -> SyncResult<Message> {
    let parsed = MessageParser::default()
        .parse(header_block)
        .ok_or_else(|| SyncError::Mime(format!("message {} has no headers", envelope.id.0)))?;
    Ok(Message {
        headers: headers(&parsed),
        snippet: envelope.snippet.clone().unwrap_or_default(),
        size,
        ..from_envelope(envelope)
    })
}

/// A message with what the envelope says and nothing downloaded.
fn from_envelope(envelope: Envelope) -> Message {
    Message {
        account: envelope.account,
        id: envelope.id,
        thread: envelope.thread,
        headers: MessageHeaders::default(),
        received: envelope.received,
        labels: envelope.labels,
        flags: envelope.flags,
        snippet: String::new(),
        body_text: String::new(),
        attachments: Vec::new(),
        size: 0,
        downloaded: false,
    }
}

fn headers(parsed: &mail_parser::Message<'_>) -> MessageHeaders {
    let mut headers = MessageHeaders {
        subject: parsed.subject().map(str::to_owned),
        from: mailboxes(parsed.from()),
        sender: mailboxes(parsed.sender()).into_iter().next(),
        reply_to: mailboxes(parsed.reply_to()),
        to: mailboxes(parsed.to()),
        cc: mailboxes(parsed.cc()),
        bcc: mailboxes(parsed.bcc()),
        date: parsed
            .date()
            .map(|d| Timestamp::from_millis(d.to_timestamp().saturating_mul(1000))),
        ..Default::default()
    };
    if let Some(id) = parsed.message_id() {
        headers.set_message_id(id);
    }
    if let Some(list) = parsed.in_reply_to() {
        headers.set_in_reply_to(&list.iter().collect::<Vec<_>>().join(" "));
    }
    if let Some(list) = parsed.references() {
        headers.set_references(&list.iter().collect::<Vec<_>>().join(" "));
    }
    headers
}

/// The mailboxes of an address header, skipping entries without a usable
/// address (an empty group, a bare name).
fn mailboxes(list: Option<AddressList<'_>>) -> Vec<Mailbox> {
    let Some(list) = list else {
        return Vec::new();
    };
    list.mailboxes()
        .filter_map(|m| {
            let address = EmailAddress::parse(m.address()?).ok()?;
            Some(Mailbox::new(m.name().map(str::to_owned), address))
        })
        .collect()
}

/// Text with every line ending as `\n`: mail is sent with CRLF, and a
/// display shows a lone CR as a line break of its own.
fn one_kind_of_newline(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The start of the body on one line, cut at a word.
fn snippet_from(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SNIPPET_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(SNIPPET_CHARS).collect();
    match cut.rfind(' ') {
        Some(space) => format!("{}…", &cut[..space]),
        None => format!("{cut}…"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(snippet: Option<&str>) -> Envelope {
        Envelope {
            account: EmailAddress::parse("me@example.org").unwrap(),
            id: ProviderMessageId("m1".into()),
            thread: ProviderThreadId("t1".into()),
            received: Timestamp::from_millis(5),
            labels: vec!["INBOX".into()],
            flags: Flags::default(),
            snippet: snippet.map(str::to_owned),
        }
    }

    const MIXED: &str = concat!(
        "From: Ada Lovelace <ada@example.org>\r\n",
        "To: me@example.org, \"Bob\" <bob@example.org>\r\n",
        "Cc: Friends: carol@example.org;\r\n",
        "Subject: =?utf-8?q?Caf=C3=A9_plans?=\r\n",
        "Date: Sat, 20 Nov 2021 14:22:01 -0800\r\n",
        "Message-ID: <abc@mail.example.org>\r\n",
        "In-Reply-To: <parent@example.org>\r\n",
        "References: <root@example.org> <parent@example.org>\r\n",
        "Content-Type: multipart/mixed; boundary=\"b\"\r\n",
        "\r\n",
        "--b\r\n",
        "Content-Type: text/html; charset=utf-8\r\n",
        "\r\n",
        "<p>Shall we meet at <b>noon</b>?</p>\r\n",
        "--b\r\n",
        "Content-Type: application/pdf; name=\"menu.pdf\"\r\n",
        "Content-Disposition: attachment; filename=\"menu.pdf\"\r\n",
        "Content-Transfer-Encoding: base64\r\n",
        "\r\n",
        "JVBERi0xLjQK\r\n",
        "--b--\r\n",
    );

    #[test]
    fn body_lines_end_in_newlines_alone() {
        let raw = concat!(
            "From: ada@example.org\r\n",
            "Subject: lines\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "one\r\ntwo\r\n\r\nthree\rfour\r\n",
        );
        let m = parse_message(envelope(None), raw.as_bytes()).unwrap();
        assert_eq!(m.body_text, "one\ntwo\n\nthree\nfour");
    }

    #[test]
    fn reads_headers_body_and_attachments() {
        let m = parse_message(envelope(None), MIXED.as_bytes()).unwrap();
        let h = &m.headers;
        assert_eq!(h.subject.as_deref(), Some("Café plans"));
        assert_eq!(h.from[0].to_string(), "Ada Lovelace <ada@example.org>");
        let to: Vec<_> = h.to.iter().map(|m| m.address.to_string()).collect();
        assert_eq!(to, ["me@example.org", "bob@example.org"]);
        assert_eq!(h.cc[0].address.to_string(), "carol@example.org");
        assert_eq!(h.message_id.as_deref(), Some("abc@mail.example.org"));
        assert_eq!(h.in_reply_to.as_deref(), Some("parent@example.org"));
        assert_eq!(h.references, ["root@example.org", "parent@example.org"]);
        assert_eq!(h.date, Some(Timestamp::from_millis(1_637_446_921_000)));

        assert!(
            m.body_text.contains("Shall we meet at noon?"),
            "{:?}",
            m.body_text
        );
        assert_eq!(m.snippet, "Shall we meet at noon?");
        assert_eq!(m.attachments.len(), 1);
        let pdf = &m.attachments[0];
        assert_eq!(pdf.filename.as_deref(), Some("menu.pdf"));
        assert_eq!(pdf.mime_type, "application/pdf");
        assert_eq!(pdf.size, 9);
        assert!(!pdf.inline);
        assert_eq!(m.size, MIXED.len() as u64);
        assert_eq!(m.labels, ["INBOX"]);
    }

    #[test]
    fn provider_snippet_wins() {
        let m = parse_message(envelope(Some("from Gmail")), MIXED.as_bytes()).unwrap();
        assert_eq!(m.snippet, "from Gmail");
    }

    #[test]
    fn long_snippets_cut_at_a_word() {
        let body = "word ".repeat(100);
        let s = snippet_from(&body);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= SNIPPET_CHARS + 1);
        assert!(!s.contains("  "));
    }

    #[test]
    fn headers_alone_make_an_undownloaded_message() {
        let block = b"From: Ada Lovelace <ada@example.org>\r\nSubject: =?utf-8?q?Caf=C3=A9?=\r\n\
                      Message-ID: <abc@x>\r\n\r\n";
        let m = parse_headers(envelope(Some("preview")), block, 4321).unwrap();
        assert!(!m.downloaded);
        assert_eq!(m.headers.subject.as_deref(), Some("Café"));
        assert_eq!(
            m.headers.from[0].to_string(),
            "Ada Lovelace <ada@example.org>"
        );
        assert_eq!(m.headers.message_id.as_deref(), Some("abc@x"));
        assert_eq!(m.snippet, "preview");
        assert_eq!(m.size, 4321);
        assert!(m.body_text.is_empty() && m.attachments.is_empty());
    }

    #[test]
    fn plain_message_without_mime() {
        let raw = b"From: x@example.org\r\nSubject: hi\r\n\r\nJust text.\r\n";
        let m = parse_message(envelope(None), raw).unwrap();
        assert_eq!(m.body_text, "Just text.");
        assert!(m.attachments.is_empty());
    }
}
