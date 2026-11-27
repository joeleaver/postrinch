//! One live account: a session per provider that hosts something it
//! keeps (docs/ofscp.md, "Sessions"), started and stopped as its groups
//! and follows change.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use postrinch_ofscp::Identity;
use postrinch_ofscp::ofscp_core::Authority;
use postrinch_store::{PlaceKind, SocialAccountState};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::session::{self, SessionCmd};
use super::{LiveEvent, Status};
use crate::uris::{parse_channel_uri, parse_group_uri};
use crate::{Social, SocialResult};

/// What an account's run is told, by the engine or by its sessions.
#[derive(Debug, Clone)]
pub(crate) enum Control {
    /// Close every session and end.
    Stop,
    /// The home session read the groups and places again: start and stop
    /// sessions to match, and have each subscribe what it now hosts.
    Refreshed,
    /// The account can no longer sign: end, saying why.
    Unusable(SocialAccountState),
    /// The people whose presence is watched changed.
    Watch,
}

/// An account's run, as [`Social::go_live`] starts it.
pub(crate) struct Account {
    pub social: Social,
    pub identity: Arc<Identity>,
    /// The account's store key.
    pub key: String,
    pub generation: u64,
    /// For its sessions to reach the run.
    pub control: mpsc::UnboundedSender<Control>,
    pub controls: mpsc::UnboundedReceiver<Control>,
    pub status: Status,
}

/// A running session.
struct Session {
    cmds: mpsc::UnboundedSender<SessionCmd>,
    task: JoinHandle<()>,
}

/// Run `account` until it is stopped or can no longer sign.
pub(crate) async fn run(mut account: Account) {
    let mut sessions: BTreeMap<Authority, Session> = BTreeMap::new();
    let mut unusable = None;
    if let Err(error) = reconcile(&account, &mut sessions).await {
        tracing::warn!(actor = %account.identity.actor(), %error, "cannot read the providers");
    }
    while let Some(control) = account.controls.recv().await {
        match control {
            Control::Stop => break,
            Control::Unusable(state) => {
                unusable = Some(state);
                break;
            }
            Control::Refreshed => {
                if let Err(error) = reconcile(&account, &mut sessions).await {
                    tracing::warn!(actor = %account.identity.actor(), %error, "cannot read the providers");
                }
                for session in sessions.values() {
                    let _ = session.cmds.send(SessionCmd::Resubscribe);
                }
            }
            Control::Watch => {
                for session in sessions.values() {
                    let _ = session.cmds.send(SessionCmd::Presence);
                }
            }
        }
    }
    for (authority, session) in std::mem::take(&mut sessions) {
        stop(&account, &authority, session).await;
    }
    let actor = account.identity.actor().to_string();
    account.social.live.presences.forget(&account.key);
    // No longer live before saying why, so whoever acts on it (signing in
    // again) can make it live again.
    account.social.live.ended(&account.key, account.generation);
    if let Some(state) = unusable {
        account.social.live.tell(LiveEvent::AccountState {
            account: actor,
            state,
        });
    }
}

/// Start a session for each provider the account needs and has none for,
/// and stop those it no longer needs.
async fn reconcile(
    account: &Account,
    sessions: &mut BTreeMap<Authority, Session>,
) -> SocialResult<()> {
    let wanted = providers(account).await?;
    let gone: Vec<Authority> = sessions
        .keys()
        .filter(|a| !wanted.contains(*a))
        .cloned()
        .collect();
    for authority in gone {
        if let Some(session) = sessions.remove(&authority) {
            stop(account, &authority, session).await;
        }
    }
    for authority in wanted {
        if sessions.contains_key(&authority) {
            continue;
        }
        let (cmds, commands) = mpsc::unbounded_channel();
        let context = session::Context {
            social: account.social.clone(),
            identity: Arc::clone(&account.identity),
            key: account.key.clone(),
            authority: authority.clone(),
            control: account.control.clone(),
            status: Arc::clone(&account.status),
        };
        account.set_status(&authority, Some(false));
        let task = tokio::spawn(session::run(context, commands));
        sessions.insert(authority, Session { cmds, task });
    }
    Ok(())
}

/// Stop a session and wait for it.
async fn stop(account: &Account, authority: &Authority, session: Session) {
    let _ = session.cmds.send(SessionCmd::Stop);
    if let Err(error) = session.task.await {
        tracing::warn!(%authority, %error, "a session's task failed");
    }
    account.set_status(authority, None);
}

/// The providers `account` keeps something on: home, and the providers
/// of its groups and of its channels (followed ones among them).
async fn providers(account: &Account) -> SocialResult<BTreeSet<Authority>> {
    let name = account.identity.actor().to_string();
    let (groups, places) = account
        .social
        .blocking(move |store| Ok((store.social_groups(&name)?, store.social_places(&name)?)))
        .await?;
    let mut wanted = BTreeSet::new();
    wanted.insert(account.identity.actor().domain().clone());
    wanted.extend(
        groups
            .iter()
            .filter_map(|g| parse_group_uri(&g.uri).map(|(authority, _)| authority)),
    );
    wanted.extend(
        places
            .iter()
            .filter(|p| p.kind == PlaceKind::Channel)
            .filter_map(|p| parse_channel_uri(&p.id).map(|at| at.authority)),
    );
    Ok(wanted)
}

impl Account {
    /// Note whether the session with `authority` is connected; `None` when
    /// there is none.
    fn set_status(&self, authority: &Authority, connected: Option<bool>) {
        let mut status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        match connected {
            Some(connected) => status.insert(authority.to_string(), connected),
            None => status.remove(&authority.to_string()),
        };
    }
}
