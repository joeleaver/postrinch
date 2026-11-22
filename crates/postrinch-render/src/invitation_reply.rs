//! An answer to an invitation by mail (iTIP `REPLY`, RFC 5546), for an
//! invitation the account's calendar does not have. It is made from the
//! invitation's own calendar part: the event's `UID`, `SEQUENCE`,
//! `RECURRENCE-ID`, times (with the zones they name), summary and organizer
//! as written, and the account's own guest line with its answer.

use postrinch_model::{Answer, EmailAddress, Timestamp};

use crate::ics::{self, Component, Property};
use crate::invitation::calendar_text;

/// What of the event a reply repeats.
const KEPT: [&str; 9] = [
    "UID",
    "SEQUENCE",
    "RECURRENCE-ID",
    "DTSTART",
    "DTEND",
    "DURATION",
    "SUMMARY",
    "LOCATION",
    "ORGANIZER",
];

/// The calendar text answering the invitation in raw message `raw` as
/// `me`. `None` when it has no invitation, or `me` is not among its guests.
pub fn reply(raw: &[u8], me: &EmailAddress, answer: Answer, now: Timestamp) -> Option<String> {
    let text = calendar_text(raw)?;
    let calendar = ics::parse(&text)
        .into_iter()
        .find(|c| c.name == "VCALENDAR")?;
    let event = calendar.children("VEVENT").next()?;
    let mine = event.all("ATTENDEE").find(|a| is(a, me))?;
    let mut guest = mine.clone();
    guest
        .params
        .retain(|(n, _)| !matches!(n.as_str(), "PARTSTAT" | "RSVP"));
    guest
        .params
        .push(("PARTSTAT".into(), partstat(answer).into()));
    let mut properties: Vec<Property> = event
        .properties
        .iter()
        .filter(|p| KEPT.contains(&p.name.as_str()))
        .cloned()
        .collect();
    properties.push(plain("DTSTAMP", &stamp(now)));
    properties.push(guest);
    let mut children: Vec<Component> = calendar.children("VTIMEZONE").cloned().collect();
    children.push(Component {
        name: "VEVENT".into(),
        properties,
        children: Vec::new(),
    });
    Some(ics::write(&Component {
        name: "VCALENDAR".into(),
        properties: vec![
            plain("PRODID", "-//Postrinch//Postrinch//EN"),
            plain("VERSION", "2.0"),
            plain("METHOD", "REPLY"),
        ],
        children,
    }))
}

fn is(attendee: &Property, me: &EmailAddress) -> bool {
    let value = attendee.value.trim();
    let address = match value.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("mailto:") => &value[7..],
        _ => value,
    };
    EmailAddress::parse(address.trim()).is_ok_and(|a| a.key() == me.key())
}

fn partstat(answer: Answer) -> &'static str {
    match answer {
        Answer::Yes => "ACCEPTED",
        Answer::Maybe => "TENTATIVE",
        Answer::No => "DECLINED",
    }
}

fn plain(name: &str, value: &str) -> Property {
    Property {
        name: name.into(),
        params: Vec::new(),
        value: value.into(),
    }
}

fn stamp(now: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(now.as_millis())
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH)
        .strftime("%Y%m%dT%H%M%SZ")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = concat!(
        "From: Ada <ada@example.org>\r\nTo: me@example.org\r\nSubject: Plans\r\n",
        "MIME-Version: 1.0\r\nContent-Type: text/calendar; method=REQUEST\r\n\r\n",
        "BEGIN:VCALENDAR\r\nMETHOD:REQUEST\r\n",
        "BEGIN:VTIMEZONE\r\nTZID:Eastern Standard Time\r\nEND:VTIMEZONE\r\n",
        "BEGIN:VEVENT\r\nUID:abc\r\nSEQUENCE:2\r\n",
        "DTSTART;TZID=Eastern Standard Time:20261124T100000\r\n",
        "DTEND;TZID=Eastern Standard Time:20261124T110000\r\n",
        "SUMMARY:Plans\r\nDESCRIPTION:Long words\r\n",
        "ORGANIZER;CN=Ada:mailto:ada@example.org\r\n",
        "ATTENDEE;CN=Bob;PARTSTAT=NEEDS-ACTION:mailto:bob@example.org\r\n",
        "ATTENDEE;CN=Me;RSVP=TRUE;PARTSTAT=NEEDS-ACTION:mailto:ME@example.org\r\n",
        "END:VEVENT\r\nEND:VCALENDAR\r\n",
    );

    #[test]
    fn answers_as_the_account_alone() {
        let me = EmailAddress::parse("me@example.org").unwrap();
        let text = reply(
            RAW.as_bytes(),
            &me,
            Answer::Maybe,
            Timestamp::from_millis(0),
        )
        .unwrap();
        assert!(text.contains("METHOD:REPLY\r\n"));
        assert!(text.contains("BEGIN:VTIMEZONE\r\nTZID:Eastern Standard Time\r\nEND:VTIMEZONE"));
        assert!(text.contains("UID:abc\r\nSEQUENCE:2\r\n"));
        assert!(text.contains("DTSTAMP:19700101T000000Z\r\n"));
        assert!(text.contains("ATTENDEE;CN=Me;PARTSTAT=TENTATIVE:mailto:ME@example.org\r\n"));
        assert!(!text.contains("bob@example.org"));
        assert!(!text.contains("DESCRIPTION"));
        // It reads back as the reply it is.
        let back = crate::invitation::from_text(&text, &jiff::tz::TimeZone::UTC).unwrap();
        assert_eq!(back.kind, postrinch_model::InvitationKind::Reply);
        assert_eq!(back.attendees[0].answer, Some(Answer::Maybe));
    }

    #[test]
    fn only_a_guest_answers() {
        let other = EmailAddress::parse("carol@example.org").unwrap();
        assert!(
            reply(
                RAW.as_bytes(),
                &other,
                Answer::Yes,
                Timestamp::from_millis(0)
            )
            .is_none()
        );
    }
}
