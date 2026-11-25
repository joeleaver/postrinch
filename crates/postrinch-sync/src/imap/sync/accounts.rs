//! Adding an IMAP account, signing one in again in the browser, and
//! removing one (docs/imap.md, "Adding an account"). The add-account
//! dialog (IMAP 10) and `postrinch-cli add-imap` both add through
//! [`ImapSync::add_account`]: nothing is stored until both servers have
//! passed the check, and a password is kept only then.

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
    /// sign-in sets its own.
    pub settings: ImapSettings,
}

/// How a new account signs in.
pub enum NewSignIn<'a> {
    /// With a password (and SMTP's own, when it has one): checked first,
    /// kept in the keychain only once both servers took it.
    Password(ImapSecret),
    /// In the browser, with `provider`'s OAuth; the grant is kept by the
    /// sign-in, and given back if the check then fails for an account
    /// that was not here before.
    OAuth {
        oauth: &'a ImapOAuth,
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
    /// A server failed the check: nothing was stored or kept. Each
    /// server's outcome says what it said.
    CheckFailed(AccountCheck),
}

impl ImapSync {
    /// Adds an IMAP account, or changes the settings of one already here:
    /// signs in (in the browser, for OAuth), checks both servers with the
    /// secret, and only then keeps the secret and stores the account.
    ///
    /// An address that is a Google account here is refused first
    /// ([`SyncError::AlreadyAdded`]), before anything opens.
    pub async fn add_account(
        &self,
        new: NewImapAccount,
        sign_in: NewSignIn<'_>,
    ) -> SyncResult<Added> {
        let NewImapAccount {
            address,
            mut display_name,
            mut settings,
        } = new;
        check_address_free(&self.store, &address, Provider::Imap).await?;
        let a = address.clone();
        let before = self.with_store(move |s| s.account(&a)).await?;
        match sign_in {
            NewSignIn::Password(secret) => {
                let check =
                    check_account(&self.connector, &address, &settings, &secret, &*self.tokens)
                        .await;
                if !check.is_ok() {
                    return Ok(Added::CheckFailed(check));
                }
                self.secrets.set(&address, &secret).await?;
            }
            NewSignIn::OAuth {
                oauth,
                provider,
                open_browser,
            } => {
                let signed = oauth.sign_in(provider, &address, open_browser).await?;
                settings.sign_in = signed.method;
                display_name = display_name.or(signed.display_name);
                let secret = self.secrets.get(&address).await?;
                let check =
                    check_account(&self.connector, &address, &settings, &secret, oauth).await;
                if !check.is_ok() {
                    // The grant was made for an account that is not to be;
                    // one already here keeps its new grant, which works.
                    if before.is_none()
                        && let Err(e) = oauth.sign_out(&address).await
                    {
                        tracing::warn!("{address}: giving the unused grant back failed: {e}");
                    }
                    return Ok(Added::CheckFailed(check));
                }
            }
        }
        // Connections made with the old settings or secret go.
        self.disconnect(&address).await;
        let mut account = before.unwrap_or_else(|| Account::new(address.clone(), Provider::Imap));
        account.display_name = display_name.or(account.display_name);
        account.imap = Some(settings);
        account.signed_in();
        let stored = account.clone();
        self.with_store(move |s| s.put_account(&stored)).await?;
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
    /// of its connections, signs it out (an OAuth grant given back where
    /// the provider takes it back) and forgets its secret, then removes
    /// it from the store. Returns how many objects went.
    pub async fn remove_account(
        &self,
        oauth: &ImapOAuth,
        account: &EmailAddress,
    ) -> SyncResult<usize> {
        self.disconnect(account).await;
        oauth.sign_out(account).await?;
        let a = account.clone();
        self.with_store(move |s| {
            s.remove_account(&a)?;
            s.collect_garbage(Duration::ZERO)
        })
        .await
    }
}
