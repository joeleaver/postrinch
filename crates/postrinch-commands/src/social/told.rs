//! New social messages to tell the person of, as desktop notices tell new
//! mail (`crate::arrived`): the unread ones made since a moment that get
//! through their places' notices ([`super::notices`]), each with what a
//! notice says and where a click on it opens.

use postrinch_store::{PlaceKind, PlaceRef, SocialMessage, social_account_key};
use serde::{Deserialize, Serialize};

use super::notices::{Heard, Hearer, Hearing, Notice};
use super::places::place_name;
use super::str_at;
use super::views_built_in::{ALL_CHANNELS, HOME, MENTIONS, MESSAGES};
use crate::{CommandResult, Commands};

/// How long a notice's snippet runs, in characters, before it is cut.
const SNIPPET: usize = 140;

/// A new message to tell the person of.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocialArrived {
    pub place: PlaceRef,
    /// A channel's name, or the other person in a conversation.
    pub place_name: String,
    /// A channel's group's name.
    pub group_name: Option<String>,
    pub conversation: bool,
    /// The message's id in its place.
    pub message: String,
    /// Its author's actor.
    pub author: String,
    pub created: i64,
    /// An article's or a memo's title.
    pub title: Option<String>,
    /// How its body starts, as plain text.
    pub snippet: String,
    /// Whether it is to the person: a private message, a mention or a
    /// reply to them.
    pub called: bool,
    /// The view a click opens: the first of the person's own views that
    /// shows the place, else Messages for a conversation, Mentions for a
    /// mention, Home for a followed channel, All channels otherwise.
    pub view: String,
}

/// What Social has to tell, from one pass over every place.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SocialNotices {
    /// How many unread messages get through their places' notices: the
    /// rail's badge for Social.
    pub noticed: usize,
    /// Those made at or after the moment asked about, newest first.
    pub arrived: Vec<SocialArrived>,
}

impl Commands {
    /// What Social has to tell, in every account this caller sees: how
    /// many unread messages get through their places' notices, and those
    /// of them made at or after `since` (milliseconds since the epoch),
    /// newest first, at most `limit`. Arrival is judged by when a message
    /// was made, not when it reached this computer: catching up after a
    /// day away tells nothing of the day, as Mail does not.
    pub fn social_notices(&self, since: i64, limit: usize) -> CommandResult<SocialNotices> {
        let mut noticed = 0;
        let mut found: Vec<(Hearing, SocialMessage, Notice)> = Vec::new();
        let mut hearer = Hearer::new(self);
        for account in self.store.social_accounts()? {
            if !self.sees_social(&account.actor) {
                continue;
            }
            for place in self.store.social_places(&account.actor)? {
                let hearing = hearer.hear(place)?;
                if hearing.heard == Heard::Nothing {
                    continue;
                }
                let unread = self
                    .store
                    .social_unread_messages(&hearing.place.account, &hearing.place.id)?;
                for message in unread {
                    let notice = self.notice_of(&hearing, &message)?;
                    if !notice.told() {
                        continue;
                    }
                    noticed += 1;
                    if limit > 0 && message.created >= since {
                        found.push((hearing.clone(), message, notice));
                    }
                }
            }
        }
        found.sort_by(|a, b| (b.1.created, &b.1.id).cmp(&(a.1.created, &a.1.id)));
        found.truncate(limit);
        let views = if found.is_empty() {
            Vec::new()
        } else {
            self.own_view_places()?
        };
        let arrived = found
            .into_iter()
            .map(|(hearing, message, notice)| self.told_of(&hearing, message, notice, &views))
            .collect::<CommandResult<_>>()?;
        Ok(SocialNotices { noticed, arrived })
    }

    /// What a notice says of `message`, and where it opens.
    fn told_of(
        &self,
        hearing: &Hearing,
        message: SocialMessage,
        notice: Notice,
        views: &[(String, Vec<PlaceRef>)],
    ) -> CommandResult<SocialArrived> {
        let place = &hearing.place;
        let r = PlaceRef {
            account: place.account.clone(),
            id: place.id.clone(),
        };
        let group_name = match &place.group {
            Some(uri) => self
                .store
                .social_group(&place.account, uri)?
                .map(|g| str_at(&g.group, "name").unwrap_or(uri).to_owned()),
            None => None,
        };
        let conversation = place.kind == PlaceKind::Conversation;
        let own = views
            .iter()
            .find(|(_, places)| places.contains(&r))
            .map(|(id, _)| id.clone());
        let view = own.unwrap_or_else(|| {
            if conversation {
                MESSAGES
            } else if message.mentions_me {
                MENTIONS
            } else if place.followed {
                HOME
            } else {
                ALL_CHANNELS
            }
            .to_owned()
        });
        let snippet = self.snippet(&message)?;
        Ok(SocialArrived {
            place_name: place_name(place),
            group_name,
            conversation,
            title: str_at(&message.message, "title").map(str::to_owned),
            author: message.author.clone(),
            created: message.created,
            called: notice == Notice::Called,
            message: message.id,
            snippet,
            view,
            place: r,
        })
    }

    /// The person's own views, each with the places it shows (accounts
    /// spelled as the store keys them), in the sidebar's order.
    fn own_view_places(&self) -> CommandResult<Vec<(String, Vec<PlaceRef>)>> {
        let mut out = Vec::new();
        for entry in self.social_views()? {
            if entry.built_in.is_some() {
                continue;
            }
            let places = self
                .view_places(&entry)?
                .into_iter()
                .map(|p| PlaceRef {
                    account: social_account_key(&p.account),
                    id: p.id,
                })
                .collect();
            out.push((entry.view.id, places));
        }
        Ok(out)
    }

    /// How `message`'s body starts, as plain text on one line, cut at
    /// [`SNIPPET`] characters.
    fn snippet(&self, message: &SocialMessage) -> CommandResult<String> {
        let rendered = self.render_social(message, false)?;
        let text = rinch_html::text(&rendered.html).visible;
        Ok(cut(&text.split_whitespace().collect::<Vec<_>>().join(" ")))
    }
}

/// `text` cut at [`SNIPPET`] characters, with an ellipsis when cut.
fn cut(text: &str) -> String {
    match text.char_indices().nth(SNIPPET) {
        Some((at, _)) => format!("{}\u{2026}", text[..at].trim_end()),
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::cut;

    #[test]
    fn a_snippet_is_cut_with_an_ellipsis_only_when_long() {
        assert_eq!(cut("short"), "short");
        let long = "word ".repeat(40);
        let snippet = cut(&long);
        assert!(snippet.ends_with('\u{2026}'));
        assert!(snippet.chars().count() <= 141);
    }
}
