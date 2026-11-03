//! The window: a header over the sidebar, the thread list and the reader.

mod list;
mod reader;
mod sidebar;
mod style;

use std::rc::Rc;

use postrinch_commands::Commands;
use postrinch_store::Store;
use rinch::prelude::*;

use crate::state::State;
use crate::sync;

/// Colours that tell accounts apart, in the order accounts were added. They
/// stay clear of the blue that marks what is chosen.
const ACCOUNT_COLORS: [&str; 5] = ["#7a4fc2", "#c2621a", "#2f8a55", "#c2417a", "#5b6170"];

pub fn account_color(index: usize) -> &'static str {
    ACCOUNT_COLORS[index % ACCOUNT_COLORS.len()]
}

/// The whole window, over one store.
pub fn app(__scope: &mut RenderScope, store: Store) -> NodeHandle {
    let state = State::new();
    state.load(Rc::new(Commands::new(store.clone())));
    if let Err(why) = sync::start(store, state.sync, state.revision) {
        tracing::error!("sync is off: {why}");
        state.sync_off.set(Some(format!("Sync is off: {why}")));
    }
    rsx! {
        div { class: "app",
            style { {style::CSS} }
            header { class: "header",
                div { class: "brand", "postrinch" }
                if let Some(error) = state.error.get() {
                    div { class: "header-error", {error.clone()} }
                }
            }
            div { class: "panes",
                {sidebar::sidebar(__scope, state)}
                {list::thread_list(__scope, state)}
                {reader::reader(__scope, state)}
            }
        }
    }
}
