//! A calendar someone shared, as Google's "shared a calendar with you" mail
//! names it: the calendar's id, from the `cid=` of its link to Google
//! Calendar (base64, or the id as it is), and who shared it.
//!
//! Google sends it from the sharer (`From`) as itself (`Sender: Google
//! Calendar <calendar-notification@google.com>`). Only mail Google itself
//! sent and signed is read so, as Gmail found when it took the mail in
//! (`Authentication-Results: mx.google.com; dkim=pass header.i=@google.com`),
//! and never an invitation (whose description anyone writes). Anyone can
//! write such a link; adding a calendar a stranger named would fill the
//! person's calendar with theirs.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_NO_PAD, URL_SAFE_NO_PAD};
use mail_parser::{MessageParser, PartKind};
use postrinch_model::EmailAddress;

/// A calendar shared with the account, as its mail says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedCalendar {
    /// The calendar's id, to add it by.
    pub id: String,
    /// Who shared it: their name and address, as the mail's From gives.
    pub by: Option<String>,
}

fn google(address: Option<&str>) -> bool {
    address
        .and_then(|a| EmailAddress::parse(a).ok())
        .is_some_and(|a| a.domain() == "google.com")
}

/// The calendar a raw message shares, if it is Google's mail sharing one.
pub fn calendar_share(raw: &[u8]) -> Option<SharedCalendar> {
    let parsed = MessageParser::default().parse(raw)?;
    let from = parsed.from().and_then(|f| f.first());
    let sender = parsed.sender().and_then(|f| f.first());
    let sent_by_google = google(sender.as_ref().and_then(|s| s.address()))
        || google(from.as_ref().and_then(|f| f.address()));
    let invitation = parsed.parts().any(|p| {
        matches!(p.kind(), PartKind::Text { .. } | PartKind::Binary { .. })
            && p.content_type().is_some_and(|c| {
                c.subtype()
                    .is_some_and(|s| s.eq_ignore_ascii_case("calendar"))
            })
    });
    if !sent_by_google || invitation || !signed_by_google(&parsed) {
        return None;
    }
    let mut text = String::new();
    for body in parsed.html_bodies().chain(parsed.text_bodies()) {
        text.push_str(&body);
    }
    let id = calendar_link_id(&text)?;
    let by = from.filter(|f| !google(f.address())).map(|f| {
        match (
            f.name().map(str::trim).filter(|n| !n.is_empty()),
            f.address(),
        ) {
            (Some(name), Some(address)) => format!("{name} ({address})"),
            (Some(name), None) => name.to_owned(),
            (None, Some(address)) => address.to_owned(),
            (None, None) => String::new(),
        }
    });
    let by = by.filter(|b| !b.is_empty());
    Some(SharedCalendar { id, by })
}

/// Whether Gmail found the message signed by google.com: the topmost
/// Authentication-Results, the one Gmail's own server added (a sender can
/// write more below it, never above).
fn signed_by_google(parsed: &mail_parser::Message<'_>) -> bool {
    let Some(header) = parsed.headers().all("Authentication-Results").next() else {
        return false;
    };
    let t = String::from_utf8_lossy(header.raw_value()).to_ascii_lowercase();
    t.trim_start().starts_with("mx.google.com")
        && t.contains("dkim=pass")
        && (t.contains("header.i=@google.com") || t.contains("header.d=google.com"))
}

/// The calendar id in the first link to Google Calendar with a `cid=`.
fn calendar_link_id(text: &str) -> Option<String> {
    let mut rest = text;
    while let Some(at) = rest.find("cid=") {
        let before = &rest[..at];
        let after = &rest[at + 4..];
        rest = after;
        let link_start = before
            .rfind(|c: char| c == '"' || c == '\'' || c.is_whitespace() || c == '<' || c == '(')
            .map_or(0, |i| i + 1);
        if !before[link_start..].contains("calendar.google.com") {
            continue;
        }
        let value: String = after
            .chars()
            .take_while(|c| {
                !matches!(c, '&' | '"' | '\'' | '<' | '>' | ')' | '#') && !c.is_whitespace()
            })
            .collect();
        let value = value.replace("&amp;", "");
        if let Some(id) = decode(&value) {
            return Some(id);
        }
    }
    None
}

/// A `cid` value as a calendar id: base64 of it, or the id percent-encoded.
fn decode(value: &str) -> Option<String> {
    let plain: String = url_decode(value);
    let looks_like_id = |s: &str| s.contains('@') && !s.contains(char::is_whitespace);
    if looks_like_id(&plain) {
        return Some(plain);
    }
    let trimmed = plain.trim_end_matches('=');
    for engine in [&URL_SAFE_NO_PAD, &STANDARD_NO_PAD] {
        if let Ok(bytes) = engine.decode(trimmed)
            && let Ok(id) = String::from_utf8(bytes)
            && looks_like_id(&id)
        {
            return Some(id);
        }
    }
    None
}

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(from: &str, sender: &str, auth: &str, body: &str) -> String {
        format!(
            "Authentication-Results: mx.google.com;\r\n       {auth}\r\nSender: {sender}\r\n\
             From: {from}\r\nTo: me@example.org\r\n\
             Subject: Accept your invitation to join shared calendar: \"Family\"\r\n\
             MIME-Version: 1.0\r\nContent-Type: text/html; charset=UTF-8\r\n\r\n{body}\r\n"
        )
    }

    const GOOGLE: &str = "Google Calendar <calendar-notification@google.com>";
    const SIGNED: &str =
        "dkim=pass header.i=@google.com header.s=20260707 header.b=abc;\r\n       spf=none";
    // As Google wrote it on 2026-11-22: base64 of the id, then the
    // recipient's (`ctok`).
    const LINK: &str = "<a href=\"https://calendar.google.com/calendar/r?cid=am9lQGpvZWxlYXZlci5jb20&amp;ctok=amxlYXZlckBhcGV4dGVsZW1lZC5jb20&amp;es=3\">Join shared calendar</a>";

    #[test]
    fn reads_google_s_share_mail() {
        let raw = mail("Joe Leaver <joe@joeleaver.com>", GOOGLE, SIGNED, LINK);
        let shared = calendar_share(raw.as_bytes()).unwrap();
        assert_eq!(shared.id, "joe@joeleaver.com");
        assert_eq!(shared.by.as_deref(), Some("Joe Leaver (joe@joeleaver.com)"));
    }

    #[test]
    fn ids_as_they_are_and_group_calendars() {
        let link = "<a href=\"https://calendar.google.com/calendar/render?cid=abc123%40group.calendar.google.com\">Add</a>";
        let raw = mail(GOOGLE, GOOGLE, SIGNED, link);
        let shared = calendar_share(raw.as_bytes()).unwrap();
        assert_eq!(shared.id, "abc123@group.calendar.google.com");
        assert_eq!(shared.by, None);
    }

    #[test]
    fn only_mail_google_sent_and_signed() {
        let from = "Joe Leaver <joe@joeleaver.com>";
        // Unsigned, or signed by someone else, or not sent by Google.
        let unsigned = mail(from, GOOGLE, "dkim=none", LINK);
        assert!(calendar_share(unsigned.as_bytes()).is_none());
        let other = mail(from, GOOGLE, "dkim=pass header.i=@evil.example", LINK);
        assert!(calendar_share(other.as_bytes()).is_none());
        let stranger = mail(from, from, SIGNED, LINK);
        assert!(calendar_share(stranger.as_bytes()).is_none());
        // A cid in a link elsewhere is not one.
        let elsewhere = mail(
            from,
            GOOGLE,
            SIGNED,
            "<a href=\"https://evil.example/?cid=am9lQGpvZWxlYXZlci5jb20\">x</a>",
        );
        assert!(calendar_share(elsewhere.as_bytes()).is_none());
        // A lower Authentication-Results the sender wrote counts for nothing.
        let forged = mail(from, GOOGLE, "dkim=none", LINK).replacen(
            "Sender:",
            "Authentication-Results: mx.google.com; dkim=pass header.i=@google.com\r\nSender:",
            1,
        );
        assert!(calendar_share(forged.as_bytes()).is_none());
    }
}
