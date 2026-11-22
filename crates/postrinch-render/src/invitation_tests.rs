use super::*;

/// An invitation as Google Calendar sends one: the event inline in the
/// alternative, and again as `invite.ics`.
const GOOGLE: &str = concat!(
    "From: Ada Lovelace <ada@example.org>\r\n",
    "To: me@example.org\r\n",
    "Subject: Invitation: Plans @ Tue 24 Nov 2026 10am - 11am\r\n",
    "MIME-Version: 1.0\r\n",
    "Content-Type: multipart/mixed; boundary=\"m\"\r\n\r\n",
    "--m\r\n",
    "Content-Type: multipart/alternative; boundary=\"a\"\r\n\r\n",
    "--a\r\nContent-Type: text/plain; charset=\"UTF-8\"\r\n\r\nYou have been invited.\r\n",
    "--a\r\nContent-Type: text/html; charset=\"UTF-8\"\r\n\r\n<p>You have been invited.</p>\r\n",
    "--a\r\nContent-Type: text/calendar; charset=\"UTF-8\"; method=REQUEST\r\n",
    "Content-Transfer-Encoding: 7bit\r\n\r\n",
    "BEGIN:VCALENDAR\r\nPRODID:-//Google Inc//Google Calendar 70.9054//EN\r\n",
    "VERSION:2.0\r\nMETHOD:REQUEST\r\nBEGIN:VEVENT\r\n",
    "DTSTART;TZID=America/New_York:20261124T100000\r\n",
    "DTEND;TZID=America/New_York:20261124T110000\r\n",
    "ORGANIZER;CN=Ada Lovelace:mailto:ada@example.org\r\n",
    "UID:abc123@google.com\r\n",
    "ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=ACCEPTED;CN=Ada Lovelace;X-NUM-GUESTS=0:mailto:ada@example.org\r\n",
    "ATTENDEE;CUTYPE=INDIVIDUAL;ROLE=REQ-PARTICIPANT;PARTSTAT=NEEDS-ACTION;CN=me@\r\n",
    " example.org;X-NUM-GUESTS=0:mailto:me@example.org\r\n",
    "SUMMARY:Plans\r\nLOCATION:Café\\, upstairs\r\nSEQUENCE:0\r\nSTATUS:CONFIRMED\r\n",
    "END:VEVENT\r\nEND:VCALENDAR\r\n",
    "--a--\r\n",
    "--m\r\nContent-Type: application/ics; name=\"invite.ics\"\r\n",
    "Content-Disposition: attachment; filename=\"invite.ics\"\r\n",
    "Content-Transfer-Encoding: base64\r\n\r\n",
    "QkVHSU46VkNBTEVOREFSDQpFTkQ6VkNBTEVOREFSDQo=\r\n",
    "--m--\r\n",
);

fn utc(s: &str) -> Timestamp {
    Timestamp::from_millis(s.parse::<jiff::Timestamp>().unwrap().as_millisecond())
}

#[test]
fn reads_google_s_invitation() {
    let inv = invitation(GOOGLE.as_bytes(), &TimeZone::UTC).unwrap();
    assert_eq!(inv.kind, InvitationKind::Request);
    assert_eq!(inv.uid, "abc123@google.com");
    assert_eq!(inv.summary.as_deref(), Some("Plans"));
    assert_eq!(inv.location.as_deref(), Some("Café, upstairs"));
    // New York is five hours behind UTC in November.
    assert_eq!(
        inv.when,
        Some(EventWhen::Times {
            start: utc("2026-11-24T15:00:00Z"),
            end: utc("2026-11-24T16:00:00Z"),
        })
    );
    assert_eq!(
        inv.organizer.unwrap().to_string(),
        "Ada Lovelace <ada@example.org>"
    );
    let answers: Vec<_> = inv
        .attendees
        .iter()
        .map(|a| (a.mailbox.address.to_string(), a.answer))
        .collect();
    assert_eq!(
        answers,
        [
            ("ada@example.org".to_owned(), Some(Answer::Yes)),
            ("me@example.org".to_owned(), None),
        ]
    );
    assert!(!inv.repeats);
}

#[test]
fn mail_without_a_calendar_has_no_invitation() {
    let raw = b"From: a@example.org\r\nSubject: hi\r\n\r\nNo event here.\r\n";
    assert!(invitation(raw, &TimeZone::UTC).is_none());
}

fn event(lines: &str) -> Invitation {
    let text = format!(
        "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VEVENT\nUID:u\n{lines}END:VEVENT\nEND:VCALENDAR\n"
    );
    from_text(&text, &TimeZone::UTC).unwrap()
}

#[test]
fn whole_days_end_the_day_before_dtend() {
    let inv = event("DTSTART;VALUE=DATE:20261224\nDTEND;VALUE=DATE:20261226\n");
    let d = |day| Day {
        year: 2026,
        month: 12,
        day,
    };
    assert_eq!(
        inv.when,
        Some(EventWhen::Days {
            first: d(24),
            last: d(25)
        })
    );
    let one = event("DTSTART:20261224\n");
    assert_eq!(
        one.when,
        Some(EventWhen::Days {
            first: d(24),
            last: d(24)
        })
    );
}

#[test]
fn a_duration_gives_the_end() {
    let inv = event("DTSTART:20261124T100000Z\nDURATION:PT1H30M\n");
    assert_eq!(
        inv.when,
        Some(EventWhen::Times {
            start: utc("2026-11-24T10:00:00Z"),
            end: utc("2026-11-24T11:30:00Z"),
        })
    );
}

#[test]
fn an_unknown_zone_is_read_as_local_time() {
    let inv = event("DTSTART;TZID=Eastern Standard Time:20261124T100000\n");
    let Some(EventWhen::Times { start, .. }) = inv.when else {
        panic!("{:?}", inv.when)
    };
    assert_eq!(start, utc("2026-11-24T10:00:00Z"));
}

#[test]
fn cancelled_and_repeating_events() {
    let text = "BEGIN:VCALENDAR\nMETHOD:CANCEL\nBEGIN:VEVENT\nUID:u\nRRULE:FREQ=WEEKLY\n\
                RECURRENCE-ID;TZID=Europe/London:20261201T090000\nEND:VEVENT\nEND:VCALENDAR\n";
    let inv = from_text(text, &TimeZone::UTC).unwrap();
    assert_eq!(inv.kind, InvitationKind::Cancel);
    assert!(inv.repeats);
    assert_eq!(inv.recurrence.as_deref(), Some("20261201T090000"));
    let status = event("STATUS:CANCELLED\n");
    assert_eq!(status.kind, InvitationKind::Cancel);
}

#[test]
fn durations() {
    assert_eq!(duration("P1W"), Some(SignedDuration::from_hours(168)));
    assert_eq!(duration("-PT15M"), Some(SignedDuration::from_mins(-15)));
    assert_eq!(duration("P1DT2H"), Some(SignedDuration::from_hours(26)));
    assert_eq!(duration("PT"), Some(SignedDuration::ZERO));
    assert!(duration("1H").is_none());
    assert!(duration("PT5").is_none());
}

#[test]
fn outlook_s_zone_is_read_by_its_own_definition() {
    let text = "BEGIN:VCALENDAR\nMETHOD:REQUEST\nBEGIN:VTIMEZONE\nTZID:Eastern Standard Time\n\
        BEGIN:STANDARD\nDTSTART:16010101T020000\nTZOFFSETFROM:-0400\nTZOFFSETTO:-0500\n\
        RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=1SU;BYMONTH=11\nEND:STANDARD\n\
        BEGIN:DAYLIGHT\nDTSTART:16010101T020000\nTZOFFSETFROM:-0500\nTZOFFSETTO:-0400\n\
        RRULE:FREQ=YEARLY;INTERVAL=1;BYDAY=2SU;BYMONTH=3\nEND:DAYLIGHT\nEND:VTIMEZONE\n\
        BEGIN:VEVENT\nUID:u\nDTSTART;TZID=Eastern Standard Time:20261124T100000\n\
        DTEND;TZID=Eastern Standard Time:20260701T110000\nEND:VEVENT\nEND:VCALENDAR\n";
    let inv = from_text(text, &TimeZone::UTC).unwrap();
    let Some(EventWhen::Times { start, .. }) = inv.when else {
        panic!("{:?}", inv.when)
    };
    assert_eq!(start, utc("2026-11-24T15:00:00Z"));
}
