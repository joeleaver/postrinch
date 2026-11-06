//! A message, ready to show: its best part, parsed and sanitized.
//!
//! Mail is hostile input (DESIGN.md "Rendering" and "Safety"). The HTML part
//! goes through rinch's cleaner (`rinch_html::clean`): parsed as a browser
//! parses it and kept only as far as an allow-list says (no scripts, forms,
//! frames, objects or event attributes), its `<style>` sheets applied inline
//! and dropped (they would reach the window's own elements), inline styles
//! without what could cover the window. Pictures the message carries
//! (`cid:`) are inlined; pictures on the web are held back unless the person
//! asked for them, as loading them tells the sender the mail was opened
//! (sanitize.rs). A message with no HTML part is its text, escaped.

mod sanitize;

use mail_parser::{MessageParser, PartKind};

pub use sanitize::{policy, sanitize};

#[cfg(target_os = "linux")]
macro_rules! platform {
    () => {
        "X11; Linux x86_64"
    };
}
#[cfg(target_os = "windows")]
macro_rules! platform {
    () => {
        "Windows NT 10.0; Win64; x64"
    };
}
#[cfg(target_os = "macos")]
macro_rules! platform {
    () => {
        "Macintosh; Intel Mac OS X 10_15_7"
    };
}
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
macro_rules! platform {
    () => {
        "Unknown"
    };
}

/// The User-Agent pictures from the web are fetched with: a browser's shape
/// with Postrinch's own name. Not "(compatible; …)": firewalls such as
/// delanceyplace.com's drop that as a bot's.
pub const PICTURE_USER_AGENT: &str = concat!(
    "Mozilla/5.0 (",
    platform!(),
    ") Postrinch/",
    env!("CARGO_PKG_VERSION")
);

/// What a message shows as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// HTML for the inside of the message's body, safe to hand to rinch.
    pub html: String,
    /// The message's own `<body>` style (its background, its margin),
    /// sanitized, for the element that holds `html`.
    pub body_style: String,
    /// Whether it came from an HTML part (else from text).
    pub from_html: bool,
    /// Pictures on the web left out because the person has not asked for
    /// them.
    pub remote_hidden: usize,
    /// The HTML part as its sender wrote it, the pictures it carries
    /// inlined, for a view that cleans it at its own width (rinch's
    /// `HtmlView` with [`policy`]); empty for a text message.
    pub document: String,
    /// Whether pictures from the web were allowed: what [`policy`] needs.
    pub remote: bool,
}

/// How to render.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Options {
    /// Load pictures from the web.
    pub remote: bool,
    /// The width the message is drawn at, for its `@media` rules; `None`
    /// leaves them out (as a wide view).
    pub width: Option<f32>,
}

/// The CSS for the element a message's `html` goes in (class `mail-html`,
/// holding a `mail-body` with the message's `body_style`): mail is drawn on
/// white, as it is written for, in a dark window too. The window and the
/// rendering comparison tool both use it, so they draw mail alike.
pub const HOST_CSS: &str = r#"
.mail-html {
    padding: 12px;
    border-radius: 6px;
    background: #ffffff;
    color: #222222;
    font-family: sans-serif;
    line-height: 1.4;
    overflow-x: auto;
}
"#;

/// Render a raw RFC 5322 message. `None` when it does not parse.
pub fn render(raw: &[u8], options: Options) -> Option<Rendered> {
    let parsed = MessageParser::default().parse(raw)?;
    let Some(html) = html_part(&parsed) else {
        let text = parsed.text_bodies().collect::<Vec<_>>().join("\n\n");
        return Some(Rendered {
            html: rinch_html::Cleaned::from_text(&text).html,
            body_style: String::new(),
            from_html: false,
            remote_hidden: 0,
            document: String::new(),
            remote: options.remote,
        });
    };
    let inline = inline_pictures(&parsed);
    let cleaned = sanitize(&html, &inline, options);
    Some(Rendered {
        from_html: true,
        document: with_pictures(html, &inline),
        ..cleaned
    })
}

/// A message's HTML part as its sender wrote it, unsanitized, with the
/// pictures it carries (`cid:`) inlined: for comparing what Postrinch draws
/// with what a browser draws. Never shown in the window. `None` without an
/// HTML part.
pub fn original_html(raw: &[u8]) -> Option<String> {
    let parsed = MessageParser::default().parse(raw)?;
    let html = html_part(&parsed)?;
    Some(with_pictures(html, &inline_pictures(&parsed)))
}

/// `html` with its `cid:` references replaced by the pictures they name.
fn with_pictures(mut html: String, inline: &[(String, String)]) -> String {
    for (id, url) in inline {
        html = html.replace(&format!("cid:{id}"), url);
    }
    html
}

fn html_part(parsed: &mail_parser::Message) -> Option<String> {
    let part = parsed
        .html_body()
        .find(|part| matches!(part.kind(), PartKind::Html))?;
    Some(part.text().unwrap_or_default().into_owned())
}

/// Pictures the message carries, by Content-ID, as data: URLs.
fn inline_pictures(parsed: &mail_parser::Message) -> Vec<(String, String)> {
    parsed
        .parts()
        .filter_map(|p| {
            let id = p.content_id()?.trim_matches(['<', '>']).to_owned();
            let ct = p.content_type()?;
            if !ct.ctype().eq_ignore_ascii_case("image") {
                return None;
            }
            let mime = format!("image/{}", ct.subtype()?.to_ascii_lowercase());
            use base64::Engine;
            let data = base64::engine::general_purpose::STANDARD.encode(p.decoded());
            Some((id, format!("data:{mime};base64,{data}")))
        })
        .collect()
}

/// A message's text, for a program reading it (an agent): what its reader
/// sees, and apart from it what the message hides from its reader.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageText {
    /// The text the reader sees: an HTML message's as drawn, else its text
    /// part.
    pub text: String,
    /// Text in the HTML that the reader cannot see (`display: none`,
    /// white on white, a 1px font…): a known way to address instructions to
    /// an AI, invisible to the person.
    pub hidden: Vec<String>,
}

/// A raw message's [`MessageText`]. `None` when it does not parse.
pub fn message_text(raw: &[u8]) -> Option<MessageText> {
    let rendered = render(raw, Options::default())?;
    if !rendered.from_html {
        let parsed = MessageParser::default().parse(raw)?;
        let text = parsed.text_bodies().collect::<Vec<_>>().join("\n\n");
        return Some(MessageText {
            text,
            hidden: Vec::new(),
        });
    }
    let text = rinch_html::text(&rendered.html);
    Some(MessageText {
        text: text.visible,
        hidden: text.hidden,
    })
}

#[cfg(test)]
mod user_agent_tests {
    #[test]
    fn picture_user_agent_is_not_a_bots() {
        let ua = super::PICTURE_USER_AGENT;
        assert!(ua.starts_with("Mozilla/5.0 ("));
        assert!(ua.contains(") Postrinch/"));
        assert!(!ua.contains("compatible"));
    }
}
