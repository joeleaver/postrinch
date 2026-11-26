//! An OFSCP message's body, ready to show (docs/ofscp.md "Reading").
//!
//! A body is written by someone else, as mail is, and is cleaned as mail is.
//! Its format is OFSCP §5.3's "Content formats" (spec PR lost-conn/ofscp#24,
//! <https://github.com/lost-conn/ofscp/pull/24>, the baseline Joe chose on
//! 2026-11-26 for spec question 14):
//!
//! - `text/plain` is shown as it is, its bare `http`/`https` addresses made
//!   links.
//! - `text/markdown` is CommonMark ([`markdown`]), raw HTML in it shown as
//!   text.
//! - `text/html` goes through `rinch_html::clean` under §5.3's allow-list
//!   ([`html`]).
//! - Any other format is shown as `text/plain`.
//!
//! In every format a link is a link only for `http`, `https`, `mailto` and
//! the message's own attachments; nothing the body names by address is
//! loaded; the message's own attachments are placed where the body names
//! them (`attachment:<id>`) and shown when the picture rule lets them be
//! (as mail's pictures from the web are, [`crate::policy`]); and mentions
//! are marked ([`mentions`]).

mod finish;
mod html;
mod markdown;
mod mentions;
mod text;
mod title;

use finish::finish;
use text::Marker;

pub use title::article_title;

/// The class a marked mention carries: `<span class="ofscp-mention"
/// data-actor="bob@b.example">@bob@b.example</span>`.
pub const MENTION_CLASS: &str = "ofscp-mention";

/// The class of a link to one of the message's attachments placed in the
/// body but not shown there: `<a class="ofscp-attachment"
/// href="attachment:<id>">`.
pub const ATTACHMENT_CLASS: &str = "ofscp-attachment";

/// One of the message's attachments, as far as its body needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Attachment<'a> {
    /// Its `id`, which the body names it by.
    pub id: &'a str,
    /// Its `mime`: an `image/…` attachment can be shown in the body.
    pub mime: &'a str,
    /// Its file name, for a link to it with no text of its own.
    pub name: Option<&'a str>,
    /// Where its picture can be loaded from, once it can: a `data:` URL of
    /// the copy in the blob store, or its own address. `None` until then.
    pub picture: Option<&'a str>,
}

/// A message's body, and what it needs to be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Body<'a> {
    /// `content.mime`.
    pub mime: &'a str,
    /// `content.text`.
    pub text: &'a str,
    /// The message's `attachments`, in order.
    pub attachments: &'a [Attachment<'a>],
    /// The message's `mentions`, when it carries the list (§5.3): only
    /// references to those are marked.
    pub mentions: Option<&'a [String]>,
    /// The provider hosting the channel (its canonical authority): whom a
    /// bare `@handle` names.
    pub domain: &'a str,
}

/// How to show a body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Load attachments' pictures from their addresses (§5.3: a local
    /// author's attachments may be loaded, a remote author's only once the
    /// person asks, or a setting of theirs says so). A picture already in the
    /// blob store (a `data:` URL) is shown either way.
    pub pictures: bool,
}

/// A body, ready to show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    /// HTML for the message's body, safe to hand to rinch.
    pub html: String,
    /// Everyone marked as mentioned (`handle@domain`, lowercased), in the
    /// order first marked.
    pub mentions: Vec<String>,
    /// The attachments the body places (shown in it, or linked from it), by
    /// id, in order. The rest are shown after the body (§5.3), the cover of
    /// an article apart.
    pub placed: Vec<String>,
    /// Pictures the body places that are not shown: not loaded yet, or not
    /// to be loaded until the person asks ([`Options::pictures`]).
    pub pictures_held: usize,
}

/// A body's format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Plain,
    Markdown,
    Html,
}

impl Format {
    /// From a `mime`, its parameters aside; anything else is shown as
    /// `text/plain` (§5.3).
    fn of(mime: &str) -> Self {
        let essence = mime.split(';').next().unwrap_or_default().trim();
        if essence.eq_ignore_ascii_case("text/markdown") {
            Self::Markdown
        } else if essence.eq_ignore_ascii_case("text/html") {
            Self::Html
        } else {
            Self::Plain
        }
    }
}

/// What the finishing pass carries from one piece of a body to the next.
struct Finishing<'a> {
    marker: Marker<'a>,
    attachments: &'a [Attachment<'a>],
    options: Options,
    placed: Vec<String>,
    pictures_held: usize,
}

impl<'a> Finishing<'a> {
    fn attachment(&self, id: &str) -> Option<&Attachment<'a>> {
        self.attachments.iter().find(|a| a.id == id)
    }

    fn place(&mut self, id: &str) {
        if !self.placed.iter().any(|p| p == id) {
            self.placed.push(id.to_owned());
        }
    }

    /// The source to show an attachment's picture from, under mail's
    /// picture rule; `None` for one that is not a picture, and for a
    /// picture held back (counted).
    fn picture(&mut self, attachment: &Attachment) -> Option<String> {
        if !attachment
            .mime
            .get(..6)
            .is_some_and(|m| m.eq_ignore_ascii_case("image/"))
        {
            return None;
        }
        let rule = crate::policy(self.options.pictures).picture;
        let shown = attachment.picture.and_then(|source| rule(source));
        if shown.is_none() {
            self.pictures_held += 1;
        }
        shown
    }
}

/// Render a message's body.
pub fn render(body: &Body, options: Options) -> Rendered {
    let mut finishing = Finishing {
        marker: Marker::new(body.mentions, body.domain),
        attachments: body.attachments,
        options,
        placed: Vec::new(),
        pictures_held: 0,
    };
    let html = match Format::of(body.mime) {
        Format::Plain => plain(body.text, &mut finishing),
        Format::Markdown => html::render(
            &markdown::to_html(body.text),
            html::Source::Markdown,
            body.attachments,
            &mut finishing,
        ),
        Format::Html => html::render(
            body.text,
            html::Source::Html,
            body.attachments,
            &mut finishing,
        ),
    };
    Rendered {
        html,
        mentions: finishing.marker.marked,
        placed: finishing.placed,
        pictures_held: finishing.pictures_held,
    }
}

/// A `text/plain` body: as it is (its line breaks and runs of spaces kept,
/// as rinch shows text mail), linked and marked. It places no attachments
/// (§5.3: plain text has no inline references).
fn plain(text: &str, finishing: &mut Finishing) -> String {
    let text = text.replace("\r\n", "\n");
    let mut out = String::with_capacity(text.len() + 80);
    out.push_str("<div style=\"white-space: pre-wrap; overflow-wrap: anywhere\">");
    finishing.marker.write(&mut out, &text, true);
    out.push_str("</div>");
    out
}

#[cfg(test)]
mod tests {
    use super::Format;

    #[test]
    fn formats_by_mime() {
        assert_eq!(
            Format::of("text/markdown; variant=CommonMark"),
            Format::Markdown
        );
        assert_eq!(Format::of(" TEXT/HTML "), Format::Html);
        assert_eq!(Format::of("text/plain"), Format::Plain);
        assert_eq!(Format::of("application/x-unknown"), Format::Plain);
        assert_eq!(Format::of(""), Format::Plain);
    }
}
