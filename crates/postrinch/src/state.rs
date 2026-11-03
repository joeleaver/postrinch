//! What the window shows, as signals, and loading it through the commands.

use std::collections::HashSet;
use std::rc::Rc;

use postrinch_commands::{
    AccountInfo, Commands, Folder, SearchResults, Sidebar, Thread, ThreadPage,
};
use postrinch_model::{EmailAddress, ProviderThreadId, SystemLabel};
use rinch::prelude::*;
use rinch::{Effect, batch};

use crate::actions::Notice;
use crate::appearance::{Appearance, Settings, is_dark, system_prefers_dark};
use crate::sync::AccountSync;
use crate::ui::LabelEdit;

/// How many threads a search shows.
const SEARCH_LIMIT: usize = 100;

/// How many more threads "Show more" loads.
pub const PAGE: usize = 100;

/// The window's pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Mail,
    Settings,
}

/// The parts of Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    Accounts,
    Appearance,
}

/// The window's state. Every field is a signal, so this is `Copy`.
#[derive(Clone, Copy)]
pub struct State {
    pub sidebar: Signal<Option<Sidebar>>,
    pub folder: Signal<Folder>,
    /// Within an inbox, one of Gmail's categories, as its tabs show them.
    pub category: Signal<Option<SystemLabel>>,
    /// How many of the folder's threads to show.
    pub shown: Signal<usize>,
    pub page: Signal<Option<ThreadPage>>,
    /// What the search box holds.
    pub search_draft: Signal<String>,
    /// The search being shown in place of the folder, if any.
    pub search: Signal<Option<String>>,
    pub results: Signal<Option<SearchResults>>,
    /// Whether the sidebar shows every label, hidden ones included.
    pub show_all: Signal<bool>,
    /// Sidebar rows the person opened or closed (see `ui::labels`).
    pub toggled: Signal<HashSet<String>>,
    pub selected: Signal<Option<(EmailAddress, ProviderThreadId)>>,
    pub thread: Signal<Option<Thread>>,
    pub sync: Signal<Vec<AccountSync>>,
    /// Bumped whenever sync changed the store, so what is shown reloads.
    pub revision: Signal<u64>,
    /// The label being renamed or coloured.
    pub editing: Signal<Option<LabelEdit>>,
    /// Light, dark or the system's choice, as saved.
    pub appearance: Signal<Appearance>,
    /// Whether the system prefers dark (dark when it does not say).
    pub system_dark: Signal<bool>,
    /// Which part of Settings is open.
    pub settings_section: Signal<SettingsSection>,
    /// Which page the window shows.
    pub view: Signal<View>,
    /// Settings' accounts, loaded while Settings is open.
    pub accounts: Signal<Vec<AccountInfo>>,
    /// The account Settings shows.
    pub account_shown: Signal<Option<EmailAddress>>,
    /// The account the person asked to remove, until they confirm.
    pub removing: Signal<Option<EmailAddress>>,
    /// What an account action wants the person to know.
    pub notice: Signal<Option<Notice>>,
    /// Why sync is not running, when it is not.
    pub sync_off: Signal<Option<String>>,
    /// The last thing that went wrong loading, for the window to say.
    pub error: Signal<Option<String>>,
}

impl State {
    pub fn new() -> Self {
        Self {
            sidebar: Signal::new(None),
            folder: Signal::new(Folder::Everywhere(SystemLabel::Inbox)),
            category: Signal::new(None),
            shown: Signal::new(PAGE),
            page: Signal::new(None),
            search_draft: Signal::new(String::new()),
            search: Signal::new(None),
            results: Signal::new(None),
            show_all: Signal::new(false),
            toggled: Signal::new(HashSet::new()),
            selected: Signal::new(None),
            thread: Signal::new(None),
            sync: Signal::new(Vec::new()),
            revision: Signal::new(0),
            editing: Signal::new(None),
            appearance: Signal::new(Settings::load().appearance),
            system_dark: Signal::new(system_prefers_dark()),
            settings_section: Signal::new(SettingsSection::Accounts),
            view: Signal::new(View::Mail),
            accounts: Signal::new(Vec::new()),
            account_shown: Signal::new(None),
            removing: Signal::new(None),
            notice: Signal::new(None),
            sync_off: Signal::new(None),
            error: Signal::new(None),
        }
    }

    /// Keep what is shown loaded: the sidebar and the folder's threads
    /// whenever sync changes the store or another folder is chosen, and
    /// the open thread whenever it changes or sync does.
    pub fn load(self, commands: Rc<Commands>) {
        let lists = commands.clone();
        Effect::new(move || {
            self.revision.get();
            let folder = self.folder.get();
            let category = self.category.get();
            let shown = self.shown.get();
            let loaded = lists.sidebar().and_then(|sidebar| {
                Ok((sidebar, lists.list_threads(&folder, category, 0, shown)?))
            });
            match loaded {
                Ok((sidebar, page)) => {
                    self.sidebar.set(Some(sidebar));
                    self.page.set(Some(page));
                }
                Err(e) => self.error.set(Some(e.to_string())),
            }
        });
        Effect::new(move || {
            self.revision.get();
            if self.view.get() == View::Settings {
                match crate::actions::accounts() {
                    Ok(accounts) => self.accounts.set(accounts),
                    Err(e) => self.error.set(Some(e)),
                }
            }
        });
        let searching = commands.clone();
        Effect::new(move || {
            self.revision.get();
            let results = match self.search.get() {
                Some(query) => match searching.search(&query, SEARCH_LIMIT) {
                    Ok(results) => Some(results),
                    Err(e) => {
                        self.error.set(Some(e.to_string()));
                        None
                    }
                },
                None => None,
            };
            self.results.set(results);
        });
        Effect::new(move || {
            self.revision.get();
            let thread = match self.selected.get() {
                Some((account, thread)) => match commands.read_thread(&account, &thread) {
                    Ok(thread) => thread,
                    Err(e) => {
                        self.error.set(Some(e.to_string()));
                        None
                    }
                },
                None => None,
            };
            self.thread.set(thread);
        });
    }

    /// Show the threads that match what the search box holds.
    pub fn run_search(self) {
        let query = self.search_draft.get();
        batch(|| {
            self.selected.set(None);
            self.view.set(View::Mail);
            self.search.set((!query.trim().is_empty()).then_some(query));
        });
    }

    /// Go back from search results to the folder.
    pub fn close_search(self) {
        batch(|| {
            self.search.set(None);
            self.search_draft.set(String::new());
        });
    }

    /// Whether the window is dark now.
    pub fn dark(self) -> bool {
        is_dark(self.appearance.get(), self.system_dark.get())
    }

    /// Choose light, dark or the system's, and save the choice.
    pub fn set_appearance(self, appearance: Appearance) {
        self.appearance.set(appearance);
        let mut settings = Settings::load();
        settings.appearance = appearance;
        if let Err(e) = settings.save() {
            self.error
                .set(Some(format!("The appearance was not saved: {e}")));
        }
    }

    pub fn open_folder(self, folder: Folder) {
        batch(|| {
            self.search.set(None);
            self.shown.set(PAGE);
            self.selected.set(None);
            self.category.set(None);
            self.folder.set(folder);
        });
    }

    pub fn open_category(self, category: Option<SystemLabel>) {
        batch(|| {
            self.shown.set(PAGE);
            self.category.set(category);
        });
    }
}
