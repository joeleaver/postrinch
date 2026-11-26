//! A run's end comes exactly once, however its task ends: with the
//! answer, or without one when the task panicked or was dropped.

use std::sync::mpsc::{Receiver, channel};

use postrinch_model::{Account, EmailAddress, Provider};

use super::*;
use crate::add_account::Step;

fn ada() -> EmailAddress {
    EmailAddress::parse("ada@example.org").unwrap()
}

fn run() -> Run {
    Run {
        address: ada(),
        generation: 3,
        from: Step::Found,
        changing: false,
    }
}

/// A run's end, and what it was handed: whether it had an answer, for
/// which address.
fn watched() -> (RunEnd, Receiver<(EmailAddress, bool)>) {
    let (tx, rx) = channel();
    let end = RunEnd::new(run(), move |run, done| {
        tx.send((run.address, done.is_some())).unwrap();
    });
    (end, rx)
}

/// Everything `rx` was handed, once its end is gone.
fn handed(rx: &Receiver<(EmailAddress, bool)>) -> Vec<(EmailAddress, bool)> {
    rx.try_iter().collect()
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .build()
        .unwrap()
}

#[test]
fn an_answer_ends_it_once() {
    let (end, rx) = watched();
    end.end(Outcome::Done(Box::new(Account::new(ada(), Provider::Imap))));
    assert_eq!(handed(&rx), [(ada(), true)], "no second end on the drop");
}

#[test]
fn a_panicking_thread_still_ends_it() {
    let (end, rx) = watched();
    let thread = std::thread::spawn(move || {
        let _end = end;
        panic!("the add went wrong");
    });
    assert!(thread.join().is_err());
    assert_eq!(handed(&rx), [(ada(), false)]);
}

#[test]
fn a_panicking_task_still_ends_it() {
    let (end, rx) = watched();
    let runtime = runtime();
    let task = runtime.spawn(async move {
        let _end = end;
        panic!("the add went wrong");
    });
    let joined = runtime.block_on(task);
    assert!(joined.unwrap_err().is_panic());
    assert_eq!(handed(&rx), [(ada(), false)]);
}

#[test]
fn a_dropped_task_still_ends_it() {
    // A task cancelled part way.
    let (end, rx) = watched();
    let runtime = runtime();
    let task = runtime.spawn(async move {
        let _end = end;
        std::future::pending::<()>().await;
    });
    task.abort();
    assert!(runtime.block_on(task).unwrap_err().is_cancelled());
    assert_eq!(handed(&rx), [(ada(), false)]);

    // A task left running when its runtime shuts down.
    let (end, rx) = watched();
    let runtime = self::runtime();
    runtime.spawn(async move {
        let _end = end;
        std::future::pending::<()>().await;
    });
    drop(runtime);
    assert_eq!(handed(&rx), [(ada(), false)]);
}
