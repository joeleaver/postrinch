//! A message with no design of its own (crate::plain), redrawn for a dark
//! window: its near-black text in the window's ink, its other text colours
//! made light at the same hue (a dark blue signature turns light blue), its
//! white and pale grey backgrounds cleared or kept as a faint lift, its
//! borders kept visible, and its links in the window's link colour. The
//! mail is rewritten as written, before rinch's cleaner reads it: in its
//! `style` attributes, its `<style>` sheets and its old colour attributes
//! (`color`, `bgcolor`, `<body text link>`). Text between tags is left as it
//! is.

use crate::colour::{self, Rgba};

/// The window's colours the mail is redrawn in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DarkColours {
    /// Text the mail draws near-black.
    pub ink: Rgba,
    /// Links that name no colour of their own.
    pub link: Rgba,
}

/// `html` (a document as written) redrawn for a dark window; see the module
/// docs.
pub fn for_dark(html: &str, colours: DarkColours) -> String {
    let mut out = String::with_capacity(html.len() + 64);
    // Links first, so a colour the mail gives its links (redrawn) wins.
    out.push_str(&format!(
        "<style>a[href] {{ color: {} }}</style>",
        colours.link.css()
    ));
    let mut rest = html;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        rest = &rest[open..];
        if rest.starts_with("<!--") {
            let end = rest.find("-->").map_or(rest.len(), |e| e + 3);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        out.push_str(&redraw_tag(tag, colours));
        rest = &rest[end..];
        if tag_name(tag).eq_ignore_ascii_case("style") {
            let close = find_ignore_case(rest, "</style").unwrap_or(rest.len());
            out.push_str(&redraw_sheet(&rest[..close], colours));
            rest = &rest[close..];
        }
    }
    out.push_str(rest);
    out
}

/// Where a tag starting at `<` ends (after its `>`), with `>` inside a
/// quoted attribute value passed over.
fn tag_end(s: &str) -> usize {
    let mut quote = None;
    for (i, c) in s.char_indices().skip(1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return i + 1,
            (None, '<') => return i,
            _ => {}
        }
    }
    s.len()
}

fn tag_name(tag: &str) -> &str {
    let name = tag.trim_start_matches('<');
    let end = name
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(name.len());
    &name[..end]
}

fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .as_bytes()
        .windows(needle.len())
        .position(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

/// A start tag with the colours in its attributes redrawn.
fn redraw_tag(tag: &str, colours: DarkColours) -> String {
    if tag.starts_with("</") || tag.starts_with("<!") {
        return tag.to_owned();
    }
    let mut out = String::with_capacity(tag.len());
    let mut at = 0;
    for (name, value) in attributes(tag) {
        let original = &tag[value.clone()];
        let redrawn = match name.as_str() {
            "style" => redraw_declarations(original, colours),
            "color" | "text" | "link" | "vlink" | "alink" => {
                redraw_attribute(original, |c| text(c, colours))
            }
            "bgcolor" => redraw_attribute(original, background),
            "bordercolor" => redraw_attribute(original, border),
            _ => continue,
        };
        out.push_str(&tag[at..value.start]);
        out.push_str(&redrawn);
        at = value.end;
    }
    out.push_str(&tag[at..]);
    out
}

fn redraw_attribute(value: &str, map: impl Fn(Rgba) -> Rgba) -> String {
    match colour::parse_attribute(&value.to_ascii_lowercase()) {
        Some(c) => map(c).css(),
        None => value.to_owned(),
    }
}

/// A tag's attributes: each name, lower-cased, and the place of its value
/// (inside any quotes) in `tag`.
fn attributes(tag: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let bytes = tag.as_bytes();
    let mut out = Vec::new();
    let mut i = 1 + tag_name(tag).len();
    while i < bytes.len() {
        while i < bytes.len() && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        let start = i;
        while i < bytes.len() && !matches!(bytes[i], b'=' | b'>' | b'/') {
            if bytes[i].is_ascii_whitespace() {
                break;
            }
            i += 1;
        }
        if i == start {
            i += 1;
            continue;
        }
        let name = tag[start..i].to_ascii_lowercase();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let value = match bytes.get(i) {
            Some(&q @ (b'"' | b'\'')) => {
                let end = tag[i + 1..]
                    .find(q as char)
                    .map_or(bytes.len(), |e| i + 1 + e);
                let range = i + 1..end;
                i = end + 1;
                range
            }
            _ => {
                let start = i;
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' {
                    i += 1;
                }
                start..i
            }
        };
        out.push((name, value));
    }
    out
}

/// A style sheet with the colours in its rules redrawn. Only what is inside
/// a rule's innermost braces is touched, so selectors (`a:link`) are not.
fn redraw_sheet(sheet: &str, colours: DarkColours) -> String {
    let mut out = String::with_capacity(sheet.len());
    let mut rest = sheet;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..=open]);
        rest = &rest[open + 1..];
        let close = rest.find('}').unwrap_or(rest.len());
        let block = &rest[..close];
        if let Some(inner) = block.find('{') {
            // An at-rule's block: its first rule starts here.
            out.push_str(&block[..inner]);
            rest = &rest[inner..];
            continue;
        }
        out.push_str(&redraw_declarations(block, colours));
        rest = &rest[close..];
    }
    out.push_str(rest);
    out
}

/// Declarations (a `style` value, a rule's block) with their colours
/// redrawn by what each property draws.
fn redraw_declarations(block: &str, colours: DarkColours) -> String {
    let mut out = String::with_capacity(block.len());
    let mut at = 0;
    for (property, value) in colour::declarations(block) {
        let map: Box<dyn Fn(Rgba) -> Rgba> = if property == "color" {
            Box::new(move |c| text(c, colours))
        } else if property.starts_with("background") {
            Box::new(background)
        } else if property.starts_with("border") || property.starts_with("outline") {
            Box::new(border)
        } else if property.ends_with("color") && !property.starts_with("mso-") {
            Box::new(move |c| text(c, colours))
        } else {
            continue;
        };
        out.push_str(&block[at..value.start]);
        out.push_str(&colour::map_colours(&block[value.clone()], map));
        at = value.end;
    }
    out.push_str(&block[at..]);
    out
}

/// A text colour on a dark page: near-black and dark grey become the ink;
/// any other turns light at its own hue, light enough to read.
fn text(c: Rgba, colours: DarkColours) -> Rgba {
    if c.a < 0.05 {
        return c;
    }
    let l = c.lightness();
    if c.chroma() <= 40 && l < 0.4 {
        return Rgba {
            a: c.a,
            ..colours.ink
        };
    }
    let least = if c.chroma() <= 40 { 0.55 } else { 0.65 };
    c.with_lightness((1.0 - l).clamp(least, 0.92))
}

/// A background on a dark page: white goes, pale grey becomes a faint lift
/// over the page, and anything darker (only in what a reply quotes) dark at
/// its own hue.
fn background(c: Rgba) -> Rgba {
    if c.a < 0.05 {
        return c;
    }
    let l = c.lightness();
    if l >= 0.96 {
        return Rgba { a: 0.0, ..c };
    }
    if c.is_light_neutral() {
        let lift = ((1.0 - l) * 1.2).clamp(0.04, 0.14);
        return Rgba {
            r: 255,
            g: 255,
            b: 255,
            a: lift,
        };
    }
    c.with_lightness((1.0 - l).min(0.3))
}

/// A border on a dark page: still a line, light or dark as it was against
/// its page, but neither glaring nor lost.
fn border(c: Rgba) -> Rgba {
    if c.a < 0.05 {
        return c;
    }
    c.with_lightness((1.0 - c.lightness()).clamp(0.25, 0.75))
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLOURS: DarkColours = DarkColours {
        ink: Rgba::rgb(0xe7, 0xe9, 0xee),
        link: Rgba::rgb(0x8a, 0xb4, 0xf8),
    };

    fn dark(html: &str) -> String {
        let out = for_dark(html, COLOURS);
        out.split_once("</style>").unwrap().1.to_owned()
    }

    #[test]
    fn near_black_text_takes_the_ink() {
        assert_eq!(
            dark(r##"<span style="color: rgb(34, 34, 34); font-size: 12px">Hi</span>"##),
            r##"<span style="color: #e7e9ee; font-size: 12px">Hi</span>"##
        );
        assert_eq!(
            dark("<font color=black>x</font>"),
            "<font color=#e7e9ee>x</font>"
        );
    }

    #[test]
    fn coloured_text_turns_light_at_its_hue() {
        let out = dark(r##"<p style="color:#1F497D">sig</p>"##);
        let value = out
            .split("color:")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        let c = colour::parse(value).unwrap();
        assert!(c.lightness() >= 0.64 && c.b > c.r, "{value}");
    }

    #[test]
    fn white_backgrounds_clear_and_greys_lift() {
        assert_eq!(
            dark(r##"<body bgcolor="#ffffff" style="background-color: white">"##),
            r##"<body bgcolor="rgba(255, 255, 255, 0.00)" style="background-color: rgba(255, 255, 255, 0.00)">"##
        );
        let out = dark(r##"<td style="background:#eeeeee">"##);
        assert!(out.contains("rgba(255, 255, 255, 0.08)"), "{out}");
    }

    #[test]
    fn sheets_change_declarations_not_selectors_or_text() {
        let html = "<style>@media screen { a:link { color: #0563C1 } } p.MsoNormal { color: windowtext; margin: 0 }</style>\
                    <p>color: black; background: white</p>";
        let out = dark(html);
        assert!(out.contains("a:link { color: #"), "{out}");
        assert!(!out.contains("#0563C1"), "{out}");
        assert!(
            out.contains("p.MsoNormal { color: #e7e9ee; margin: 0 }"),
            "{out}"
        );
        assert!(
            out.ends_with("<p>color: black; background: white</p>"),
            "{out}"
        );
    }

    #[test]
    fn links_get_the_window_link_colour_first() {
        assert!(
            for_dark("<a href=x>x</a>", COLOURS)
                .starts_with("<style>a[href] { color: #8ab4f8 }</style>")
        );
    }

    #[test]
    fn a_quoted_greater_than_does_not_end_a_tag() {
        assert_eq!(
            dark(r##"<p title="a > b" style="color:#000">x</p>"##),
            r##"<p title="a > b" style="color:#e7e9ee">x</p>"##
        );
    }
}
