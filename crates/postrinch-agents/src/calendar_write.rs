//! Agents changing the person's calendars. A change that tells nobody
//! (an event of the person's own with no guests; a guest taking an event
//! off its own calendar) is made at once. One that would tell other people
//! (inviting, updating or cancelling guests; answering an organizer) leaves
//! the machine, so it waits for the person, as a proposed message does.

use postrinch_commands::Commands;
use postrinch_model::{
    Answer, CalendarEvent, Day, EmailAddress, EventChange, EventDraft, EventWhen,
};
use serde_json::{Value, json};

use crate::calendar::moment;
use crate::remote::Remote;
use crate::views::say;
use crate::wire::Call;

const NO_GOOGLE: &str =
    "Postrinch can't reach Google now (sync is off), so it can't change calendars.";

const WAITS: &str = "the person: it would tell other people, so Postrinch shows it to them \
                     and only they make it";

pub(crate) fn run(
    commands: &Commands,
    call: Call,
    agent: &str,
    remote: Option<&dyn Remote>,
) -> Result<Value, String> {
    match call {
        Call::SaveEvent {
            account,
            calendar,
            event,
            summary,
            starts,
            ends,
            first_day,
            last_day,
            location,
            description,
            guests,
            reason,
        } => {
            let calendar = match calendar {
                Some(c) => c,
                None => own_calendar(commands, &account)?,
            };
            check_writable(commands, &account, &calendar)?;
            let base = match &event {
                Some(id) => Some(commands.event_by_id(&account, &calendar, id).map_err(say)?),
                None => None,
            };
            if let Some(base) = &base
                && !organizes(base, &account)
            {
                return Err(
                    "That event is someone else's: its account is a guest, so may only answer it (answer_event)."
                        .into(),
                );
            }
            let mut draft = match &base {
                Some(base) => EventDraft::of(base, &account),
                None => EventDraft {
                    calendar: calendar.clone(),
                    summary: String::new(),
                    location: String::new(),
                    description: String::new(),
                    when: EventWhen::Times {
                        start: postrinch_model::Timestamp::from_millis(0),
                        end: postrinch_model::Timestamp::from_millis(0),
                    },
                    time_zone: None,
                    guests: Vec::new(),
                },
            };
            draft.calendar = calendar;
            draft.time_zone = jiff::tz::TimeZone::system().iana_name().map(str::to_owned);
            if let Some(s) = summary {
                draft.summary = s;
            }
            if let Some(l) = location {
                draft.location = l;
            }
            if let Some(d) = description {
                draft.description = d;
            }
            match (starts, ends, first_day, last_day) {
                (None, None, None, None) if base.is_some() => {}
                (Some(s), Some(e), None, None) => {
                    let (s, e) = (moment(&s)?, moment(&e)?);
                    if e <= s {
                        return Err("It ends before it starts.".into());
                    }
                    draft.when = EventWhen::Times { start: s, end: e };
                }
                (None, None, Some(f), l) => {
                    let first = day(&f)?;
                    let last = match l {
                        Some(l) => day(&l)?,
                        None => first,
                    };
                    if last < first {
                        return Err("It ends before it starts.".into());
                    }
                    draft.when = EventWhen::Days { first, last };
                }
                _ => {
                    return Err(
                        "Give starts and ends for an event at a time, or first_day (and last_day) for whole days."
                            .into(),
                    );
                }
            }
            if draft.summary.trim().is_empty() {
                return Err("Give the event a summary.".into());
            }
            if let Some(g) = guests {
                draft.guests = g;
            }
            let had_guests = base.as_ref().is_some_and(|b| others(b, &account) > 0);
            if !draft.guests.is_empty() || had_guests {
                let p = commands
                    .propose_event_change(
                        &account,
                        EventChange::Save { event, draft },
                        agent,
                        reason,
                    )
                    .map_err(say)?;
                return Ok(json!({ "proposed": p.id, "waits_for": WAITS }));
            }
            let id = remote
                .ok_or(NO_GOOGLE)?
                .save_event(&account, event.as_deref(), &draft)?;
            Ok(json!({ "saved": id, "calendar": draft.calendar }))
        }
        Call::DeleteEvent {
            account,
            calendar,
            event,
            reason,
        } => {
            check_writable(commands, &account, &calendar)?;
            let found = commands
                .event_by_id(&account, &calendar, &event)
                .map_err(say)?;
            let tells = organizes(&found, &account) && others(&found, &account) > 0;
            if tells {
                let change = EventChange::Delete {
                    calendar,
                    event,
                    summary: found.summary.clone(),
                    when: found.when,
                };
                let p = commands
                    .propose_event_change(&account, change, agent, reason)
                    .map_err(say)?;
                return Ok(json!({ "proposed": p.id, "waits_for": WAITS }));
            }
            remote
                .ok_or(NO_GOOGLE)?
                .delete_event(&account, &calendar, &event, false)?;
            Ok(json!({ "deleted": event }))
        }
        Call::AnswerEvent {
            account,
            calendar,
            event,
            answer,
            reason,
        } => {
            let answer = Answer::from_word(&answer)
                .ok_or_else(|| format!("answer is one of yes, maybe or no, not {answer:?}."))?;
            let found = commands
                .event_by_id(&account, &calendar, &event)
                .map_err(say)?;
            if !found.guest {
                return Err(format!(
                    "{account} isn't among that event's guests, so there is nothing to answer."
                ));
            }
            let change = EventChange::Answer {
                calendar,
                event,
                summary: found.summary.clone(),
                when: found.when,
                answer,
            };
            let p = commands
                .propose_event_change(&account, change, agent, reason)
                .map_err(say)?;
            Ok(json!({ "proposed": p.id, "waits_for": WAITS }))
        }
        _ => unreachable!("only calendar changes come here"),
    }
}

/// The account's own calendar.
fn own_calendar(commands: &Commands, account: &EmailAddress) -> Result<String, String> {
    commands
        .calendars()
        .map_err(say)?
        .into_iter()
        .find(|(a, c)| a == account && c.primary)
        .map(|(_, c)| c.id)
        .ok_or_else(|| format!("{account} has no calendar here yet: sync hasn't brought it."))
}

fn check_writable(
    commands: &Commands,
    account: &EmailAddress,
    calendar: &str,
) -> Result<(), String> {
    commands.check(account).map_err(say)?;
    let found = commands
        .calendars()
        .map_err(say)?
        .into_iter()
        .find(|(a, c)| a == account && c.id == calendar);
    match found {
        Some((_, c)) if c.writable => Ok(()),
        Some(_) => Err(format!("{account} may not change calendar {calendar:?}.")),
        None => Err(format!(
            "{account} has no calendar {calendar:?}: list_calendars names them."
        )),
    }
}

/// Whether the account organizes the event (or nobody does).
fn organizes(e: &CalendarEvent, account: &EmailAddress) -> bool {
    e.organizer
        .as_ref()
        .is_none_or(|o| o.address.key() == account.key() || o.address.to_string() == e.calendar)
}

/// Guests other than the account.
fn others(e: &CalendarEvent, account: &EmailAddress) -> usize {
    e.attendees
        .iter()
        .filter(|a| a.mailbox.address.key() != account.key())
        .count()
}

fn day(text: &str) -> Result<Day, String> {
    let d: jiff::civil::Date = text
        .trim()
        .parse()
        .map_err(|_| format!("{text:?} is not a day: give it as 2026-11-25."))?;
    Ok(Day {
        year: d.year(),
        month: d.month(),
        day: d.day(),
    })
}
