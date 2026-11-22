//! The window: a header over the sidebar, the thread list and the reader.

mod about;
mod agent_dialogs;
mod agents_settings;
mod attachment_cards;
mod chooser;
mod compose;
mod compose_files;
mod icons;
mod keys;
mod label_edit;
mod label_new;
mod label_picker;
mod labels;
pub(crate) mod list;
mod list_head;
mod logo;
mod outgoing;
mod palette;
mod people_picker;
mod reader;
mod reading_settings;
mod rule_editor;
mod rule_editor_notes;
mod rule_from_message;
mod rule_pickers;
mod rule_run_dialog;
mod rules_page;
mod search;
mod settings;
mod sidebar;
mod splitter;
mod style;
pub(crate) mod thread_actions;
mod view_head;
mod view_panel;
pub(crate) mod view_panel_mail;

use std::rc::Rc;

use postrinch_commands::Commands;
use postrinch_store::Store;
use rinch::prelude::*;

use crate::state::{State, View};
use crate::sync;
use crate::{actions, agents, appearance, changes, update};

/// Colours that tell accounts apart, in the order accounts were added. They
/// stay clear of the blue that marks what is chosen.
const ACCOUNT_COLORS: [&str; 5] = ["#7a4fc2", "#c2621a", "#2f8a55", "#c2417a", "#5b6170"];

pub use label_edit::LabelEdit;
pub use label_new::NewLabel;

pub fn account_color(index: usize) -> &'static str {
    ACCOUNT_COLORS[index % ACCOUNT_COLORS.len()]
}

/// The whole window, over one store.
pub fn app(__scope: &mut RenderScope, state: State, store: Store) -> NodeHandle {
    let commands = Rc::new(Commands::new(store.clone()));
    let sync = match sync::start(store, state.sync, state.revision) {
        Ok(handle) => Some(handle),
        Err(off) => {
            tracing::error!("{} ({})", off.said, off.detail);
            state.sync_off.set(Some(off.said.to_owned()));
            None
        }
    };
    changes::start(
        (*commands).clone(),
        state,
        sync.as_ref().map(|s| s.send.clone()),
    );
    agents::start((*commands).clone(), state, sync.as_ref().cloned());
    if let Some(sync) = &sync {
        crate::rules_ui::set_sender(sync.send.clone());
    }
    crate::rules::start(
        (*commands).clone(),
        sync.as_ref().map(|s| s.send.clone()),
        state.revision,
    );
    actions::init(commands.clone(), sync);
    update::start(state);
    state.mark_read_when_opened();
    state.save_edits_as_they_come();
    state.open_by_setting();
    crate::pictures::fetch_web_pictures();
    appearance::watch_system(state.system_dark);
    state.load(&commands);
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
        ThemeProvider {
            dark_mode_fn: Rc::new(move || state.dark()),
        BorderlessWindow {
            title: "",
            radius: "md",
            left_section: Some(brand),
            right_section: Some(status),
            on_minimize: minimize_current_window,
            on_maximize: toggle_maximize_current_window,
            on_close: close_current_window,
            div { class: "app",
                style { {palette::LIGHT} }
                if state.dark() {
                    style { {palette::DARK} }
                }
                style { {style::CSS} }
                style { {postrinch_render::HOST_CSS} }
                style { {agents_settings::CSS} }
                style { {compose::CSS} }
                style { {search::CSS} }
                style { {outgoing::CSS} }
                style { {about::CSS} }
                style { {thread_actions::CSS} }
                style { {rules_page::CSS} {rule_editor::CSS} {rule_from_message::CSS} {rule_run_dialog::CSS} }
                style { {list_head::CSS} {attachment_cards::CSS} {compose_files::CSS} {label_new::CSS} }
                style { {view_head::CSS} {view_panel::CSS} {chooser::CSS} }
                {label_edit::label_dialog(__scope, state)}
                {label_new::new_label_dialog(__scope, state)}
                {compose::compose_dialog(__scope, state)}
                if state.view.get() == View::Mail {
                    div { class: "panes",
                        {sidebar::sidebar(__scope, state)}
                        {splitter::splitter(__scope, state, splitter::Pane::Sidebar)}
                        {list::thread_list(__scope, state)}
                        {splitter::splitter(__scope, state, splitter::Pane::List)}
                        // The view panel stands where the reader does, so
                        // the list beside it shows the view as changed.
                        if state.view_edit.get().is_some() {
                            {view_panel::view_panel(__scope, state)}
                        } else {
                            {reader::reader(__scope, state)}
                        }
                    }
                } else {
                    {settings::settings(__scope, state)}
                }
            }
        }
        }
    };
    keys::listen(window.clone(), state);
    window
}

/// The right of the title bar: Write (a pencil, after the title bar mockup),
/// search, and anything that went wrong.
#[component]
fn title_status(state: State) -> NodeHandle {
    rsx! {
        div { class: "title-status",
            {about::update_pill(__scope, state)}
            button {
                class: "title-write",
                title: "Write (C)",
                aria-label: "Write",
                onclick: move || state.write(postrinch_commands::Start::New),
                {icons::icon(__scope, icons::Icon::Write)}
            }
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
