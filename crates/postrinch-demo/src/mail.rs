//! Writing a demo message as RFC 5322 text, with MIME parts, as a mail
//! program would have sent it. Text parts are base64, so a long line or a
//! word outside ASCII never needs care; headers outside ASCII are encoded
//! words (RFC 2047).

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use jiff::Zoned;

use crate::people::Person;
use crate::threads::{Body, MessageSpec};

/// One message, ready to send: what it says, and who it answers.
pub struct Outgoing<'a> {
    pub spec: &'a MessageSpec,
    pub subject: &'a str,
    pub date: &'a Zoned,
    /// Its own `Message-ID`, without angle brackets.
    pub message_id: &'a str,
    /// The thread's earlier messages' ids, oldest first.
    pub earlier: &'a [String],
}

/// The whole message, headers and body.
pub fn render(out: &Outgoing) -> Vec<u8> {
    let spec = out.spec;
    let mut head = String::new();
    header(&mut head, "Message-ID", &format!("<{}>", out.message_id));
    header(
        &mut head,
        "Date",
        &jiff::fmt::rfc2822::to_string(out.date).expect("the demo's dates are in range"),
    );
    header(&mut head, "From", &mailbox(&spec.from));
    header(&mut head, "To", &mailboxes(&spec.to));
    if !spec.cc.is_empty() {
        header(&mut head, "Cc", &mailboxes(&spec.cc));
    }
    header(&mut head, "Subject", &encoded(out.subject));
    if let Some(last) = out.earlier.last() {
        header(&mut head, "In-Reply-To", &format!("<{last}>"));
        let all: Vec<String> = out.earlier.iter().map(|id| format!("<{id}>")).collect();
        header(&mut head, "References", &all.join(" "));
    }
    header(&mut head, "MIME-Version", "1.0");

    let mut boundaries = Boundaries(0);
    let body = body_part(spec, &mut boundaries);
    let whole = if spec.attachments.is_empty() {
        body
    } else {
        let mut parts = vec![body];
        for file in &spec.attachments {
            parts.push(Part::leaf(
                &format!("{}; name=\"{}\"", file.mime, file.name),
                Some(&format!("attachment; filename=\"{}\"", file.name)),
                &file.bytes,
            ));
        }
        Part::multipart("mixed", parts, &mut boundaries)
    };
    head.push_str(&whole.head);
    head.push_str("\r\n");
    head.push_str(&whole.body);
    head.into_bytes()
}

/// The readable part: text, text with HTML beside it, and an invitation's
/// calendar beside them.
fn body_part(spec: &MessageSpec, boundaries: &mut Boundaries) -> Part {
    let mut parts = match &spec.body {
        Body::Text(text) => vec![Part::text("plain", text)],
        Body::Html { text, html } => vec![Part::text("plain", text), Part::text("html", html)],
    };
    if let Some(ics) = &spec.invitation {
        parts.push(Part::leaf(
            "text/calendar; charset=\"utf-8\"; method=REQUEST",
            None,
            ics.as_bytes(),
        ));
    }
    match parts.len() {
        1 => parts.remove(0),
        _ => Part::multipart("alternative", parts, boundaries),
    }
}

/// A MIME part: its own headers, then its body.
struct Part {
    head: String,
    body: String,
}

impl Part {
    /// A text part, its line breaks made CRLF first as MIME's canonical
    /// form for text is (RFC 2045 §6.8).
    fn text(subtype: &str, text: &str) -> Self {
        let canonical = text.replace("\r\n", "\n").replace('\n', "\r\n");
        Self::leaf(
            &format!("text/{subtype}; charset=\"utf-8\""),
            None,
            canonical.as_bytes(),
        )
    }

    fn leaf(content_type: &str, disposition: Option<&str>, bytes: &[u8]) -> Self {
        let mut head = String::new();
        header(&mut head, "Content-Type", content_type);
        if let Some(disposition) = disposition {
            header(&mut head, "Content-Disposition", disposition);
        }
        header(&mut head, "Content-Transfer-Encoding", "base64");
        Self {
            head,
            body: base64_lines(bytes),
        }
    }

    fn multipart(subtype: &str, parts: Vec<Part>, boundaries: &mut Boundaries) -> Self {
        let boundary = boundaries.next();
        let mut head = String::new();
        header(
            &mut head,
            "Content-Type",
            &format!("multipart/{subtype}; boundary=\"{boundary}\""),
        );
        let mut body = String::new();
        for part in parts {
            body.push_str(&format!("--{boundary}\r\n"));
            body.push_str(&part.head);
            body.push_str("\r\n");
            body.push_str(&part.body);
        }
        body.push_str(&format!("--{boundary}--\r\n"));
        Self { head, body }
    }
}

/// Boundaries unique within one message.
struct Boundaries(u32);

impl Boundaries {
    fn next(&mut self) -> String {
        self.0 += 1;
        format!("=_demo_part_{}", self.0)
    }
}

fn header(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push_str(": ");
    out.push_str(value);
    out.push_str("\r\n");
}

fn mailbox(person: &Person) -> String {
    let name = if person.name.is_ascii() {
        format!("\"{}\"", person.name.replace(['"', '\\'], ""))
    } else {
        encoded(person.name)
    };
    // An encoded name ends its own line, so the address after it never
    // takes that line past RFC 2047's 76 characters.
    let gap = if person.name.is_ascii() { " " } else { "\r\n " };
    format!("{name}{gap}<{}>", person.address)
}

/// One mailbox to a line, so no encoded name shares one with another.
fn mailboxes(people: &[Person]) -> String {
    people
        .iter()
        .map(mailbox)
        .collect::<Vec<_>>()
        .join(",\r\n ")
}

/// The most bytes of text one encoded word carries: 39 bytes are 52 of
/// base64, and with `=?utf-8?B?` and `?=` the word is 64 characters. After
/// the longest header name the demo writes ("Subject: "), the line is 73,
/// inside RFC 2047's 76.
const WORD_BYTES: usize = 39;

/// `text` as it may stand in a header: as it is when it is ASCII, else
/// RFC 2047 encoded words, each whole characters, on folded lines.
fn encoded(text: &str) -> String {
    if text.is_ascii() {
        return text.to_owned();
    }
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in text.chars() {
        if chunk.len() + c.len_utf8() > WORD_BYTES {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    words.push(chunk);
    words
        .iter()
        .map(|w| format!("=?utf-8?B?{}?=", STANDARD.encode(w)))
        .collect::<Vec<_>>()
        .join("\r\n ")
}

/// Base64 in lines of 76, each ending in CRLF.
fn base64_lines(bytes: &[u8]) -> String {
    let encoded = STANDARD.encode(bytes);
    let mut out = String::with_capacity(encoded.len() + encoded.len() / 38);
    for line in encoded.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ago;
    use crate::people::{INES, PRIYA, SAM_WORK, THEO};
    use crate::threads::{Attachment, msg};

    fn parsed(spec: &MessageSpec, earlier: &[String]) -> postrinch_model::Message {
        parse(&raw(spec, "Café menu — round 2", earlier))
    }

    fn raw(spec: &MessageSpec, subject: &str, earlier: &[String]) -> Vec<u8> {
        let date: Zoned = "2026-11-24T10:00[Europe/Lisbon]".parse().unwrap();
        render(&Outgoing {
            spec,
            subject,
            date: &date,
            message_id: "m2@harborcoffee.example",
            earlier,
        })
    }

    fn parse(raw: &[u8]) -> postrinch_model::Message {
        let date: Zoned = "2026-11-24T10:00[Europe/Lisbon]".parse().unwrap();
        let envelope = postrinch_sync::Envelope {
            account: SAM_WORK.email(),
            id: postrinch_model::ProviderMessageId("m2".into()),
            thread: postrinch_model::ProviderThreadId("m1".into()),
            received: crate::clock::timestamp(&date),
            labels: Vec::new(),
            flags: Default::default(),
            snippet: None,
        };
        postrinch_sync::parse_message(envelope, raw).unwrap()
    }

    #[test]
    fn reads_back_as_sent() {
        let spec = msg(
            PRIYA,
            &[SAM_WORK],
            ago(5),
            "Hi Sam,\n\nThe new menu, attached.",
        )
        .cc(&[THEO])
        .attach(Attachment::new(
            "menu.csv",
            "text/csv",
            b"item,price\nflat white,4\n",
        ));
        let earlier = vec!["m1@lindenworks.example".to_owned()];
        let message = parsed(&spec, &earlier);
        let h = &message.headers;
        assert_eq!(h.subject.as_deref(), Some("Café menu — round 2"));
        assert_eq!(h.from[0].name.as_deref(), Some("Priya Nand"));
        assert_eq!(h.cc[0].address, THEO.email());
        assert_eq!(h.in_reply_to.as_deref(), Some("m1@lindenworks.example"));
        assert_eq!(message.body_text, "Hi Sam,\n\nThe new menu, attached.");
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].filename.as_deref(), Some("menu.csv"));
        assert_eq!(message.attachments[0].size, 24);
    }

    #[test]
    fn long_encoded_headers_keep_their_lines_short() {
        let subject = "Inês Duarte — notas da revisão do logótipo e das embalagens, ronda 2";
        let spec = msg(INES, &[SAM_WORK, INES, THEO], ago(5), "Olá.");
        let raw = raw(&spec, subject, &[]);
        let text = String::from_utf8(raw.clone()).unwrap();
        let head = &text[..text.find("\r\n\r\n").unwrap()];
        for line in head.split("\r\n").filter(|l| l.contains("=?")) {
            assert!(line.len() <= 76, "{} long: {line}", line.len());
        }
        let message = parse(&raw);
        assert_eq!(message.headers.subject.as_deref(), Some(subject));
        assert_eq!(message.headers.from[0].name.as_deref(), Some(INES.name));
        assert_eq!(message.headers.to.len(), 3);
        assert_eq!(message.headers.to[1].address, INES.email());
    }

    #[test]
    fn an_invitation_carries_its_calendar() {
        let ics = "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\nEND:VCALENDAR\r\n";
        let spec = msg(PRIYA, &[SAM_WORK], ago(5), "Join us.").invite(ics.to_owned());
        let message = parsed(&spec, &[]);
        assert!(postrinch_commands::may_invite(&message));
    }
}
