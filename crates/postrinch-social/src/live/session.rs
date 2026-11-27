//! One session: an account's socket to one provider (§7.1), connected
//! again and again until it is stopped or the key is refused for good.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use postrinch_ofscp::Identity;
use postrinch_ofscp::http::Client;
use postrinch_ofscp::ofscp_core::{Actor, Authority, LocalId};
use postrinch_ofscp::ws::{self, CloseReason, ConnectError, Connection, Incoming};
use postrinch_store::SocialAccountState;
use postrinch_sync_core::Backoff;
use tokio::sync::mpsc;
use tokio::time::Instant;

use super::subscribe::Scope;
use super::{Control, LiveEvent, Status, notices};
use crate::sync::Pages;
use crate::{Social, SocialError, SocialResult};

/// What a session is told by its account's run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionCmd {
    /// Subscribe what the provider now hosts for the account, and drop
    /// what it no longer does.
    Resubscribe,
    /// Watch the presence of the people now watched.
    Presence,
    /// Close the socket and end.
    Stop,
}

/// Who a session is for, and where.
pub(crate) struct Context {
    pub social: Social,
    pub identity: Arc<Identity>,
    /// The account's store key.
    pub key: String,
    pub authority: Authority,
    pub control: mpsc::UnboundedSender<Control>,
    pub status: Status,
}

/// How a connection ended, and what follows.
pub(crate) enum End {
    /// Asked to stop, or the account is gone.
    Stop,
    /// The account can no longer sign.
    Unusable(SocialAccountState),
    /// Try again after a wait.
    Retry(String),
}

impl From<SocialError> for End {
    fn from(error: SocialError) -> Self {
        match error {
            SocialError::Unusable { state, .. } => End::Unusable(state),
            SocialError::NoAccount(_) => End::Stop,
            other => End::Retry(other.to_string()),
        }
    }
}

/// A signed-in connection and what is subscribed on it.
pub(crate) struct Live<'a> {
    pub cx: &'a Context,
    pub conn: Connection,
    /// The places subscribed, by the id the provider knows them by (a
    /// channel's local id, or a dmId).
    pub subscribed: BTreeMap<LocalId, Scope>,
    /// Places the provider took away (`unsubscribed`, `access_revoked`):
    /// not subscribed again until the groups are read again (§7.1).
    pub revoked: HashSet<LocalId>,
    /// The people whose presence is subscribed on this connection.
    pub watching: BTreeSet<Actor>,
}

/// Run a session until it is stopped or the account can no longer sign.
pub(crate) async fn run(cx: Context, mut cmds: mpsc::UnboundedReceiver<SessionCmd>) {
    let options = cx.social.live.options.clone();
    let mut backoff = Backoff::with(options.first_backoff, options.max_backoff);
    loop {
        let end = cx.connected(&mut cmds, &mut backoff).await;
        cx.set_connected(false);
        cx.social.live.presences.lost(&cx.key, &cx.authority);
        let (reason, retry_in) = match end {
            End::Stop => ("closed by this client".to_owned(), None),
            End::Unusable(state) => {
                let _ = cx.control.send(Control::Unusable(state));
                (format!("the account cannot sign ({state:?})"), None)
            }
            End::Retry(reason) => (reason, Some(backoff.next_delay())),
        };
        tracing::info!(actor = %cx.identity.actor(), provider = %cx.authority, %reason, ?retry_in, "session ended");
        cx.social.live.tell(LiveEvent::Disconnected {
            account: cx.identity.actor().to_string(),
            provider: cx.authority.to_string(),
            reason,
            retry_in,
        });
        let Some(wait) = retry_in else { return };
        let until = Instant::now() + wait;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(until) => break,
                cmd = cmds.recv() => match cmd {
                    // What is subscribed and watched is worked out anew on
                    // connecting, so other commands can wait.
                    Some(SessionCmd::Resubscribe | SessionCmd::Presence) => {}
                    Some(SessionCmd::Stop) | None => return,
                },
            }
        }
    }
}

impl Context {
    pub(crate) fn is_home(&self) -> bool {
        self.authority == *self.identity.actor().domain()
    }

    /// The account's actor, as stored.
    pub(crate) fn actor(&self) -> String {
        self.identity.actor().to_string()
    }

    /// The signed client for this session's provider.
    pub(crate) fn client(&self) -> Client {
        self.social.client(&self.identity, &self.authority)
    }

    /// The signed client for the home provider.
    pub(crate) fn home(&self) -> Client {
        self.social
            .client(&self.identity, self.identity.actor().domain())
    }

    /// A signed request's answer: a signature home refuses marks the
    /// account.
    pub(crate) async fn checked<T>(
        &self,
        client: &Client,
        result: postrinch_ofscp::Result<T>,
    ) -> SocialResult<T> {
        self.social.checked(&self.identity, client, result).await
    }

    pub(crate) fn tell(&self, event: LiveEvent) {
        self.social.live.tell(event);
    }

    fn set_connected(&self, connected: bool) {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(now) = status.get_mut(&self.authority.to_string()) {
            *now = connected;
        }
    }

    /// Connect, catch up, and keep what arrives until the connection ends.
    async fn connected(
        &self,
        cmds: &mut mpsc::UnboundedReceiver<SessionCmd>,
        backoff: &mut Backoff,
    ) -> End {
        let options = self.social.live.options.ws.clone();
        let (conn, mut events) = match ws::connect(&self.client(), options).await {
            Ok(pair) => pair,
            Err(ConnectError::Refused {
                close: CloseReason::Revoked,
                ..
            }) => return self.revoked().await,
            Err(error) => return End::Retry(error.to_string()),
        };
        self.set_connected(true);
        self.tell(LiveEvent::Connected {
            account: self.actor(),
            provider: self.authority.to_string(),
        });
        let mut live = Live {
            cx: self,
            conn: conn.clone(),
            subscribed: BTreeMap::new(),
            revoked: HashSet::new(),
            watching: BTreeSet::new(),
        };
        if let Err(end) = live.catch_up().await {
            conn.close().await;
            return end;
        }
        backoff.reset();
        loop {
            tokio::select! {
                incoming = events.recv() => match incoming {
                    Some(Incoming::Event(envelope)) => {
                        if let Err(end) = live.handle(envelope.body).await {
                            conn.close().await;
                            return end;
                        }
                    }
                    Some(Incoming::Closed(reason)) => return self.closed(reason).await,
                    None => return self.closed(conn.close_reason().unwrap_or(CloseReason::Client)).await,
                },
                cmd = cmds.recv() => {
                    let done = match cmd {
                        Some(SessionCmd::Resubscribe) => live.resubscribe().await.map(drop),
                        Some(SessionCmd::Presence) => live.watch().await,
                        Some(SessionCmd::Stop) | None => {
                            conn.close().await;
                            return End::Stop;
                        }
                    };
                    if let Err(error) = done {
                        conn.close().await;
                        return error.into();
                    }
                }
            }
        }
    }

    /// What a closed connection means.
    async fn closed(&self, reason: CloseReason) -> End {
        match reason {
            CloseReason::Revoked => self.revoked().await,
            other => End::Retry(other.to_string()),
        }
    }

    /// The provider closed with `4003`: the key is revoked or the account
    /// gone (§4.7, §7.1). Home's word is final: the account must sign in
    /// again. Another provider's is checked with home, the key's only
    /// judge (§4.6); while home still takes the key, the session tries
    /// again later.
    async fn revoked(&self) -> End {
        let actor = self.identity.actor();
        if self.is_home() {
            let key_id = Some(self.identity.key_id().to_owned());
            let state = SocialAccountState::NeedsSignIn;
            let otherwise = SocialError::Unusable {
                actor: actor.to_string(),
                state,
            };
            return match self.social.mark(actor, key_id, state, otherwise).await {
                Ok(_) => End::Unusable(state),
                Err(error) => End::Retry(error.to_string()),
            };
        }
        let home = self.home();
        match self.checked(&home, home.me().await).await {
            Ok(_) => End::Retry(format!("{} refused the key (4003)", self.authority)),
            Err(error) => error.into(),
        }
    }
}

impl Live<'_> {
    /// Catch up after connecting: home reads again what nothing replays;
    /// then every place is subscribed from its cursor, and what cannot be
    /// replayed is backfilled.
    async fn catch_up(&mut self) -> Result<(), End> {
        if self.cx.is_home() {
            self.refresh().await?;
            notices::catch_up(self.cx).await?;
        }
        let backfilled = self.resubscribe().await?;
        self.watch().await?;
        self.cx.tell(LiveEvent::CaughtUp {
            account: self.cx.actor(),
            provider: self.cx.authority.to_string(),
            backfilled,
        });
        Ok(())
    }

    /// Read the account's groups, places, private conversations, read
    /// markers and follows again (docs/ofscp.md: after every reconnect),
    /// and have every session subscribe what it now hosts. Places with a
    /// cursor are left to their subscriptions.
    pub(crate) async fn refresh(&mut self) -> Result<(), End> {
        let social = &self.cx.social;
        {
            let _held = social.hold(self.cx.identity.actor()).await;
            let account = social.account(self.cx.identity.actor()).await?;
            social
                .sync_as(&account, &self.cx.identity, Pages::Unresumable)
                .await?;
        }
        self.revoked.clear();
        self.cx.tell(LiveEvent::Groups {
            account: self.cx.actor(),
        });
        let _ = self.cx.control.send(Control::Refreshed);
        Ok(())
    }
}
