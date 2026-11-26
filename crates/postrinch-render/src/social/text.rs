//! A run of a message's text written as HTML: escaped, its mentions marked,
//! and (where the format allows it) its bare `http`/`https` addresses made
//! links.

use std::collections::HashSet;

use linkify::{LinkFinder, LinkKind};

use super::MENTION_CLASS;
use super::mentions::references;

/// What the text of one body marks, and what it has marked so far.
pub(super) struct Marker<'a> {
    /// The message's own `mentions`, lowercased, when it carries the list:
    /// only references to them are marked.
    listed: Option<HashSet<String>>,
    /// The provider hosting the channel, whom a bare `@handle` names.
    domain: &'a str,
    /// Everyone marked, in the order first marked.
    pub marked: Vec<String>,
    /// The same, to look up: a body may mention as many people as it likes.
    marked_set: HashSet<String>,
}

impl<'a> Marker<'a> {
    pub fn new(listed: Option<&[String]>, domain: &'a str) -> Self {
        Self {
            listed: listed.map(|l| l.iter().map(|a| a.to_ascii_lowercase()).collect()),
            domain,
            marked: Vec::new(),
            marked_set: HashSet::new(),
        }
    }

    /// Write `text` to `out`, escaped, its mentions marked and, with
    /// `linkify`, its bare web addresses made links.
    pub fn write(&mut self, out: &mut String, text: &str, linkify: bool) {
        if !linkify {
            self.write_unlinked(out, text);
            return;
        }
        let mut finder = LinkFinder::new();
        finder.kinds(&[LinkKind::Url]).url_must_have_scheme(true);
        for span in finder.spans(text) {
            let piece = span.as_str();
            match span.kind() {
                Some(LinkKind::Url) if is_web(piece) => {
                    out.push_str("<a href=\"");
                    escape_into(out, piece);
                    out.push_str("\">");
                    escape_into(out, piece);
                    out.push_str("</a>");
                }
                _ => self.write_unlinked(out, piece),
            }
        }
    }

    /// Text with its mentions marked and nothing made a link.
    fn write_unlinked(&mut self, out: &mut String, text: &str) {
        let mut from = 0;
        for reference in references(text, self.domain) {
            if self
                .listed
                .as_ref()
                .is_some_and(|listed| !listed.contains(&reference.actor))
            {
                continue;
            }
            escape_into(out, &text[from..reference.range.start]);
            out.push_str(&format!("<span class=\"{MENTION_CLASS}\" data-actor=\""));
            escape_into(out, &reference.actor);
            out.push_str("\">");
            escape_into(out, &text[reference.range.clone()]);
            out.push_str("</span>");
            from = reference.range.end;
            if self.marked_set.insert(reference.actor.clone()) {
                self.marked.push(reference.actor);
            }
        }
        escape_into(out, &text[from..]);
    }
}

/// Whether an address found in text is one to make a link: `http` and
/// `https` only (§5.3: plain text may link bare web addresses; linkify also
/// finds `ftp:` and the like).
fn is_web(address: &str) -> bool {
    let lower = address.get(..8).unwrap_or(address).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// `text` escaped for HTML text or a quoted attribute value.
pub(super) fn escape_into(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\0' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Marker;

    fn write(text: &str, listed: Option<&[String]>, linkify: bool) -> (String, Vec<String>) {
        let mut marker = Marker::new(listed, "a.example");
        let mut out = String::new();
        marker.write(&mut out, text, linkify);
        (out, marker.marked)
    }

    #[test]
    fn mentions_are_marked_and_text_escaped() {
        let (html, marked) = write("<b>@bob</b> & @bob", None, false);
        assert_eq!(
            html,
            "&lt;b&gt;<span class=\"ofscp-mention\" data-actor=\"bob@a.example\">@bob</span>\
             &lt;/b&gt; &amp; <span class=\"ofscp-mention\" data-actor=\"bob@a.example\">@bob</span>"
        );
        assert_eq!(marked, ["bob@a.example"]);
    }

    #[test]
    fn with_a_list_only_those_on_it_are_marked() {
        let listed = ["Carol@B.example".to_owned()];
        let (html, marked) = write("@bob @carol@b.example", Some(&listed), false);
        assert!(html.starts_with("@bob "), "{html}");
        assert_eq!(marked, ["carol@b.example"]);
    }

    #[test]
    fn web_addresses_become_links_and_others_do_not() {
        let (html, _) = write(
            "see https://e.example/a?b=1&c=2. or ftp://e.example/ javascript:alert(1)",
            None,
            true,
        );
        assert!(
            html.contains(r#"<a href="https://e.example/a?b=1&amp;c=2">"#),
            "{html}"
        );
        assert!(!html.contains("href=\"ftp"), "{html}");
        assert!(!html.contains("href=\"javascript"), "{html}");
    }

    #[test]
    fn a_mention_inside_an_address_stays_part_of_it() {
        let (html, marked) = write("https://e.example/@bob and @bob", None, true);
        assert_eq!(marked, ["bob@a.example"]);
        assert_eq!(html.matches("ofscp-mention").count(), 1, "{html}");
    }
}
