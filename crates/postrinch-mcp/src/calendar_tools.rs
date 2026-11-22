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
    /// least `minutes` that no event makes busy (counting each account's
    /// own calendar, as Google's free/busy does, less events they declined;
    /// whole-day events do not count), within working hours if given.
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
