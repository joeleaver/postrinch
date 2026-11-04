//! Links in mail: a click on one opens it in the system's browser (or mail
//! app, for `mailto:`). Only `http`, `https` and `mailto` links reach here:
//! the cleaner drops every other scheme's `href` (rinch_html's
//! `Policy::link_schemes`), and this checks again before handing the link
//! to the system, which would run whatever program a custom scheme names.

/// Open `href` if it is a web or mail link.
pub fn open(href: &str) {
    let lower = href.trim().to_ascii_lowercase();
    if !(lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:"))
    {
        tracing::info!("not opening a link that is not http, https or mailto");
        return;
    }
    if let Err(e) = rinch::prelude::open_url(href.trim()) {
        tracing::warn!("opening a link failed: {e:?}");
    }
}
