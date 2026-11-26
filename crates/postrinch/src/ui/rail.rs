//! The modes rail, after the Social mockup (design/mockups/ofscp-social.html):
//! a narrow strip at the far left of the whole window with each mode as an
//! icon and its name under it, a badge of what is unread, and Settings at
//! its foot. It replaces the Mail | Calendar switch that stood atop the
//! sidebar. Social joins it with its window (docs/ofscp.md, PR 15).

use std::collections::HashMap;

use postrinch_commands::ViewTally;
use rinch::prelude::*;

use crate::state::{State, View};

/// How wide the rail is, in px: the panes beside it start here.
pub const WIDTH: f32 = 68.0;

/// The built-in view whose unread threads Mail's badge counts.
const INBOX: &str = "inbox";

/// A mode's name, under its icon.
fn mode_name(mode: View) -> &'static str {
    match mode {
        View::Mail => "Mail",
        View::Calendar => "Calendar",
        View::Settings => "Settings",
    }
}

/// What a mode's badge says: nothing when nothing is unread, and at most
/// "99+", so it stays inside the rail.
fn badge_text(unread: usize) -> Option<String> {
    match unread {
        0 => None,
        1..=99 => Some(unread.to_string()),
        _ => Some("99+".to_owned()),
    }
}

/// The unread threads a mode has, from the views' counts: Mail's are the
/// inbox's (none until the inbox is counted). Calendar has none to
/// count, and Settings is not a mode with mail.
fn unread(counts: &HashMap<String, ViewTally>, mode: View) -> usize {
    match mode {
        View::Mail => counts.get(INBOX).map_or(0, |t| t.unread),
        View::Calendar | View::Settings => 0,
    }
}

/// The button's name for a screen reader: the mode, and its unread count.
fn spoken(mode: View, unread: usize) -> String {
    match unread {
        0 => mode_name(mode).to_owned(),
        n => format!("{}, {} unread", mode_name(mode), n),
    }
}

#[component]
pub fn rail(state: State) -> NodeHandle {
    rsx! {
        nav { class: "rail", aria-label: "Modes", style: format!("width: {WIDTH}px"),
            {mode_button(__scope, state, View::Mail)}
            {mode_button(__scope, state, View::Calendar)}
            div { class: "rail-gap" }
            {mode_button(__scope, state, View::Settings)}
        }
    }
}

#[component]
fn mode_button(state: State, mode: View) -> NodeHandle {
    let count = Memo::new(move || state.view_counts.with(|c| unread(c, mode)));
    rsx! {
        button {
            class: {move || if state.view.get() == mode { "rail-mode on" } else { "rail-mode" }},
            title: mode_name(mode),
            aria-label: {move || spoken(mode, count.get())},
            aria-current: {move || if state.view.get() == mode { "page" } else { "false" }},
            onclick: move || state.view.set(mode),
            {mode_icon(__scope, mode)}
            span { class: "rail-name", {mode_name(mode)} }
            span {
                class: "rail-badge",
                style: {move || if count.get() > 0 { "" } else { "display: none" }},
                {move || badge_text(count.get()).unwrap_or_default()}
            }
        }
    }
}

/// A mode's icon: a 22 px stroke drawing in the text colour, as the
/// mockup draws it.
#[component]
fn mode_icon(mode: View) -> NodeHandle {
    let (a, b) = match mode {
        View::Mail => (ENVELOPE, "M3 7l9 6 9-6"),
        View::Calendar => (CALENDAR, "M16 3v4M8 3v4M4 11h16"),
        View::Settings => (GEAR_HUB, GEAR),
    };
    // The paths are written into the svg itself: a list of nodes embedded
    // in an svg arrives wrapped in a div, which the svg renderer does not
    // draw (as in icons.rs).
    rsx! {
        svg {
            xmlns: "http://www.w3.org/2000/svg",
            viewBox: "0 0 24 24",
            width: "22",
            height: "22",
            fill: "none",
            stroke: "currentColor",
            stroke-width: "1.8",
            stroke-linecap: "round",
            stroke-linejoin: "round",
            aria-hidden: "true",
            path { d: {a} }
            path { d: {b} }
        }
    }
}

const ENVELOPE: &str = "M5 5h14a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z";
const CALENDAR: &str = "M6 5h12a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z";
const GEAR_HUB: &str = "M9 12a3 3 0 1 0 6 0a3 3 0 1 0-6 0";
const GEAR: &str = "M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z";

pub const CSS: &str = r#"
.rail {
    flex-shrink: 0; box-sizing: border-box;
    display: flex; flex-direction: column; align-items: center; gap: 4px;
    padding: 10px 0; background: var(--rail); border-right: 1px solid var(--line);
}
.rail-gap { flex: 1; }
.rail-mode {
    width: 56px; padding: 6px 0 5px; border: 0; border-radius: 8px; position: relative;
    display: flex; flex-direction: column; align-items: center; gap: 2px;
    background: transparent; color: var(--ink-2); font: inherit; font-size: 11px; cursor: pointer;
}
.rail-mode:hover { background: var(--hover); }
.rail-mode:focus-visible { outline: 2px solid var(--accent); outline-offset: -2px; }
.rail-mode.on {
    background: var(--surface); color: var(--accent-ink); font-weight: 600;
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.08);
}
.rail-badge {
    position: absolute; top: 2px; right: 6px;
    padding: 0 6px; border-radius: 9px; background: var(--accent); color: var(--surface);
    font-size: 11px; font-weight: 600; line-height: 17px;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn tally(key: &str, all: usize, unread: usize) -> (String, ViewTally) {
        let key = key.to_owned();
        (key.clone(), ViewTally { key, all, unread })
    }

    #[test]
    fn each_mode_is_named_under_its_icon() {
        assert_eq!(mode_name(View::Mail), "Mail");
        assert_eq!(mode_name(View::Calendar), "Calendar");
        assert_eq!(mode_name(View::Settings), "Settings");
    }

    #[test]
    fn mail_counts_the_inboxs_unread_threads_and_nothing_else() {
        let counts: HashMap<_, _> =
            [tally("inbox", 40, 7), tally("everything-else", 90, 30)].into();
        assert_eq!(unread(&counts, View::Mail), 7);
        assert_eq!(unread(&counts, View::Calendar), 0);
        assert_eq!(unread(&counts, View::Settings), 0);
    }

    #[test]
    fn mail_has_no_badge_until_the_inbox_is_counted() {
        let counts: HashMap<_, _> = [tally("sent", 12, 3)].into();
        assert_eq!(unread(&counts, View::Mail), 0);
        assert_eq!(unread(&HashMap::new(), View::Mail), 0);
    }

    #[test]
    fn a_badge_shows_only_what_is_unread_and_stays_short() {
        assert_eq!(badge_text(0), None);
        assert_eq!(badge_text(1).as_deref(), Some("1"));
        assert_eq!(badge_text(99).as_deref(), Some("99"));
        assert_eq!(badge_text(100).as_deref(), Some("99+"));
        assert_eq!(badge_text(12_345).as_deref(), Some("99+"));
    }

    #[test]
    fn a_screen_reader_hears_the_mode_and_its_unread_count() {
        assert_eq!(spoken(View::Calendar, 0), "Calendar");
        assert_eq!(spoken(View::Mail, 12), "Mail, 12 unread");
        assert_eq!(spoken(View::Mail, 250), "Mail, 250 unread");
    }
}
