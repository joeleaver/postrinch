//! Live sessions (docs/ofscp.md, "Sync engine"; the plan's PR 9): what
//! happens after the first sync, as it happens.
//!
//! [`Social::go_live`] keeps an account connected: one WebSocket per
//! provider that hosts something it keeps ([`account`]): the home
//! provider always, and every provider of a group it is in or a channel
//! it follows. Each socket is a session ([`session`]) that:
//!
//! - reconnects after a drop, backing off from 0.5 s, doubling to 30 s,
//!   with jitter ([`LiveOptions`]). A close `4003` (the key is revoked)
//!   puts the account in [`SocialAccountState::NeedsSignIn`] and ends
//!   every session; a `4001` is retried;
//! - after every connection, catches up: the home session first reads
//!   again over REST what nothing replays (the groups and places, private
//!   conversations, read markers, follows and notifications, §7.1); then
//!   each session subscribes every place it hosts from its stored cursor
//!   ([`subscribe`]), and backfills over REST each place the provider
//!   answers `truncated` for, or that has no cursor yet ([`backfill`]);
//! - keeps what arrives ([`events`], [`apply`]): messages, edits,
//!   deletes, reactions, private messages, the home provider's read
//!   markers ([`markers`]), notifications ([`notices`]), group changes and
//!   presence ([`presence`]). Delivery is at least once (§7.1), so what is
//!   already kept as it arrives is a duplicate, and dropped.
//!
//! Unread follows the home provider's read markers for places it holds,
//! and the place's own marker elsewhere (§7.7). Whether a message mentions
//! the person is read from its body by the spec's mention syntax (§10.1),
//! for every place, as the first sync does.
//!
//! What changes is told on [`Social::live_events`], for the window's
//! notices and counts. A live account also sends its outbox
//! ([`crate::outbox`]).

mod account;
pub(crate) mod apply;
mod backfill;
mod events;
pub(crate) mod markers;
mod notices;
mod presence;
mod session;
mod subscribe;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use postrinch_ofscp::ofscp_core::Actor;
use postrinch_ofscp::ofscp_core::wire::Presence;
use postrinch_ofscp::ws::WsOptions;
use postrinch_store::{SocialAccountState, social_account_key};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

pub(crate) use account::Control;
pub(crate) use presence::Presences;

use crate::{Social, SocialResult, parse_actor};

/// How live sessions reconnect and subscribe.
#[derive(Debug, Clone)]
pub struct LiveOptions {
    /// The wait after the first failure in a row (with jitter, half to
    /// all of it).
    pub first_backoff: Duration,
    /// The longest wait between attempts.
    pub max_backoff: Duration,
    /// The socket's timeouts and limits.
    pub ws: WsOptions,
    /// The most places one `subscribe` names.
    pub subscribe_batch: usize,
}

impl Default for LiveOptions {
    fn default() -> Self {
        Self {
            first_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            ws: WsOptions::default(),
            subscribe_batch: 100,
        }
    }
}

/// What changed in a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageChange {
    /// It is new here.
    New,
    /// It was edited.
    Edited,
    /// It was deleted, and is now a tombstone.
    Deleted,
}

/// Something that happened live, for an account (its actor, as stored).
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    /// A session signed in to `provider`.
    Connected { account: String, provider: String },
    /// A session caught up after connecting: everything missed while it
    /// was away is kept. `backfilled` are the places read again over REST.
    CaughtUp {
        account: String,
        provider: String,
        backfilled: Vec<String>,
    },
    /// A session ended. It tries again after `retry_in`; `None` when it
    /// will not (the account must sign in again, or went offline).
    Disconnected {
        account: String,
        provider: String,
        reason: String,
        retry_in: Option<Duration>,
    },
    /// A message was kept, or changed.
    Message {
        account: String,
        place: String,
        id: String,
        change: MessageChange,
        unread: bool,
        mentions_me: bool,
    },
    /// A message's reactions changed.
    Reactions {
        account: String,
        place: String,
        id: String,
    },
    /// The place's read marker moved, on another device.
    Read {
        account: String,
        place: String,
        marker: String,
    },
    /// A new entry in the notification feed (§10.1).
    Notification { account: String, id: String },
    /// A place was kept or changed (a private conversation).
    Place { account: String, place: String },
    /// The groups and places were read again (§5.7 `groups.updated`, or
    /// a reconnect).
    Groups { account: String },
    /// Someone's presence changed (§7.5). Kept in memory only.
    Presence {
        account: String,
        user: String,
        presence: Presence,
    },
    /// Someone started or stopped typing in a place.
    Typing {
        account: String,
        place: String,
        user: String,
        typing: bool,
    },
    /// A queued change reached its provider, and left the outbox.
    Sent {
        account: String,
        client_id: String,
        place: Option<String>,
    },
    /// A queued change was not sent: it is tried again later, or, when
    /// `refused`, its provider refused it for good and it waits for the
    /// person to give it up.
    NotSent {
        account: String,
        client_id: String,
        place: Option<String>,
        error: String,
        refused: bool,
    },
    /// The account can no longer connect: it must sign in again, or set
    /// its clock right.
    AccountState {
        account: String,
        state: SocialAccountState,
    },
}

/// A session of an account, as [`Social::sessions`] gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStatus {
    /// The provider's domain.
    pub provider: String,
    /// Whether its socket is signed in now.
    pub connected: bool,
}

/// Every account's live sessions, shared by the engine's clones.
pub(crate) struct Registry {
    pub(crate) options: LiveOptions,
    pub(crate) events: broadcast::Sender<LiveEvent>,
    pub(crate) presences: Presences,
    accounts: Mutex<HashMap<String, Running>>,
    /// Whether accounts added or signed in again go live by themselves
    /// ([`Social::keep_connected`]).
    auto: AtomicBool,
    generation: AtomicU64,
}

/// An account that is live.
struct Running {
    generation: u64,
    control: mpsc::UnboundedSender<Control>,
    task: JoinHandle<()>,
    status: Status,
}

/// Each session's provider and whether it is connected.
pub(crate) type Status = Arc<Mutex<BTreeMap<String, bool>>>;

impl Registry {
    pub(crate) fn new(options: LiveOptions) -> Self {
        Self {
            options,
            events: broadcast::channel(1024).0,
            presences: Presences::default(),
            accounts: Mutex::default(),
            auto: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        }
    }

    fn accounts(&self) -> std::sync::MutexGuard<'_, HashMap<String, Running>> {
        self.accounts.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Tell `event` to whoever listens; nobody listening is fine.
    pub(crate) fn tell(&self, event: LiveEvent) {
        let _ = self.events.send(event);
    }

    /// Forget a run of `account` that ended by itself, unless another
    /// replaced it.
    pub(crate) fn ended(&self, account: &str, generation: u64) {
        let mut accounts = self.accounts();
        if accounts
            .get(account)
            .is_some_and(|r| r.generation == generation)
        {
            accounts.remove(account);
        }
    }

    /// Send `control` to `account`'s run, if it is live.
    pub(crate) fn control(&self, account: &str, control: Control) {
        if let Some(running) = self.accounts().get(account) {
            let _ = running.control.send(control);
        }
    }
}

impl Social {
    /// The engine, reconnecting and subscribing live as `options` say.
    pub fn with_live_options(mut self, options: LiveOptions) -> Self {
        self.live = Arc::new(Registry::new(options));
        self
    }

    /// What happens live, from now on. A listener that falls behind loses
    /// the oldest events (they are in the store all the same).
    pub fn live_events(&self) -> broadcast::Receiver<LiveEvent> {
        self.live.events.subscribe()
    }

    /// Keep `actor`'s account connected until [`Social::go_offline`], or
    /// until it must sign in again. Nothing happens if it is live already.
    /// An account that must sign in again is refused with the reason.
    ///
    /// The sessions run on the Tokio runtime and hold the engine: they end
    /// on [`Social::go_offline`] (or with the runtime), not when the
    /// caller's `Social` is dropped.
    pub async fn go_live(&self, actor: &str) -> SocialResult<()> {
        let actor = parse_actor(actor)?;
        let key = social_account_key(&actor.to_string());
        if self.live.accounts().contains_key(&key) {
            return Ok(());
        }
        let account = self.account(&actor).await?;
        let identity = self.identity(&account).await?;
        let mut accounts = self.live.accounts();
        if accounts.contains_key(&key) {
            return Ok(());
        }
        let generation = self.live.generation.fetch_add(1, Ordering::Relaxed);
        let (control, controls) = mpsc::unbounded_channel();
        let status = Status::default();
        let task = tokio::spawn(account::run(account::Account {
            social: self.clone(),
            identity,
            key: key.clone(),
            generation,
            control: control.clone(),
            controls,
            status: Arc::clone(&status),
        }));
        accounts.insert(
            key,
            Running {
                generation,
                control,
                task,
                status,
            },
        );
        Ok(())
    }

    /// Close `actor`'s sessions, and wait until they are closed.
    pub async fn go_offline(&self, actor: &str) -> SocialResult<()> {
        self.stop_live(&parse_actor(actor)?).await;
        Ok(())
    }

    /// Stop `actor`'s run and wait for it; whether it was live.
    pub(crate) async fn stop_live(&self, actor: &Actor) -> bool {
        let running = self
            .live
            .accounts()
            .remove(&social_account_key(&actor.to_string()));
        let Some(running) = running else {
            return false;
        };
        let _ = running.control.send(Control::Stop);
        if let Err(error) = running.task.await {
            tracing::warn!(%actor, %error, "a live account's task failed");
        }
        true
    }

    /// Keep every account connected that can be, and from now on each
    /// account added or signed in again too. An account that must sign in
    /// again is left offline until it does.
    pub async fn keep_connected(&self) -> SocialResult<()> {
        self.live.auto.store(true, Ordering::Relaxed);
        let accounts = self.blocking(|store| store.social_accounts()).await?;
        for account in accounts {
            if account.state == SocialAccountState::NeedsSignIn {
                continue;
            }
            if let Err(error) = self.go_live(&account.actor).await {
                tracing::warn!(actor = %account.actor, %error, "not connected");
            }
        }
        Ok(())
    }

    /// Bring `actor` live if [`Social::keep_connected`] asked for every
    /// account; a failure is only logged.
    pub(crate) async fn live_if_kept(&self, actor: &str) {
        if self.live.auto.load(Ordering::Relaxed)
            && let Err(error) = self.go_live(actor).await
        {
            tracing::warn!(%actor, %error, "not connected");
        }
    }

    /// `actor`'s sessions and whether each is connected; none when it is
    /// not live.
    pub fn sessions(&self, actor: &str) -> Vec<SessionStatus> {
        let Ok(actor) = parse_actor(actor) else {
            return Vec::new();
        };
        let accounts = self.live.accounts();
        let Some(running) = accounts.get(&social_account_key(&actor.to_string())) else {
            return Vec::new();
        };
        let status = running.status.lock().unwrap_or_else(|p| p.into_inner());
        status
            .iter()
            .map(|(provider, &connected)| SessionStatus {
                provider: provider.clone(),
                connected,
            })
            .collect()
    }
}
