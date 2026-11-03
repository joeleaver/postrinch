//! What the window shows, as signals, and loading it through the commands.

use std::collections::HashSet;
use std::rc::Rc;

use postrinch_commands::{Commands, Folder, Sidebar, Thread, ThreadPage};
use postrinch_model::{EmailAddress, ProviderThreadId, SystemLabel};
use rinch::Effect;
use rinch::prelude::*;

use crate::sync::AccountSync;

/// How many more threads "Show more" loads.
pub const PAGE: usize = 100;

/// The window's state. Every field is a signal, so this is `Copy`.
#[derive(Clone, Copy)]
pub struct State {
    pub sidebar: Signal<Option<Sidebar>>,
    pub folder: Signal<Folder>,
    /// How many of the folder's threads to show.
    pub shown: Signal<usize>,
    pub page: Signal<Option<ThreadPage>>,
    /// Whether the sidebar shows every label, hidden ones included.
    pub show_all: Signal<bool>,
    /// Sidebar rows the person opened or closed (see `ui::labels`).
    pub toggled: Signal<HashSet<String>>,
    pub selected: Signal<Option<(EmailAddress, ProviderThreadId)>>,
    pub thread: Signal<Option<Thread>>,
    pub sync: Signal<Vec<AccountSync>>,
    /// Bumped whenever sync changed the store, so what is shown reloads.
    pub revision: Signal<u64>,
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
            shown: Signal::new(PAGE),
            page: Signal::new(None),
            show_all: Signal::new(false),
            toggled: Signal::new(HashSet::new()),
            selected: Signal::new(None),
            thread: Signal::new(None),
            sync: Signal::new(Vec::new()),
            revision: Signal::new(0),
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
            let shown = self.shown.get();
            let loaded = lists
                .sidebar()
                .and_then(|sidebar| Ok((sidebar, lists.list_threads(&folder, 0, shown)?)));
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

    pub fn open_folder(self, folder: Folder) {
        self.shown.set(PAGE);
        self.selected.set(None);
        self.folder.set(folder);
    }
}
