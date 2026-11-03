//! Keys that work anywhere in the window, outside text fields.

use rinch::prelude::*;

use crate::state::{State, View};
use crate::ui::list::row_ids;
use crate::ui::thread_actions;
use postrinch_commands::ThreadAction;

/// Listen for the window's keys: j and k (or the arrows) move through the
/// list, Escape steps back out of a search, Settings or an open thread, and
/// Gmail's keys act on the open thread (see [`act`]).
pub fn listen(root: NodeHandle, state: State) {
    on_app_key(move |key: &KeyEventData| {
        let in_field = root
            .active_element()
            .and_then(|el| el.tag_name())
            .is_some_and(|t| t == "input" || t == "textarea");
        if !key.is_down() || in_field || key.ctrl || key.alt || key.meta {
            return false;
        }
        match key.key.as_str() {
            "j" | "ArrowDown" => step(state, 1),
            "k" | "ArrowUp" => step(state, -1),
            "Escape" => back(state),
            key => act(state, key),
        }
    });
}

/// Open the next (`by` = 1) or previous (-1) thread in the list.
fn step(state: State, by: isize) -> bool {
    if state.view.get() != View::Mail {
        return false;
    }
    let rows = row_ids(state);
    if rows.is_empty() {
        return false;
    }
    let current = state
        .selected
        .get()
        .and_then(|chosen| rows.iter().position(|r| *r == chosen));
    let next = match current {
        Some(i) => (i as isize + by).clamp(0, rows.len() as isize - 1) as usize,
        None => 0,
    };
    state.selected.set(Some(rows[next].clone()));
    true
}

/// Step back: out of Settings, out of a search, or close the open thread.
fn back(state: State) -> bool {
    if state.view.get() == View::Settings {
        state.view.set(View::Mail);
    } else if state.search.get().is_some() {
        state.close_search();
    } else if state.selected.get().is_some() {
        state.selected.set(None);
    } else {
        return false;
    }
    true
}

/// Gmail's keys for the open thread: e archives (or moves back to the
/// inbox), # trashes (or restores, in the trash), Shift+U marks it unread
/// and Shift+I read, s stars or unstars, and l opens the label menu.
fn act(state: State, key: &str) -> bool {
    if state.view.get() != View::Mail || state.selected.get().is_none() {
        return false;
    }
    let action = match key {
        "e" => thread_actions::archive_or_restore(state),
        "#" => thread_actions::trash_or_restore(state),
        "U" => ThreadAction::MarkUnread,
        "I" => ThreadAction::MarkRead,
        "s" => thread_actions::star_or_unstar(state),
        "l" => {
            state.labelling.set(true);
            return true;
        }
        _ => return false,
    };
    state.act_on_open(action);
    true
}
