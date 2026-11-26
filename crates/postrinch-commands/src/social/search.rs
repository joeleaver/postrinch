//! Searching every place of the accounts a caller sees: words through the
//! full-text index, best first; `from:` and `tag:` narrowing them, or,
//! with no words, listing every message they name, newest first (the
//! grammar is [`super::filters`]'s).

use std::collections::HashMap;

use postrinch_store::{SocialMessage, social_account_key};

use super::filters::Filter;
use super::overlay::{PlaceOutbox, SocialItem};
use crate::{CommandError, CommandResult, Commands};

impl Commands {
    /// Up to `limit` messages `text` finds, replies among them, with their
    /// pending changes made.
    pub fn search_social(&self, text: &str, limit: usize) -> CommandResult<Vec<SocialItem>> {
        let filter = Filter::of_search(text)?;
        if filter == Filter::default() {
            return Err(CommandError::Invalid(
                "Search for some words, from:someone or tag:something.".into(),
            ));
        }
        if limit == 0 {
            return Err(CommandError::Invalid(
                "Ask for at least one message.".into(),
            ));
        }
        let mut shown = Shown::new(self);
        match filter.text() {
            Some(query) => self.search_words(&query, &filter, limit, &mut shown),
            None => self.search_every(&filter, limit, &mut shown),
        }
    }

    /// The best matches for `query` that `filter` keeps: as many as the
    /// index gives until `limit` are kept or it has no more.
    fn search_words(
        &self,
        query: &str,
        filter: &Filter,
        limit: usize,
        shown: &mut Shown,
    ) -> CommandResult<Vec<SocialItem>> {
        let mut asked = limit.saturating_mul(4).max(50);
        loop {
            let found = self.store.search_social_messages(query, asked)?;
            let exhausted = found.len() < asked;
            let mut out = Vec::new();
            for message in found {
                if let Some(item) = shown.item(message, filter, Found::ByIndex)? {
                    out.push(item);
                    if out.len() == limit {
                        return Ok(out);
                    }
                }
            }
            if exhausted {
                return Ok(out);
            }
            asked = asked.saturating_mul(2);
        }
    }

    /// Every message, newest first, that `filter` keeps.
    fn search_every(
        &self,
        filter: &Filter,
        limit: usize,
        shown: &mut Shown,
    ) -> CommandResult<Vec<SocialItem>> {
        let mut out = Vec::new();
        for account in self.store.social_accounts()? {
            if !self.sees_social(&account.actor) {
                continue;
            }
            for place in self.store.social_places(&account.actor)? {
                for message in self
                    .store
                    .social_place_messages(&account.actor, &place.id)?
                {
                    out.extend(shown.item(message, filter, Found::ByScan)?);
                }
            }
        }
        out.sort_by(|a, b| {
            (b.message.created, &b.message.id).cmp(&(a.message.created, &a.message.id))
        });
        out.truncate(limit);
        Ok(out)
    }
}

/// Messages found, as shown to this caller, each place's outbox read once.
struct Shown<'a> {
    commands: &'a Commands,
    outboxes: HashMap<(String, String), PlaceOutbox>,
}

impl<'a> Shown<'a> {
    fn new(commands: &'a Commands) -> Self {
        Self {
            commands,
            outboxes: HashMap::new(),
        }
    }

    /// `message` as shown, when this caller sees its account and `filter`
    /// keeps it as changed. A deleted message is not found.
    fn item(
        &mut self,
        message: SocialMessage,
        filter: &Filter,
        found: Found,
    ) -> CommandResult<Option<SocialItem>> {
        if !self.commands.sees_social(&message.account) {
            return Ok(None);
        }
        let account = social_account_key(&message.account);
        let key = (account.clone(), message.place.clone());
        let outbox = match self.outboxes.entry(key) {
            std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::hash_map::Entry::Vacant(v) => {
                let outbox = self.commands.place_outbox(&v.key().0, &v.key().1)?;
                v.insert(outbox)
            }
        };
        let mut item = outbox.lay_over(message);
        // The index matched the words in the text it holds (the plain
        // text and the author's name), which the filter can't see; only an
        // edit being sent changes them.
        let edited = outbox.edits(&item.message.id);
        let kept = match found {
            Found::ByIndex if !edited => filter.keeps_besides_words(&item.message),
            _ => filter.keeps(&item.message),
        };
        if item.message.deleted || !kept {
            return Ok(None);
        }
        item.replies = self.commands.replies_of(outbox, &item)?;
        Ok(Some(item))
    }
}

/// How a message was found.
#[derive(Clone, Copy)]
enum Found {
    /// By the full-text index, for the filter's words.
    ByIndex,
    /// By going through every message.
    ByScan,
}
