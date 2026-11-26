//! A `text/markdown` body written as HTML (OFSCP §5.3, spec PR
//! lost-conn/ofscp#24, <https://github.com/lost-conn/ofscp/pull/24>):
//! CommonMark through `pulldown-cmark`, with GitHub's tables and
//! strikethrough (its bare-address links are made by
//! [`finish`](super::finish)), and these rules of §5.3's:
//!
//! - **Raw HTML is shown as text**, never interpreted: an HTML block or
//!   inline HTML is written as the text it is.
//! - **A picture by address is a link**, its alt text the link's text: it is
//!   never loaded. `![caption](attachment:<id>)` places one of the message's
//!   attachments, as `<img data-attachment>` does in HTML.
//! - **Only `http`, `https`, `mailto` and `attachment:` links are links**;
//!   any other is its text.
//!
//! What it writes is then cleaned under the HTML body's allow-list
//! ([`html`](super::html)), which also cuts deep nesting.

use pulldown_cmark::{CowStr, Event, LinkType, Options, Parser, Tag, TagEnd, html::push_html};

use super::text::escape_into;

/// The scheme of a reference to one of the message's attachments.
const ATTACHMENT: &str = "attachment:";

/// The Markdown `text` as HTML, to be cleaned.
pub(super) fn to_html(text: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut events = Events {
        inner: Parser::new_ext(text, options),
        links: Vec::new(),
        in_html_block: false,
    };
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    push_html(&mut out, std::iter::from_fn(|| events.next_event()));
    out
}

struct Events<'a> {
    inner: Parser<'a>,
    /// For each link open, whether it was kept.
    links: Vec<bool>,
    /// Inside an HTML block, written as a paragraph of its lines.
    in_html_block: bool,
}

impl<'a> Events<'a> {
    fn next_event(&mut self) -> Option<Event<'a>> {
        // A loop, not a call back into itself: a run of links left out
        // (`[](x)[](x)…`) is as long as the sender likes.
        loop {
            let event = match self.inner.next()? {
                Event::Start(Tag::HtmlBlock) => {
                    self.in_html_block = true;
                    Event::Start(Tag::Paragraph)
                }
                Event::End(TagEnd::HtmlBlock) => {
                    self.in_html_block = false;
                    Event::End(TagEnd::Paragraph)
                }
                Event::Html(line) if self.in_html_block => self.html_line(line),
                Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
                Event::Start(Tag::Link {
                    link_type,
                    dest_url,
                    title,
                    id,
                }) => {
                    let kept = is_active(link_type, &dest_url);
                    self.links.push(kept);
                    if !kept {
                        continue;
                    }
                    Event::Start(Tag::Link {
                        link_type,
                        dest_url,
                        title,
                        id,
                    })
                }
                Event::End(TagEnd::Link) => {
                    if self.links.pop() != Some(true) {
                        continue;
                    }
                    Event::End(TagEnd::Link)
                }
                Event::Start(Tag::Image { dest_url, .. }) => self.picture(&dest_url),
                event => event,
            };
            return Some(event);
        }
    }

    /// One line of an HTML block, as text: the line break kept as one.
    fn html_line(&mut self, line: CowStr<'a>) -> Event<'a> {
        let mut html = String::new();
        escape_into(&mut html, line.trim_end_matches(['\n', '\r']));
        if line.ends_with('\n') {
            html.push_str("<br>");
        }
        Event::InlineHtml(html.into())
    }

    /// A picture, whose alt text is the events up to its end: written whole
    /// here, as `<img data-attachment>` for an attachment, as a link for a
    /// web address, and as its alt text for anything else.
    fn picture(&mut self, dest: &str) -> Event<'a> {
        let alt = self.alt_text();
        let mut html = String::new();
        let dest = dest.trim();
        let lower = dest.to_ascii_lowercase();
        if lower.starts_with(ATTACHMENT) {
            let id = &dest[ATTACHMENT.len()..];
            html.push_str("<img data-attachment=\"");
            escape_into(&mut html, id);
            html.push_str("\" alt=\"");
            escape_into(&mut html, &alt);
            html.push_str("\">");
        } else if (lower.starts_with("http://") || lower.starts_with("https://"))
            && self.links.last() != Some(&true)
        {
            // Inside a link it cannot be one: its alt text, below.
            html.push_str("<a href=\"");
            escape_into(&mut html, dest);
            html.push_str("\">");
            escape_into(&mut html, if alt.is_empty() { dest } else { &alt });
            html.push_str("</a>");
        } else {
            escape_into(&mut html, &alt);
        }
        Event::InlineHtml(html.into())
    }

    /// The text of the events up to the end of the picture just started:
    /// its alt text, as CommonMark takes it (marks and links dropped).
    fn alt_text(&mut self) -> String {
        let mut alt = String::new();
        let mut depth = 1usize;
        for event in self.inner.by_ref() {
            match event {
                Event::Start(Tag::Image { .. }) => depth += 1,
                Event::End(TagEnd::Image) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) => alt.push_str(&t),
                Event::SoftBreak | Event::HardBreak => alt.push(' '),
                _ => {}
            }
        }
        alt
    }
}

/// Whether a link is one to keep (§5.3): `http`, `https`, `mailto`, or an
/// `attachment:` reference (checked against the attachments later). An
/// email autolink (`<bob@e.example>`) is `mailto`.
fn is_active(link_type: LinkType, dest: &str) -> bool {
    if link_type == LinkType::Email {
        return true;
    }
    let lower = dest.trim().to_ascii_lowercase();
    ["http://", "https://", "mailto:", "attachment:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme))
}

#[cfg(test)]
mod tests {
    use super::to_html;

    #[test]
    fn raw_html_is_text() {
        let html = to_html("<script>alert(1)</script>\n\nhi <b onclick=x>there</b>");
        assert!(!html.contains("<script"), "{html}");
        assert!(
            html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{html}"
        );
        assert!(
            html.contains("&lt;b onclick=x&gt;there&lt;/b&gt;"),
            "{html}"
        );
    }

    #[test]
    fn pictures_by_address_become_links_and_attachments_are_placed() {
        let html = to_html(
            "![a *cat*](https://e.example/cat.png) ![logo](attachment:att_1) ![x](file:///etc/passwd)",
        );
        assert!(
            html.contains(r#"<a href="https://e.example/cat.png">a cat</a>"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<img data-attachment="att_1" alt="logo">"#),
            "{html}"
        );
        assert!(!html.contains("file:"), "{html}");
        assert!(!html.contains("<img src"), "{html}");
    }

    #[test]
    fn only_allowed_schemes_link() {
        let html = to_html(
            "[a](javascript:alert(1)) [b](https://e.example/) [c](/relative) <bob@e.example> [d](attachment:att_2)",
        );
        assert!(!html.contains("javascript"), "{html}");
        assert!(!html.contains("/relative"), "{html}");
        assert!(
            html.contains(r#"<a href="https://e.example/">b</a>"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<a href="mailto:bob@e.example">"#),
            "{html}"
        );
        assert!(
            html.contains(r#"<a href="attachment:att_2">d</a>"#),
            "{html}"
        );
        // A link left out is its text.
        assert!(
            html.starts_with("<p>a <a") && html.contains("</a> c <a"),
            "{html}"
        );
    }

    #[test]
    fn tables_and_strikethrough() {
        let html = to_html("| a | b |\n|:-|-:|\n| 1 | 2 |\n\n~~gone~~");
        assert!(
            html.contains("<table>") && html.contains("<del>gone</del>"),
            "{html}"
        );
    }
}
