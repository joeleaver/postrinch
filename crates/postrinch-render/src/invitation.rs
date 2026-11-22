//! The invitation a message carries: its `text/calendar` part (or an
//! attached `.ics`), read as the event it describes.
//!
//! Times written in a named time zone are read in that zone when it is one
//! the system knows (IANA names, as Google and Apple write); others (the
//! Windows names Outlook writes) are read as the person's own time. The
//! person's calendar's copy of the event, when there is one, is shown
//! instead (DESIGN.md "Calendar").

use jiff::civil::{Date, DateTime};
use jiff::tz::TimeZone;
use jiff::{SignedDuration, ToSpan};
use mail_parser::MessageParser;
use postrinch_model::{
    Answer, Attendee, Day, EmailAddress, EventWhen, Invitation, InvitationKind, Mailbox, Timestamp,
};

use crate::ics::{self, Component, Property};

/// The invitation in a raw message, reading floating times in `local`.
/// `None` when it has no calendar part, or none with an event.
pub fn invitation(raw: &[u8], local: &TimeZone) -> Option<Invitation> {
    let parsed = MessageParser::default().parse(raw)?;
    let mut found = None;
    for part in parsed.parts() {
        let Some(ct) = part.content_type() else {
            continue;
        };
        let kind = (
            ct.ctype().to_ascii_lowercase(),
            ct.subtype().map(str::to_ascii_lowercase),
        );
        let calendar = match kind {
            (t, Some(s)) if t == "text" && s == "calendar" => true,
            (t, Some(s)) if t == "application" && s == "ics" => true,
            _ => false,
        };
        if !calendar {
            continue;
        }
        let text = String::from_utf8_lossy(&part.decoded()).into_owned();
        if let Some(invitation) = from_text(&text, local) {
            // The inline part says what it asks (its METHOD); an attached
            // copy sometimes leaves that out, so the first with one wins.
            let asks = ct.attribute("method").is_some();
            found = Some(invitation);
            if asks {
                break;
            }
        }
    }
    found
}

/// The first event in iCalendar text, as an invitation.
pub(crate) fn from_text(text: &str, local: &TimeZone) -> Option<Invitation> {
    let calendar = ics::parse(text)
        .into_iter()
        .find(|c| c.name == "VCALENDAR")?;
    let kind = match calendar
        .get("METHOD")
        .map(|m| m.value.trim().to_ascii_uppercase())
        .as_deref()
    {
        Some("CANCEL") => InvitationKind::Cancel,
        Some("REPLY") => InvitationKind::Reply,
        Some("REQUEST") => InvitationKind::Request,
        _ => InvitationKind::Publish,
    };
    let event = calendar.children("VEVENT").next()?;
    let uid = event.get("UID")?.value.trim().to_owned();
    if uid.is_empty() {
        return None;
    }
    let cancelled = event
        .get("STATUS")
        .is_some_and(|s| s.value.trim().eq_ignore_ascii_case("CANCELLED"));
    Some(Invitation {
        kind: if cancelled {
            InvitationKind::Cancel
        } else {
            kind
        },
        uid,
        recurrence: event
            .get("RECURRENCE-ID")
            .map(|r| r.value.trim().to_owned()),
        summary: text_of(event, "SUMMARY"),
        location: text_of(event, "LOCATION"),
        when: when(event, local),
        repeats: event.get("RRULE").is_some() || event.get("RDATE").is_some(),
        organizer: event.get("ORGANIZER").and_then(mailbox),
        attendees: event
            .all("ATTENDEE")
            .filter_map(|a| {
                Some(Attendee {
                    mailbox: mailbox(a)?,
                    answer: a.param("PARTSTAT").and_then(answer),
                })
            })
            .collect(),
    })
}

fn text_of(event: &Component, name: &str) -> Option<String> {
    let text = event.get(name)?.text();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// `CN=Name:mailto:address` as a mailbox.
fn mailbox(p: &Property) -> Option<Mailbox> {
    let value = p.value.trim();
    let address = match value.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => &value[7..],
        _ => value,
    };
    let address = EmailAddress::parse(address.trim()).ok()?;
    Some(Mailbox::new(p.param("CN").map(str::to_owned), address))
}

fn answer(partstat: &str) -> Option<Answer> {
    match partstat.trim().to_ascii_uppercase().as_str() {
        "ACCEPTED" => Some(Answer::Yes),
        "TENTATIVE" => Some(Answer::Maybe),
        "DECLINED" => Some(Answer::No),
        _ => None,
    }
}

/// When the event is: `DTSTART` until `DTEND`, or for its `DURATION`.
fn when(event: &Component, local: &TimeZone) -> Option<EventWhen> {
    let start = moment(event.get("DTSTART")?, local)?;
    let end = event.get("DTEND").and_then(|e| moment(e, local));
    let length = event.get("DURATION").and_then(|d| duration(&d.value));
    match start {
        Moment::Day(first) => {
            // An end day is the day after the last.
            let after = match (end, length) {
                (Some(Moment::Day(d)), _) if d > first => d,
                (_, Some(len)) if len.as_secs() >= 86_400 => {
                    first.checked_add((len.as_secs() / 86_400).days()).ok()?
                }
                _ => first.tomorrow().ok()?,
            };
            Some(EventWhen::Days {
                first: day(first),
                last: day(after.yesterday().ok()?),
            })
        }
        Moment::At(start) => {
            let end = match (end, length) {
                (Some(Moment::At(e)), _) if e >= start => e,
                (_, Some(len)) => start.checked_add(len).ok()?,
                _ => start,
            };
            Some(EventWhen::Times {
                start: Timestamp::from_millis(start.as_millisecond()),
                end: Timestamp::from_millis(end.as_millisecond()),
            })
        }
    }
}

#[derive(Clone, Copy)]
enum Moment {
    Day(Date),
    At(jiff::Timestamp),
}

fn moment(p: &Property, local: &TimeZone) -> Option<Moment> {
    let value = p.value.trim();
    let is_date = p
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || (value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit()));
    if is_date {
        return Some(Moment::Day(Date::strptime("%Y%m%d", value).ok()?));
    }
    if let Some(utc) = value.strip_suffix(['Z', 'z']) {
        let at = DateTime::strptime("%Y%m%dT%H%M%S", utc).ok()?;
        return Some(Moment::At(at.to_zoned(TimeZone::UTC).ok()?.timestamp()));
    }
    let at = DateTime::strptime("%Y%m%dT%H%M%S", value).ok()?;
    let zone = p
        .param("TZID")
        .and_then(|id| TimeZone::get(id.trim_start_matches('/')).ok())
        .unwrap_or_else(|| local.clone());
    Some(Moment::At(at.to_zoned(zone).ok()?.timestamp()))
}

/// An iCalendar `DURATION`: `P1D`, `PT1H30M`, `P2W`, `-PT15M`.
fn duration(text: &str) -> Option<SignedDuration> {
    let text = text.trim();
    let (sign, text) = match text.strip_prefix('-') {
        Some(rest) => (-1, rest),
        None => (1, text.strip_prefix('+').unwrap_or(text)),
    };
    let text = text.strip_prefix(['P', 'p'])?;
    let (mut secs, mut number, mut in_time) = (0i64, String::new(), false);
    for c in text.chars() {
        match c.to_ascii_uppercase() {
            'T' => in_time = true,
            d if d.is_ascii_digit() => number.push(d),
            unit => {
                let n: i64 = number.parse().ok()?;
                number.clear();
                secs += n * match (unit, in_time) {
                    ('W', false) => 7 * 86_400,
                    ('D', false) => 86_400,
                    ('H', true) => 3_600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
            }
        }
    }
    number
        .is_empty()
        .then(|| SignedDuration::from_secs(sign * secs))
}

fn day(d: Date) -> Day {
    Day {
        year: d.year(),
        month: d.month(),
        day: d.day(),
    }
}

#[cfg(test)]
#[path = "invitation_tests.rs"]
mod tests;
