//! A place's newest page of messages, kept (docs/ofscp.md, "Storage":
//! a place keeps the newest page when first seen, everything live after
//! that, and older pages on scrolling).

use postrinch_ofscp::http::PageRequest;
use postrinch_ofscp::ofscp_core::wire::{Message, MessagesPage, Reaction};

use super::Run;
use super::message::{Reader, keep, messages_of};
use super::places::{Marker, Source, Target};
use crate::SocialResult;

impl Run<'_> {
    /// Fetch and keep `target`'s newest page.
    pub(super) async fn page(&mut self, target: &Target) -> SocialResult<()> {
        let Some(page) = self.newest(target).await? else {
            return Ok(());
        };
        let older = page.page.next_cursor.clone();
        let messages = messages_of(page.items);
        let (account, place) = (self.actor(), target.place.clone());
        let kept_place = self
            .social
            .blocking(move |store| store.social_place(&account, &place))
            .await?;
        // A place with no provider marker reads by its own once it has one.
        let marker = match (&target.marker, kept_place.and_then(|p| p.read_marker)) {
            (Marker::Local, Some(local)) => Marker::Provider(Some(local)),
            (marker, _) => marker.clone(),
        };
        let unread = unread(&messages, &marker);
        let reader = Reader {
            account: self.identity.actor(),
            place: &target.place,
            host: &target.host,
        };
        let kept: Vec<_> = messages
            .iter()
            .zip(&unread)
            .map(|((message, reactions), &unread)| keep(&reader, message, reactions, unread))
            .collect();
        let newest = messages
            .last()
            .map(|(m, _)| (m.id.to_string(), m.created_at.as_millisecond()));
        let count = kept.len();
        let (account, place) = (self.actor(), target.place.clone());
        self.social
            .blocking(move |store| {
                for k in &kept {
                    store.put_social_message(&k.message, &k.search_text)?;
                }
                store.change_social_place(&account, &place, |p| {
                    if let Some((id, created)) = newest {
                        p.last_activity = p.last_activity.max(Some(created));
                        match marker {
                            Marker::Provider(read) => p.read_marker = read.or(p.read_marker.take()),
                            Marker::Local => p.read_marker = Some(id),
                        }
                    }
                    // The first page sets where scrolling back starts; later
                    // syncs leave it to the scrolling.
                    if p.cursors.older.is_none() && !p.cursors.complete {
                        p.cursors.complete = older.is_none();
                        p.cursors.older = older;
                    }
                })?;
                Ok(())
            })
            .await?;
        self.synced.messages += count;
        Ok(())
    }

    /// The newest page of `target`, or `None` when it is out of reach.
    async fn newest(&mut self, target: &Target) -> SocialResult<Option<MessagesPage>> {
        let request = PageRequest::newest();
        match &target.source {
            Source::Channel {
                client,
                group,
                channel,
            } => {
                let found = client.messages(group, channel, &request).await;
                self.reachable(client, found).await
            }
            Source::Conversation { dm } => {
                let home = self.home.clone();
                let found = home.dm_messages(dm, &request).await;
                self.reachable(&home, found).await
            }
        }
    }
}

/// Which of a page's messages (oldest first) are unread by `marker`: those
/// after the newest one read. A marker not on the page is older than all
/// of it, as markers only move forward and this is the newest page.
fn unread(messages: &[(Message, Vec<Reaction>)], marker: &Marker) -> Vec<bool> {
    match marker {
        Marker::Local => vec![false; messages.len()],
        Marker::Provider(None) => vec![true; messages.len()],
        Marker::Provider(Some(read)) => {
            let at = messages.iter().position(|(m, _)| m.id.as_str() == read);
            (0..messages.len())
                .map(|i| at.is_none_or(|at| i > at))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn page(ids: &[&str]) -> Vec<(Message, Vec<Reaction>)> {
        ids.iter()
            .map(|id| {
                let m: Message = serde_json::from_value(json!({
                    "id": id, "author": "bob@a.example", "type": "message",
                    "content": { "text": "x", "mime": "text/plain" },
                    "attachments": [], "createdAt": "2026-01-01T12:00:00Z",
                }))
                .unwrap();
                (m, Vec::new())
            })
            .collect()
    }

    #[test]
    fn messages_after_the_marker_are_unread() {
        let p = page(&["m1", "m2", "m3"]);
        assert_eq!(
            unread(&p, &Marker::Provider(Some("m2".into()))),
            [false, false, true]
        );
        assert_eq!(unread(&p, &Marker::Provider(None)), [true, true, true]);
        assert_eq!(
            unread(&p, &Marker::Provider(Some("m0".into()))),
            [true, true, true]
        );
        assert_eq!(unread(&p, &Marker::Local), [false, false, false]);
    }
}
