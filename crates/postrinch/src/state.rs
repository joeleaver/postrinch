//! What the window shows, as signals, and loading it through the commands.

use std::collections::HashSet;

use postrinch_commands::{
    AccountInfo, Commands, Folder, LabelChoice, SearchResults, Sidebar, Thread, ThreadPage,
};
use postrinch_model::{EmailAddress, ProviderThreadId, SystemLabel};
use rinch::prelude::*;
use rinch::{Effect, batch};

use crate::actions::Notice;
use crate::appearance::{Appearance, Settings, is_dark, system_prefers_dark};
use crate::changes::Change;
use crate::loader::{self, Request};
use crate::reading::{AfterMove, MarkRead};
use crate::sync::AccountSync;
use crate::ui::LabelEdit;

/// The panes' widths until the person drags them.
pub const SIDEBAR_WIDTH: f32 = 240.0;
pub const LIST_WIDTH: f32 = 420.0;

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
    Reading,
    Agents,
    Appearance,
    About,
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
    /// Downloads the person asked for that have finished, to forget.
    pub downloaded: Signal<Vec<(String, String)>>,
    /// Light, dark or the system's choice, as saved.
    pub appearance: Signal<Appearance>,
    /// When an opened conversation is marked read, as saved.
    pub mark_read: Signal<MarkRead>,
    /// What opens after the open conversation is archived or trashed.
    pub after_move: Signal<AfterMove>,
    /// The labels the open conversation's account has, for the label menu.
    pub label_choices: Signal<Vec<LabelChoice>>,
    /// Whether the label menu is open.
    pub labelling: Signal<bool>,
    /// Actions shown but not yet in every list loaded (see changes.rs).
    pub pending: Signal<Vec<Change>>,
    /// Ticks when an opened conversation may be due to be marked read.
    pub read_due: Signal<u64>,
    /// Whether the system prefers dark (dark when it does not say).
    pub system_dark: Signal<bool>,
    /// The sidebar's and the thread list's widths, in px.
    pub sidebar_width: Signal<f32>,
    pub list_width: Signal<f32>,
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
    /// Why agents cannot connect, when they cannot.
    pub agents_off: Signal<Option<String>>,
    /// The agents, read while Settings › Agents is open.
    pub agent_list: Signal<Vec<postrinch_agents::Agent>>,
    /// The dialog Settings › Agents has open.
    pub agent_dialog: Signal<Option<crate::agents::AgentDialog>>,
    /// The agent the person asked to remove, until they confirm.
    pub agent_removing: Signal<Option<postrinch_agents::Agent>>,
    /// The open thread's messages as they show, by message id.
    pub rendered: Signal<std::collections::HashMap<String, postrinch_commands::Rendered>>,
    /// Messages whose pictures from the web the person asked to see.
    pub pictures_shown: Signal<HashSet<String>>,
    /// Senders whose pictures always load (saved in Settings).
    pub pictures_from: Signal<HashSet<String>>,
    /// The draft the compose dialog has open.
    pub composing: Signal<Option<crate::writing::Compose>>,
    /// Why the open draft could not be sent or kept.
    pub compose_error: Signal<Option<String>>,
    /// Mail on its way out and agents' proposals, oldest first.
    pub outgoing: Signal<Vec<postrinch_commands::Queued>>,
    /// Why sync is not running, when it is not.
    pub sync_off: Signal<Option<String>>,
    /// The last thing that went wrong loading, for the window to say.
    pub error: Signal<Option<String>>,
    /// Where keeping the installed app current stands.
    pub update: Signal<crate::update::UpdateStatus>,
    /// A newer version downloaded and verified, waiting for a restart.
    pub update_ready: Signal<Option<String>>,
}

impl State {
    pub fn new() -> Self {
        let settings = Settings::load();
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
            downloaded: Signal::new(Vec::new()),
            appearance: Signal::new(settings.appearance),
            mark_read: Signal::new(settings.mark_read),
            after_move: Signal::new(settings.after_move),
            label_choices: Signal::new(Vec::new()),
            labelling: Signal::new(false),
            pending: Signal::new(Vec::new()),
            read_due: Signal::new(0),
            sidebar_width: Signal::new(settings.sidebar_width.unwrap_or(SIDEBAR_WIDTH)),
            list_width: Signal::new(settings.list_width.unwrap_or(LIST_WIDTH)),
            system_dark: Signal::new(system_prefers_dark()),
            settings_section: Signal::new(SettingsSection::Accounts),
            view: Signal::new(View::Mail),
            accounts: Signal::new(Vec::new()),
            account_shown: Signal::new(None),
            removing: Signal::new(None),
            notice: Signal::new(None),
            agents_off: Signal::new(None),
            agent_list: Signal::new(Vec::new()),
            agent_dialog: Signal::new(None),
            agent_removing: Signal::new(None),
            rendered: Signal::new(Default::default()),
            pictures_shown: Signal::new(HashSet::new()),
            pictures_from: Signal::new(settings.pictures_from.iter().cloned().collect()),
            composing: Signal::new(None),
            compose_error: Signal::new(None),
            outgoing: Signal::new(Vec::new()),
            sync_off: Signal::new(None),
            error: Signal::new(None),
            update: Signal::new(crate::update::UpdateStatus::NotInstalled),
            update_ready: Signal::new(None),
        }
    }

    /// Keep what is shown loaded: the sidebar and the folder's threads
    /// whenever sync changes the store or another folder is chosen, and
    /// the open thread whenever it changes or sync does.
    /// Keep what is shown loaded, through the loader: the sidebar and the
    /// folder's threads whenever sync changes the store or another folder
    /// is chosen, the open thread and the search likewise, and Settings'
    /// accounts while Settings is open.
    pub fn load(self, commands: &Commands) {
        let loader = match loader::start(commands.clone(), self) {
            Ok(loader) => loader,
            Err(e) => {
                self.error.set(Some(e));
                return;
            }
        };
        let ask = move |request: Request| {
            if loader.send(request).is_err() {
                tracing::error!("the loader has stopped");
            }
        };
        let lists = ask.clone();
        Effect::new(move || {
            self.revision.get();
            lists(Request::Lists {
                folder: self.folder.get(),
                category: self.category.get(),
                shown: self.shown.get(),
            });
        });
        let accounts = ask.clone();
        Effect::new(move || {
            self.revision.get();
            if self.view.get() == View::Settings {
                accounts(Request::Accounts);
            }
        });
        Effect::new(move || {
            if self.view.get() == View::Settings
                && self.settings_section.get() == SettingsSection::Agents
            {
                self.load_agents();
            }
        });
        let search = ask.clone();
        Effect::new(move || {
            self.revision.get();
            search(Request::Search(self.search.get()));
        });
        let labels = ask.clone();
        Effect::new(move || {
            self.revision.get();
            if self.labelling.get()
                && let Some((account, _)) = self.selected.get()
            {
                labels(Request::Labels(account));
            }
        });
        Effect::new(move || {
            self.revision.get();
            ask(Request::Thread {
                thread: self.selected.get(),
                pictures: Box::new(crate::pictures::Pictures {
                    messages: self.pictures_shown.get(),
                    senders: self.pictures_from.get(),
                }),
            });
        });
        // An open thread's messages known only by their headers download
        // at once: the person is reading them.
        Effect::new(move || {
            let Some(thread) = self.thread.get() else {
                return;
            };
            let waiting: Vec<_> = thread
                .messages
                .iter()
                .filter(|m| !m.downloaded)
                .map(|m| m.id.clone())
                .collect();
            if !waiting.is_empty() {
                self.download_now(thread.summary.account.clone(), waiting);
            }
        });
        Effect::new(move || crate::actions::finished(&self.downloaded.get()));
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

    /// Keep the panes' widths for next time.
    pub fn save_pane_widths(self) {
        let mut settings = Settings::load();
        settings.sidebar_width = Some(self.sidebar_width.get());
        settings.list_width = Some(self.list_width.get());
        if let Err(e) = settings.save() {
            tracing::warn!("the pane widths were not saved: {e}");
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
