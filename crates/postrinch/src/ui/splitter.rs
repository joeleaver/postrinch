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
            // The sidebar starts at the window's left edge; the list starts
            // where the sidebar ends.
            Pane::Sidebar => state.sidebar_width.set(x.clamp(SIDEBAR.0, SIDEBAR.1)),
            Pane::List => state
                .list_width
                .set((x - state.sidebar_width.get()).clamp(LIST.0, LIST.1)),
        })
        .on_end(move |_, _| state.save_pane_widths())
        .start();
}
