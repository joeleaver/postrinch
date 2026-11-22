//! Loading what the window shows, on threads of their own, so the window
//! never waits for the store: the UI thread asks, the loader answers into
//! the window's signals. When asks pile up, only the newest of each kind
//! is answered. The open thread and its labels load on a thread apart from
//! lists and search, so a folder that takes seconds to list never holds up
//! what the person is reading.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use std::collections::HashMap;

use postrinch_commands::{Commands, Grouping};
use postrinch_model::{EmailAddress, ProviderThreadId, SavedView};

use crate::changes;
use crate::state::State;

/// The least time between two hand-backs of freed memory (`serve`).
const RELEASE_EVERY: Duration = Duration::from_secs(30);
/// How many people a sender or recipient picker offers.
const PEOPLE_LIMIT: usize = 30;
/// How many threads a search shows.
const SEARCH_LIMIT: usize = 100;
/// Loads slower than this are logged, to find what to make faster.
const SLOW: Duration = Duration::from_millis(250);

/// Something the window wants loaded.
#[derive(Debug, Clone)]
pub enum Request {
    /// A page of the view's threads, then the sidebar and the views.
    Lists {
        /// The view as shown, in its own order.
        view: SavedView,
        shown: usize,
        /// Conversations, or one message at a time.
        grouping: Grouping,
    },
    /// Settings' accounts.
    Accounts,
    /// A search's results, or none.
    Search {
        query: Option<String>,
        /// The accounts it looks in (every account for none).
        accounts: Vec<EmailAddress>,
        grouping: Grouping,
    },
    /// The open thread, or none.
    Thread {
        thread: Option<(EmailAddress, ProviderThreadId)>,
        /// Whose pictures from the web to load.
        pictures: Box<crate::pictures::Pictures>,
    },
    /// The labels an account's mail can be given, for the label menu.
    Labels(EmailAddress),
    /// People whose address or name holds this, to pick as senders or
    /// recipients.
    People(String),
}

impl Request {
    /// Requests of the same kind replace each other.
    fn kind(&self) -> usize {
        match self {
            Request::Lists { .. } => 0,
            Request::Accounts => 1,
            Request::Search { .. } => 2,
            Request::Thread { .. } => 3,
            Request::Labels(_) => 4,
            Request::People(_) => 5,
        }
    }
}

/// Where the window sends what it wants loaded.
#[derive(Clone)]
pub struct Loader {
    /// The open thread and its labels: quick, and what the person waits on.
    quick: Sender<Request>,
    /// Lists, search and accounts: they can take seconds.
    slow: Sender<Request>,
}

impl Loader {
    pub fn send(&self, request: Request) -> Result<(), mpsc::SendError<Request>> {
        match request {
            Request::Thread { .. } | Request::Labels(_) | Request::People(_) => {
                self.quick.send(request)
            }
            _ => self.slow.send(request),
        }
    }
}

/// Start the loader's two threads.
pub fn start(commands: Commands, state: State) -> Result<Loader, String> {
    let spawn = |name: &str, commands: Commands| {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name(name.into())
            .spawn(move || serve(&commands, state, &rx))
            .map(|_| tx)
            .map_err(|e| format!("cannot start the loader: {e}"))
    };
    Ok(Loader {
        quick: spawn("loader-quick", commands.clone())?,
        slow: spawn("loader", commands)?,
    })
}

fn serve(commands: &Commands, state: State, rx: &Receiver<Request>) {
    let mut released: Option<Instant> = None;
    while let Ok(first) = rx.recv() {
        // Everything asked for meanwhile; the newest of each kind wins.
        let mut latest: [Option<Request>; 6] = Default::default();
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
        // Lists and search let go of a lot; hand it back once they are in
        // (the first lists at startup, then at most every RELEASE_EVERY), or
        // the window keeps its size from the biggest load it ever made.
        if released.is_none_or(|at| at.elapsed() >= RELEASE_EVERY) {
            commands.release_memory();
            released = Some(Instant::now());
        }
    }
}

fn answer(commands: &Commands, state: State, request: Request) {
    let failed = |e: String| state.error.send(Some(e));
    match request {
        Request::Lists {
            view,
            shown,
            grouping,
        } => {
            // Every action made by now shows in what is read next.
            let as_of = changes::made();
            // The list first, what the person waits on; the sidebar's
            // counts after.
            let page = match grouping {
                Grouping::Conversations => commands.list_view(&view, view.order, 0, shown),
                Grouping::Messages => commands.list_view_messages(&view, view.order, 0, shown),
            };
            match page {
                Ok(page) => {
                    let key = crate::recent::ListKey { view, grouping };
                    // Not a list the person has since left.
                    if crate::recent::loaded(key, &page) {
                        state.page.send(Some(page));
                        state
                            .pending
                            .update_send(move |p| p.retain(|c| c.number > as_of));
                    }
                }
                Err(e) => failed(e.to_string()),
            }
            match commands.sidebar() {
                Ok(sidebar) => state.sidebar.send(Some(sidebar)),
                Err(e) => failed(e.to_string()),
            }
            match commands.views() {
                Ok(views) => {
                    let saved: Vec<SavedView> = views.iter().map(|e| e.view.clone()).collect();
                    state.views.send(views);
                    match commands.view_counts(&saved) {
                        Ok(counts) => state.view_counts.send(
                            counts
                                .into_iter()
                                .map(|t| (t.key.clone(), t))
                                .collect::<HashMap<_, _>>(),
                        ),
                        Err(e) => failed(e.to_string()),
                    }
                }
                Err(e) => failed(e.to_string()),
            }
            match commands.outgoing() {
                Ok(outgoing) => state.outgoing.send(outgoing),
                Err(e) => failed(e.to_string()),
            }
            match commands.rule_proposals() {
                Ok(proposals) => state.rules.proposals.send(proposals),
                Err(e) => failed(e.to_string()),
            }
        }
        Request::Accounts => match commands.accounts() {
            Ok(accounts) => state.accounts.send(accounts),
            Err(e) => failed(e.to_string()),
        },
        Request::Search { query: None, .. } => {
            state.search_error.send(None);
            state.results.send(None);
        }
        Request::Search {
            query: Some(query),
            accounts,
            grouping,
        } => match commands.search_grouped(&query, &accounts, SEARCH_LIMIT, grouping) {
            Ok(results) => {
                state.search_error.send(None);
                state.results.send(Some(results));
            }
            Err(postrinch_commands::CommandError::Invalid(why)) => {
                state.search_error.send(Some(why));
                state.results.send(Some(postrinch_commands::SearchResults {
                    query,
                    threads: Vec::new(),
                    more: false,
                    remote: None,
                }));
            }
            Err(e) => failed(e.to_string()),
        },
        Request::People(part) => match commands.people(&part, PEOPLE_LIMIT) {
            Ok(people) => state.people.send((part, people)),
            Err(e) => failed(e.to_string()),
        },
        Request::Thread { thread: None, .. } => state.thread.send(None),
        Request::Thread {
            thread: Some((account, thread)),
            pictures,
        } => match commands.read_thread(&account, &thread) {
            Ok(thread) => {
                // Rendered before the thread is shown, so its messages are
                // drawn once, as they look.
                if let Some(thread) = &thread {
                    state
                        .rendered
                        .send(crate::pictures::render(commands, thread, &pictures));
                    state
                        .thumbnails
                        .send(crate::attachments::thumbnails(commands, thread));
                }
                state.thread.send(thread);
            }
            Err(e) => failed(e.to_string()),
        },
        Request::Labels(account) => match commands.label_choices(&account) {
            Ok(choices) => state.label_choices.send(choices),
            Err(e) => failed(e.to_string()),
        },
    }
}
