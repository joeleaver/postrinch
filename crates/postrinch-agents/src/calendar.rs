//! The calendar calls: the person's calendars, their events, when they are
//! free, and answering invitations (which waits for the person). What an
//! event says (its summary, place, description, people) was written by
//! whoever made it, so it is under `untrusted`.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::TimeZone;
use postrinch_commands::{Commands, Happening, Hours};
use postrinch_model::{Answer, EventWhen, Mailbox, Timestamp};
use serde_json::{Value, json};

use crate::views::say;
use crate::wire::Call;

pub(crate) fn run(commands: &Commands, call: Call, agent: &str) -> Result<Value, String> {
    match call {
        Call::Calendars => {
            let all = commands.calendars().map_err(say)?;
            Ok(Value::Array(
                all.into_iter()
                    .map(|(account, c)| {
                        json!({
                            "account": account,
                            "calendar": c.id,
                            "primary": c.primary,
                            "writable": c.writable,
                            "role": c.role.word(),
                            "shown": c.shown,
                            "time_zone": c.time_zone,
                            "untrusted": { "name": c.name },
                        })
                    })
                    .collect(),
            ))
        }
        Call::Events { accounts, from, to } => {
            let (from, to) = (moment(&from)?, moment(&to)?);
            let found = commands.events_between(&accounts, from, to).map_err(say)?;
            Ok(json!({
                "time_zone": zone_name(),
                "events": found.iter().map(happening).collect::<Vec<_>>(),
            }))
        }
        Call::FreeTime {
            accounts,
            from,
            to,
            minutes,
            day_starts,
            day_ends,
            calendars,
        } => {
            let (from, to) = (moment(&from)?, moment(&to)?);
            let hours = match (day_starts, day_ends) {
                (Some(s), Some(e)) => {
                    let hours = Hours {
                        from: clock(&s)?,
                        to: clock(&e)?,
                    };
                    if hours.to <= hours.from {
                        return Err("day_ends must come after day_starts.".into());
                    }
                    Some(hours)
                }
                (None, None) => None,
                _ => return Err("Give both day_starts and day_ends, or neither.".into()),
            };
            let found = commands
                .free_time(&accounts, (from, to), minutes, hours, &calendars)
                .map_err(say)?;
            Ok(json!({
                "time_zone": zone_name(),
                "counts": "events that make the person busy in these calendars, less those \
                           declined or cancelled; whole-day events do not",
                "calendars": found.counted.iter().map(|(a, c)| json!({
                    "account": a,
                    "calendar": c.id,
                    "untrusted": { "name": c.name },
                })).collect::<Vec<_>>(),
                "free": found.free.iter().map(|(s, e)| json!({
                    "starts": local(*s),
                    "ends": local(*e),
                })).collect::<Vec<_>>(),
            }))
        }
        Call::AnswerInvitation {
            account,
            message,
            answer,
            reason,
        } => {
            let answer = Answer::from_word(&answer)
                .ok_or_else(|| format!("answer is one of yes, maybe or no, not {answer:?}."))?;
            let proposal = commands
                .propose_answer(&account, &message, answer, agent, reason)
                .map_err(say)?;
            Ok(json!({
                "proposed": proposal.answer.word(),
                "message": proposal.message.0,
                "waits_for": "the person: answering tells the organizer, so Postrinch shows \
                              your answer on the invitation and only the person gives it",
            }))
        }
        _ => unreachable!("only calendar calls come here"),
    }
}

fn happening(h: &Happening) -> Value {
    let e = &h.event;
    let mut out = json!({
        "account": h.account,
        "calendar": e.calendar,
        "event": e.id,
        "busy": e.busy,
        "cancelled": e.cancelled,
        "repeating": e.repeating,
        "invited": e.guest,
        "your_answer": e.answer.map(Answer::word),
        "link": e.link,
        "untrusted": {
            "summary": e.summary,
            "location": e.location,
            "description": e.description,
            "organizer": e.organizer.as_ref().map(mailbox),
            "guests": e.attendees.iter().map(|a| json!({
                "guest": mailbox(&a.mailbox),
                "answer": a.answer.map(Answer::word),
            })).collect::<Vec<_>>(),
        },
    });
    match e.when {
        EventWhen::Days { first, last } => {
            let day =
                |d: postrinch_model::Day| format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
            out["first_day"] = json!(day(first));
            out["last_day"] = json!(day(last));
        }
        EventWhen::Times { start, end } => {
            out["starts"] = json!(local(start));
            out["ends"] = json!(local(end));
        }
    }
    out
}

fn mailbox(m: &Mailbox) -> Value {
    match &m.name {
        Some(name) => json!({ "name": name, "address": m.address }),
        None => json!({ "address": m.address }),
    }
}

/// A moment as the agent writes it: RFC 3339 (`2026-11-24T10:00:00-05:00`),
/// a wall time in the person's zone (`2026-11-24T10:00`), or a date for its
/// midnight there.
pub(crate) fn moment(text: &str) -> Result<Timestamp, String> {
    let text = text.trim();
    let tz = TimeZone::system();
    let at = if let Ok(t) = text.parse::<jiff::Timestamp>() {
        Ok(t)
    } else if let Ok(d) = text.parse::<DateTime>() {
        d.to_zoned(tz).map(|z| z.timestamp())
    } else if let Ok(d) = text.parse::<Date>() {
        d.to_zoned(tz).map(|z| z.timestamp())
    } else {
        return Err(format!(
            "{text:?} is not a time: give RFC 3339 (2026-11-24T10:00:00-05:00), \
             a local time (2026-11-24T10:00) or a date (2026-11-24)."
        ));
    };
    at.map(|t| Timestamp::from_millis(t.as_millisecond()))
        .map_err(|e| format!("{text:?}: {e}"))
}

fn clock(text: &str) -> Result<Time, String> {
    text.trim()
        .parse::<Time>()
        .map_err(|_| format!("{text:?} is not a time of day: give HH:MM, as 09:00."))
}

/// A moment in RFC 3339, in the person's zone.
fn local(t: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(t.as_millis())
        .map(|t| {
            t.to_zoned(TimeZone::system())
                .strftime("%Y-%m-%dT%H:%M:%S%:z")
                .to_string()
        })
        .unwrap_or_default()
}

fn zone_name() -> String {
    TimeZone::system()
        .iana_name()
        .unwrap_or("the system's")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moments_in_three_forms() {
        let utc = moment("2026-11-24T10:00:00Z").unwrap();
        assert_eq!(moment("2026-11-24T05:00:00-05:00").unwrap(), utc);
        assert!(moment("2026-11-24").is_ok());
        assert!(moment("2026-11-24T10:00").is_ok());
        assert!(moment("next tuesday").unwrap_err().contains("RFC 3339"));
        assert_eq!(clock("09:30").unwrap(), Time::constant(9, 30, 0, 0));
    }
}
