//! Loading what the window shows, on a thread of its own, so the window
//! never waits for the store: the UI thread asks, the loader answers into
//! the window's signals. When asks pile up, only the newest of each kind
//! is answered.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use postrinch_commands::{Commands, Folder};
use postrinch_model::{EmailAddress, ProviderThreadId, SystemLabel};

use crate::state::State;

/// How many threads a search shows.
const SEARCH_LIMIT: usize = 100;
/// Loads slower than this are logged, to find what to make faster.
const SLOW: Duration = Duration::from_millis(250);

/// Something the window wants loaded.
#[derive(Debug, Clone)]
pub enum Request {
    /// The sidebar and a page of the folder's threads.
    Lists {
        folder: Folder,
        category: Option<SystemLabel>,
        shown: usize,
    },
    /// Settings' accounts.
    Accounts,
    /// A search's results, or none.
    Search(Option<String>),
    /// The open thread, or none.
    Thread(Option<(EmailAddress, ProviderThreadId)>),
}

impl Request {
    /// Requests of the same kind replace each other.
    fn kind(&self) -> usize {
        match self {
            Request::Lists { .. } => 0,
            Request::Accounts => 1,
            Request::Search(_) => 2,
            Request::Thread(_) => 3,
        }
    }
}

/// Start the loader; returns where to send it requests.
pub fn start(commands: Commands, state: State) -> Result<Sender<Request>, String> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("loader".into())
        .spawn(move || serve(&commands, state, &rx))
        .map_err(|e| format!("cannot start the loader: {e}"))?;
    Ok(tx)
}

fn serve(commands: &Commands, state: State, rx: &Receiver<Request>) {
    while let Ok(first) = rx.recv() {
        // Everything asked for meanwhile; the newest of each kind wins.
        let mut latest: [Option<Request>; 4] = Default::default();
        let k = first.kind();
        latest[k] = Some(first);
        while let Ok(more) = rx.try_recv() {
            let k = more.kind();
            latest[k] = Some(more);
        }
        for request in latest.into_iter().flatten() {
            let started = Instant::now();
            let what = format!("{request:?}");
            answer(commands, state, request);
            let took = started.elapsed();
            if took > SLOW {
                tracing::warn!("loading took {took:?}: {what}");
            }
        }
    }
}

fn answer(commands: &Commands, state: State, request: Request) {
    let failed = |e: String| state.error.send(Some(e));
    match request {
        Request::Lists {
            folder,
            category,
            shown,
        } => match commands.sidebar() {
            Ok(sidebar) => {
                state.sidebar.send(Some(sidebar));
                match commands.list_threads(&folder, category, 0, shown) {
                    Ok(page) => state.page.send(Some(page)),
                    Err(e) => failed(e.to_string()),
                }
            }
            Err(e) => failed(e.to_string()),
        },
        Request::Accounts => match commands.accounts() {
            Ok(accounts) => state.accounts.send(accounts),
            Err(e) => failed(e.to_string()),
        },
        Request::Search(None) => state.results.send(None),
        Request::Search(Some(query)) => match commands.search(&query, SEARCH_LIMIT) {
            Ok(results) => state.results.send(Some(results)),
            Err(e) => failed(e.to_string()),
        },
        Request::Thread(None) => state.thread.send(None),
        Request::Thread(Some((account, thread))) => match commands.read_thread(&account, &thread) {
            Ok(thread) => state.thread.send(thread),
            Err(e) => failed(e.to_string()),
        },
    }
}
