//! Keys that work anywhere in the window, outside text fields.

use rinch::prelude::*;

use crate::state::{State, View};
use crate::ui::list::row_ids;

/// Listen for the window's keys: j and k (or the arrows) move through the
/// list, Escape steps back out of a search, Settings or an open thread.
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
            _ => false,
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
