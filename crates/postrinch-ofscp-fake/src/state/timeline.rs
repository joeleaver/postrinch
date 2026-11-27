//! A timeline: the messages of one channel or one DM conversation, and the
//! log of events that resume replays (§7.1, §7.2).
//!
//! Messages and events share one per-timeline sequence, and a cursor is
//! that sequence number tagged with the timeline it belongs to. So the
//! cursor of a live event and the cursors of REST pages are in one space,
//! as §7.1 asks, and a cursor from another timeline is refused.

use std::collections::HashMap;

use jiff::Timestamp;
use ofscp_core::wire::{Message, Page, Paged, Reaction};
use ofscp_core::{Actor, LocalId};

use crate::problem::Problem;

/// One message as stored, with the reactions on it.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    pub(crate) seq: u64,
    pub(crate) message: Message,
    pub(crate) reactions: Vec<Reaction>,
}

/// A timeline event as replayed (§7.1 *Resuming*).
#[derive(Debug, Clone)]
pub(crate) enum TimelineEvent {
    Created(Message),
    Updated(Message),
    Deleted { id: LocalId, at: Timestamp },
}

/// An event with its cursor.
#[derive(Debug, Clone)]
pub(crate) struct Logged {
    pub(crate) cursor: String,
    pub(crate) event: TimelineEvent,
}

/// What a resume found after a cursor.
#[derive(Debug, Clone)]
pub(crate) enum Replay {
    Events(Vec<Logged>),
    Truncated,
}

#[derive(Debug)]
pub(crate) struct Timeline {
    tag: String,
    seq: u64,
    messages: Vec<Stored>,
    index: HashMap<LocalId, usize>,
    log: Vec<(u64, TimelineEvent)>,
    client_ids: HashMap<(Actor, String), LocalId>,
}

/// Paging parameters, as §7.2 reads them.
#[derive(Debug, Clone, Default)]
pub(crate) struct PageQuery {
    pub(crate) cursor: Option<String>,
    pub(crate) forward: bool,
    pub(crate) limit: usize,
}

impl Timeline {
    pub(crate) fn new(tag: &str) -> Self {
        Self {
            tag: tag.to_owned(),
            seq: 0,
            messages: Vec::new(),
            index: HashMap::new(),
            log: Vec::new(),
            client_ids: HashMap::new(),
        }
    }

    fn cursor(&self, seq: u64) -> String {
        format!("{}.{seq}", self.tag)
    }

    fn parse_cursor(&self, cursor: &str) -> Option<u64> {
        let (tag, seq) = cursor.rsplit_once('.')?;
        (tag == self.tag).then(|| seq.parse().ok())?
    }

    pub(crate) fn get(&self, id: &LocalId) -> Option<&Stored> {
        self.index.get(id).map(|&i| &self.messages[i])
    }

    pub(crate) fn get_mut(&mut self, id: &LocalId) -> Option<&mut Stored> {
        self.index.get(id).map(|&i| &mut self.messages[i])
    }

    /// The message an earlier create with this key made (§7.1 idempotency).
    pub(crate) fn by_client_id(&self, author: &Actor, client_id: &str) -> Option<&Stored> {
        let id = self
            .client_ids
            .get(&(author.clone(), client_id.to_owned()))?;
        self.get(id)
    }

    pub(crate) fn messages(&self) -> impl Iterator<Item = &Stored> {
        self.messages.iter()
    }

    /// Store a new message; returns its event's cursor.
    pub(crate) fn push(&mut self, message: Message) -> String {
        self.seq += 1;
        let seq = self.seq;
        if let Some(client_id) = &message.client_message_id {
            self.client_ids.insert(
                (message.author.clone(), client_id.clone()),
                message.id.clone(),
            );
        }
        self.index.insert(message.id.clone(), self.messages.len());
        self.log
            .push((seq, TimelineEvent::Created(message.clone())));
        self.messages.push(Stored {
            seq,
            message,
            reactions: Vec::new(),
        });
        self.cursor(seq)
    }

    /// Log a change already made to a stored message; returns its cursor.
    pub(crate) fn log_change(&mut self, event: TimelineEvent) -> String {
        self.seq += 1;
        self.log.push((self.seq, event));
        self.cursor(self.seq)
    }

    /// The events strictly after `cursor`, or `Truncated` when there are
    /// more than `limit` of them or the cursor is not this timeline's.
    pub(crate) fn replay(&self, cursor: &str, limit: Option<usize>) -> Replay {
        let Some(after) = self.parse_cursor(cursor).filter(|&s| s <= self.seq) else {
            return Replay::Truncated;
        };
        let events: Vec<Logged> = self
            .log
            .iter()
            .filter(|(seq, _)| *seq > after)
            .map(|(seq, event)| Logged {
                cursor: self.cursor(*seq),
                event: event.clone(),
            })
            .collect();
        match limit {
            Some(limit) if events.len() > limit => Replay::Truncated,
            _ => Replay::Events(events),
        }
    }

    /// One §7.2 page of the messages `keep` selects.
    pub(crate) fn page(
        &self,
        query: &PageQuery,
        keep: impl Fn(&Stored) -> bool,
    ) -> Result<Paged<Message>, Problem> {
        let at = match &query.cursor {
            None => None,
            Some(c) => Some(
                self.parse_cursor(c)
                    .ok_or_else(|| Problem::bad_request("the cursor is not from this timeline"))?,
            ),
        };
        let beyond = |s: &Stored| match at {
            None => true,
            Some(at) if query.forward => s.seq > at,
            Some(at) => s.seq < at,
        };
        let mut chosen: Vec<&Stored> = if query.forward {
            self.messages
                .iter()
                .filter(|s| beyond(s) && keep(s))
                .collect()
        } else {
            self.messages
                .iter()
                .rev()
                .filter(|s| beyond(s) && keep(s))
                .collect()
        };
        let more = chosen.len() > query.limit;
        chosen.truncate(query.limit);
        let page = Page {
            next_cursor: if more {
                chosen.last().map(|s| self.cursor(s.seq))
            } else {
                None
            },
            prev_cursor: chosen.first().map(|s| self.cursor(s.seq)),
        };
        Ok(Paged {
            items: chosen.into_iter().map(|s| s.message.clone()).collect(),
            page,
        })
    }
}
