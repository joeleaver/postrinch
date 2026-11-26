//! Starting sync: the [`MailSync`] façade over every account, Google
//! accounts' over the Gmail API when Google sign-in is set up in this copy
//! of Postrinch, IMAP accounts' always, kept in sync and sending on a
//! thread of its own.

use std::sync::Arc;
use std::thread;

use postrinch_model::{EmailAddress, Provider};
use postrinch_store::Store;
use postrinch_sync::google::{ClientConfig, GmailApi, GmailSync, GoogleAuth};
use postrinch_sync::imap::{Connector, ImapOAuth, ImapSecrets, ImapSync, OAuthClients, TlsConfig};
use postrinch_sync::{KeepOptions, MailSync, SyncEvent, SyncResult};
use postrinch_sync_core::Keychain;
use rinch::prelude::*;
use tokio::runtime::Handle;
use tokio::sync::Notify;

use crate::sync::{AccountSync, PassEnd, report};

/// What the window needs to act on accounts while sync runs.
#[derive(Clone)]
pub struct SyncHandle {
    /// Google sign-in; none when it is not set up in this copy, which
    /// turns off Google accounts only.
    pub auth: Option<Arc<GoogleAuth>>,
    /// IMAP accounts' browser sign-ins, which also give OAuth accounts
    /// their access tokens.
    pub imap_oauth: Arc<ImapOAuth>,
    pub store: Store,
    /// The runtime sync runs on, for sign-ins and removals.
    pub runtime: Handle,
    /// Starts sync's next pass at once.
    pub wake: Arc<Notify>,
    /// Sends the changes made here at once.
    pub send: Arc<Notify>,
    mail: MailSync,
}

impl SyncHandle {
    /// The sync for every account, sending each call to the account's
    /// provider.
    pub fn mail(&self) -> &MailSync {
        &self.mail
    }

    /// Wakes both of sync's loops, after an account signed in or was
    /// removed: its mail comes at once, and what waited to be sent goes
    /// (an account that had to sign in sent nothing meanwhile); a removed
    /// account's loops end rather than wait out their turn.
    pub fn accounts_changed(&self) {
        wake_both(&self.wake, &self.send);
    }

    /// Runs `call` on the account's Google Calendar on sync's runtime,
    /// waiting for it, from a thread of the window's own (never one of the
    /// runtime's). An IMAP account has no calendar
    /// ([`postrinch_sync::SyncError::NoCalendars`]).
    pub fn on_calendar<T>(
        &self,
        account: &EmailAddress,
        call: impl AsyncFnOnce(&GmailSync) -> SyncResult<T>,
    ) -> SyncResult<T> {
        self.runtime
            .block_on(async { call(self.mail.calendar(account).await?).await })
    }
}

/// Why sync, or a part of it, is not running: a sentence for the
/// sidebar, and the detail for the log.
#[derive(Debug)]
pub struct SyncOff {
    pub said: &'static str,
    pub detail: String,
}

impl SyncOff {
    fn new(said: &'static str, detail: impl Into<String>) -> Self {
        Self {
            said,
            detail: detail.into(),
        }
    }

    fn not_started(detail: impl Into<String>) -> Self {
        Self::new("Sync is off: it could not start.", detail)
    }
}

/// Start keeping every account in sync, and sending the changes made here,
/// on a thread of its own. `status` follows what each account's sync is
/// doing; `revision` is bumped when the store changed; `ended` names each
/// account whose pass ended (`report`). Fails, saying why,
/// when sync cannot run at all; with it running, says why Google accounts
/// are off when this copy has no Google sign-in and the store has Google
/// accounts to miss it.
pub fn start(
    store: Store,
    status: Signal<Vec<AccountSync>>,
    revision: Signal<u64>,
    ended: Signal<Option<PassEnd>>,
) -> Result<(SyncHandle, Option<SyncOff>), SyncOff> {
    let (auth, google_off) = match google() {
        Ok(auth) => (Some(auth), None),
        Err(off) => {
            let has_google = store
                .accounts()
                .map(|all| all.iter().any(|a| a.provider == Provider::Google))
                .unwrap_or(true);
            (None, has_google.then_some(off))
        }
    };
    let gmail = auth
        .clone()
        .map(|auth| GmailSync::new(Arc::new(GmailApi::new(auth)), store.clone()));
    let imap_oauth = ImapOAuth::new(
        OAuthClients::load(),
        ImapSecrets::keychain(),
        Arc::new(Keychain::GOOGLE),
    )
    .map_err(|e| SyncOff::not_started(format!("IMAP sign-in: {e}")))?;
    let imap_oauth = Arc::new(imap_oauth);
    let tls = TlsConfig::platform().map_err(|e| SyncOff::not_started(format!("TLS: {e}")))?;
    let imap = ImapSync::new(
        store.clone(),
        Connector::new(tls),
        ImapSecrets::keychain(),
        imap_oauth.clone(),
    );
    let mail = MailSync::new(store.clone(), gmail, imap);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| SyncOff::not_started(format!("its runtime: {e}")))?;
    let (wake, send) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let fetch = wake.clone();
    let events = Arc::new(move |event: SyncEvent| report(event, status, revision, ended, &fetch));
    let handle = SyncHandle {
        auth,
        imap_oauth,
        store,
        runtime: runtime.handle().clone(),
        wake: wake.clone(),
        send: send.clone(),
        mail: mail.clone(),
    };
    let options = KeepOptions::default();
    thread::Builder::new()
        .name("sync".into())
        .spawn(move || {
            runtime.block_on(async {
                tokio::join!(
                    mail.keep_in_sync(options, events.clone(), wake),
                    mail.keep_sending(options, events, send),
                )
            })
        })
        .map_err(|e| SyncOff::not_started(format!("its thread: {e}")))?;
    Ok((handle, google_off))
}

/// Google sign-in, with this computer's OAuth client file or else the
/// client this build carries ([`ClientConfig::find`]).
fn google() -> Result<Arc<GoogleAuth>, SyncOff> {
    let path = ClientConfig::default_path();
    let config = match ClientConfig::find(path.as_deref()) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return Err(SyncOff::new(
                "Google accounts are off: Google sign-in is not set up on this computer.",
                "no client file and no built-in client",
            ));
        }
        Err(e) => {
            return Err(SyncOff::new(
                "Google accounts are off: the Google sign-in settings could not be read.",
                e.to_string(),
            ));
        }
    };
    let auth = GoogleAuth::new(config, Arc::new(Keychain::GOOGLE), ImapSecrets::keychain())
        .map_err(|e| {
            SyncOff::new(
                "Google accounts are off: Google sign-in could not start.",
                format!("sign-in: {e}"),
            )
        })?;
    Ok(Arc::new(auth))
}

/// Wakes the sync loop and the sending loop.
fn wake_both(wake: &Notify, send: &Notify) {
    wake.notify_one();
    send.notify_one();
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn an_account_change_wakes_both_loops() {
        let (wake, send) = (Notify::new(), Notify::new());
        wake_both(&wake, &send);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let both = async { tokio::join!(wake.notified(), send.notified()) };
            tokio::time::timeout(Duration::from_secs(5), both)
                .await
                .expect("both loops are woken");
        });
    }
}
