//! An IMAP account's settings as its settings page changes them
//! (docs/imap.md, "Account settings"): its servers, checked before they
//! are kept; signing in again, with a password or in the browser; which
//! folder plays each role; and whether the provider files sent mail.

use postrinch_model::{Account, EmailAddress, FolderRole, FolderRoles, ImapSettings, SignInMethod};

use super::ImapSync;
use super::accounts::{Added, NewImapAccount, NewSignIn, OpenBrowser};
use super::folders::shown_name;
use crate::imap::{ImapOAuth, ImapSecret, check_account};
use crate::{SyncError, SyncResult};

/// How an account signs in again.
pub enum SignInAgain<'a> {
    /// In the browser, for an account that signs in with OAuth.
    Browser(OpenBrowser<'a>),
    /// With its password (and SMTP's own, when it has one), for an
    /// account that signs in with one: checked against both servers
    /// before it is kept.
    Password(ImapSecret),
}

/// One of an account's folders, as its settings page offers them for a
/// role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountFolder {
    /// Its name as the server spells it: what [`FolderRoles`] keeps.
    pub name: String,
    /// Its name as the person reads it, `/` between levels.
    pub shown: String,
    /// The role sync gave it when it last listed the folders.
    pub role: Option<FolderRole>,
}

impl ImapSync {
    /// Changes the servers of an account already here, signing in with
    /// the secret it keeps: both servers are checked first, and nothing
    /// changes when one fails. The sign-in method stays the stored one,
    /// and so does everything sync learned or the person chose apart from
    /// the servers ([`ImapSettings::servers_over`]).
    pub async fn change_servers(
        &self,
        account: &EmailAddress,
        mut settings: ImapSettings,
    ) -> SyncResult<Added> {
        let stored = self.settings(account).await?;
        settings.sign_in = stored.sign_in;
        let secret = self.secrets.get(account).await?;
        let check =
            check_account(&self.connector, account, &settings, &secret, &*self.tokens).await;
        if !check.is_ok() {
            return Ok(Added::CheckFailed(check));
        }
        let a = account.clone();
        let changed = self
            .blocking(move |s| {
                s.update_account(&a, |account| {
                    if let Some(stored) = account.imap.take() {
                        account.imap = Some(settings.servers_over(stored));
                    }
                })?
                .ok_or_else(|| SyncError::Config(format!("{a} is not an account here")))
            })
            .await?;
        // Connections made with the old servers go.
        self.disconnect(account).await;
        self.busy.forget(account);
        Ok(Added::Account(Box::new(changed)))
    }

    /// Signs an account in again ("Sign in again" on one that has to): in
    /// the browser, for OAuth, or with a password checked against both
    /// servers and kept only once both took it ([`Self::add_account`],
    /// with the stored servers). The way must be the account's own: a
    /// password for an OAuth account, or the browser for a password one,
    /// is refused with [`SyncError::Config`].
    pub async fn sign_in_again(
        &self,
        oauth: &ImapOAuth,
        account: &EmailAddress,
        how: SignInAgain<'_>,
    ) -> SyncResult<Added> {
        let settings = self.settings(account).await?;
        match (how, &settings.sign_in) {
            (SignInAgain::Browser(open_browser), SignInMethod::OAuth2 { .. }) => {
                let account = self
                    .sign_in_in_browser(oauth, account, open_browser)
                    .await?;
                Ok(Added::Account(Box::new(account)))
            }
            (SignInAgain::Password(secret), SignInMethod::Password)
            | (SignInAgain::Password(secret), SignInMethod::SeparateSmtpPassword) => {
                let new = NewImapAccount {
                    address: account.clone(),
                    display_name: None,
                    settings,
                };
                self.add_account(oauth, new, NewSignIn::Password(secret))
                    .await
            }
            (SignInAgain::Browser(_), _) => Err(SyncError::Config(format!(
                "{account} signs in with a password, not in the browser"
            ))),
            (SignInAgain::Password(_), _) => Err(SyncError::Config(format!(
                "{account} signs in in the browser, not with a password"
            ))),
        }
    }

    /// Keeps which folder the person chose for each role (by its name on
    /// the server; `None` leaves the role to the server's SPECIAL-USE, or
    /// the usual names). The next pass gives the folders their roles.
    pub async fn set_folder_roles(
        &self,
        account: &EmailAddress,
        roles: FolderRoles,
    ) -> SyncResult<Account> {
        self.change_imap(account, move |settings| settings.roles = roles)
            .await
    }

    /// The person sets whether the provider files sent mail in Sent
    /// itself (`Some`), or hands it back to Postrinch to learn from sends
    /// (`None`): see [`postrinch_store::Store::choose_server_files_sent`].
    pub async fn set_server_files_sent(
        &self,
        account: &EmailAddress,
        choice: Option<bool>,
    ) -> SyncResult<ImapSettings> {
        let a = account.clone();
        self.with_store(move |s| s.choose_server_files_sent(&a, choice))
            .await
    }

    /// The account's folders as sync last listed them, the inbox left
    /// out (it plays no other role), in the order the person reads them.
    pub async fn account_folders(&self, account: &EmailAddress) -> SyncResult<Vec<AccountFolder>> {
        let a = account.clone();
        let mut folders: Vec<AccountFolder> = self
            .with_store(move |s| s.imap_folders(&a))
            .await?
            .into_iter()
            .filter(|f| !f.name.eq_ignore_ascii_case("INBOX"))
            .map(|f| AccountFolder {
                shown: shown_name(&f.name, f.delimiter),
                name: f.name,
                role: f.role,
            })
            .collect();
        folders.sort_by_cached_key(|f| f.shown.to_lowercase());
        Ok(folders)
    }

    /// Changes the account's IMAP settings under the store's account lock,
    /// so nothing sync learns meanwhile is written over.
    async fn change_imap(
        &self,
        account: &EmailAddress,
        change: impl FnOnce(&mut ImapSettings) + Send + 'static,
    ) -> SyncResult<Account> {
        let a = account.clone();
        self.blocking(move |s| {
            let mut found = false;
            let changed = s.update_account(&a, |account| {
                if let Some(settings) = account.imap.as_mut() {
                    found = true;
                    change(settings);
                }
            })?;
            match changed {
                Some(account) if found => Ok(account),
                _ => Err(SyncError::Config(format!("{a} is not an IMAP account"))),
            }
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_names_are_shown_decoded_with_slashes() {
        assert_eq!(shown_name("INBOX.Sent", Some('.')), "INBOX/Sent");
        assert_eq!(shown_name("Entw&APw-rfe", Some('/')), "Entwürfe");
        assert_eq!(shown_name("Archive", None), "Archive");
    }
}
