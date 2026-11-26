//! An HTML body, or a Markdown body once written as HTML, cleaned as mail is
//! (`rinch_html::clean`: parsed as a browser parses it, its nesting cut, the
//! picture and link rules applied) under OFSCP §5.3's own allow-list (spec PR
//! lost-conn/ofscp#24, <https://github.com/lost-conn/ofscp/pull/24>), then
//! finished by [`finish`](super::finish): attachments placed, mentions
//! marked.
//!
//! §5.3 drops every `src`: a body shows the message's own attachments by
//! `<img data-attachment="<id>">`, and no picture from anywhere else. A
//! picture by address is therefore its caption only, never a link to the
//! address as Markdown's is: the sanitizer MUST drop `src`, which outweighs
//! the SHOULD to offer it as a link. The allow-list cannot keep
//! `data-attachment` as itself (rinch keeps no `data-*` attribute from a
//! document; its DOM reads many), so rinch carries it through renamed
//! ([`CARRIED`]).

use std::sync::Arc;

use rinch_html::{AllowList, CARRIED, Policy, clean};

use super::finish;

/// §5.3's elements.
pub(super) const TAGS: &[&str] = &[
    "p",
    "br",
    "hr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "strong",
    "b",
    "em",
    "i",
    "u",
    "s",
    "del",
    "sub",
    "sup",
    "code",
    "pre",
    "blockquote",
    "ul",
    "ol",
    "li",
    "a",
    "img",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "span",
];

/// §5.3's attributes, but `data-attachment` (see the module docs).
const ATTRIBUTES: &[(&str, &str)] = &[
    ("a", "href"),
    ("img", "alt"),
    ("th", "colspan"),
    ("th", "rowspan"),
    ("td", "colspan"),
    ("td", "rowspan"),
];

/// What Markdown writes beyond them, kept for a Markdown body only (it is
/// written by [`markdown`](super::markdown), not by the sender): a list's
/// first number, and a table column's alignment (`text-align`, through
/// rinch's style filter).
const MARKDOWN_ATTRIBUTES: &[(&str, &str)] = &[("ol", "start"), ("th", "style"), ("td", "style")];

/// Elements removed with everything inside them (§5.3).
const REMOVED: &[&str] = &[
    "script", "style", "iframe", "object", "embed", "template", "noscript",
];

/// The schemes a link may have (§5.3); `attachment:` links are checked
/// against the message's attachments afterwards.
const LINK_SCHEMES: &[&str] = &["http", "https", "mailto", "attachment"];

/// Which source an HTML document came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Source {
    /// The sender's `text/html`.
    Html,
    /// Written from the sender's `text/markdown`.
    Markdown,
}

/// The policy a body is cleaned under.
pub(super) fn policy(source: Source) -> Policy {
    let mut attributes: Vec<(String, String)> = ATTRIBUTES
        .iter()
        .map(|(t, a)| ((*t).to_owned(), (*a).to_owned()))
        .collect();
    if source == Source::Markdown {
        attributes.extend(
            MARKDOWN_ATTRIBUTES
                .iter()
                .map(|(t, a)| ((*t).to_owned(), (*a).to_owned())),
        );
    }
    Policy {
        // Nothing a body names by address is loaded (§5.3); its own
        // attachments are placed afterwards, under the picture rule.
        picture: Arc::new(|_| None),
        link_schemes: LINK_SCHEMES.iter().map(|s| (*s).to_owned()).collect(),
        user_css: String::new(),
        allow: Some(AllowList {
            tags: TAGS.iter().map(|s| (*s).to_owned()).collect(),
            attributes,
            removed_with_content: REMOVED.iter().map(|s| (*s).to_owned()).collect(),
            carried: vec![("img".to_owned(), ATTACHMENT_ATTRIBUTE.to_owned())],
        }),
    }
}

/// The attribute a body places an attachment with (§5.3).
const ATTACHMENT_ATTRIBUTE: &str = "data-attachment";

/// `html` from `source`, cleaned and finished.
pub(super) fn render(html: &str, source: Source, finishing: &mut super::Finishing) -> String {
    let cleaned = clean(html, &policy(source));
    finish(
        &cleaned.html,
        &format!("{CARRIED}{ATTACHMENT_ATTRIBUTE}"),
        source == Source::Markdown,
        finishing,
    )
}
