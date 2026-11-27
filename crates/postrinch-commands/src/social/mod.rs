//! Social (OFSCP) accounts' commands: views, places, pages, threads,
//! search, drafts, posting and other changes, read markers, follows and
//! the profile (docs/ofscp.md, "Organizing: views, not providers",
//! "Reading", "Writing" and "Storage").
//!
//! The commands read and write the store only. A change that speaks to a
//! provider (a post, an edit, a reaction, a follow, the profile) is queued
//! in the outbox for the sync engine to send, and every read lays the
//! pending ones over what sync kept ([`overlay`]): a message being sent
//! shows at once, marked pending, and sync never undoes a change. Read
//! state is the person's and changes here at once; a channel the home
//! provider holds also has its read marker moved there (§7.7).
//!
//! Records keep protocol objects whole as JSON (`postrinch-store`); the
//! few fields the commands read are named after the spec's section.

mod accounts;
mod drafts;
mod filters;
mod media;
mod notices;
mod overlay;
mod pages;
mod permissions;
mod places;
mod posting;
mod read;
mod render;
mod search;
mod threads;
mod told;
mod views;
mod views_built_in;

use postrinch_store::ContentKind;
use serde_json::Value;

pub use accounts::{SocialAccountInfo, SocialAvatar};
pub use notices::{Heard, Notice, PROVIDER_DEFAULT, heard, notice};
pub use overlay::{SocialItem, SocialPending};
pub use pages::{SocialCursor, SocialPage};
pub use places::{SocialGroupInfo, SocialPlaceInfo};
pub use postrinch_render::social::Rendered as SocialRendered;
pub use postrinch_store::{
    AvatarChange, ProfileChange, SocialContent, SocialCount, SocialDraft, SocialFilters, SocialView,
};
pub use threads::SocialThread;
pub use told::SocialArrived;
pub use views::{SocialViewEntry, SocialViewTally};
pub use views_built_in::{ALL_CHANNELS, BuiltInView, DRAFTS, HOME, MENTIONS, MESSAGES};

/// The protocol's name for a kind: a message's `type` (§5.3).
pub fn type_name(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Chat => "message",
        ContentKind::Memo => "memo",
        ContentKind::Article => "article",
    }
}

/// The kind a message's `type` names; `None` for a type this version does
/// not know, shown as a generic message (§2.3).
pub fn kind_of(type_name: &str) -> Option<ContentKind> {
    match type_name {
        "message" => Some(ContentKind::Chat),
        "memo" => Some(ContentKind::Memo),
        "article" => Some(ContentKind::Article),
        _ => None,
    }
}

/// The time now, in milliseconds since the epoch, as the store keeps it.
fn now() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// A new id, for a view, a draft or a change's client id: 32 hex digits,
/// within the protocol's local-id characters (§2.4).
fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// The domain of an actor (`handle@domain`), lower-cased: its home
/// provider (§2.2).
fn actor_domain(actor: &str) -> String {
    actor
        .rsplit_once('@')
        .map_or(actor, |(_, domain)| domain)
        .to_ascii_lowercase()
}

/// The authority of an object URI (`https://{domain}/api/…`, §2.4),
/// lower-cased: the provider holding it. `None` for what is not a URI (a
/// conversation's dmId).
fn uri_domain(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.is_empty()).then(|| authority.to_ascii_lowercase())
}

/// Whether two actors are one, in any spelling.
fn same_actor(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// The string at `key` of a JSON object.
fn str_at<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

#[cfg(test)]
mod tests;
