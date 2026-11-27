//! What a sync finds gone (docs/ofscp.md, "Groups on other providers"):
//! a group that answers `403` or `404` is no longer the person's, so its
//! pointer at home is dropped and what is kept of it forgotten; and what
//! the home provider no longer lists is forgotten here too.

use std::collections::HashSet;

use postrinch_ofscp::ofscp_core::wire::GroupPointer;
use postrinch_store::{GroupFound, PlaceKind, Store, StoreResult};

use super::Run;
use crate::{SocialResult, refused};

impl Run<'_> {
    /// `pointer`'s group answered `403` or `404`: drop the pointer, if it
    /// is one to another provider (a group at home is listed by
    /// membership, not by a pointer), and forget the group here, all but
    /// the channels in `followed`.
    pub(super) async fn group_gone(
        &mut self,
        pointer: &GroupPointer,
        uri: &str,
        followed: &HashSet<String>,
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
        let (account, uri, followed) = (self.actor(), uri.to_owned(), followed.clone());
        self.social
            .blocking(move |store| forget_group(store, &account, &uri, &followed))
            .await
    }

    /// Forget the groups the home provider no longer lists the person in
    /// (`listed` are those it does), all but the channels in `followed`.
    pub(super) async fn forget_groups(
        &mut self,
        listed: &HashSet<String>,
        followed: &HashSet<String>,
    ) -> SocialResult<()> {
        let (account, listed, followed) = (self.actor(), listed.clone(), followed.clone());
        self.social
            .blocking(move |store| {
                for group in store.social_groups(&account)? {
                    if group.found == GroupFound::Member && !listed.contains(&group.uri) {
                        forget_group(store, &account, &group.uri, &followed)?;
                    }
                }
                Ok(())
            })
            .await
    }

    /// Forget the channels kept for being in one of the person's groups or
    /// followed that are not among `places` (no longer listed, followed or
    /// readable), except what is spared. Private conversations stay: a
    /// sync reads only the newest of them.
    pub(super) async fn forget_places(&mut self, places: &HashSet<String>) -> SocialResult<()> {
        let (account, places, spared) = (self.actor(), places.clone(), self.spared.clone());
        self.social
            .blocking(move |store| {
                let members: HashSet<String> = store
                    .social_groups(&account)?
                    .into_iter()
                    .filter(|g| g.found == GroupFound::Member)
                    .map(|g| g.uri)
                    .collect();
                for place in store.social_places(&account)? {
                    let group = place.group.as_ref();
                    let ours = place.followed || group.is_some_and(|g| members.contains(g));
                    let spared =
                        spared.contains(&place.id) || group.is_some_and(|g| spared.contains(g));
                    let gone = !places.contains(&place.id);
                    if place.kind == PlaceKind::Channel && ours && gone && !spared {
                        store.remove_social_place(&account, &place.id)?;
                    }
                }
                Ok(())
            })
            .await
    }
}

/// Forget group `uri` with its channels, all but those in `followed`: they
/// are kept apart from it, as a channel followed without joining is.
fn forget_group(
    store: &Store,
    account: &str,
    uri: &str,
    followed: &HashSet<String>,
) -> StoreResult<()> {
    for place in store.social_places_of_group(account, uri)? {
        if followed.contains(&place.id) {
            store.change_social_place(account, &place.id, |p| p.group = None)?;
        }
    }
    store.remove_social_group(account, uri)
}
