//! Groups the home provider's list leaves out (§5.7 *My groups*): ones
//! joined on another provider whose pointer did not reach home, ones the
//! person asked to join and may since have been let into, and the groups
//! of channels followed without joining.
//!
//! The group's own provider is the authority on membership (§5.7), so a
//! group kept here and missing from home's list is asked there: a member
//! still gets the pointer written again and is synced as listed; anyone
//! else is left to be forgotten (a group joined) or kept as asked.

use std::collections::HashSet;

use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::wire::{Group, GroupPointer, GroupPointerRequest};
use postrinch_ofscp::ofscp_core::{Authority, LocalId, ObjectUri};
use postrinch_store::{GroupFound, SocialGroup};

use super::places::to_json;
use super::{Reach, Run};
use crate::uris::{group_uri, parse_group_uri};
use crate::{SocialResult, refused};

/// What became of the groups home leaves out.
#[derive(Default)]
pub(super) struct Outside {
    /// Groups the person is in after all, to sync as if home listed them.
    pub pointers: Vec<GroupPointer>,
    /// Groups whose provider refused the signature: kept as they are.
    pub spared: HashSet<String>,
}

impl Run<'_> {
    /// Ask about each group kept as joined or asked that home's list
    /// (`listed`, by URI) leaves out.
    pub(super) async fn outside(&mut self, listed: &HashSet<String>) -> SocialResult<Outside> {
        let account = self.actor();
        let kept = self
            .social
            .blocking(move |store| store.social_groups(&account))
            .await?;
        let mut outside = Outside::default();
        for group in kept {
            let ours = matches!(group.found, GroupFound::Member | GroupFound::Asked);
            if !ours || listed.contains(&group.uri) {
                continue;
            }
            let Some((authority, id)) = parse_group_uri(&group.uri) else {
                continue;
            };
            let client = self.client_for(&authority);
            let found = client.group(&id).await;
            match self.reachable(&client, found).await? {
                Reach::Got(found) => {
                    if let Some(pointer) = self.member_after_all(&group, &found).await? {
                        outside.pointers.push(pointer);
                    } else if group.found == GroupFound::Asked {
                        self.keep_outside(&group.uri, &authority, &found, GroupFound::Asked)
                            .await?;
                    }
                }
                Reach::Gone if group.found == GroupFound::Asked => {
                    // Nobody asked can read it: it is gone.
                    let (account, uri) = (self.actor(), group.uri.clone());
                    self.social
                        .blocking(move |store| store.remove_social_group(&account, &uri))
                        .await?;
                }
                Reach::Gone => {}
                Reach::Refused => {
                    self.spared.insert(group.uri.clone());
                    outside.spared.insert(group.uri.clone());
                }
            }
        }
        Ok(outside)
    }

    /// The pointer to sync `group` by, if its provider says the person is
    /// a member (§5.5 `membership`). A group elsewhere gets its pointer at
    /// home written again; if home will not take it now, a later sync
    /// tries again.
    async fn member_after_all(
        &mut self,
        group: &SocialGroup,
        found: &Group,
    ) -> SocialResult<Option<GroupPointer>> {
        let Some(membership) = &found.membership else {
            return Ok(None);
        };
        let uri = ObjectUri::parse(&group.uri).expect("a kept group's URI parses");
        let pointer = GroupPointer {
            group: uri.clone(),
            role: Some(membership.role.clone()),
            updated_at: membership.joined_at,
        };
        if uri.authority() == self.identity.actor().domain() {
            return Ok(Some(pointer));
        }
        let home = self.home.clone();
        let request = GroupPointerRequest {
            role: Some(membership.role.clone()),
        };
        match home.put_group_pointer(&uri, &request).await {
            Ok(_) => {}
            Err(error) if refused::account_state(&home, &error).is_none() => {
                tracing::warn!(group = %group.uri, %error, "group pointer not written; tried again on the next sync");
            }
            Err(error) => return self.checked(&home, Err(error)).await,
        }
        Ok(Some(pointer))
    }

    /// Keep the group of a channel followed without joining, unless the
    /// person is in it or asked to be. Each group is read once a sync;
    /// `seen` holds those read, by URI.
    pub(super) async fn followed_group(
        &mut self,
        client: &Client,
        group: &LocalId,
        seen: &mut HashSet<String>,
    ) -> SocialResult<()> {
        let authority = client.provider().endpoint().authority().clone();
        let uri = group_uri(&authority, group);
        if !seen.insert(uri.clone()) {
            return Ok(());
        }
        let (account, key) = (self.actor(), uri.clone());
        let kept = self
            .social
            .blocking(move |store| store.social_group(&account, &key))
            .await?;
        if kept.is_some_and(|g| matches!(g.found, GroupFound::Member | GroupFound::Asked)) {
            return Ok(());
        }
        let found = client.group(group).await;
        match self.reachable(client, found).await? {
            Reach::Got(found) => {
                self.keep_outside(&uri, &authority, &found, GroupFound::Followed)
                    .await
            }
            Reach::Gone => Ok(()),
            Reach::Refused => {
                self.spared.insert(uri);
                Ok(())
            }
        }
    }

    /// Forget the groups kept for a followed channel that no channel
    /// followed now names (`seen`), but those spared.
    pub(super) async fn forget_followed_groups(
        &mut self,
        seen: &HashSet<String>,
    ) -> SocialResult<()> {
        let (account, seen, spared) = (self.actor(), seen.clone(), self.spared.clone());
        self.social
            .blocking(move |store| {
                for group in store.social_groups(&account)? {
                    let stale = group.found == GroupFound::Followed
                        && !seen.contains(&group.uri)
                        && !spared.contains(&group.uri);
                    if stale {
                        store.remove_social_group(&account, &group.uri)?;
                    }
                }
                Ok(())
            })
            .await
    }

    /// Keep a group the person is not in, as `found` says they know it,
    /// keeping its notices.
    async fn keep_outside(
        &mut self,
        uri: &str,
        authority: &Authority,
        group: &Group,
        found: GroupFound,
    ) -> SocialResult<()> {
        let record = SocialGroup {
            account: self.actor(),
            uri: uri.to_owned(),
            provider: authority.to_string(),
            group: to_json(group),
            role: None,
            found,
            notices: None,
        };
        self.social
            .blocking(move |store| {
                let notices = store
                    .social_group(&record.account, &record.uri)?
                    .and_then(|g| g.notices);
                store.put_social_group(&SocialGroup { notices, ..record })
            })
            .await
    }
}
