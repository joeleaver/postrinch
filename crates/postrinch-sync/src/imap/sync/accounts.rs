//! Adding an IMAP account, signing one in again in the browser, and
//! removing one (docs/imap.md, "Adding an account"). The add-account
//! dialog and `postrinch-cli add-imap` both add through
//! [`ImapSync::add_account`]: nothing is stored until both servers have
//! passed the check, and a password or OAuth grant is kept only then, so
//! a failed check leaves an account already here exactly as it was.

use std::time::Duration;

use postrinch_model::{Account, EmailAddress, ImapSettings, Provider};
use tokio::sync::oneshot;

use super::ImapSync;
use crate::imap::{AccountCheck, ImapOAuth, ImapSecret, OAuthGrant, OAuthProvider, check_account};
use crate::{SyncError, SyncResult, check_address_free};

/// What the browser is asked to open, for an OAuth sign-in.
pub type OpenBrowser<'a> = Box<dyn FnOnce(&str) -> std::io::Result<()> + Send + 'a>;

/// A new IMAP account, as the person gave it or discovery found it.
#[derive(Debug, Clone)]
pub struct NewImapAccount {
    pub address: EmailAddress,
    /// The person's name on mail they send; for an OAuth account, the
    /// provider's name for them when this is none.
    pub display_name: Option<String>,
    /// The servers. Its `sign_in` is the password kind for
    /// [`NewSignIn::Password`] (one password, or SMTP's own); an OAuth
    /// sign-in sets its own. For an account already here, its folder
    /// roles and whether the provider files sent mail are not used: the
    /// stored ones stay.
    pub settings: ImapSettings,
}

/// How a new account signs in.
pub enum NewSignIn<'a> {
    /// With a password (and SMTP's own, when it has one): checked first,
    /// kept in the keychain only once both servers took it.
    Password(ImapSecret),
    /// In the browser, with `provider`'s OAuth: the grant is checked
    /// first, kept only once both servers took it, and otherwise given
    /// back. With `cancel`, the person can stop waiting for the browser.
    OAuth {
        provider: OAuthProvider,
        open_browser: OpenBrowser<'a>,
        cancel: Option<SignInCancel>,
    },
}

/// Lets the person stop an OAuth sign-in while its browser is open
/// ([`NewSignIn::OAuth`]): made with [`sign_in_cancel`].
pub struct SignInCancel(oneshot::Receiver<()>);

/// Stops the sign-in its [`SignInCancel`] belongs to.
pub struct SignInCanceller(oneshot::Sender<()>);

/// A way to stop a browser sign-in, and the sign-in's side of it.
pub fn sign_in_cancel() -> (SignInCanceller, SignInCancel) {
    let (tx, rx) = oneshot::channel();
    (SignInCanceller(tx), SignInCancel(rx))
}

impl SignInCanceller {
    /// Stops the wait for the browser. Once the browser has come back
    /// the sign-in goes on: its servers are being checked, and stopping
    /// then would leave the keychain half-changed.
    pub fn cancel(self) {
        // The sign-in may be over already, and so not listening.
        let _ = self.0.send(());
    }
}

impl SignInCancel {
    /// Ends when the person cancels; never, once its canceller is gone
    /// without cancelling.
    async fn cancelled(self) {
        if self.0.await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// How adding an account went.
#[derive(Debug)]
pub enum Added {
    /// It is stored, signed in; sync takes it from here.
    /// (Boxed: an account is far bigger than a check.)
    Account(Box<Account>),
    /// A server failed the check: nothing was stored or kept, and an
    /// account already here is as it was. Each server's outcome says what
    /// it said.
    CheckFailed(AccountCheck),
}

impl ImapSync {
    /// Adds an IMAP account, or changes the servers and sign-in of one
    /// already here: signs in (in the browser, for OAuth), checks both
    /// servers with the secret, and only then keeps the secret and stores
    /// the account. An account already here keeps what the person chose
    /// and sync learned ([`ImapSettings::servers_over`]); a grant its new
    /// secret replaced is given back ([`ImapOAuth`]'s `retire`). Both
    /// servers just answered, so any busy wait for the account is over.
    ///
    /// When the secret cannot be kept or the account stored after the
    /// check passed, the error is returned with the account's secret put
    /// back as it was, and a new grant given back as after a failed check.
    ///
    /// An address that is a Google account here is refused first
    /// ([`SyncError::AlreadyAdded`]), before anything opens.
    pub async fn add_account(
        &self,
        oauth: &ImapOAuth,
        new: NewImapAccount,
        sign_in: NewSignIn<'_>,
    ) -> SyncResult<Added> {
        let NewImapAccount {
            address,
            mut display_name,
            mut settings,
        } = new;
        check_address_free(&self.store, &address, Provider::Imap).await?;
        let (secret, grant) = match sign_in {
            NewSignIn::Password(secret) => {
                let check =
                    check_account(&self.connector, &address, &settings, &secret, &*self.tokens)
                        .await;
                if !check.is_ok() {
                    return Ok(Added::CheckFailed(check));
                }
                (secret, None)
            }
            NewSignIn::OAuth {
                provider,
                open_browser,
                cancel,
            } => {
                let granting = oauth.grant(provider, &address, open_browser);
                let grant = match cancel {
                    // A grant made as the person cancelled is dropped
                    // unkept: unused, it lapses.
                    Some(cancel) => tokio::select! {
                        grant = granting => grant?,
                        () = cancel.cancelled() => {
                            return Err(SyncError::SignIn("the sign-in was cancelled".into()));
                        }
                    },
                    None => granting.await?,
                };
                settings.sign_in = grant.method();
                display_name = display_name.or(grant.display_name());
                let secret = grant.secret();
                let check =
                    check_account(&self.connector, &address, &settings, &secret, &grant).await;
                if !check.is_ok() {
                    oauth.give_back(grant).await;
                    return Ok(Added::CheckFailed(check));
                }
                (secret, Some(grant))
            }
        };

        // From reading the secret the account had to storing the account,
        // no refresh of its access token runs (one would keep a rotated
        // refresh token over the new secret); see `ImapOAuth::replacing`.
        let mut replacing = match oauth.replacing(&address).await {
            Ok(replacing) => replacing,
            Err(e) => return Err(abandon(oauth, grant, e).await),
        };
        if let Err(e) = replacing.write(&secret).await {
            replacing.undo().await;
            return Err(abandon(oauth, grant, e).await);
        }
        let a = address.clone();
        let stored = self
            .with_store(move |s| {
                s.change_account(&a, |before| {
                    let mut account =
                        before.unwrap_or_else(|| Account::new(a.clone(), Provider::Imap));
                    account.display_name = display_name.or(account.display_name.take());
                    account.imap = Some(match account.imap.take() {
                        Some(stored) => settings.servers_over(stored),
                        None => settings,
                    });
                    account.signed_in();
                    account
                })
            })
            .await;
        let account = match stored {
            Ok(account) => account,
            Err(e) => {
                replacing.undo().await;
                return Err(abandon(oauth, grant, e).await);
            }
        };
        let replaced = replacing.finish(grant);
        // Connections made with the old settings or secret go, and so does
        // any wait for a server that was busy.
        self.disconnect(&address).await;
        self.busy.forget(&address);
        if let Some(replaced) = replaced {
            oauth.retire(&address, &replaced, &secret).await;
        }
        Ok(Added::Account(Box::new(account)))
    }

    /// Signs an OAuth account in again in the browser, and marks it
    /// signed in ([`ImapSync::sign_in_again`] with
    /// [`super::SignInAgain::Browser`]).
    pub(super) async fn sign_in_in_browser(
        &self,
        oauth: &ImapOAuth,
        account: &EmailAddress,
        open_browser: OpenBrowser<'_>,
    ) -> SyncResult<Account> {
        let settings = self.settings(account).await?;
        oauth
            .sign_in_again(account, &settings.sign_in, open_browser)
            .await?;
        self.disconnect(account).await;
        let a = account.clone();
        // Under the store's account lock, so nothing learned meanwhile
        // (`Store::server_files_sent`) is written over.
        self.blocking(move |s| {
            s.update_account(&a, Account::signed_in)?
                .ok_or_else(|| SyncError::Config(format!("{a} is not an account here")))
        })
        .await
    }

    /// Removes an IMAP account and its mail from this computer: lets go
    /// of its connections and any busy server's wait, signs it out (an
    /// OAuth grant given back where the provider takes it back) and
    /// forgets its secret, then removes it from the store. Returns how
    /// many objects went.
    pub async fn remove_account(
        &self,
        oauth: &ImapOAuth,
        account: &EmailAddress,
    ) -> SyncResult<usize> {
        self.disconnect(account).await;
        self.busy.forget(account);
        oauth.sign_out(account).await?;
        let a = account.clone();
        self.with_store(move |s| {
            s.remove_account(&a)?;
            s.collect_garbage(Duration::ZERO)
        })
        .await
    }
}

/// A checked secret that could not be kept, with the account left as it
/// was: its grant, if it was one, is given back under the same rule as
/// after a failed check ([`ImapOAuth`]'s `give_back`). Returns `error`.
async fn abandon(oauth: &ImapOAuth, grant: Option<OAuthGrant>, error: SyncError) -> SyncError {
    if let Some(grant) = grant {
        oauth.give_back(grant).await;
    }
    error
}
