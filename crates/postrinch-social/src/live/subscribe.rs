//! Subscribing what a provider hosts for the account, each place from
//! its stored cursor (§7.1 "Resuming after a disconnect").

use std::collections::{BTreeMap, BTreeSet};

use postrinch_ofscp::ofscp_core::ws::Subscribe;
use postrinch_ofscp::ofscp_core::{DmId, LocalId};
use postrinch_ofscp::ws::RequestError;
use postrinch_store::PlaceKind;

use super::backfill::backfill;
use super::session::Live;
use crate::SocialResult;
use crate::uris::parse_channel_uri;

/// A place as a session subscribes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Scope {
    /// A channel of the session's provider.
    Channel {
        place: String,
        group: LocalId,
        channel: LocalId,
    },
    /// A private conversation, at home (§7.4: a `dmId` may be subscribed,
    /// and resumed, as a channel is).
    Conversation { place: String, dm: DmId },
}

impl Scope {
    /// The place's id in the store.
    pub(crate) fn place(&self) -> &str {
        match self {
            Scope::Channel { place, .. } | Scope::Conversation { place, .. } => place,
        }
    }
}

/// A place the provider hosts, with the cursor it resumes from.
struct Hosted {
    scope: Scope,
    cursor: Option<String>,
}

impl Live<'_> {
    /// Subscribe every place the provider hosts that is not subscribed,
    /// from its cursor, and unsubscribe what it no longer hosts. A place
    /// the provider answers `truncated` for, or with no cursor yet, is
    /// backfilled over REST (§7.2); those are given back.
    pub(crate) async fn resubscribe(&mut self) -> SocialResult<Vec<String>> {
        let hosted = self.hosted().await?;
        let dropped: Vec<LocalId> = self
            .subscribed
            .keys()
            .filter(|id| !hosted.contains_key(*id))
            .cloned()
            .collect();
        if !dropped.is_empty() {
            match self.conn.unsubscribe(dropped.clone()).await {
                Ok(_) => {}
                // Not subscribed there any more anyway.
                Err(RequestError::Rejected(error)) => {
                    tracing::info!(provider = %self.cx.authority, %error.message, "unsubscribe refused");
                }
                Err(error) => return Err(error.into()),
            }
            for id in &dropped {
                self.subscribed.remove(id);
            }
        }
        let new: Vec<(&LocalId, &Hosted)> = hosted
            .iter()
            .filter(|(id, _)| !self.subscribed.contains_key(*id) && !self.revoked.contains(*id))
            .collect();
        let mut to_backfill = Vec::new();
        let batch = self.cx.social.live.options.subscribe_batch.max(1);
        for chunk in new.chunks(batch) {
            let since: BTreeMap<LocalId, String> = chunk
                .iter()
                .filter_map(|(id, h)| h.cursor.clone().map(|c| ((*id).clone(), c)))
                .collect();
            let subscribe = Subscribe {
                channels: chunk.iter().map(|(id, _)| (*id).clone()).collect(),
                include: None,
                since: (!since.is_empty()).then_some(since),
            };
            let ack = match self.conn.subscribe(subscribe).await {
                Ok(ack) => ack,
                // None of the batch could be subscribed: gone, not readable,
                // or over the provider's limit. The next refresh tries again.
                Err(RequestError::Rejected(error)) => {
                    tracing::warn!(provider = %self.cx.authority, code = %error.code, %error.message, "subscribe refused");
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let truncated: BTreeSet<LocalId> =
                ack.truncated.unwrap_or_default().into_iter().collect();
            for id in ack.channels {
                let Some(found) = hosted.get(&id) else {
                    continue;
                };
                if found.cursor.is_none() || truncated.contains(&id) {
                    to_backfill.push(found.scope.clone());
                }
                self.subscribed.insert(id, found.scope.clone());
            }
        }
        let mut backfilled = Vec::new();
        for scope in to_backfill {
            backfill(self.cx, &scope).await?;
            backfilled.push(scope.place().to_owned());
        }
        Ok(backfilled)
    }

    /// The places the account keeps that this provider hosts, by the id
    /// the provider knows them by: the channels on it, and at home the
    /// private conversations.
    async fn hosted(&self) -> SocialResult<BTreeMap<LocalId, Hosted>> {
        let account = self.cx.actor();
        let places = self
            .cx
            .social
            .blocking(move |store| store.social_places(&account))
            .await?;
        let mut hosted = BTreeMap::new();
        for place in places {
            let cursor = place.cursors.live.clone();
            match place.kind {
                PlaceKind::Channel => {
                    let Some(at) = parse_channel_uri(&place.id) else {
                        continue;
                    };
                    if at.authority != self.cx.authority {
                        continue;
                    }
                    let scope = Scope::Channel {
                        place: place.id,
                        group: at.group,
                        channel: at.channel.clone(),
                    };
                    hosted.insert(at.channel, Hosted { scope, cursor });
                }
                PlaceKind::Conversation if self.cx.is_home() => {
                    let (Ok(dm), Ok(id)) = (DmId::parse(&place.id), LocalId::new(&place.id)) else {
                        continue;
                    };
                    let scope = Scope::Conversation {
                        place: place.id,
                        dm,
                    };
                    hosted.insert(id, Hosted { scope, cursor });
                }
                _ => {}
            }
        }
        Ok(hosted)
    }
}
