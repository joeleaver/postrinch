//! A view's filters and a search's words (docs/ofscp.md "Organizing"):
//! kinds, words, `from:` and `tag:`. Words are all wanted, in any case; a
//! phrase is written in double quotes. `from:` names an author by actor or
//! by handle; `tag:` a message's tag, with or without its `#`.

use postrinch_store::{ContentKind, SocialFilters, SocialMessage};
use serde_json::Value;

use super::{kind_of, str_at};
use crate::{CommandError, CommandResult};

/// What a message must be to be shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Filter {
    /// The kinds wanted; empty for every kind.
    kinds: Vec<ContentKind>,
    /// Words and phrases, lower-cased.
    words: Vec<String>,
    /// Authors, lower-cased: an actor, or a handle on any provider.
    from: Vec<String>,
    /// Tags, lower-cased, without their `#`.
    tags: Vec<String>,
}

impl Filter {
    /// A view's filters.
    pub fn of_view(filters: &SocialFilters) -> CommandResult<Self> {
        Ok(Self {
            kinds: filters.kinds.clone(),
            ..Self::of_search(&filters.search)?
        })
    }

    /// A search, as the person wrote it.
    pub fn of_search(text: &str) -> CommandResult<Self> {
        let mut filter = Self::default();
        for token in tokens(text) {
            let (op, value) = match token.split_once(':') {
                Some((op, value)) if !token.starts_with('"') => (Some(op), value),
                _ => (None, token.as_str()),
            };
            let value = value.trim_matches('"').trim().to_lowercase();
            match op.map(str::to_ascii_lowercase).as_deref() {
                Some(op @ ("from" | "tag")) if value.is_empty() => {
                    return Err(CommandError::Invalid(format!(
                        "{op}: needs something after it, as in {op}:{}.",
                        if op == "from" { "ada" } else { "news" }
                    )));
                }
                Some("from") => filter.from.push(value),
                Some("tag") => filter.tags.push(value.trim_start_matches('#').to_owned()),
                _ => {
                    let word = token.trim_matches('"').trim().to_lowercase();
                    if !word.is_empty() {
                        filter.words.push(word);
                    }
                }
            }
        }
        Ok(filter)
    }

    /// Its words as a full-text query in rhypedb's language: each wanted
    /// (`+word`, `+"a phrase"`). `None` without words.
    pub fn text(&self) -> Option<String> {
        let terms: Vec<String> = self
            .words
            .iter()
            .map(|w| {
                let w = w.replace('"', "");
                if w.contains(char::is_whitespace) {
                    format!("+\"{w}\"")
                } else {
                    format!("+{w}")
                }
            })
            .collect();
        (!terms.is_empty()).then(|| terms.join(" "))
    }

    /// Whether `message` is shown.
    pub fn keeps(&self, message: &SocialMessage) -> bool {
        let kind_wanted = self.kinds.is_empty()
            || kind_of(&message.kind).is_some_and(|kind| self.kinds.contains(&kind));
        kind_wanted
            && self.from.iter().all(|from| by(&message.author, from))
            && self.tags.iter().all(|tag| tagged(&message.message, tag))
            && (self.words.is_empty() || {
                let text = haystack(message);
                self.words.iter().all(|w| text.contains(w.as_str()))
            })
    }
}

/// The words of `text`, a quoted phrase (or an operator's quoted value) as
/// one.
fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                current.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Whether `author` is the one `from` names: the actor itself, or its
/// handle on any provider.
fn by(author: &str, from: &str) -> bool {
    let author = author.to_lowercase();
    if from.contains('@') {
        author == from
    } else {
        author
            .split_once('@')
            .is_some_and(|(handle, _)| handle == from)
    }
}

/// Whether a message's `tags` hold `tag`.
fn tagged(object: &Value, tag: &str) -> bool {
    object
        .get("tags")
        .and_then(Value::as_array)
        .is_some_and(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .any(|t| t.trim_start_matches('#').to_lowercase() == tag)
        })
}

/// What a message's words are looked for in, lower-cased: its body, its
/// title and its author.
fn haystack(message: &SocialMessage) -> String {
    let object = &message.message;
    let body = object
        .get("content")
        .and_then(|c| str_at(c, "text"))
        .unwrap_or_default();
    let title = str_at(object, "title").unwrap_or_default();
    format!("{body}\n{title}\n{}", message.author).to_lowercase()
}
