//! Read state (docs/ofscp.md "Sync engine", Unread): the person's, kept
//! here and changed at once. A channel the home provider holds has a read
//! marker there too (§7.7), moved forward through the outbox; other places
//! have none (§7.7 L1932), so theirs is kept here alone.

use postrinch_store::{PlaceKind, SocialChange, SocialPlace, social_account_key};

use super::{actor_domain, uri_domain};
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// Mark read a place's messages up to `up_to` (by id), or all of them
    /// when `None`. Returns how many were unread.
    pub fn mark_social_read(
        &self,
        account: &str,
        place: &str,
        up_to: Option<&str>,
    ) -> CommandResult<usize> {
        let place = self.check_place(account, place)?;
        let account = social_account_key(&place.account);
        let newest = match up_to {
            Some(id) => match self.store.social_message(&account, &place.id, id)? {
                Some(message) => message,
                None => {
                    return Err(CommandError::Invalid(format!(
                        "There is no message {id} here."
                    )));
                }
            },
            None => match self
                .store
                .social_unread_messages(&account, &place.id)?
                .into_iter()
                .next()
            {
                Some(message) => message,
                None => return Ok(0),
            },
        };
        let marked = self
            .store
            .set_social_messages_read(&account, &place.id, newest.created)?;
        let marker = self.move_marker(&place, &newest.id, newest.created)?;
        if marker != Marker::Behind && has_marker(&place) {
            let change = SocialChange::Read {
                message: newest.id.clone(),
            };
            self.queue_change(&account, Some(&place.id), &change)?;
        }
        Ok(marked)
    }

    /// Mark one message unread again. The read marker stays where it is:
    /// markers move forward only (§7.7).
    pub fn mark_social_unread(
        &self,
        account: &str,
        place: &str,
        message: &str,
    ) -> CommandResult<()> {
        let place = self.check_place(account, place)?;
        if !self
            .store
            .set_social_message_unread(&place.account, &place.id, message, true)?
        {
            return Err(CommandError::Invalid(format!(
                "There is no message {message} here."
            )));
        }
        Ok(())
    }

    /// Move the place's read marker to `id`, made at `created`, if that is
    /// further on than where it is.
    fn move_marker(&self, place: &SocialPlace, id: &str, created: i64) -> CommandResult<Marker> {
        let account = social_account_key(&place.account);
        loop {
            let Some(now) = self.store.social_place(&account, &place.id)? else {
                return Ok(Marker::Behind);
            };
            let marker = now.read_marker.clone();
            if let Some(at) = &marker {
                match self.store.social_message(&account, &place.id, at)? {
                    Some(m) if (m.created, m.id.as_str()) >= (created, id) => {
                        return Ok(Marker::Behind);
                    }
                    Some(_) => {}
                    // A marker at a message not kept here (one not fetched
                    // yet, newer or older) can't be ordered here: it stays,
                    // and the provider, which moves markers forward only
                    // (§7.7), decides.
                    None => return Ok(Marker::Unknown),
                }
            }
            // Only if sync did not move it meanwhile; else look again.
            let mut moved = false;
            self.store.change_social_place(&account, &place.id, |p| {
                if p.read_marker == marker {
                    p.read_marker = Some(id.to_owned());
                    moved = true;
                }
            })?;
            if moved {
                return Ok(Marker::Moved);
            }
        }
    }
}

/// Where a read marker is, against a message read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// Moved on to the message.
    Moved,
    /// At or past it already: nothing to tell the provider.
    Behind,
    /// At a message not kept here, so not known to be before or after it.
    Unknown,
}

/// Whether the place has a read marker at its provider: a channel the
/// account's home provider holds (§7.7).
fn has_marker(place: &SocialPlace) -> bool {
    place.kind == PlaceKind::Channel
        && uri_domain(&place.id).is_some_and(|host| host == actor_domain(&place.account))
}
