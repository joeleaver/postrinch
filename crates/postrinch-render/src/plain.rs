//! Whether an HTML message has a design of its own. One that has not (a
//! message written as text and sent as HTML, a reply from Gmail or Outlook,
//! a notice that is text on white) reads best in the window's own colours,
//! dark ones included (crate::dark); one that has (a newsletter, a receipt
//! with coloured bands and buttons) is drawn on the white it was designed
//! on. What decides it is the page under the text: a message whose
//! backgrounds are all white, pale grey or clear, with no background
//! picture, has no design of its own, whatever colours its text is in
//! (a signature in blue, Outlook's style sheet). The earlier messages a
//! reply quotes are left out of the question: they are folded, and when
//! shown, the quote sheet gives them the window's colours too
//! (crate::quotes).

use crate::colour;

/// Whether `html` sets a background of its own outside what it quotes,
/// other than white, pale grey or clear: a colour or a picture, in a tag's
/// attributes (`bgcolor`, `background`, a `style`) or in a `<style>` sheet.
/// Text between tags is not looked at, so a message that talks about
/// colour is still plain. When in doubt it says yes: drawing a designed
/// message in the window's colours would break it.
pub fn has_own_design(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    let mut rest = lower.as_str();
    // The tag a quote began with, and how deep inside it the scan is.
    let mut quote: Option<(String, usize)> = None;
    while let Some(open) = rest.find('<') {
        rest = &rest[open + 1..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let closing = tag.starts_with('/');
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if let Some((quoted, depth)) = &mut quote {
            if name == *quoted && !tag.ends_with('/') {
                if closing {
                    *depth -= 1;
                    if *depth == 0 {
                        quote = None;
                    }
                } else {
                    *depth += 1;
                }
            }
            continue;
        }
        if outlook_reply(tag) {
            // Everything after Outlook's reply header is the quote.
            return false;
        }
        if !closing && starts_quote(&name, tag) {
            quote = Some((name, 1));
            continue;
        }
        if tag_has_design(tag) {
            return true;
        }
        if name == "style" && !closing {
            let sheet_end = rest.find("</style").unwrap_or(rest.len());
            let sheet = &rest[..sheet_end];
            if sheet.split('}').any(|rule| {
                rule.rsplit_once('{')
                    .is_some_and(|(_, block)| block_has_design(block))
            }) {
                return true;
            }
            rest = &rest[sheet_end..];
        }
    }
    false
}

/// Whether a tag (lower-cased, without `<`) gives a background of its own.
fn tag_has_design(tag: &str) -> bool {
    if attribute(tag, "background").is_some() {
        return true;
    }
    if let Some(value) = attribute(tag, "bgcolor")
        && !colour::parse_attribute(&value).is_some_and(|c| c.is_light_neutral())
    {
        return true;
    }
    attribute(tag, "style").is_some_and(|style| block_has_design(&style))
}

/// Whether declarations (a `style`, a rule's block) give a background of
/// their own: a picture, a gradient, or a colour that is not light and
/// neutral.
fn block_has_design(block: &str) -> bool {
    colour::declarations(block)
        .into_iter()
        .any(|(property, range)| {
            if !property.starts_with("background") {
                return false;
            }
            let value = block[range].replace("!important", "");
            let value = value.trim();
            if value.contains("url(") || value.contains("gradient") {
                return true;
            }
            if matches!(
                value,
                "" | "none" | "inherit" | "initial" | "unset" | "transparent"
            ) {
                return false;
            }
            let found = colour::colours_in(value);
            if property == "background-color" && found.is_empty() {
                // A colour it cannot read (`var(…)`, a name it does not know).
                return true;
            }
            found.iter().any(|(_, c)| !c.is_light_neutral())
        })
}

/// Whether a start tag (lower-cased, without `<`) begins a quote, as
/// [`crate::QUOTE_SELECTORS`] name them.
fn starts_quote(name: &str, tag: &str) -> bool {
    let class = attribute(tag, "class").unwrap_or_default();
    let classes: Vec<&str> = class.split_whitespace().collect();
    (name == "blockquote" && attribute(tag, "type").as_deref() == Some("cite"))
        || classes.iter().any(|c| {
            matches!(
                *c,
                "gmail_quote" | "gmail_quote_container" | "yahoo_quoted" | "protonmail_quote"
            )
        })
        || (name == "div" && attribute(tag, "name").as_deref() == Some("messagereplysection"))
}

fn outlook_reply(tag: &str) -> bool {
    matches!(
        attribute(tag, "id").as_deref(),
        Some("divrplyfwdmsg" | "appendonsend")
    )
}

/// An attribute's value in a start tag, quoted or not.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut at = 0;
    while let Some(found) = tag[at..].find(name) {
        let start = at + found;
        at = start + name.len();
        let before = tag[..start].chars().last();
        if !before.is_some_and(char::is_whitespace) {
            continue;
        }
        let after = tag[at..].trim_start();
        let Some(value) = after.strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        return Some(match value.chars().next() {
            Some(q @ ('"' | '\'')) => value[1..].split(q).next().unwrap_or("").to_owned(),
            _ => value.split_whitespace().next().unwrap_or("").to_owned(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::has_own_design;

    #[test]
    fn mail_written_as_text_has_none() {
        assert!(!has_own_design(
            "<div dir=\"ltr\">Thursday works.<br>Any colour you like.</div>"
        ));
        assert!(!has_own_design("<style>p { margin: 0 }</style><p>Hi</p>"));
    }

    #[test]
    fn coloured_text_on_white_is_not_a_design() {
        // A Gmail signature, Outlook's sheet, a notice on white.
        assert!(!has_own_design(
            "<span style=\"color:rgb(34,34,34);background-color:rgb(255,255,255)\">Joe</span>"
        ));
        assert!(!has_own_design(
            "<style>a:link, span.MsoHyperlink { color: #0563C1 } p.MsoNormal { color: windowtext }</style>\
             <body lang=EN-US link=\"#0563C1\"><p class=MsoNormal><font color=\"#1F497D\">Sig</font></p>"
        ));
        assert!(!has_own_design(
            "<body bgcolor=\"#ffffff\"><td style=\"background:#f6f8fa; border:1px solid #ddd\">x</td>"
        ));
        assert!(!has_own_design(
            "<td style=\"background-image: none\">x</td>"
        ));
    }

    #[test]
    fn backgrounds_count_in_attributes_styles_and_sheets() {
        assert!(has_own_design("<td bgcolor=\"#28a745\">x</td>"));
        assert!(has_own_design("<td background=\"bg.png\">x</td>"));
        assert!(has_own_design(
            "<a style=\"background-color: #0b57d0\">Go</a>"
        ));
        assert!(has_own_design(
            "<div style=\"background: url(x.png)\">x</div>"
        ));
        assert!(has_own_design("<style>.a { background: #000 }</style>"));
        assert!(has_own_design(
            "<style>@media x { .b { background-color: #fff2cc } }</style>"
        ));
        assert!(has_own_design(
            "<p style=\"background-color: var(--x)\">x</p>"
        ));
    }

    #[test]
    fn what_a_reply_quotes_is_left_out() {
        let reply = "<div dir=\"ltr\">Thanks!</div><div class=\"gmail_quote\">On Mon wrote:\
                     <blockquote class=\"gmail_quote\"><div><span style=\"color:red\">x</span>\
                     <div>nested</div></div></blockquote></div>";
        assert!(!has_own_design(reply));
        let after = format!("{reply}<p style=\"background: navy\">signature</p>");
        assert!(has_own_design(&after));
        assert!(!has_own_design(
            "<p>Ok</p><blockquote type=\"cite\"><td bgcolor=red>x</td></blockquote>"
        ));
        assert!(!has_own_design(
            "<p>Ok</p><div id=\"divRplyFwdMsg\"><b>From:</b></div><div style=\"background:black\">x</div>"
        ));
    }
}
