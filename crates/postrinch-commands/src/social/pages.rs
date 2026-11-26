//! Pages of messages: a place's, or a view's, its places merged by when
//! each message was made (docs/ofscp.md "Organizing", "Reading"). Only
//! messages that answer none are listed; replies are a thread's
//! ([`super::threads`]), but for Mentions, which lists every message that
//! mentions the person. Newest first, a page at a time, each page going on
//! from the oldest message of the one before.

use std::collections::{HashMap, HashSet};

use postrinch_store::{PlaceRef, SocialMessage, SocialPlace, social_account_key};
use serde::{Deserialize, Serialize};

use super::filters::Filter;
use super::overlay::{PlaceOutbox, SocialItem, client_ids};
use super::views_built_in::BuiltInView;
use crate::{CommandError, CommandResult, Commands};

/// How many messages are read from a place at a time while filling a page.
const CHUNK: usize = 50;

/// A page of messages, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocialPage {
    pub items: Vec<SocialItem>,
    /// Where the next older page starts; `None` when this is the last.
    pub older: Option<SocialCursor>,
}

/// Where a page starts: just older than this message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialCursor {
    pub created: i64,
    /// Its account, by key.
    pub account: String,
    pub place: String,
    pub id: String,
}

impl SocialCursor {
    fn of(message: &SocialMessage) -> Self {
        Self {
            created: message.created,
            account: social_account_key(&message.account),
            place: message.place.clone(),
            id: message.id.clone(),
        }
    }
}

impl Commands {
    /// Up to `limit` messages of a view, from just before `before` or from
    /// the newest; of one of its places alone with `tab`, or of all of them
    /// ("All").
    pub fn social_view_page(
        &self,
        view: &str,
        tab: Option<&PlaceRef>,
        before: Option<&SocialCursor>,
        limit: usize,
    ) -> CommandResult<SocialPage> {
        check_limit(limit)?;
        let entry = self.social_view(view)?;
        if let Some(kind) = entry.built_in.filter(|k| !k.shows_messages()) {
            return Err(CommandError::Invalid(format!(
                "{} lists {}, not messages.",
                entry.view.name,
                if kind == BuiltInView::Drafts {
                    "drafts"
                } else {
                    "places"
                }
            )));
        }
        let filter = Filter::of_view(&entry.view.filters)?;
        let mut places = self.view_places(&entry)?;
        if let Some(tab) = tab {
            places.retain(|p| {
                social_account_key(&p.account) == social_account_key(&tab.account) && p.id == tab.id
            });
            if places.is_empty() {
                return Err(CommandError::Invalid(format!(
                    "{} isn't one of {}'s places.",
                    tab.id, entry.view.name
                )));
            }
        }
        let items = if entry.built_in == Some(BuiltInView::Mentions) {
            self.mentions(&places, before, &filter)?
        } else {
            let mut items = Vec::new();
            for place in &places {
                items.extend(self.place_items(place, before, limit, &filter)?);
            }
            items
        };
        Ok(page(items, limit))
    }

    /// Up to `limit` of a place's messages that answer none, from just
    /// before `before` or from the newest.
    pub fn social_place_page(
        &self,
        account: &str,
        place: &str,
        before: Option<&SocialCursor>,
        limit: usize,
    ) -> CommandResult<SocialPage> {
        check_limit(limit)?;
        let place = self.check_place(account, place)?;
        let items = self.place_items(&place, before, limit, &Filter::default())?;
        Ok(page(items, limit))
    }

    /// Up to `limit + 1` of a place's messages that answer none and that
    /// `filter` keeps, older than `before`, newest first: those sync kept
    /// and those being sent, with their pending changes made.
    pub(crate) fn place_items(
        &self,
        place: &SocialPlace,
        before: Option<&SocialCursor>,
        limit: usize,
        filter: &Filter,
    ) -> CommandResult<Vec<SocialItem>> {
        let (account, id) = (social_account_key(&place.account), place.id.as_str());
        let outbox = self.place_outbox(&account, id)?;
        // From the cursor's time on, those made at that very time sorted
        // out below by the whole order.
        let mut from: Option<(i64, String)> =
            before.map(|c| (c.created.saturating_add(1), String::new()));
        let mut items = Vec::new();
        let mut kept = HashSet::new();
        loop {
            let at = from.as_ref().map(|(created, id)| (*created, id.as_str()));
            let chunk = self.store.social_messages(&account, id, at, CHUNK)?;
            let read = chunk.len();
            from = chunk.last().map(|m| (m.created, m.id.clone()));
            kept.extend(client_ids(&chunk));
            for message in chunk {
                if before.is_none_or(|c| older(&message, c)) {
                    let item = outbox.lay_over(message, &account);
                    if filter.keeps(&item.message) {
                        items.push(item);
                    }
                }
            }
            if read < CHUNK || items.len() > limit {
                break;
            }
        }
        items.extend(
            outbox
                .queued(&account, id, None, &kept)
                .into_iter()
                .filter(|item| before.is_none_or(|c| older(&item.message, c)))
                .filter(|item| filter.keeps(&item.message)),
        );
        newest_first(&mut items);
        items.truncate(limit + 1);
        self.count_replies(&outbox, &mut items)?;
        Ok(items)
    }

    /// The messages of `places` that mention the person and `filter`
    /// keeps, older than `before`, newest first.
    fn mentions(
        &self,
        places: &[SocialPlace],
        before: Option<&SocialCursor>,
        filter: &Filter,
    ) -> CommandResult<Vec<SocialItem>> {
        let wanted: HashSet<(String, &str)> = places
            .iter()
            .map(|p| (social_account_key(&p.account), p.id.as_str()))
            .collect();
        let mut outboxes: HashMap<(String, String), PlaceOutbox> = HashMap::new();
        let mut items = Vec::new();
        for message in self.store.social_mentions(usize::MAX)? {
            let key = (social_account_key(&message.account), message.place.clone());
            if !wanted.contains(&(key.0.clone(), key.1.as_str()))
                || before.is_some_and(|c| !older(&message, c))
            {
                continue;
            }
            let outbox = match outboxes.entry(key) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    let (account, place) = v.key();
                    let outbox = self.place_outbox(account, place)?;
                    v.insert(outbox)
                }
            };
            let account = social_account_key(&message.account);
            let mut item = outbox.lay_over(message, &account);
            if filter.keeps(&item.message) {
                item.replies = self.replies_of(outbox, &item)?;
                items.push(item);
            }
        }
        Ok(items)
    }

    /// Fill in how many messages answer each of `items`, all of one place.
    pub(crate) fn count_replies(
        &self,
        outbox: &PlaceOutbox,
        items: &mut [SocialItem],
    ) -> CommandResult<()> {
        for item in items {
            item.replies = self.replies_of(outbox, item)?;
        }
        Ok(())
    }

    /// How many messages answer `item`, those being sent among them.
    pub(crate) fn replies_of(
        &self,
        outbox: &PlaceOutbox,
        item: &SocialItem,
    ) -> CommandResult<usize> {
        if !item.posted {
            return Ok(0);
        }
        let m = &item.message;
        let kept = self.store.social_reply_count(&m.account, &m.place, &m.id)?;
        let queued = outbox.queued_replies(&m.id);
        if queued.is_empty() {
            return Ok(kept);
        }
        // A post sync has kept, its change not yet taken off the outbox,
        // is counted once.
        let sent = client_ids(&self.store.social_replies(&m.account, &m.place, &m.id)?);
        Ok(kept + queued.into_iter().filter(|id| !sent.contains(*id)).count())
    }
}

/// The first `limit` of `items`, newest first, and where the next page
/// starts when there are more.
fn page(mut items: Vec<SocialItem>, limit: usize) -> SocialPage {
    newest_first(&mut items);
    let more = items.len() > limit;
    items.truncate(limit);
    let older = more
        .then(|| items.last().map(|i| SocialCursor::of(&i.message)))
        .flatten();
    SocialPage { items, older }
}

/// Sort newest first, in the whole order pages go by.
fn newest_first(items: &mut [SocialItem]) {
    items.sort_by(|a, b| order(&b.message).cmp(&order(&a.message)));
}

/// Where a message comes in the order pages go by, oldest least.
fn order(m: &SocialMessage) -> (i64, String, &str, &str) {
    (m.created, social_account_key(&m.account), &m.place, &m.id)
}

/// Whether `message` comes before the cursor's, in that order.
fn older(message: &SocialMessage, c: &SocialCursor) -> bool {
    order(message) < (c.created, c.account.clone(), &c.place, &c.id)
}

/// An error for a page of no messages.
fn check_limit(limit: usize) -> CommandResult<()> {
    if limit == 0 {
        return Err(CommandError::Invalid(
            "Ask for at least one message.".into(),
        ));
    }
    Ok(())
}
