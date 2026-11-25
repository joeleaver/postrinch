//! What every way of adding an account checks first: an address is one
//! account, a Google account or an IMAP account, never both. The store
//! keeps accounts by address, and a Google account and Google over IMAP
//! would share one Google grant (docs/imap.md, "Credentials").

use postrinch_model::{EmailAddress, Provider};
use postrinch_store::Store;

use crate::{SyncError, SyncResult};

/// Refuse to add `address` as a `provider` account when the store already
/// has it as the other kind, with [`SyncError::AlreadyAdded`] ("… is
/// already added as a Google account"). An account of the same kind is
/// not refused: that is signing it in again.
///
/// The sign-ins check the keychain themselves ([`GoogleAuth::sign_in`],
/// [`ImapOAuth::sign_in`]); this covers an account whose sign-in is not in
/// the keychain (a password account not yet saved, one signed out). The
/// add-account flows call it before keeping the account:
/// [`sign_in_google`] does, and the IMAP dialog must before it stores its
/// account.
///
/// [`GoogleAuth::sign_in`]: crate::google::GoogleAuth::sign_in
/// [`ImapOAuth::sign_in`]: crate::imap::ImapOAuth::sign_in
/// [`sign_in_google`]: crate::google::sign_in_google
pub async fn check_address_free(
    store: &Store,
    address: &EmailAddress,
    provider: Provider,
) -> SyncResult<()> {
    let store = store.clone();
    let key = address.clone();
    let existing = tokio::task::spawn_blocking(move || store.account(&key))
        .await
        .map_err(|e| SyncError::Io(std::io::Error::other(e)))??;
    match existing {
        Some(account) if account.provider != provider => Err(SyncError::AlreadyAdded {
            address: account.address,
            existing: account.provider,
        }),
        _ => Ok(()),
    }
}
