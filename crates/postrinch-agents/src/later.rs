//! Work an answer waits for only so long. A change of many thousands of
//! threads, or one made while sync is busy writing, can take longer than an
//! agent's call may; then the answer says how far it got, and the work
//! carries on here, the window told when it is done.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::serve::OnChange;
use crate::wire::Changed;

/// How long an answer waits, well inside the bridge's two minutes.
const WAIT: Duration = Duration::from_secs(60);

/// What became of the work by the time the answer goes.
pub(crate) enum Finished<T> {
    Done(T),
    /// Still going, with how much it had done (as the work counts).
    Going(usize),
}

/// Run `work` on a thread of its own, giving it a counter of how much it
/// has done, and wait up to [`WAIT`] for it. If it finishes after the
/// answer has gone, `changed` is told to the window then.
pub(crate) fn within<T: Clone + Send + 'static>(
    on_change: &OnChange,
    changed: Changed,
    work: impl FnOnce(&AtomicUsize) -> Result<T, String> + Send + 'static,
) -> Result<Finished<T>, String> {
    let done = Arc::new(AtomicUsize::new(0));
    let (finished, waiting) = mpsc::channel();
    let counter = done.clone();
    let on_change = on_change.clone();
    thread::Builder::new()
        .name("agent work".into())
        .spawn(move || {
            let outcome = work(&counter);
            // Nobody is waiting: the answer went before it was done.
            if finished.send(outcome.clone()).is_err() {
                match outcome {
                    Ok(_) => on_change(&changed),
                    Err(e) => tracing::warn!("an agent's change stopped partway: {e}"),
                }
            }
        })
        .map_err(|e| format!("Postrinch could not start the change: {e}."))?;
    match waiting.recv_timeout(WAIT) {
        Ok(outcome) => outcome.map(Finished::Done),
        Err(_) => Ok(Finished::Going(done.load(Ordering::Relaxed))),
    }
}
