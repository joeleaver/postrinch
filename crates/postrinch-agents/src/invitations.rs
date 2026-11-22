//! An invitation in mail as an agent reads it, beside its message. Every
//! word of it is the sender's, so it sits under the message's `untrusted`.

use postrinch_commands::{Commands, may_invite};
use postrinch_model::{EventWhen, Invitation, InvitationKind, Mailbox, Message, Timestamp};
use serde_json::{Value, json};

/// The message's invitation, or `null`.
pub(crate) fn of(commands: &Commands, m: &Message) -> Value {
    if !may_invite(m) {
        return Value::Null;
    }
    match commands.invitation(&m.account, &m.id) {
        Ok(Some(invitation)) => shown(&invitation),
        _ => Value::Null,
    }
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
