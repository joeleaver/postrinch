//! Quoted text: the earlier messages a reply carries under it, folded under
//! "Show quoted text". Nothing is removed. The window gives rinch's mail
//! view a user style sheet ([`quote_sheet`]) that hides what these
//! selectors match while the quote is folded, and empties it to show it;
//! the view says how many elements matched, so the button shows only when
//! a message has a quote. Text mail is made a document of its own
//! ([`text_document`]), its quoted lines in a `blockquote type=cite`, so
//! the same sheet folds them.

/// What mail programs mark a quote with. Kept simple (no `:nth-child`, no
/// complex right side for `~`): matching cost grows with what the sender
/// wrote.
pub const QUOTE_SELECTORS: &[&str] = &[
    // Apple Mail, Thunderbird, and Postrinch's own text mail.
    "blockquote[type=cite]",
    // Gmail: the attribution line and the quote, in one wrapper.
    "div.gmail_quote",
    "div.gmail_quote_container",
    // Thunderbird's attribution line.
    ".moz-cite-prefix",
    // Yahoo and Proton.
    ".yahoo_quoted",
    ".protonmail_quote",
    // Apple Mail on iOS.
    "div[name=messageReplySection]",
    // Outlook: everything after its reply header.
    "#appendonsend ~ *",
    "#divRplyFwdMsg",
    "#divRplyFwdMsg ~ *",
];

/// The user style sheet for a message: the quote hidden while folded,
/// nothing once the person shows it.
pub fn quote_sheet(shown: bool) -> String {
    let declarations = if shown { "" } else { "display: none" };
    QUOTE_SELECTORS
        .iter()
        .map(|selector| format!("{selector} {{ {declarations} }}\n"))
        .collect()
}

/// A text message as a document for the mail view, as rinch draws text
/// (`white-space: pre-wrap`), each run of quoted lines (`>`), with the
/// attribution line just above it ("On …, … wrote:"), in a
/// `blockquote type=cite`.
pub fn text_document(text: &str) -> String {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.split('\n').collect();
    let quoted = |line: &str| line.trim_start().starts_with('>');
    let mut in_quote = vec![false; lines.len()];
    for (i, line) in lines.iter().enumerate() {
        in_quote[i] = quoted(line);
    }
    // The attribution goes with the quote under it, blank lines between
    // them too.
    for i in 0..lines.len() {
        if in_quote[i] || !lines[i].trim_end().ends_with(':') {
            continue;
        }
        let next = (i + 1..lines.len()).find(|&j| !lines[j].trim().is_empty());
        if let Some(j) = next
            && in_quote[j]
        {
            in_quote[i..j].iter_mut().for_each(|q| *q = true);
        }
    }
    let mut out = String::from("<div style=\"white-space: pre-wrap; overflow-wrap: anywhere\">");
    let mut i = 0;
    while i < lines.len() {
        let quote = in_quote[i];
        let start = i;
        while i < lines.len() && in_quote[i] == quote {
            i += 1;
        }
        let run = escape(&lines[start..i].join("\n"));
        if quote {
            out.push_str("<blockquote type=\"cite\" style=\"margin: 0\">");
            out.push_str(&run);
            out.push_str("</blockquote>");
        } else {
            out.push_str(&run);
            // A line break just before a block ends the line and shows
            // nothing, so the line the text ended on needs one more.
            if i < lines.len() {
                out.push('\n');
            }
        }
    }
    out.push_str("</div>");
    out
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_lines_and_their_attribution_go_in_one_quote() {
        let text = "Works for me.\n\nOn Mon, Ada wrote:\n\n> Thursday?\n> Or Friday\n\nSam";
        let doc = text_document(text);
        assert_eq!(
            doc,
            "<div style=\"white-space: pre-wrap; overflow-wrap: anywhere\">Works for me.\n\n\
             <blockquote type=\"cite\" style=\"margin: 0\">On Mon, Ada wrote:\n\n&gt; Thursday?\n&gt; Or Friday</blockquote>\
             \nSam</div>"
        );
    }

    #[test]
    fn a_colon_with_no_quote_under_it_stays() {
        let doc = text_document("Agenda:\n1. Rota");
        assert!(!doc.contains("blockquote"));
        assert!(doc.contains("Agenda:\n1. Rota"));
    }

    #[test]
    fn the_sheet_hides_until_shown() {
        assert!(quote_sheet(false).contains("div.gmail_quote { display: none }"));
        assert!(!quote_sheet(true).contains("display: none"));
        assert_eq!(quote_sheet(true).lines().count(), QUOTE_SELECTORS.len());
    }
}
