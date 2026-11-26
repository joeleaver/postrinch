//! The last pass over a cleaned body: its attachments placed, its links
//! checked against §5.3's rule, its mentions marked and (for Markdown) its
//! bare web addresses made links.
//!
//! It reads what `rinch_html::clean` wrote, which html5ever serialized
//! (every element closed but the void ones, attributes quoted, text
//! escaped), so html5ever's tokenizer reads it exactly as it was written; no
//! tree is built, and nothing here recurses. What it writes is its own:
//! tags it knows, with values it escaped.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};

use super::html::TAGS;
use super::text::escape_into;
use super::{ATTACHMENT_CLASS, Finishing};

/// `cleaned` finished: `mark` is the attribute rinch carried a picture's
/// `data-attachment` through as (see [`html`](super::html)).
pub(in crate::social) fn finish(
    cleaned: &str,
    mark: &str,
    linkify: bool,
    finishing: &mut Finishing,
) -> String {
    let sink = Sink {
        state: RefCell::new(State {
            out: String::with_capacity(cleaned.len()),
            text: String::new(),
            links: Vec::new(),
            in_link: 0,
            in_code: 0,
            mark,
            linkify,
            finishing,
        }),
    };
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(cleaned));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    let mut state = tokenizer.sink.state.into_inner();
    state.flush();
    state.out
}

struct Sink<'s, 'f, 'a> {
    state: RefCell<State<'s, 'f, 'a>>,
}

struct State<'s, 'f, 'a> {
    out: String,
    /// Text not yet written: a run of text arrives in pieces.
    text: String,
    /// For each `<a>` open, whether it was kept as a link.
    links: Vec<bool>,
    /// How many kept links, and `code`/`pre` elements, are open: text in
    /// them is written as it is.
    in_link: usize,
    in_code: usize,
    mark: &'s str,
    linkify: bool,
    finishing: &'f mut Finishing<'a>,
}

impl TokenSink for Sink<'_, '_, '_> {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut state = self.state.borrow_mut();
        match token {
            Token::CharacterTokens(text) => state.text.push_str(&text),
            Token::NullCharacterToken => state.text.push('\u{FFFD}'),
            Token::TagToken(tag) => {
                state.flush();
                match tag.kind {
                    TagKind::StartTag => state.start(&tag),
                    TagKind::EndTag => state.end(&tag),
                }
            }
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

fn attribute<'t>(tag: &'t Tag, name: &str) -> Option<&'t str> {
    tag.attrs
        .iter()
        .find(|a| &*a.name.local == name)
        .map(|a| &*a.value)
}

impl State<'_, '_, '_> {
    fn flush(&mut self) {
        if self.text.is_empty() {
            return;
        }
        let text = std::mem::take(&mut self.text);
        if self.in_link > 0 || self.in_code > 0 {
            escape_into(&mut self.out, &text);
        } else {
            self.finishing
                .marker
                .write(&mut self.out, &text, self.linkify);
        }
    }

    fn start(&mut self, tag: &Tag) {
        let name = &*tag.name;
        match name {
            "img" => self.picture(tag),
            "a" => self.link(tag),
            "br" | "hr" => {
                self.out.push('<');
                self.out.push_str(name);
                self.out.push('>');
            }
            _ if TAGS.contains(&name) => {
                if matches!(name, "code" | "pre") {
                    self.in_code += 1;
                }
                self.out.push('<');
                self.out.push_str(name);
                for a in &tag.attrs {
                    let key = &*a.name.local;
                    if key == self.mark {
                        continue;
                    }
                    self.out.push(' ');
                    self.out.push_str(key);
                    self.out.push_str("=\"");
                    escape_into(&mut self.out, &a.value);
                    self.out.push('"');
                }
                self.out.push('>');
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: &Tag) {
        let name = &*tag.name;
        match name {
            "img" | "br" | "hr" => {}
            "a" => {
                if self.links.pop() == Some(true) {
                    self.in_link -= 1;
                    self.out.push_str("</a>");
                }
            }
            _ if TAGS.contains(&name) => {
                if matches!(name, "code" | "pre") {
                    self.in_code = self.in_code.saturating_sub(1);
                }
                self.out.push_str("</");
                self.out.push_str(name);
                self.out.push('>');
            }
            _ => {}
        }
    }

    /// A link: kept only for `http`, `https` and `mailto`, and for
    /// `attachment:` naming one of the message's attachments (§5.3). Any
    /// other (another scheme, an address with none) is its text.
    fn link(&mut self, tag: &Tag) {
        let href = attribute(tag, "href").unwrap_or_default().trim();
        let (scheme, rest) = href.split_once(':').unwrap_or_default();
        let scheme = scheme.to_ascii_lowercase();
        let kept = match scheme.as_str() {
            "http" | "https" | "mailto" if !rest.is_empty() => {
                self.out.push_str("<a href=\"");
                escape_into(&mut self.out, href);
                self.out.push_str("\">");
                true
            }
            "attachment" if self.finishing.attachment(rest).is_some() => {
                self.finishing.place(rest);
                self.attachment_link(rest);
                true
            }
            _ => false,
        };
        if kept {
            self.in_link += 1;
        }
        self.links.push(kept);
    }

    fn attachment_link(&mut self, id: &str) {
        self.out.push_str(&format!(
            "<a class=\"{ATTACHMENT_CLASS}\" href=\"attachment:"
        ));
        escape_into(&mut self.out, id);
        self.out.push_str("\">");
    }

    /// A picture: the attachment it places, if it names one of the
    /// message's, shown when the picture rule lets it be and else a link to
    /// it; with none, its caption (§5.3: nothing is fetched).
    fn picture(&mut self, tag: &Tag) {
        let caption = attribute(tag, "alt").unwrap_or_default().to_owned();
        let Some(attachment) =
            attribute(tag, self.mark).and_then(|id| self.finishing.attachment(id))
        else {
            escape_into(&mut self.out, &caption);
            return;
        };
        let source = self.finishing.picture(&attachment);
        if let Some(source) = source {
            self.finishing.place(attachment.id);
            self.out.push_str("<img src=\"");
            escape_into(&mut self.out, &source);
            self.out.push_str("\" alt=\"");
            escape_into(&mut self.out, &caption);
            self.out.push_str("\">");
            return;
        }
        let label = if caption.is_empty() {
            attachment.name.unwrap_or(attachment.id).to_owned()
        } else {
            caption
        };
        if self.in_link > 0 {
            // A link cannot hold another: the caption stays, and the
            // attachment is shown after the body.
            escape_into(&mut self.out, &label);
            return;
        }
        self.finishing.place(attachment.id);
        self.attachment_link(attachment.id);
        escape_into(&mut self.out, &label);
        self.out.push_str("</a>");
    }
}
