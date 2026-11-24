//! A demo store: two made-up Gmail accounts with a few weeks of mail, their
//! labels, inbox categories, views, rules and a week of calendar events,
//! written into an empty folder for the window to open. It is for the
//! website's screenshots and for trying the window with no real account;
//! nothing in it is anyone's real mail (people.rs).
//!
//! Mail is stored the way sync stores it: each message is written as RFC
//! 5322 text, read back with `postrinch_sync::parse_message` and kept with
//! its raw bytes, so the window renders it, finds it and shows its
//! attachments and invitation exactly as it would a downloaded message.
//! Times are relative to the moment it is seeded, so the store looks
//! current whenever it is made.

mod accounts;
mod calendar;
mod clock;
mod content_home;
mod content_work;
mod mail;
mod newsletters;
mod pdf;
mod people;
mod threads;
mod views_rules;

use std::path::{Path, PathBuf};

use jiff::Zoned;
use postrinch_commands::{CommandError, Commands};
use postrinch_store::{Store, StoreError};
use postrinch_sync::SyncError;

pub use people::{SAM_HOME, SAM_WORK};

#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error(
        "{0} is not empty: the demo store is written only into a new or empty folder, never over a store"
    )]
    NotEmpty(PathBuf),
    #[error("{0} is Postrinch's own store, which the demo never writes to")]
    RealStore(PathBuf),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("the store: {0}")]
    Store(#[from] StoreError),
    #[error("a demo message did not read back: {0}")]
    Mime(#[from] SyncError),
    #[error("{0}")]
    Command(#[from] CommandError),
    #[error("a demo time: {0}")]
    Time(#[from] jiff::Error),
}

pub type DemoResult<T> = Result<T, DemoError>;

/// What a seeding wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seeded {
    pub accounts: usize,
    pub threads: usize,
    pub messages: usize,
    pub events: usize,
    pub views: usize,
    pub rules: usize,
}

/// Write the demo store into `root`, a store folder as the window opens it
/// (`<data dir>/postrinch`), with times relative to `now`. `root` must be
/// new or empty, and must not be this machine's own store.
pub fn seed(root: &Path, now: &Zoned) -> DemoResult<Seeded> {
    check_target(root)?;
    let store = Store::open(root)?;
    let clock = clock::Clock::new(now.clone());
    let accounts = accounts::put(&store)?;
    let mut specs = content_home::threads();
    specs.extend(content_work::threads(&clock)?);
    let stored = threads::put_all(&store, &clock, specs)?;
    let events = calendar::put(&store, &clock)?;
    let commands = Commands::new(store);
    let (views, rules) = views_rules::put(&commands, clock::timestamp(clock.now()))?;
    Ok(Seeded {
        accounts,
        threads: stored.threads,
        messages: stored.messages,
        events,
        views,
        rules,
    })
}

/// Refuse a folder that holds anything, and the machine's own store even
/// when empty: the demo is only ever written somewhere new.
fn check_target(root: &Path) -> DemoResult<()> {
    if let Some(own) = dirs::data_dir().map(|d| d.join("postrinch"))
        && same_place(&own, root)
    {
        return Err(DemoError::RealStore(root.to_owned()));
    }
    match std::fs::read_dir(root) {
        Ok(mut entries) => match entries.next() {
            Some(_) => Err(DemoError::NotEmpty(root.to_owned())),
            None => Ok(()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Whether two paths name one folder, as far as can be told: by their
/// canonical forms where they exist, else as written.
fn same_place(a: &Path, b: &Path) -> bool {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_owned());
    canonical(a) == canonical(b)
}
