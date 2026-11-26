//! An article's title when it has no `title` field (OFSCP §5.3, spec PR
//! lost-conn/ofscp#24, <https://github.com/lost-conn/ofscp/pull/24>): the
//! body's first block, when that is a level-1 heading. In `text/markdown`
//! an ATX (`# Title`) or Setext (`Title` over `===`) heading, in `text/html`
//! an `<h1>`. A level-1 heading further down is not a title, and the
//! heading stays in the body.

use std::cell::RefCell;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

use super::Format;

/// The title an article's body gives itself, as text; `None` when its first
/// block is not a level-1 heading, or the heading is empty.
pub fn article_title(mime: &str, text: &str) -> Option<String> {
    let title = match Format::of(mime) {
        Format::Plain => return None,
        Format::Markdown => markdown_title(text)?,
        Format::Html => html_title(text)?,
    };
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

fn markdown_title(text: &str) -> Option<String> {
    let mut events = Parser::new(text);
    let Some(Event::Start(Tag::Heading {
        level: HeadingLevel::H1,
        ..
    })) = events.next()
    else {
        return None;
    };
    let mut title = String::new();
    for event in events {
        match event {
            Event::End(TagEnd::Heading(_)) => return Some(title),
            Event::Text(t) | Event::Code(t) | Event::InlineHtml(t) => title.push_str(&t),
            Event::SoftBreak | Event::HardBreak => title.push(' '),
            _ => {}
        }
    }
    None
}

/// The text of the `<h1>` the body starts with, read from the body as
/// cleaned to be shown (so as a browser parses it, its nesting cut).
fn html_title(text: &str) -> Option<String> {
    let cleaned = rinch_html::clean(text, &super::html::policy(super::html::Source::Html, &[]));
    let sink = Sink(RefCell::new(Seen::Nothing));
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(&cleaned.html));
    let _ = tokenizer.feed(&input);
    tokenizer.end();
    match tokenizer.sink.0.into_inner() {
        Seen::Title(title) => Some(title),
        _ => None,
    }
}

/// How far into the body the reading has got.
enum Seen {
    /// Nothing but white space yet.
    Nothing,
    /// Inside the leading `<h1>`, with its text so far.
    Heading(String),
    /// The leading `<h1>`'s text, whole.
    Title(String),
    /// The body starts with something else.
    NoTitle,
}

struct Sink(RefCell<Seen>);

impl TokenSink for Sink {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        let mut seen = self.0.borrow_mut();
        let next = match (std::mem::replace(&mut *seen, Seen::NoTitle), token) {
            (done @ (Seen::Title(_) | Seen::NoTitle), _) => done,
            (Seen::Nothing, Token::CharacterTokens(t)) if t.trim().is_empty() => Seen::Nothing,
            (Seen::Nothing, Token::TagToken(tag))
                if tag.kind == TagKind::StartTag && &*tag.name == "h1" =>
            {
                Seen::Heading(String::new())
            }
            (Seen::Nothing, Token::EOFToken | Token::ParseError(_)) => Seen::Nothing,
            (Seen::Nothing, _) => Seen::NoTitle,
            (Seen::Heading(mut title), Token::CharacterTokens(t)) => {
                title.push_str(&t);
                Seen::Heading(title)
            }
            (Seen::Heading(title), Token::TagToken(tag))
                if tag.kind == TagKind::EndTag && &*tag.name == "h1" =>
            {
                Seen::Title(title)
            }
            (Seen::Heading(mut title), Token::TagToken(tag)) if &*tag.name == "br" => {
                title.push(' ');
                Seen::Heading(title)
            }
            (heading @ Seen::Heading(_), _) => heading,
        };
        *seen = next;
        TokenSinkResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::article_title;

    #[test]
    fn a_leading_level_one_heading_is_the_title() {
        assert_eq!(
            article_title("text/markdown", "# Our *first* year\n\nBody").as_deref(),
            Some("Our first year")
        );
        assert_eq!(
            article_title("text/markdown", "Setext\ntitle\n===\n\nBody").as_deref(),
            Some("Setext title")
        );
        assert_eq!(
            article_title("text/html", "\n <h1>The <em>news</em></h1><p>Body</p>").as_deref(),
            Some("The news")
        );
    }

    #[test]
    fn anything_else_first_is_no_title() {
        assert_eq!(article_title("text/markdown", "Intro\n\n# Later"), None);
        assert_eq!(article_title("text/markdown", "## Second level"), None);
        assert_eq!(article_title("text/markdown", "#\n\nBody"), None);
        assert_eq!(
            article_title("text/html", "<p>Intro</p><h1>Later</h1>"),
            None
        );
        assert_eq!(article_title("text/html", "Intro <h1>Later</h1>"), None);
        assert_eq!(article_title("text/plain", "# Not markdown"), None);
    }

    #[test]
    fn a_heading_hidden_in_a_removed_element_is_not_the_title() {
        assert_eq!(
            article_title(
                "text/html",
                "<noscript><h1>Hidden</h1></noscript><p>Body</p>"
            ),
            None
        );
    }
}
