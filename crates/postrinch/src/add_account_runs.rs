//! The adds (and changes of servers) the dialog starts: one per address
//! at a time, the browser's wait the only part that can be cancelled,
//! and each one's answer going where the person is. The dialog shows it
//! when the person is still waiting on that add; once they closed the
//! dialog or moved on, a notice says how it went, so an add that outlives
//! the dialog is never lost.

use std::cell::RefCell;

use postrinch_discover::Progress;
use postrinch_model::EmailAddress;
use postrinch_sync::imap::SignInCanceller;

use crate::actions::Notice;
use crate::add_account::Step;
use crate::add_account_flow::Outcome;
use crate::add_account_run::{AddUi, note_progress};

thread_local! {
    /// Stops the browser sign-in an add waits on, with the dialog
    /// generation the add started at.
    static CANCEL: RefCell<Option<(u64, SignInCanceller)>> = const { RefCell::new(None) };
}

/// One add, or change of an account's servers, under way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub address: EmailAddress,
    /// The dialog's generation as it started: while the dialog is still
    /// at it, the person is waiting on this add.
    pub generation: u64,
    /// Where the person asked from, where a failure leaves them.
    pub from: Step,
    /// It changes an account's servers rather than adding one.
    pub changing: bool,
}

/// Where an add's answer goes.
#[derive(Debug)]
pub enum Answer {
    /// The person is still waiting on it in the dialog.
    Dialog(Outcome),
    /// The person closed the dialog or moved on: a notice says how it
    /// went, unless they stopped it themselves.
    Away(Option<Notice>),
}

impl AddUi {
    /// Whether the dialog is busy with what it last asked for (discovery,
    /// the browser, the check), so a second Enter or click asks nothing.
    pub fn working(self) -> bool {
        matches!(
            self.step.get(),
            Some(Step::LookingUp | Step::Browser | Step::Checking)
        )
    }

    /// Starts an add for `address`, asked for from `from`, the dialog
    /// going to `step`; or says why not: one for the address is under way
    /// already, and it says how it went when it ends.
    pub fn start(
        self,
        address: EmailAddress,
        from: Step,
        step: Step,
        changing: bool,
    ) -> Result<Run, String> {
        if self.under_way.get().contains(&address) {
            return Err(format!(
                "Postrinch is still busy with {address}, and says how that went when it is done."
            ));
        }
        self.go(step);
        self.under_way.update(|u| u.push(address.clone()));
        Ok(Run {
            address,
            generation: self.generation.get(),
            from,
            changing,
        })
    }

    /// The browser came back for `run`: from here the servers are being
    /// checked, which cannot be stopped, so the dialog says so and offers
    /// no Cancel.
    pub fn granted(self, run: &Run) {
        drop_canceller(run.generation);
        if self.generation.get() == run.generation && self.step.get() == Some(Step::Browser) {
            self.step.set(Some(Step::Checking));
        }
    }

    /// `run` ended with `done`: it is no longer under way, and its answer
    /// goes to the dialog if the person is still waiting on it there.
    pub fn answer(self, run: &Run, done: Outcome) -> Answer {
        self.under_way
            .update(|u| u.retain(|address| *address != run.address));
        drop_canceller(run.generation);
        if self.generation.get() == run.generation {
            Answer::Dialog(done)
        } else {
            Answer::Away(away_notice(run, done))
        }
    }

    /// `run`'s task ended without an answer (it panicked or was dropped):
    /// it is no longer under way all the same, and the person is told it
    /// stopped, in the dialog if they are still waiting on it there and
    /// by a notice otherwise. Whether it went through is not known, so
    /// neither says it did or did not.
    pub fn unanswered(self, run: &Run) -> Answer {
        let error = "Postrinch stopped before this finished, so it may not have gone through. \
                     Try again."
            .to_owned();
        match self.answer(
            run,
            Outcome::Failed {
                step: run.from,
                error,
            },
        ) {
            Answer::Away(_) => Answer::Away(Some(Notice {
                text: unfinished_text(run),
                trouble: true,
            })),
            dialog => dialog,
        }
    }

    /// Discovery's `progress`, for the lookup at `generation`: dropped
    /// once the person has gone back from it, so an abandoned lookup's
    /// steps never show in a new one.
    pub fn noted(self, generation: u64, progress: Progress) {
        if self.generation.get() == generation {
            self.looking
                .update(move |steps| note_progress(steps, progress));
        }
    }
}

/// What the person is told of `run` once they have left it.
pub fn away_notice(run: &Run, done: Outcome) -> Option<Notice> {
    let address = &run.address;
    match done {
        Outcome::Done(_) => Some(Notice {
            text: done_text(run),
            trouble: false,
        }),
        Outcome::Failed { error, .. } => Some(Notice {
            text: if run.changing {
                format!("{address}'s servers were not changed. {error}")
            } else {
                format!("{address} was not added. {error}")
            },
            trouble: true,
        }),
        // The person stopped it, so there is nothing to tell.
        Outcome::Cancelled => None,
    }
}

/// What a run that went through says.
pub fn done_text(run: &Run) -> String {
    if run.changing {
        format!("{}'s servers are changed.", run.address)
    } else {
        format!("{} is added. Its mail is on the way.", run.address)
    }
}

/// What the person is told of `run` once they have left it, when it
/// stopped without saying how it went.
pub fn unfinished_text(run: &Run) -> String {
    let address = &run.address;
    if run.changing {
        format!(
            "Postrinch stopped before it finished changing {address}'s servers. \
             Its server settings show what they are now."
        )
    } else {
        format!(
            "Postrinch stopped before it finished adding {address}. \
             Settings › Accounts shows whether it is there."
        )
    }
}

/// Keeps the way to stop the browser sign-in the add started at
/// `generation` waits on.
pub fn keep_canceller(generation: u64, canceller: SignInCanceller) {
    CANCEL.with(|c| *c.borrow_mut() = Some((generation, canceller)));
}

/// Stops the browser sign-in an add waits on, if one does.
pub fn cancel_browser() {
    if let Some((_, canceller)) = CANCEL.with(|c| c.borrow_mut().take()) {
        canceller.cancel();
    }
}

/// Lets go of the add at `generation`'s canceller, once its browser is
/// back or it ended; another add's is left alone.
fn drop_canceller(generation: u64) {
    CANCEL.with(|c| {
        let mut kept = c.borrow_mut();
        if kept.as_ref().is_some_and(|(g, _)| *g == generation) {
            kept.take();
        }
    });
}

#[cfg(test)]
#[path = "add_account_runs_tests.rs"]
mod tests;
