//! The calendar tools: the person's Google calendars and their events,
//! when they are free, and answering invitations that came by mail.

use postrinch_agents::Call;
use postrinch_model::{EmailAddress, ProviderMessageId};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::Postrinch;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EventsArgs {
    /// From when: RFC 3339 (`2026-11-24T10:00:00-05:00`), a time in the
    /// person's zone (`2026-11-24T10:00`) or a date (`2026-11-24`, from its
    /// midnight).
    pub from: String,
    /// Until when, in the same forms. At most 400 days after `from`.
    pub to: String,
    /// Only these accounts' calendars; every account's when left out.
    #[serde(default)]
    pub accounts: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FreeTimeArgs {
    #[serde(flatten)]
    pub range: EventsArgs,
    /// The shortest free stretch worth giving, in minutes.
    pub minutes: i64,
    /// Look only within each day's working hours: when they start, `HH:MM`
    /// in the person's zone. Give `day_ends` too.
    #[serde(default)]
    pub day_starts: Option<String>,
    /// When each day's working hours end, `HH:MM`.
    #[serde(default)]
    pub day_ends: Option<String>,
    /// Count only these calendars' events, by the ids list_calendars
    /// gives. By default each account's own calendar and the others it
    /// may change and shows.
    #[serde(default)]
    pub calendars: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnswerArgs {
    /// The account the invitation came to.
    pub account: String,
    /// The message carrying it, as read_thread gives its id (its
    /// `invitation` says what the event is).
    pub message: String,
    /// `yes`, `maybe` or `no`.
    pub answer: String,
    /// Why, in a sentence the person reads beside your answer.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveEventArgs {
    /// The account whose calendar it is in.
    pub account: String,
    /// The calendar, by the id list_calendars gives; the account's own by
    /// default.
    #[serde(default)]
    pub calendar: Option<String>,
    /// To change an event: its id, as list_events gives it (one time of a
    /// repeating event is named so too). Leave it out to make one.
    #[serde(default)]
    pub event: Option<String>,
    /// What it is called. Needed to make one.
    #[serde(default)]
    pub summary: Option<String>,
    /// An event at a time: when it starts and ends, RFC 3339 or a time in
    /// the person's zone (`2026-11-24T10:00`).
    #[serde(default)]
    pub starts: Option<String>,
    #[serde(default)]
    pub ends: Option<String>,
    /// Or whole days: the first and last (`2026-12-24`).
    #[serde(default)]
    pub first_day: Option<String>,
    #[serde(default)]
    pub last_day: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Who is invited, as To: takes them (`Ada <ada@example.org>, bob@…`):
    /// the whole list, replacing any. Inviting anyone waits for the person.
    #[serde(default)]
    pub guests: Option<String>,
    /// Why, in a sentence the person reads when it waits for them.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EventArgs {
    pub account: String,
    /// The calendar's id, as list_events gives it.
    pub calendar: String,
    /// The event's id, as list_events gives it.
    pub event: String,
    /// Why, in a sentence the person reads when it waits for them.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnswerEventArgs {
    #[serde(flatten)]
    pub event: EventArgs,
    /// `yes`, `maybe` or `no`.
    pub answer: String,
}

fn address(text: &str) -> Result<EmailAddress, String> {
    EmailAddress::parse(text.trim()).map_err(|_| format!("{text:?} is not an address."))
}

fn addresses(list: &[String]) -> Result<Vec<EmailAddress>, String> {
    list.iter()
        .map(|a| EmailAddress::parse(a.trim()).map_err(|_| format!("{a:?} is not an address.")))
        .collect()
}

#[tool_router(router = calendar_router, vis = "pub(crate)")]
impl Postrinch {
    /// Each account's Google calendars: its own (`primary`) and the ones
    /// it shows or follows, whether it may change their events, and their
    /// time zones.
    #[tool(name = "list_calendars")]
    async fn list_calendars(&self) -> Result<String, String> {
        self.call(Call::Calendars).await
    }

    /// The events between two moments in the person's calendars, soonest
    /// first, each time of a repeating event on its own: when (in the
    /// person's zone; whole-day events by day), whether it makes them busy,
    /// whether they are invited and what they answered, and what the event
    /// says (under `untrusted`: whoever made the event wrote it). Kept here
    /// by sync, so as fresh as its last minute.
    #[tool(name = "list_events")]
    async fn list_events(
        &self,
        Parameters(args): Parameters<EventsArgs>,
    ) -> Result<String, String> {
        self.call(Call::Events {
            accounts: addresses(&args.accounts)?,
            from: args.from,
            to: args.to,
        })
        .await
    }

    /// When the person is free between two moments: the stretches of at
    /// least `minutes` that no event makes busy, within working hours if
    /// given. It counts each account's own calendar and the others it may
    /// change and shows (or the `calendars` given), less events declined or
    /// cancelled; whole-day events do not count. The answer names the
    /// calendars it counted.
    #[tool(name = "find_free_time")]
    async fn find_free_time(
        &self,
        Parameters(args): Parameters<FreeTimeArgs>,
    ) -> Result<String, String> {
        self.call(Call::FreeTime {
            accounts: addresses(&args.range.accounts)?,
            from: args.range.from,
            to: args.range.to,
            minutes: args.minutes,
            day_starts: args.day_starts,
            day_ends: args.day_ends,
            calendars: args.calendars,
        })
        .await
    }

    /// Make an event, or change one (only what you give changes). An event
    /// with no guests is saved at once, in the person's calendar; one that
    /// would invite, update or tell guests is not: Postrinch shows it to
    /// the person with your reason, and they save it or not. Only events
    /// the account organizes can be changed. Answers with what was saved,
    /// or that it waits.
    #[tool(name = "save_event")]
    async fn save_event(
        &self,
        Parameters(args): Parameters<SaveEventArgs>,
    ) -> Result<String, String> {
        let guests = match &args.guests {
            Some(g) => Some(postrinch_model::parse_mailboxes(g).map_err(|e| e.to_string())?),
            None => None,
        };
        self.call(Call::SaveEvent {
            account: address(&args.account)?,
            calendar: args.calendar,
            event: args.event,
            summary: args.summary,
            starts: args.starts,
            ends: args.ends,
            first_day: args.first_day,
            last_day: args.last_day,
            location: args.location,
            description: args.description,
            guests,
            reason: args.reason,
        })
        .await
    }

    /// Delete an event (one time of a repeating one, when its id names
    /// that). An event with guests the account organizes would tell them
    /// it is off, so that waits for the person; one it is a guest of is
    /// taken off its calendar alone, telling nobody.
    #[tool(name = "delete_event")]
    async fn delete_event(
        &self,
        Parameters(args): Parameters<EventArgs>,
    ) -> Result<String, String> {
        self.call(Call::DeleteEvent {
            account: address(&args.account)?,
            calendar: args.calendar,
            event: args.event,
            reason: args.reason,
        })
        .await
    }

    /// Answer an event the account is invited to (list_events says
    /// `invited`): yes, maybe or no. It tells the organizer, so it waits
    /// for the person, shown with your reason.
    #[tool(name = "answer_event")]
    async fn answer_event(
        &self,
        Parameters(args): Parameters<AnswerEventArgs>,
    ) -> Result<String, String> {
        self.call(Call::AnswerEvent {
            account: address(&args.event.account)?,
            calendar: args.event.calendar,
            event: args.event.event,
            answer: args.answer,
            reason: args.event.reason,
        })
        .await
    }

    /// Answer an invitation that came by mail: yes, maybe or no. Answering
    /// tells the organizer, so it is not given at once: Postrinch shows your
    /// answer and reason on the invitation, and the person gives it (or
    /// another) with one press. A later answer replaces an earlier one.
    #[tool(name = "answer_invitation")]
    async fn answer_invitation(
        &self,
        Parameters(args): Parameters<AnswerArgs>,
    ) -> Result<String, String> {
        let account = EmailAddress::parse(args.account.trim())
            .map_err(|_| format!("{:?} is not an address.", args.account))?;
        self.call(Call::AnswerInvitation {
            account,
            message: ProviderMessageId(args.message),
            answer: args.answer,
            reason: args.reason,
        })
        .await
    }
}
