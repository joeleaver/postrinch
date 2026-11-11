//! Whether an HTML message carries colours of its own. One that does not
//! (a message written as plain text, sent as HTML) reads best in the
//! window's own colours, dark ones included; one that does is drawn on the
//! white it was designed on. The earlier messages a reply quotes are left
//! out of the question: they are folded, and when shown, the quote sheet
//! gives them the window's colours too (crate::quotes).

/// Whether `html` sets any colour or background outside what it quotes:
/// in a tag's attributes (`bgcolor`, `background`, `color`, `<body text>`,
/// a `style`) or in a `<style>` sheet. Text between tags is not looked at,
/// so a message that talks about colour is still plain. When in doubt it
/// says yes: drawing a styled message on the window's colours would break
/// it.
pub fn has_own_colours(html: &str) -> bool {
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
        if tag.contains("color") || tag.contains("background") {
            return true;
        }
        if name == "body" && tag.contains("text=") {
            return true;
        }
        if name == "style" && !closing {
            let sheet_end = rest.find("</style").unwrap_or(rest.len());
            let sheet = &rest[..sheet_end];
            if sheet.contains("color") || sheet.contains("background") {
                return true;
            }
            rest = &rest[sheet_end..];
        }
    }
    false
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
    use super::has_own_colours;

    #[test]
    fn mail_written_as_text_has_none() {
        assert!(!has_own_colours(
            "<div dir=\"ltr\">Thursday works.<br>Any colour you like.</div>"
        ));
        assert!(!has_own_colours("<style>p { margin: 0 }</style><p>Hi</p>"));
    }

    #[test]
    fn styles_attributes_and_sheets_count() {
        assert!(has_own_colours("<td bgcolor=\"#eee\">x</td>"));
        assert!(has_own_colours("<p style=\"color: red\">x</p>"));
        assert!(has_own_colours("<font COLOR=\"red\">x</font>"));
        assert!(has_own_colours("<body text=\"#333\">x</body>"));
        assert!(has_own_colours("<style>.a { background: #000 }</style>"));
    }

    #[test]
    fn what_a_reply_quotes_is_left_out() {
        let reply = "<div dir=\"ltr\">Thanks!</div><div class=\"gmail_quote\">On Mon wrote:\
                     <blockquote class=\"gmail_quote\"><div><span style=\"color:red\">x</span>\
                     <div>nested</div></div></blockquote></div>";
        assert!(!has_own_colours(reply));
        let after = format!("{reply}<p style=\"color: blue\">signature</p>");
        assert!(has_own_colours(&after));
        assert!(!has_own_colours(
            "<p>Ok</p><blockquote type=\"cite\"><font color=red>x</font></blockquote>"
        ));
        assert!(!has_own_colours(
            "<p>Ok</p><div id=\"divRplyFwdMsg\"><b>From:</b></div><div style=\"color:black\">x</div>"
        ));
    }
}
