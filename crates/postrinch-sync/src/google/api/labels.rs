//! Gmail's calls to make, rename and delete the person's labels.

use postrinch_model::EmailAddress;
use reqwest::Method;
use serde_json::json;

use super::types::LabelResource;
use super::{FG, GmailApi};
use crate::google::quota::Cost;
use crate::{SyncError, SyncResult};

impl GmailApi {
    /// Make a label shown as Gmail shows labels the person makes.
    pub(crate) async fn create_label(
        &self,
        account: &EmailAddress,
        name: &str,
    ) -> SyncResult<LabelResource> {
        let body = json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
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
        read(&answer)
    }

    /// Give a label a new name; its mail keeps it.
    pub(crate) async fn rename_label(
        &self,
        account: &EmailAddress,
        id: &str,
        name: &str,
    ) -> SyncResult<LabelResource> {
        let answer = self
            .request(
                Method::PATCH,
                account,
                (Cost::Settings, FG),
                &format!("labels/{id}"),
                &[],
                Some(&json!({ "name": name })),
                true,
            )
            .await?;
        read(&answer)
    }
}

fn read(body: &[u8]) -> SyncResult<LabelResource> {
    serde_json::from_slice(body)
        .map_err(|e| SyncError::api("Gmail", 200, format!("unreadable response to labels: {e}")))
}
