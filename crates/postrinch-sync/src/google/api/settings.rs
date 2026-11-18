//! Gmail's calls for rules: filters, verified forwarding addresses, hidden
//! labels, and the settings message Postrinch keeps in the account.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use postrinch_model::EmailAddress;
use reqwest::Method;
use serde::Deserialize;
use serde_json::json;

use super::{FG, GmailApi};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

/// A filter, as Gmail lists it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterResource {
    pub id: String,
    #[serde(default)]
    pub criteria: Criteria,
    #[serde(default)]
    pub action: FilterAction,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Criteria {
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub query: Option<String>,
    pub negated_query: Option<String>,
    #[serde(default)]
    pub has_attachment: bool,
    pub size: Option<u64>,
    pub size_comparison: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FilterAction {
    #[serde(default)]
    pub add_label_ids: Vec<String>,
    #[serde(default)]
    pub remove_label_ids: Vec<String>,
    pub forward: Option<String>,
}

#[derive(Deserialize)]
struct FilterList {
    #[serde(default)]
    filter: Vec<FilterResource>,
}

/// A forwarding address, and whether its owner confirmed it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Forwarding {
    pub forwarding_email: String,
    #[serde(default)]
    pub verification_status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForwardingList {
    #[serde(default)]
    forwarding_addresses: Vec<Forwarding>,
}

#[derive(Deserialize)]
struct Id {
    id: String,
}

#[derive(Deserialize)]
struct Ids {
    #[serde(default)]
    messages: Vec<Id>,
}

impl GmailApi {
    pub(crate) async fn filters(&self, account: &EmailAddress) -> SyncResult<Vec<FilterResource>> {
        let list: FilterList = self
            .get(account, (Cost::Settings, FG), "settings/filters", &[])
            .await?;
        Ok(list.filter)
    }

    /// Make a filter that runs `query` and changes labels or forwards.
    /// Returns its id.
    pub(crate) async fn create_filter(
        &self,
        account: &EmailAddress,
        query: &str,
        add: &[String],
        remove: &[String],
        forward: Option<&str>,
    ) -> SyncResult<String> {
        let mut action = json!({});
        if !add.is_empty() {
            action["addLabelIds"] = json!(add);
        }
        if !remove.is_empty() {
            action["removeLabelIds"] = json!(remove);
        }
        if let Some(to) = forward {
            action["forward"] = json!(to);
        }
        let body = json!({ "criteria": { "query": query }, "action": action });
        // Not resent on a dropped connection: Gmail may have made it, and
        // a second would be a duplicate. The next sync finds it as a
        // filter made elsewhere at worst.
        let answer = self
            .request(
                Method::POST,
                account,
                (Cost::Settings, FG),
                "settings/filters",
                &[],
                Some(&body),
                false,
            )
            .await?;
        Ok(read::<Id>(&answer, "settings/filters")?.id)
    }

    /// Delete a filter; one already gone is no error.
    pub(crate) async fn delete_filter(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        let path = format!("settings/filters/{id}");
        match self
            .request(
                Method::DELETE,
                account,
                (Cost::Settings, FG),
                &path,
                &[],
                None,
                true,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.is_not_found() => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The addresses Gmail has verified for forwarding.
    pub(crate) async fn forwarding(&self, account: &EmailAddress) -> SyncResult<Vec<String>> {
        let list: ForwardingList = self
            .get(
                account,
                (Cost::Settings, FG),
                "settings/forwardingAddresses",
                &[],
            )
            .await?;
        Ok(list
            .forwarding_addresses
            .into_iter()
            .filter(|f| f.verification_status == "accepted")
            .map(|f| f.forwarding_email)
            .collect())
    }

    /// Make a label neither Gmail's label list nor its message list shows.
    /// Returns its id.
    pub(crate) async fn create_hidden_label(
        &self,
        account: &EmailAddress,
        name: &str,
    ) -> SyncResult<String> {
        let body = json!({
            "name": name,
            "labelListVisibility": "labelHide",
            "messageListVisibility": "hide",
        });
        let answer = self
            .request(
                Method::POST,
                account,
                (Cost::Settings, FG),
                "labels",
                &[],
                Some(&body),
                false,
            )
            .await?;
        Ok(read::<Id>(&answer, "labels")?.id)
    }

    /// Delete a label of Postrinch's own; one already gone is no error.
    pub(crate) async fn delete_label(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        let path = format!("labels/{id}");
        match self
            .request(
                Method::DELETE,
                account,
                (Cost::Settings, FG),
                &path,
                &[],
                None,
                true,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.is_not_found() => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The ids of the messages carrying `label`, newest first.
    pub(crate) async fn labelled(
        &self,
        account: &EmailAddress,
        label: &str,
    ) -> SyncResult<Vec<String>> {
        let list: Ids = self
            .get(
                account,
                (Cost::ListMessages, FG),
                "messages",
                &[("labelIds", label), ("maxResults", "50")],
            )
            .await?;
        Ok(list.messages.into_iter().map(|m| m.id).collect())
    }

    /// Put a message in the account, under `label` only, as if it had
    /// always been there: not sent, not delivered, no filters run.
    pub(crate) async fn insert(
        &self,
        account: &EmailAddress,
        raw: &[u8],
        label: &str,
    ) -> SyncResult<String> {
        let body = json!({ "raw": URL_SAFE_NO_PAD.encode(raw), "labelIds": [label] });
        let answer = self
            .request(
                Method::POST,
                account,
                (Cost::InsertMessage, FG),
                "messages",
                &[("internalDateSource", "dateHeader")],
                Some(&body),
                false,
            )
            .await?;
        Ok(read::<Id>(&answer, "messages")?.id)
    }

    /// Delete a message for good. Only for Postrinch's own settings
    /// messages, which nobody reads.
    pub(crate) async fn delete_message(&self, account: &EmailAddress, id: &str) -> SyncResult<()> {
        let path = format!("messages/{id}");
        match self
            .request(
                Method::DELETE,
                account,
                (Cost::DeleteMessage, FG),
                &path,
                &[],
                None,
                true,
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.is_not_found() => Ok(()),
            Err(e) => Err(e),
        }
    }
}

fn read<T: for<'de> Deserialize<'de>>(body: &[u8], what: &str) -> SyncResult<T> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api("Gmail", 200, format!("unreadable response to {what}: {e}")))
}
