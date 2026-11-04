//! A message, ready to show: its best part, parsed and sanitized.
//!
//! Mail is hostile input (DESIGN.md "Rendering" and "Safety"). The HTML part
//! is parsed with the HTML5 algorithm (html5ever, through ammonia) and kept
//! only as far as an allow-list says: no scripts, forms, frames, objects,
//! event attributes or `<style>` sheets (they would reach the window's own
//! elements); inline styles lose what could cover the window or fetch from
//! the web. Pictures the message carries (`cid:`) are inlined; pictures on
//! the web are held back unless the person asked for them, as loading them
//! tells the sender the mail was opened. A message with no HTML part is its
//! text, escaped.

mod sanitize;

use mail_parser::{MessageParser, PartKind};

pub use sanitize::sanitize;

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
}

/// How to render.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Load pictures from the web.
    pub remote: bool,
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
            html: text_html(&text),
            body_style: String::new(),
            from_html: false,
            remote_hidden: 0,
        });
    };
    let cleaned = sanitize(&html, &inline_pictures(&parsed), options);
    Some(Rendered {
        from_html: true,
        ..cleaned
    })
}

/// A message's HTML part as its sender wrote it, unsanitized, with the
/// pictures it carries (`cid:`) inlined: for comparing what Postrinch draws
/// with what a browser draws. Never shown in the window. `None` without an
/// HTML part.
pub fn original_html(raw: &[u8]) -> Option<String> {
    let parsed = MessageParser::default().parse(raw)?;
    let mut html = html_part(&parsed)?;
    for (id, url) in inline_pictures(&parsed) {
        html = html.replace(&format!("cid:{id}"), &url);
    }
    Some(html)
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

/// Plain text as HTML: escaped, its line breaks and runs of spaces kept.
fn text_html(text: &str) -> String {
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace("\r\n", "\n");
    format!("<div style=\"white-space: pre-wrap; overflow-wrap: anywhere\">{escaped}</div>")
}
