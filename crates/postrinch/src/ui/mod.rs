//! The window: a header over the sidebar, the thread list and the reader.

mod label_edit;
mod labels;
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

pub use label_edit::LabelEdit;

pub fn account_color(index: usize) -> &'static str {
    ACCOUNT_COLORS[index % ACCOUNT_COLORS.len()]
}

/// The whole window, over one store.
pub fn app(__scope: &mut RenderScope, state: State, store: Store) -> NodeHandle {
    state.load(Rc::new(Commands::new(store.clone())));
    if let Err(why) = sync::start(store, state.sync, state.revision) {
        tracing::error!("sync is off: {why}");
        state.sync_off.set(Some(format!("Sync is off: {why}")));
    }
    let brand: SectionRenderer = Rc::new(|__scope| rsx! { div { class: "brand", "postrinch" } });
    let status: SectionRenderer = Rc::new(move |__scope| title_status(__scope, state));
    rsx! {
        BorderlessWindow {
            title: "",
            radius: "none",
            left_section: Some(brand),
            right_section: Some(status),
            on_minimize: minimize_current_window,
            on_maximize: toggle_maximize_current_window,
            on_close: close_current_window,
            div { class: "app",
                style { {style::CSS} }
                {label_edit::label_dialog(__scope, state)}
                div { class: "panes",
                    {sidebar::sidebar(__scope, state)}
                    {list::thread_list(__scope, state)}
                    {reader::reader(__scope, state)}
                }
            }
        }
    }
}

/// The right of the title bar: anything that went wrong loading.
#[component]
fn title_status(state: State) -> NodeHandle {
    rsx! {
        div { class: "title-status",
            for error in state.error.get() {
                div { key: error.clone(), class: "header-error", {error.clone()} }
            }
        }
    }
}
