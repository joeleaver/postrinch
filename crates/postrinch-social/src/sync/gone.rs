//! What a sync finds gone (docs/ofscp.md, "Groups on other providers"):
//! a group that answers `403` or `404` is no longer the person's, so its
//! pointer at home is dropped and what is kept of it forgotten; and what
//! the home provider no longer lists is forgotten here too.

use std::collections::HashSet;

use postrinch_ofscp::ofscp_core::wire::GroupPointer;
use postrinch_store::{GroupFound, PlaceKind};

use super::Run;
use crate::{SocialResult, refused};

impl Run<'_> {
    /// `pointer`'s group answered `403` or `404`: drop the pointer, if it
    /// is one to another provider (a group at home is listed by
    /// membership, not by a pointer), and forget the group here.
    pub(super) async fn group_gone(
        &mut self,
        pointer: &GroupPointer,
        uri: &str,
    ) -> SocialResult<()> {
        if pointer.group.authority() != self.identity.actor().domain() {
            let home = self.home.clone();
            match home.delete_group_pointer(&pointer.group).await {
                Ok(()) => {}
                // The pointer stays, and goes on a later sync.
                Err(error) if refused::account_state(&home, &error).is_none() => {
                    tracing::warn!(group = %pointer.group, %error, "group pointer not dropped");
                }
                Err(error) => return self.checked(&home, Err(error)).await,
            }
        }
        let (account, uri) = (self.actor(), uri.to_owned());
        self.social
            .blocking(move |store| store.remove_social_group(&account, &uri))
            .await
    }

    /// Forget the groups the home provider no longer lists the person in,
    /// and the channels kept for being in one of them or followed that are
    /// not among `places` (no longer listed, followed or readable).
    /// Private conversations stay: a sync reads only the newest of them.
    pub(super) async fn forget_the_rest(
        &mut self,
        groups: HashSet<String>,
        places: HashSet<String>,
    ) -> SocialResult<()> {
        let account = self.actor();
        self.social
            .blocking(move |store| {
                let mut members = HashSet::new();
                for group in store.social_groups(&account)? {
                    if group.found != GroupFound::Member {
                        continue;
                    }
                    if groups.contains(&group.uri) {
                        members.insert(group.uri);
                    } else {
                        store.remove_social_group(&account, &group.uri)?;
                    }
                }
                for place in store.social_places(&account)? {
                    let ours =
                        place.followed || place.group.as_ref().is_some_and(|g| members.contains(g));
                    if place.kind == PlaceKind::Channel && ours && !places.contains(&place.id) {
                        store.remove_social_place(&account, &place.id)?;
                    }
                }
                Ok(())
            })
            .await
    }
}
