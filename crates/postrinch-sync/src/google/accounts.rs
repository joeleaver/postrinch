//! Adding and removing Google accounts: what the window and the command
//! line both do.

use std::time::Duration;

use postrinch_model::{Account, EmailAddress};
use postrinch_store::Store;

use super::GoogleAuth;
use crate::SyncResult;

/// Sign a Google account in through the browser and store it, or store its
/// new sign-in if it is already here. `login_hint` suggests which account
/// to Google. `open_browser` is handed the sign-in page's address.
pub async fn sign_in_google(
    auth: &GoogleAuth,
    store: &Store,
    login_hint: Option<&EmailAddress>,
    open_browser: impl FnOnce(&str) -> std::io::Result<()>,
) -> SyncResult<Account> {
    let account = auth.sign_in(login_hint, open_browser).await?;
    let store = store.clone();
    let stored = account.clone();
    tokio::task::spawn_blocking(move || store.put_account(&stored))
        .await
        .map_err(|e| crate::SyncError::Io(std::io::Error::other(e)))??;
    Ok(account)
}

/// Sign a Google account out and delete it, and its mail, from this
/// computer. Nothing is deleted from Gmail. Returns how many raw messages
/// were deleted.
pub async fn remove_google_account(
    auth: &GoogleAuth,
    store: &Store,
    account: &EmailAddress,
) -> SyncResult<usize> {
    auth.sign_out(account).await?;
    let store = store.clone();
    let account = account.clone();
    let removed = tokio::task::spawn_blocking(move || -> SyncResult<usize> {
        store.remove_account(&account)?;
        Ok(store.collect_garbage(Duration::ZERO)?)
    })
    .await
    .map_err(|e| crate::SyncError::Io(std::io::Error::other(e)))??;
    Ok(removed)
}
