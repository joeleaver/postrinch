//! A place's newest page of messages, kept (docs/ofscp.md, "Storage":
//! a place keeps the newest page when first seen, everything live after
//! that, and older pages on scrolling).

use postrinch_ofscp::http::PageRequest;
use postrinch_ofscp::ofscp_core::Authority;
use postrinch_ofscp::ofscp_core::wire::MessagesPage;

use super::Run;
use super::message::{Held, Reader, held_by, in_order, keep};
use super::places::{Marker, Source, Target};
use crate::SocialResult;
use crate::live::apply;

impl Run<'_> {
    /// Fetch and keep `target`'s newest page.
    pub(super) async fn page(&mut self, target: &Target) -> SocialResult<()> {
        let Some(Newest {
            mut held,
            live,
            sent: sent_cursor,
            older,
            complete,
        }) = self.newest(target).await?
        else {
            return Ok(());
        };
        in_order(&mut held);
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
        let unread = unread(&held, &target.host, &marker);
        let reader = Reader {
            account: self.identity.actor(),
            place: &target.place,
            host: &target.host,
        };
        let kept: Vec<_> = held
            .iter()
            .zip(&unread)
            .map(|(held, &unread)| keep(&reader, held, unread))
            .collect();
        let last_activity = held.last().map(|h| h.message.created_at.as_millisecond());
        // A marker names a message the place's host holds.
        let newest_here = held
            .iter()
            .rev()
            .find(|h| h.holder == target.host)
            .map(|h| h.message.id.to_string());
        let count = kept.len();
        let (account, place) = (self.actor(), target.place.clone());
        let sent = self
            .social
            .blocking(move |store| {
                let mut sent = Vec::new();
                for k in &kept {
                    let put = store.put_social_message(&k.message, &k.search_text)?;
                    sent.extend(apply::sent(&k.message, &put));
                }
                store.change_social_place(&account, &place, |p| {
                    p.last_activity = p.last_activity.max(last_activity);
                    match marker {
                        Marker::Provider(read) => p.read_marker = read.or(p.read_marker.take()),
                        Marker::Local => {
                            if newest_here.is_some() {
                                p.read_marker = newest_here;
                            }
                        }
                    }
                    // Live events resume from the newest message here, until
                    // one of them says where they are.
                    if p.cursors.live.is_none() {
                        p.cursors.live = live;
                    }
                    if p.cursors.sent.is_none() {
                        p.cursors.sent = sent_cursor;
                    }
                    // The first page sets where scrolling back starts; later
                    // syncs leave it to the scrolling.
                    if p.cursors.older.is_none() && !p.cursors.complete {
                        p.cursors.complete = complete;
                        p.cursors.older = older;
                    }
                })?;
                Ok(sent)
            })
            .await?;
        for event in sent {
            self.social.live.tell(event);
        }
        self.synced.messages += count;
        Ok(())
    }

    /// The newest page of `target`, or `None` when it is out of reach.
    async fn newest(&mut self, target: &Target) -> SocialResult<Option<Newest>> {
        let request = PageRequest::newest();
        match &target.source {
            Source::Channel {
                client,
                group,
                channel,
            } => {
                let found = client.messages(group, channel, &request).await;
                let page = self.reachable(client, found).await?.got();
                Ok(page.map(|page| Newest::new(&target.host, page)))
            }
            Source::Conversation { dm, other } => {
                let home = self.home.clone();
                let found = home.dm_messages(dm, &request).await;
                let Some(page) = self.reachable(&home, found).await?.got() else {
                    return Ok(None);
                };
                let mut newest = Newest::new(&target.host, page);
                if let Some(other) = other {
                    let found = other.dm_messages(dm, &request).await;
                    if let Some(theirs) = self.reachable(other, found).await?.got() {
                        newest.add_theirs(other.provider().endpoint().authority(), theirs);
                    }
                }
                Ok(Some(newest))
            }
        }
    }
}

/// A place's newest messages, and where scrolling back starts.
struct Newest {
    held: Vec<Held>,
    /// The cursor of the newest message at the place's host: where live
    /// events resume from (§7.1: event cursors and page cursors share one
    /// space, and a page's `prevCursor` marks its first item, §7.2).
    live: Option<String>,
    /// In a conversation with someone on another provider, the cursor of
    /// the newest message there: where its `dmId` subscription there
    /// resumes from (§7.4).
    sent: Option<String>,
    /// The cursor of the next older page, at the place's host (home, for
    /// a conversation).
    older: Option<String>,
    /// Whether nothing is older.
    complete: bool,
}

impl Newest {
    /// The newest page `host` gave.
    fn new(host: &Authority, page: MessagesPage) -> Self {
        Self {
            held: held_by(host, page.items),
            complete: page.page.next_cursor.is_none(),
            live: page.page.prev_cursor,
            sent: None,
            older: page.page.next_cursor,
        }
    }

    /// Add the other provider's newest page of a conversation (§7.4: it
    /// holds the messages the person sent). The conversation is complete
    /// only when both pages are the last; going back through the other
    /// provider's older pages is the backfill's (the plan's PR 9), which
    /// keeps that provider's cursor too.
    fn add_theirs(&mut self, holder: &Authority, theirs: MessagesPage) {
        self.sent = theirs.page.prev_cursor;
        self.held.extend(held_by(holder, theirs.items));
        self.complete &= theirs.page.next_cursor.is_none();
    }
}

/// Which of a page's messages (oldest first) are unread by `marker`, a
/// marker of `host`'s: those it holds after the newest one read. A marker
/// not on the page is older than all of it, as markers only move forward
/// and this is the newest page. The messages of a conversation held
/// elsewhere are the person's own, sent from here.
pub(crate) fn unread(held: &[Held], host: &Authority, marker: &Marker) -> Vec<bool> {
    let here = |h: &Held| h.holder == *host;
    match marker {
        Marker::Local => vec![false; held.len()],
        Marker::Provider(None) => held.iter().map(here).collect(),
        Marker::Provider(Some(read)) => {
            let at = held
                .iter()
                .position(|h| here(h) && h.message.id.as_str() == read);
            held.iter()
                .enumerate()
                .map(|(i, h)| here(h) && at.is_none_or(|at| i > at))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use postrinch_ofscp::ofscp_core::wire::Message;
    use serde_json::json;

    use super::*;

    fn host(name: &str) -> Authority {
        Authority::canonicalize(name).unwrap()
    }

    fn page(ids: &[(&str, &str)]) -> Vec<Held> {
        ids.iter()
            .map(|(holder, id)| {
                let message: Message = serde_json::from_value(json!({
                    "id": id, "author": "bob@a.example", "type": "message",
                    "content": { "text": "x", "mime": "text/plain" },
                    "attachments": [], "createdAt": "2026-01-01T12:00:00Z",
                }))
                .unwrap();
                Held {
                    holder: host(holder),
                    message,
                    reactions: Vec::new(),
                }
            })
            .collect()
    }

    #[test]
    fn messages_after_the_marker_are_unread() {
        let a = host("a.example");
        let p = page(&[
            ("a.example", "m1"),
            ("a.example", "m2"),
            ("a.example", "m3"),
        ]);
        assert_eq!(
            unread(&p, &a, &Marker::Provider(Some("m2".into()))),
            [false, false, true]
        );
        assert_eq!(unread(&p, &a, &Marker::Provider(None)), [true, true, true]);
        assert_eq!(
            unread(&p, &a, &Marker::Provider(Some("m0".into()))),
            [true, true, true]
        );
        assert_eq!(unread(&p, &a, &Marker::Local), [false, false, false]);
    }

    #[test]
    fn a_marker_names_only_messages_its_provider_holds() {
        let a = host("a.example");
        // The person's own m2, held at b, shares an id with the marker.
        let p = page(&[
            ("a.example", "m1"),
            ("b.example", "m2"),
            ("a.example", "m3"),
        ]);
        assert_eq!(
            unread(&p, &a, &Marker::Provider(Some("m2".into()))),
            [true, false, true]
        );
        assert_eq!(unread(&p, &a, &Marker::Provider(None)), [true, false, true]);
    }
}
