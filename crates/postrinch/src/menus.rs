//! The menu bar, drawn in the title bar. Every item is something the
//! window can already do; items arrive with the features behind them.

use postrinch_commands::Folder;
use postrinch_model::SystemLabel;
use rinch::menu::{Menu, MenuItem};
use rinch::prelude::*;

use crate::state::{State, View};

pub fn menus(state: State) -> Vec<(&'static str, Menu)> {
    let file = Menu::new()
        .item(
            MenuItem::new("Settings…")
                .shortcut("Ctrl+,")
                .on_click(move || state.view.set(View::Settings)),
        )
        .separator()
        .item(
            MenuItem::new("Quit")
                .shortcut("Ctrl+Q")
                .on_click(close_current_window),
        );
    let account = Menu::new()
        .item(MenuItem::new("Add Google account…").on_click(move || state.sign_in(None)))
        .item(
            MenuItem::new("Sync now")
                .shortcut("F5")
                .on_click(move || state.sync_now()),
        )
        .separator()
        .item(MenuItem::new("Manage accounts…").on_click(move || state.view.set(View::Settings)));
    let go = |folder: Folder| {
        move || {
            state.view.set(View::Mail);
            state.open_folder(folder.clone());
        }
    };
    let view = Menu::new()
        .item(
            MenuItem::new("All inboxes")
                .shortcut("Ctrl+1")
                .on_click(go(Folder::Everywhere(SystemLabel::Inbox))),
        )
        .item(
            MenuItem::new("Sent")
                .shortcut("Ctrl+2")
                .on_click(go(Folder::Everywhere(SystemLabel::Sent))),
        )
        .item(
            MenuItem::new("Drafts")
                .shortcut("Ctrl+3")
                .on_click(go(Folder::Everywhere(SystemLabel::Drafts))),
        )
        .separator()
        .item(
            MenuItem::new("Show all labels")
                .on_click(move || state.show_all.update(|on| *on = !*on)),
        );
    vec![("File", file), ("View", view), ("Account", account)]
}
