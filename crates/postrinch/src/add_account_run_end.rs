//! The end of an add (or change of servers), which always comes. The
//! task an add runs in carries a `RunEnd` and hands it the answer as its
//! last step; if the task never gets there, because it panicked or was
//! dropped, the `RunEnd` says so as it goes. Either way the address is
//! let go of exactly once, so a later add for it is never refused for an
//! add that is no longer running.

use crate::add_account_flow::Outcome;
use crate::add_account_runs::Run;

/// What a run's end is handed to: the run, and its answer, or none when
/// the task ended without one.
type Post = Box<dyn FnOnce(Run, Option<Outcome>) + Send>;

/// The end of `run`, still to come.
pub struct RunEnd {
    /// Taken by whichever comes first, the answer or the drop.
    pending: Option<(Run, Post)>,
}

impl RunEnd {
    /// The end of `run`, handed to `post` once: with the answer when the
    /// task gives one, without when it ends without.
    pub fn new(run: Run, post: impl FnOnce(Run, Option<Outcome>) + Send + 'static) -> Self {
        Self {
            pending: Some((run, Box::new(post))),
        }
    }

    /// The run ended with `done`.
    pub fn end(mut self, done: Outcome) {
        if let Some((run, post)) = self.pending.take() {
            post(run, Some(done));
        }
    }
}

impl Drop for RunEnd {
    /// The task ended without an answer: it panicked, or it was dropped
    /// (cancelled, or its runtime shut down).
    fn drop(&mut self) {
        if let Some((run, post)) = self.pending.take() {
            post(run, None);
        }
    }
}

#[cfg(test)]
#[path = "add_account_run_end_tests.rs"]
mod tests;
