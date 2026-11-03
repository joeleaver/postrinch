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
}

#[component]
pub fn icon(which: Icon) -> NodeHandle {
    match which {
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
