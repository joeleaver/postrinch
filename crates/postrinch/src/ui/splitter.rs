//! The handles between the panes, which drag to resize them.

use rinch::prelude::*;

use crate::state::State;

/// The narrowest and widest each pane can be dragged to, in px.
const SIDEBAR: (f32, f32) = (180.0, 420.0);
const LIST: (f32, f32) = (300.0, 760.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Sidebar,
    List,
}

/// A handle on the right edge of `pane`.
#[component]
pub fn splitter(state: State, pane: Pane) -> NodeHandle {
    rsx! {
        div {
            class: "splitter",
            title: "Drag to resize",
            onmousedown: move || start(state, pane),
        }
    }
}

fn start(state: State, pane: Pane) {
    Drag::absolute()
        .on_move(move |x, _| match pane {
            Pane::Sidebar => state.sidebar_width.set(dragged_to(pane, x, 0.0)),
            Pane::List => state
                .list_width
                .set(dragged_to(pane, x, state.sidebar_width.get())),
        })
        .on_end(move |_, _| state.save_pane_widths())
        .start();
}

/// The width `pane` takes when its handle is dragged to `x`, the window's
/// own x: the sidebar starts where the modes rail ends, and the list where
/// the sidebar (`sidebar` px wide) ends.
fn dragged_to(pane: Pane, x: f32, sidebar: f32) -> f32 {
    let from_rail = x - crate::ui::rail::WIDTH;
    match pane {
        Pane::Sidebar => from_rail.clamp(SIDEBAR.0, SIDEBAR.1),
        Pane::List => (from_rail - sidebar).clamp(LIST.0, LIST.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::rail::WIDTH as RAIL;

    #[test]
    fn the_sidebar_is_measured_from_the_rail_not_the_window_edge() {
        assert_eq!(dragged_to(Pane::Sidebar, RAIL + 250.0, 0.0), 250.0);
        assert_eq!(dragged_to(Pane::Sidebar, RAIL + 10.0, 0.0), SIDEBAR.0);
        assert_eq!(dragged_to(Pane::Sidebar, RAIL + 900.0, 0.0), SIDEBAR.1);
    }

    #[test]
    fn the_list_is_measured_from_where_the_sidebar_ends() {
        assert_eq!(dragged_to(Pane::List, RAIL + 240.0 + 400.0, 240.0), 400.0);
        assert_eq!(dragged_to(Pane::List, RAIL + 240.0 + 50.0, 240.0), LIST.0);
        assert_eq!(dragged_to(Pane::List, RAIL + 240.0 + 2000.0, 240.0), LIST.1);
    }
}
