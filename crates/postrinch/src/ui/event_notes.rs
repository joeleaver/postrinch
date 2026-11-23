//! An event's notes in the calendar's pane, drawn as mail is
//! (postrinch_render::notes): Google's HTML with its bold, lists and
//! links, or text with its web addresses made links; in the window's
//! colours, dark ones too, unless the notes bring a design of their own.
//! A click on a link opens it.

use rinch::prelude::*;

use crate::state::State;

#[component]
pub fn event_notes(state: State, notes: String) -> NodeHandle {
    let rendered = postrinch_render::notes(&notes);
    rsx! {
        div { class: "event-notes",
            // Drawn again when the window turns light or dark.
            for drawn in [(rendered.clone(), rendered.plain && state.dark())] {
                div { key: drawn.1,
                    {view(__scope, drawn.0.clone(), drawn.1)}
                }
            }
        }
    }
}

/// Where an event is: a meeting's web address as a short "Join" link
/// (its whole address in the tooltip), anything else as its words.
#[component]
pub fn place(location: String, class: &'static str) -> NodeHandle {
    let at = location.trim().to_owned();
    let web = (at.starts_with("https://") || at.starts_with("http://"))
        && !at.contains(char::is_whitespace);
    if !web {
        return rsx! { div { class: class, {at.clone()} } };
    }
    let label = format!("Join on {}", service(&at));
    let (href, title) = (at.clone(), at.clone());
    rsx! {
        div { class: class,
            button { class: "link-button event-join", title: title,
                onclick: move || crate::links::open(&href), {label} }
        }
    }
}

/// The meeting service a web address is on, by its host.
fn service(url: &str) -> String {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    let known = [
        ("teams.microsoft.com", "Teams"),
        ("teams.live.com", "Teams"),
        ("meet.google.com", "Google Meet"),
        ("zoom.us", "Zoom"),
        ("webex.com", "Webex"),
        ("whereby.com", "Whereby"),
        ("gotomeeting.com", "GoTo Meeting"),
    ];
    known
        .iter()
        .find(|(h, _)| host == *h || host.ends_with(&format!(".{h}")))
        .map_or(host.clone(), |(_, name)| (*name).to_owned())
}

#[component]
fn view(rendered: postrinch_render::Rendered, dark: bool) -> NodeHandle {
    let document = crate::ui::reader::document(&rendered, dark);
    let policy = postrinch_render::policy(rendered.remote);
    let card = if rendered.plain {
        "mail-html plain event-notes-body"
    } else {
        "mail-html event-notes-body"
    };
    rsx! {
        div { class: card,
            HtmlView {
                document: document,
                policy: policy,
                // A long address in a link breaks where it must.
                user_css_fn: move || "a { word-break: break-all; } span, p, div, td { overflow-wrap: anywhere; }".to_owned(),
                onlink: move |href: String| crate::links::open(&href),
            }
        }
    }
}

pub const CSS: &str = r#"
.event-notes { border-top: 1px solid var(--line-soft); padding-top: 10px; }
.event-notes-body { padding: 0; font-size: 13px; }
.mail-html.event-notes-body:not(.plain) { padding: 10px; }
.event-join { padding: 0; text-align: left; }
"#;

#[cfg(test)]
mod tests {
    use super::service;

    #[test]
    fn meeting_services_by_host() {
        assert_eq!(
            service("https://teams.microsoft.com/l/meetup-join/19%3a"),
            "Teams"
        );
        assert_eq!(
            service("https://us06web.zoom.us/w/89434802236?tk=x"),
            "Zoom"
        );
        assert_eq!(
            service("https://meet.google.com/dit-kich-bgz"),
            "Google Meet"
        );
        assert_eq!(service("https://www.example.org/room"), "example.org");
    }
}
