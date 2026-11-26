//! An HTML body, or a Markdown body once written as HTML, cleaned as mail is
//! (`rinch_html::clean`: parsed as a browser parses it, its nesting cut, the
//! picture and link rules applied) under OFSCP §5.3's own allow-list (spec PR
//! lost-conn/ofscp#24, <https://github.com/lost-conn/ofscp/pull/24>), then
//! finished by [`finish`](super::finish): attachments placed, mentions
//! marked.
//!
//! §5.3 drops every `src`: a body shows the message's own attachments by
//! `<img data-attachment="<id>">`, and no picture from anywhere else. The
//! allow-list cannot keep `data-attachment` (rinch keeps no `data-*`
//! attribute from a document; its DOM reads many), so the user sheet carries
//! it: one rule per attachment the message has, `img[data-attachment="…"]`,
//! matched against the body as written, marks each picture with its rule's
//! index, and the index names the attachment.

use std::sync::Arc;

use rinch_html::{AllowList, Policy, USER_MARK, clean};

use super::{Attachment, finish};

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
pub(super) fn policy(source: Source, attachments: &[Attachment]) -> Policy {
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
        user_css: attachment_sheet(attachments),
        allow: Some(AllowList {
            tags: TAGS.iter().map(|s| (*s).to_owned()).collect(),
            attributes,
            removed_with_content: REMOVED.iter().map(|s| (*s).to_owned()).collect(),
        }),
    }
}

/// One rule per attachment, in order: rule `i` marks the pictures that
/// place `attachments[i]`.
fn attachment_sheet(attachments: &[Attachment]) -> String {
    let mut sheet = String::new();
    for attachment in attachments {
        sheet.push_str("img[data-attachment=\"");
        css_string_into(&mut sheet, attachment.id);
        sheet.push_str("\"] {}\n");
    }
    sheet
}

/// `value` escaped for a double-quoted CSS string.
fn css_string_into(out: &mut String, value: &str) {
    for c in value.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => out.push_str(&format!("\\{:x} ", u32::from(c))),
            c => out.push(c),
        }
    }
}

/// `html` from `source`, cleaned and finished.
pub(super) fn render(
    html: &str,
    source: Source,
    attachments: &[Attachment],
    finishing: &mut super::Finishing,
) -> String {
    let cleaned = clean(html, &policy(source, attachments));
    finish(
        &cleaned.html,
        USER_MARK,
        source == Source::Markdown,
        finishing,
    )
}

#[cfg(test)]
mod tests {
    use super::{attachment_sheet, css_string_into};
    use crate::social::Attachment;

    #[test]
    fn an_id_cannot_end_its_selector() {
        let mut out = String::new();
        css_string_into(&mut out, "a\"] * { x } img[\\\n");
        assert_eq!(out, "a\\\"] * { x } img[\\\\\\a ");
        let sheet = attachment_sheet(&[Attachment {
            id: "att_1",
            mime: "image/png",
            name: None,
            picture: None,
        }]);
        assert_eq!(sheet, "img[data-attachment=\"att_1\"] {}\n");
    }
}
