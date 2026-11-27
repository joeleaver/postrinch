//! Desktop notices for new social messages (docs/ofscp.md, "Organizing:
//! views, not providers"): those their places' notices let through
//! (`postrinch_commands::Commands::social_notices`), told as new mail is (notify.rs),
//! under the same Settings › Notifications: what a notice shows, several
//! at once, quiet hours and not while the window is in front. A click
//! opens the view the message shows in, on its place's tab.

use postrinch_commands::SocialArrived;
use rinch::notification::Notification;
use rinch::prelude::*;

use crate::notices::{NoticePrefs, Shows};
use crate::notify::{show, told};
use crate::state::{State, View};

/// Those of `found` not told yet, each told once.
pub fn fresh(found: Vec<SocialArrived>) -> Vec<SocialArrived> {
    found
        .into_iter()
        .filter(|a| {
            !told(format!(
                "s|{}|{}|{}",
                a.place.account.to_lowercase(),
                a.place.id,
                a.message
            ))
        })
        .collect()
}

/// Where a message is, as a notice says it: a channel with its group, or
/// private messages.
pub fn where_from(a: &SocialArrived) -> String {
    if a.conversation {
        return "private messages".to_owned();
    }
    match &a.group_name {
        Some(group) => format!("{group} \u{203a} {}", a.place_name),
        None => a.place_name.clone(),
    }
}

/// A notice's title and body for one message, as the person's settings
/// say what a notice shows.
pub fn one(a: &SocialArrived, shows: Shows) -> (String, String) {
    let who = if a.conversation {
        a.author.clone()
    } else {
        format!("{} in {}", a.author, where_from(a))
    };
    match shows {
        Shows::Everything => {
            let body = match &a.title {
                Some(title) if !a.snippet.is_empty() => format!("{title}\n{}", a.snippet),
                Some(title) => title.clone(),
                None => a.snippet.clone(),
            };
            (who, body)
        }
        // Only an article or a memo has a subject; for a chat message the
        // title's who and where are the whole notice, its words kept back.
        Shows::WhoAndSubject => (who, a.title.clone().unwrap_or_default()),
        Shows::Nothing if a.conversation => ("New private message".to_owned(), String::new()),
        Shows::Nothing => (format!("New message in {}", where_from(a)), String::new()),
    }
}

/// One notice for several messages: how many and where, and who wrote
/// them unless notices show nothing.
pub fn all(found: &[SocialArrived], shows: Shows) -> (String, String) {
    let mut places: Vec<String> = Vec::new();
    let mut who: Vec<&str> = Vec::new();
    for a in found {
        let place = where_from(a);
        if !places.contains(&place) {
            places.push(place);
        }
        if !who.contains(&a.author.as_str()) {
            who.push(&a.author);
        }
    }
    let title = format!("{} new messages in {}", found.len(), places.join(", "));
    let body = match shows {
        Shows::Nothing => String::new(),
        _ => who.join(", "),
    };
    (title, body)
}

impl State {
    /// Tell of new social messages, as new mail is told.
    pub(crate) fn tell_social(self, prefs: &NoticePrefs, found: Vec<SocialArrived>) {
        if prefs.several.together(found.len()) {
            let (title, body) = all(&found, prefs.shows);
            let state = self;
            show(Notification::new(title).body(body).on_click(move || {
                show_current_window();
                state.view.set(View::Social);
            }));
            return;
        }
        for a in found {
            let (title, body) = one(&a, prefs.shows);
            let state = self;
            show(Notification::new(title).body(body).on_click(move || {
                show_current_window();
                state.view.set(View::Social);
                state.open_social_view(&a.view);
                state.choose_social_tab(Some(a.place.clone()));
            }));
        }
    }
}

#[cfg(test)]
#[path = "social_notify_tests.rs"]
mod tests;
