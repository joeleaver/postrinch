//! Calendar sharing for agents: who one of the person's calendars is
//! shared with, and adding a calendar someone shared with them. Sharing a
//! calendar with someone, or stopping, is the person's to do: it tells
//! other people, and opens the calendar to them.

use postrinch_commands::Commands;
use postrinch_model::{CalendarRole, ShareWho};
use serde_json::{Value, json};

use crate::gmail::Gmail;
use crate::views::say;
use crate::wire::Call;

const NO_GOOGLE: &str =
    "Postrinch can't reach Google now (sync is off), so it can't read or add calendars.";

pub(crate) fn run(
    commands: &Commands,
    call: Call,
    gmail: Option<&dyn Gmail>,
) -> Result<Value, String> {
    match call {
        Call::CalendarShares { account, calendar } => {
            commands.check(&account).map_err(say)?;
            let all = commands.calendars().map_err(say)?;
            let Some((_, info)) = all
                .iter()
                .find(|(a, c)| a.key() == account.key() && c.id == calendar)
            else {
                return Err(format!(
                    "{account} has no calendar {calendar:?}. list_calendars shows the ones it has."
                ));
            };
            if info.role != CalendarRole::Owner {
                return Err(
                    "Only a calendar's owners see who it is shared with, and this one is shared with the account."
                        .into(),
                );
            }
            let shares = gmail
                .ok_or(NO_GOOGLE)?
                .calendar_shares(&account, &calendar)?;
            Ok(json!({
                "account": account,
                "calendar": calendar,
                "shared_with": shares.iter().map(|s| json!({
                    "who": match &s.who {
                        ShareWho::Person(a) => json!({ "person": a }),
                        ShareWho::Group(a) => json!({ "group": a }),
                        ShareWho::Domain(d) => json!({ "domain": d }),
                        ShareWho::Anyone => json!("anyone: the calendar is public"),
                    },
                    "role": s.role.word(),
                    "may": s.role.says(),
                })).collect::<Vec<_>>(),
                "sharing": "only the person shares a calendar or stops: in Postrinch's calendar, \
                            a click on its name",
            }))
        }
        Call::AddCalendar { account, calendar } => {
            commands.check(&account).map_err(say)?;
            let all = commands.calendars().map_err(say)?;
            if all
                .iter()
                .any(|(a, c)| a.key() == account.key() && c.id == calendar)
            {
                return Ok(
                    json!({ "account": account, "calendar": calendar, "added": false,
                                  "why": "it is among the account's calendars already" }),
                );
            }
            let info = gmail.ok_or(NO_GOOGLE)?.add_calendar(&account, &calendar)?;
            Ok(json!({
                "account": account,
                "calendar": info.id,
                "added": true,
                "role": info.role.word(),
                "untrusted": { "name": info.name },
            }))
        }
        _ => unreachable!("only calendar sharing calls come here"),
    }
}
