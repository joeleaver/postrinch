//! An invitation in mail as an agent reads it, beside its message. Every
//! word of it is the sender's, so it sits under the message's `untrusted`.

use postrinch_commands::{Commands, may_invite, may_share_calendar};
use postrinch_model::{EventWhen, Invitation, InvitationKind, Mailbox, Message, Timestamp};
use serde_json::{Value, json};

/// The message's invitation, or `null`.
pub(crate) fn of(commands: &Commands, m: &Message) -> Value {
    if !may_invite(m) {
        return Value::Null;
    }
    match commands.invitation(&m.account, &m.id) {
        Ok(Some(invitation)) => {
            let mut out = shown(&invitation);
            if invitation.kind == InvitationKind::Request {
                // What in the person's calendars is at the same time.
                let clashes = commands.clashes(&invitation).unwrap_or_default();
                out["clashes"] = json!(
                    clashes
                        .iter()
                        .map(|h| json!({
                            "account": h.account,
                            "calendar": h.event.calendar,
                            "event": h.event.id,
                            "summary": h.event.summary,
                        }))
                        .collect::<Vec<_>>()
                );
            }
            out
        }
        _ => Value::Null,
    }
}

/// The calendar Google's signed mail shares with the account, or `null`:
/// its id (to add_calendar), whether it is added, and who shared it (a
/// name they chose, so under `untrusted`).
pub(crate) fn shared_calendar(commands: &Commands, m: &Message) -> Value {
    if !may_share_calendar(m) {
        return Value::Null;
    }
    let Ok(Some(shared)) = commands.calendar_share(&m.account, &m.id) else {
        return Value::Null;
    };
    let added = commands
        .calendars()
        .unwrap_or_default()
        .iter()
        .any(|(a, c)| a.key() == m.account.key() && c.id == shared.id);
    json!({
        "calendar": shared.id,
        "added": added,
        "untrusted": { "by": shared.by },
    })
}

fn shown(i: &Invitation) -> Value {
    let mut out = json!({
        "asks": match i.kind {
            InvitationKind::Request => "an answer",
            InvitationKind::Cancel => "nothing: the event is cancelled",
            InvitationKind::Reply => "nothing: it is a guest's answer",
            InvitationKind::Publish => "nothing: it is an event to add",
        },
        "summary": i.summary,
        "location": i.location,
        "repeats": i.repeats,
        "organizer": i.organizer.as_ref().map(mailbox),
        "guests": i.attendees.iter().map(|a| json!({
            "guest": mailbox(&a.mailbox),
            "answer": a.answer.map(|a| a.word()),
        })).collect::<Vec<_>>(),
    });
    match i.when {
        Some(EventWhen::Days { first, last }) => {
            let day =
                |d: postrinch_model::Day| format!("{:04}-{:02}-{:02}", d.year, d.month, d.day);
            out["first_day"] = json!(day(first));
            out["last_day"] = json!(day(last));
        }
        Some(EventWhen::Times { start, end }) => {
            out["starts"] = json!(utc(start));
            out["ends"] = json!(utc(end));
        }
        None => {}
    }
    out
}

fn mailbox(m: &Mailbox) -> Value {
    match &m.name {
        Some(name) => json!({ "name": name, "address": m.address }),
        None => json!({ "address": m.address }),
    }
}

fn utc(t: Timestamp) -> String {
    jiff::Timestamp::from_millisecond(t.as_millis())
        .map_or_else(|_| String::new(), |t| t.to_string())
}
