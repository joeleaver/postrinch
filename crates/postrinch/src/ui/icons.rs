//! The reader's action icons: 16 px stroke drawings in the text colour,
//! as the mockups draw them.

use rinch::prelude::*;

/// Which icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Archive,
    Inbox,
    Trash,
    Restore,
    Unread,
    /// An opened envelope: mark read.
    Read,
    Label,
    Star,
    Starred,
    /// A pencil: write new mail.
    Write,
    Reply,
    ReplyAll,
    Forward,
    Paperclip,
    /// A down arrow onto a line: save a file.
    Save,
    Picture,
    Warning,
    Close,
    /// A page with a folded corner: a file.
    File,
    /// Three dots: more actions.
    More,
}

#[component]
pub fn icon(which: Icon) -> NodeHandle {
    match which {
        // Three small filled circles, drawn as closed arcs.
        Icon::More => one(
            __scope,
            "M4 12a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0M10.5 12a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0M17 12a1.5 1.5 0 1 0 3 0a1.5 1.5 0 1 0-3 0",
            "currentColor",
        ),
        Icon::Archive => two(__scope, "M3 4h18v5H3z", "M5 9v10h14V9M10 13h4", "none"),
        Icon::Inbox => two(
            __scope,
            "M3 13h5l2 3h4l2-3h5",
            "M5 5h14l2 8v6H3v-6z",
            "none",
        ),
        Icon::Trash => one(
            __scope,
            "M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3",
            "none",
        ),
        Icon::Restore => two(__scope, "M4 12a8 8 0 1 0 3-6.2", "M4 4v5h5", "none"),
        Icon::Unread => two(__scope, "M3 5h18v14H3z", "M3 7l9 6 9-6", "none"),
        Icon::Read => two(__scope, "M3 10l9-6 9 6v10H3z", "M3 10l9 6 9-6", "none"),
        Icon::Label => two(
            __scope,
            "M3 12V4h8l10 10-8 8L3 12z",
            "M8 6.5a1.5 1.5 0 1 0 0 3a1.5 1.5 0 1 0 0-3",
            "none",
        ),
        Icon::Star => one(__scope, STAR, "none"),
        Icon::Starred => one(__scope, STAR, "currentColor"),
        Icon::Write => two(
            __scope,
            "M12 20h9",
            "M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z",
            "none",
        ),
        Icon::Reply => two(__scope, "M9 14L4 9l5-5", "M4 9h10a6 6 0 0 1 6 6v4", "none"),
        Icon::ReplyAll => two(
            __scope,
            "M7 14L2 9l5-5M12 14L7 9l5-5",
            "M7 9h8a6 6 0 0 1 6 6v4",
            "none",
        ),
        Icon::Forward => two(
            __scope,
            "M15 14l5-5-5-5",
            "M20 9H10a6 6 0 0 0-6 6v4",
            "none",
        ),
        Icon::Paperclip => one(
            __scope,
            "M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L15 7",
            "none",
        ),
        Icon::Save => two(__scope, "M12 4v11M7 10l5 5 5-5", "M5 20h14", "none"),
        Icon::Picture => two(
            __scope,
            "M3 4h18v16H3z",
            "M21 17l-5-5-9 8M9 8a2 2 0 1 0 0 4a2 2 0 1 0 0-4",
            "none",
        ),
        Icon::Warning => two(__scope, "M12 3l9 16H3z", "M12 10v4M12 17h.01", "none"),
        Icon::Close => one(__scope, "M6 6l12 12M18 6L6 18", "none"),
        Icon::File => two(__scope, "M6 3h9l4 4v14H6z", "M14 3v5h5", "none"),
    }
}

const STAR: &str = "M12 3l2.7 5.6 6.1.9-4.4 4.3 1 6.1L12 17l-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z";

// The paths are written into the svg itself: a list of nodes embedded in
// an svg arrives wrapped in a div, which the svg renderer does not draw.

#[component]
fn one(d: &'static str, fill: &'static str) -> NodeHandle {
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            viewBox: "0 0 24 24",
            width: "16",
            height: "16",
            fill: {fill},
            stroke: "currentColor",
            stroke-width: "2",
            stroke-linecap: "round",
            stroke-linejoin: "round",
            aria-hidden: "true",
            path { d: {d} }
        }
    }
}

#[component]
fn two(a: &'static str, b: &'static str, fill: &'static str) -> NodeHandle {
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            viewBox: "0 0 24 24",
            width: "16",
            height: "16",
            fill: {fill},
            stroke: "currentColor",
            stroke-width: "2",
            stroke-linecap: "round",
            stroke-linejoin: "round",
            aria-hidden: "true",
            path { d: {a} }
            path { d: {b} }
        }
    }
}
