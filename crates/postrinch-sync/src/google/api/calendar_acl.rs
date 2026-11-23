//! The Calendar API's calls on who a calendar is shared with (its access
//! control list), and on which calendars the account lists: adding one
//! shared with it, or taking one off.

use postrinch_model::{CalendarRole, CalendarShare, EmailAddress, ShareWho};
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::calendar::SERVICE;
use super::calendar_list::CalendarResource;
use super::{FG, GmailApi, Payload};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

/// A calendar's or a rule's id as one part of an address.
fn part(id: &str) -> String {
    url::form_urlencoded::byte_serialize(id.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

#[derive(Debug, Deserialize)]
struct AclScope {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AclRule {
    id: String,
    role: String,
    scope: AclScope,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AclList {
    #[serde(default)]
    items: Vec<AclRule>,
    #[serde(default)]
    next_page_token: Option<String>,
}

/// A rule as a share; `None` for the role "none" and for rules naming
/// nobody.
pub(crate) fn to_share(rule: &AclRule) -> Option<CalendarShare> {
    let role = CalendarRole::from_word(&rule.role)?;
    let value = rule.scope.value.clone();
    let who = match rule.scope.kind.as_str() {
        "user" => ShareWho::Person(value?),
        "group" => ShareWho::Group(value?),
        "domain" => ShareWho::Domain(value?),
        "default" => ShareWho::Anyone,
        _ => return None,
    };
    Some(CalendarShare {
        rule: rule.id.clone(),
        who,
        role,
    })
}

fn scope(who: &ShareWho) -> serde_json::Value {
    match who {
        ShareWho::Person(a) => json!({ "type": "user", "value": a }),
        ShareWho::Group(a) => json!({ "type": "group", "value": a }),
        ShareWho::Domain(d) => json!({ "type": "domain", "value": d }),
        ShareWho::Anyone => json!({ "type": "default" }),
    }
}

impl GmailApi {
    fn acl_url(&self, calendar: &str, rule: Option<&str>) -> String {
        match rule {
            Some(r) => format!(
                "{}/calendars/{}/acl/{}",
                self.calendar,
                part(calendar),
                part(r)
            ),
            None => format!("{}/calendars/{}/acl", self.calendar, part(calendar)),
        }
    }

    /// Everyone a calendar is shared with. Only its owners may ask.
    pub(crate) async fn calendar_acl(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<Vec<CalendarShare>> {
        let url = self.acl_url(calendar, None);
        let mut all = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut query = vec![("maxResults", "250")];
            if let Some(p) = &page {
                query.push(("pageToken", p.as_str()));
            }
            let body = self
                .call(
                    Method::GET,
                    account,
                    (Cost::Calendar, FG),
                    (&url, SERVICE),
                    &query,
                    None,
                    true,
                )
                .await?;
            let list: AclList = read(&body, "sharing")?;
            all.extend(list.items.iter().filter_map(to_share));
            match list.next_page_token {
                Some(next) => page = Some(next),
                None => return Ok(all),
            }
        }
    }

    /// Share a calendar, or change how; with `tell`, Google emails them.
    pub(crate) async fn share_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
        who: &ShareWho,
        role: CalendarRole,
        tell: bool,
    ) -> SyncResult<CalendarShare> {
        let url = self.acl_url(calendar, None);
        let body = json!({ "role": role.word(), "scope": scope(who) });
        let made = self
            .call(
                Method::POST,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[("sendNotifications", if tell { "true" } else { "false" })],
                Some(Payload::Json(&body)),
                // The same rule twice is the same rule.
                true,
            )
            .await?;
        let rule: AclRule = read(&made, "sharing rule")?;
        to_share(&rule).ok_or_else(|| SyncError::api(SERVICE, 200, "an unreadable sharing rule"))
    }

    /// Stop sharing a calendar by one rule.
    pub(crate) async fn unshare_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
        rule: &str,
    ) -> SyncResult<()> {
        let url = self.acl_url(calendar, Some(rule));
        let done = self
            .call(
                Method::DELETE,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[],
                None,
                true,
            )
            .await;
        match done {
            Ok(_)
            | Err(SyncError::Api {
                status: 404 | 410, ..
            }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Add a calendar shared with the account to its list.
    pub(crate) async fn add_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<CalendarResource> {
        let url = format!("{}/users/me/calendarList", self.calendar);
        let body = json!({ "id": calendar, "selected": true });
        let made = self
            .call(
                Method::POST,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[],
                Some(Payload::Json(&body)),
                true,
            )
            .await?;
        read(&made, "calendar")
    }

    /// Take a calendar off the account's list; it is not deleted.
    pub(crate) async fn remove_listed_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> SyncResult<()> {
        let url = format!("{}/users/me/calendarList/{}", self.calendar, part(calendar));
        let done = self
            .call(
                Method::DELETE,
                account,
                (Cost::Calendar, FG),
                (&url, SERVICE),
                &[],
                None,
                true,
            )
            .await;
        match done {
            Ok(_)
            | Err(SyncError::Api {
                status: 404 | 410, ..
            }) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn read<T: serde::de::DeserializeOwned>(body: &[u8], what: &str) -> SyncResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api(SERVICE, 200, format!("unreadable {what}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_as_shares() {
        let list: AclList = serde_json::from_str(
            r#"{"items":[
              {"id":"user:joe@example.org","role":"owner","scope":{"type":"user","value":"joe@example.org"}},
              {"id":"domain:example.org","role":"freeBusyReader","scope":{"type":"domain","value":"example.org"}},
              {"id":"default","role":"none","scope":{"type":"default"}},
              {"id":"default2","role":"reader","scope":{"type":"default"}}
            ]}"#,
        )
        .unwrap();
        let shares: Vec<_> = list.items.iter().filter_map(to_share).collect();
        assert_eq!(shares.len(), 3);
        assert_eq!(shares[0].who, ShareWho::Person("joe@example.org".into()));
        assert_eq!(shares[1].role, CalendarRole::FreeBusy);
        assert_eq!(shares[2].who, ShareWho::Anyone);
    }
}
