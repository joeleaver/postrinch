//! Adding an IMAP account, signing one in again in the browser, and
//! removing one (docs/imap.md, "Adding an account"). The add-account
//! dialog (IMAP 10) and `postrinch-cli add-imap` both add through
//! [`ImapSync::add_account`]: nothing is stored until both servers have
//! passed the check, and a password or OAuth grant is kept only then, so
//! a failed check leaves an account already here exactly as it was.

use std::time::Duration;

use postrinch_model::{Account, EmailAddress, ImapSettings, Provider};

use super::ImapSync;
use crate::imap::{AccountCheck, ImapOAuth, ImapSecret, OAuthProvider, check_account};
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
    /// back.
    OAuth {
        provider: OAuthProvider,
        open_browser: OpenBrowser<'a>,
    },
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
            } => {
                let grant = oauth.grant(provider, &address, open_browser).await?;
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

        // The secret the account had, if any, to give back once replaced.
        let replaced = self.secrets.get(&address).await.ok();
        match grant {
            Some(grant) => {
                oauth.keep(grant).await?;
            }
            None => self.secrets.set(&address, &secret).await?,
        }
        // Connections made with the old settings or secret go, and so does
        // any wait for a server that was busy.
        self.disconnect(&address).await;
        self.busy.forget(&address);
        let a = address.clone();
        let account = self
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
            .await?;
        if let Some(replaced) = replaced {
            oauth.retire(&address, &replaced, &secret).await;
        }
        Ok(Added::Account(Box::new(account)))
    }

    /// Signs an OAuth account in again in the browser ("Sign in again" on
    /// one that has to), and marks it signed in. An account that signs in
    /// with a password has no browser sign-in: it gives its password
    /// again in the account's settings.
    pub async fn sign_in_again(
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
        self.blocking(move |s| {
            let mut stored = s
                .account(&a)?
                .ok_or_else(|| SyncError::Config(format!("{a} is not an account here")))?;
            stored.signed_in();
            s.put_account(&stored)?;
            Ok(stored)
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
