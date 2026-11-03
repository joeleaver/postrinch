//! Changing mail from the window: archive, trash, read, star, label.
//!
//! An action is shown at once and made on a thread of its own, in the order
//! the person acted: [`State::act`] records it as pending, which the lists
//! and the reader apply over what they loaded, then hands it to the changes
//! thread, which makes it in the store (and so the outbox) and asks for the
//! changes to be sent. A pending action is dropped once a list load that
//! began after it was made has arrived (the loader does this, by [`made`]):
//! that load already shows it, and a load still running from before cannot
//! bring an archived thread back meanwhile.

use std::cell::OnceCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use postrinch_commands::{Commands, Folder, ThreadAction, ThreadRef, ThreadRow};
use postrinch_model::{SystemLabel, ThreadSummary};
use tokio::sync::Notify;

use crate::reading::AfterMove;
use crate::state::State;
use crate::ui::list::{is_inbox, row_ids};

/// The newest action the changes thread has made, by its number.
static MADE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static CHANGES: OnceCell<Sender<Change>> = const { OnceCell::new() };
}

/// An action, numbered in the order the person took them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub number: u64,
    pub threads: Vec<ThreadRef>,
    pub action: ThreadAction,
}

/// The newest action made in the store; a load that begins after reading
/// this shows every action up to it.
pub fn made() -> u64 {
    MADE.load(Ordering::Acquire)
}

/// Start the changes thread. `send` wakes sync's sender, when sync runs.
pub fn start(commands: Commands, state: State, send: Option<Arc<Notify>>) {
    let (tx, rx) = mpsc::channel();
    let spawned = thread::Builder::new()
        .name("changes".into())
        .spawn(move || serve(&commands, state, send.as_deref(), &rx));
    match spawned {
        Ok(_) => CHANGES.with(|c| {
            if c.set(tx).is_err() {
                tracing::error!("the changes thread was started twice");
            }
        }),
        Err(e) => state
            .error
            .set(Some(format!("Changes cannot be made: {e}"))),
    }
}

fn serve(commands: &Commands, state: State, send: Option<&Notify>, rx: &Receiver<Change>) {
    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        batch.extend(rx.try_iter());
        for change in batch {
            if let Err(e) = commands.act(&change.threads, &change.action) {
                tracing::error!("{:?} failed: {e}", change.action);
                state
                    .error
                    .send(Some(format!("That change was not made: {e}")));
            }
            MADE.store(change.number, Ordering::Release);
        }
        if let Some(send) = send {
            send.notify_one();
        }
        state.revision.update_send(|n| *n += 1);
    }
}

impl State {
    /// Do `action` to `threads`: shown at once, made in the background.
    /// When it takes the open thread out of the list, what opens next
    /// follows the person's setting.
    pub fn act(self, threads: Vec<ThreadRef>, action: ThreadAction) {
        if threads.is_empty() {
            return;
        }
        let sender = CHANGES.with(|c| c.get().cloned());
        let Some(sender) = sender else {
            self.error.set(Some("Changes cannot be made.".into()));
            return;
        };
        let leaving = removes(&action, &self.listing());
        let next = match self.selected.get() {
            Some(open) if leaving && threads.contains(&open) => match self.after_move.get() {
                AfterMove::OpenNext => Some(next_after(self, &threads)),
                AfterMove::BackToList => Some(None),
            },
            _ => None,
        };
        let number = self.pending.with(|p| p.last().map_or(made(), |c| c.number)) + 1;
        let change = Change {
            number,
            threads,
            action,
        };
        self.pending.update(|p| p.push(change.clone()));
        if let Some(next) = next {
            self.selected.set(next);
        }
        if sender.send(change).is_err() {
            self.error.set(Some("Changes cannot be made.".into()));
        }
    }

    /// Do `action` to the open thread, if one is open.
    pub fn act_on_open(self, action: ThreadAction) {
        if let Some(open) = self.selected.get() {
            self.act(vec![open], action);
        }
    }

    /// Whether a pending action takes `thread` out of what is listed.
    pub fn pending_removal(self, thread: &ThreadRef) -> bool {
        let listing = self.listing();
        self.pending.with(|p| {
            p.iter()
                .filter(|c| c.threads.contains(thread))
                .fold(false, |gone, c| {
                    if removes(&c.action, &listing) {
                        true
                    } else if restores(&c.action) {
                        false
                    } else {
                        gone
                    }
                })
        })
    }

    /// What the list shows now, as far as filing goes.
    fn listing(self) -> Listing {
        let folder = self.folder.get();
        Listing {
            searching: self.search.get().is_some(),
            inbox: is_inbox(&folder, self.sidebar.get().as_ref()),
            folder,
        }
    }

    /// A row with the pending actions on its thread applied.
    pub fn patched(self, mut row: ThreadRow) -> ThreadRow {
        let thread = (row.summary.account.clone(), row.summary.id.clone());
        self.pending.with(|p| {
            for change in p.iter().filter(|c| c.threads.contains(&thread)) {
                patch(&mut row.summary, &change.action);
            }
        });
        row
    }
}

/// Show a pending action on a thread's summary: what it does to read,
/// starred and the labels it was given. Where the thread is filed shows by
/// whether it stays listed.
pub fn patch(summary: &mut ThreadSummary, action: &ThreadAction) {
    match action {
        ThreadAction::MarkRead => summary.unread_count = 0,
        ThreadAction::MarkUnread => summary.unread_count = summary.unread_count.max(1),
        ThreadAction::Star => summary.starred = true,
        ThreadAction::Unstar => summary.starred = false,
        ThreadAction::AddLabel(label) if !summary.labels.contains(label) => {
            summary.labels.push(label.clone());
        }
        ThreadAction::RemoveLabel(label) => summary.labels.retain(|l| l != label),
        _ => {}
    }
}

/// What the list shows, as far as filing goes.
struct Listing {
    folder: Folder,
    inbox: bool,
    searching: bool,
}

/// Whether `action` takes a thread out of the listing.
fn removes(action: &ThreadAction, listing: &Listing) -> bool {
    let in_trash = matches!(listing.folder, Folder::Everywhere(SystemLabel::Trash));
    let filed = !listing.searching;
    match action {
        ThreadAction::Trash => !filed || !in_trash,
        ThreadAction::Untrash => filed && in_trash,
        ThreadAction::Archive => filed && listing.inbox,
        ThreadAction::RemoveLabel(label) => {
            filed && matches!(&listing.folder, Folder::Label { label: l, .. } if l == label)
        }
        _ => false,
    }
}

/// Whether `action` undoes an earlier removal.
fn restores(action: &ThreadAction) -> bool {
    matches!(
        action,
        ThreadAction::MoveToInbox | ThreadAction::Untrash | ThreadAction::AddLabel(_)
    )
}

/// The thread to open once `leaving` leaves the list: the one after the
/// open one, else the one before it, else none.
fn next_after(state: State, leaving: &[ThreadRef]) -> Option<ThreadRef> {
    let ids = row_ids(state);
    let open = state.selected.get()?;
    let at = ids.iter().position(|id| *id == open)?;
    ids[at + 1..]
        .iter()
        .chain(ids[..at].iter().rev())
        .find(|id| !leaving.contains(id))
        .cloned()
}
