//! The window: a header over the sidebar, the thread list and the reader.

mod keys;
mod label_edit;
mod labels;
mod list;
mod logo;
mod reader;
mod settings;
mod sidebar;
mod style;

use std::rc::Rc;

use postrinch_commands::Commands;
use postrinch_store::Store;
use rinch::prelude::*;

use crate::actions;
use crate::state::{State, View};
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
    let commands = Rc::new(Commands::new(store.clone()));
    let sync = match sync::start(store, state.sync, state.revision) {
        Ok(handle) => Some(handle),
        Err(why) => {
            tracing::error!("sync is off: {why}");
            state.sync_off.set(Some(format!("Sync is off: {why}")));
            None
        }
    };
    actions::init(commands.clone(), sync);
    state.load(commands);
    let brand: SectionRenderer = Rc::new(|__scope| {
        rsx! {
            div { class: "brand",
                {logo::logo(__scope)}
                span { "postrinch" }
            }
        }
    });
    let status: SectionRenderer = Rc::new(move |__scope| title_status(__scope, state));
    let window = rsx! {
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
                if state.view.get() == View::Mail {
                    div { class: "panes",
                        {sidebar::sidebar(__scope, state)}
                        {list::thread_list(__scope, state)}
                        {reader::reader(__scope, state)}
                    }
                } else {
                    {settings::settings(__scope, state)}
                }
            }
        }
    };
    keys::listen(window.clone(), state);
    window
}

/// The right of the title bar: search, and anything that went wrong.
#[component]
fn title_status(state: State) -> NodeHandle {
    rsx! {
        div { class: "title-status",
            div { class: "title-search",
                TextInput {
                    size: "xs",
                    placeholder: "Search all mail",
                    value_fn: move || state.search_draft.get(),
                    oninput: move |v: String| state.search_draft.set(v),
                    onsubmit: move || state.run_search(),
                }
            }
            for error in state.error.get() {
                div { key: error.clone(), class: "header-error", {error.clone()} }
            }
        }
    }
}
