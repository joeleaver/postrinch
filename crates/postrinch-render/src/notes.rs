//! An event's notes, drawn as mail is. Google keeps them as HTML of its
//! own (`<p>`, `<b>`, `<a href>`, `<br>`, lists) when they were written in
//! Google Calendar, and as plain text when an app wrote them; either may
//! hold links. They go through the same cleaning as a message, so what
//! one draws safely the other does too.

use crate::{Options, Rendered, render};

/// Whether the notes are HTML rather than text.
pub fn notes_are_html(notes: &str) -> bool {
    let lower = notes.to_ascii_lowercase();
    [
        "<br", "<p>", "<p ", "</p>", "<a ", "</a>", "<b>", "</b>", "<i>", "<u>", "<ul", "<ol",
        "<li", "<div", "<span", "<strong", "<em>", "<html",
    ]
    .iter()
    .any(|tag| lower.contains(tag))
}

/// Notes as a message would be rendered: pictures from the web held back.
pub fn notes(notes: &str) -> Rendered {
    let kind = if notes_are_html(notes) {
        "html"
    } else {
        "plain"
    };
    let raw = format!(
        "MIME-Version: 1.0\r\nContent-Type: text/{kind}; charset=UTF-8\r\n\
         Content-Transfer-Encoding: 8bit\r\n\r\n{notes}"
    );
    render(raw.as_bytes(), Options::default()).unwrap_or_else(|| Rendered {
        html: rinch_html::Cleaned::from_text(notes).html,
        body_style: String::new(),
        from_html: false,
        remote_hidden: 0,
        document: crate::text_document(notes),
        remote: false,
        plain: true,
    })
}

/// Notes as text, to edit: HTML notes as what they show, line by line.
pub fn notes_text(notes: &str) -> String {
    if !notes_are_html(notes) {
        return notes.to_owned();
    }
    rinch_html::text(&self::notes(notes).html)
        .visible
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_s_html_notes_draw_as_html() {
        let html =
            "<p><b>Agenda</b></p><p>Join: <a href=\"https://meet.example/x\">the call</a></p>";
        assert!(notes_are_html(html));
        let drawn = notes(html);
        assert!(drawn.from_html);
        assert!(drawn.html.contains("<b>Agenda</b>"), "{}", drawn.html);
        assert!(drawn.html.contains("href=\"https://meet.example/x\""));
        let text = notes_text(html);
        assert!(
            text.contains("Agenda") && text.contains("the call"),
            "{text}"
        );
        assert!(!text.contains('<'));
    }

    #[test]
    fn text_notes_stay_text_with_their_links() {
        let text = "Public holiday\nSee https://g.co/calendar.";
        assert!(!notes_are_html(text));
        let drawn = notes(text);
        assert!(!drawn.from_html);
        assert!(
            drawn
                .document
                .contains("<a href=\"https://g.co/calendar\">https://g.co/calendar</a>."),
            "{}",
            drawn.document
        );
        assert_eq!(notes_text(text), text);
        // A sum is not a tag.
        assert!(!notes_are_html("Budget < 5 and > 2"));
    }
}
