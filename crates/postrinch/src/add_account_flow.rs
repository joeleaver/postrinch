//! The add-account dialog's slow work, apart from the window: looking the
//! address up, and adding the account (or changing its servers, or
//! signing it in again) through [`ImapSync`], with what a failure says
//! and which step it leaves the person on. `add_account_run` runs these
//! on sync's runtime; the tests run them against the fake servers.

use postrinch_discover::{Discoverer, Discovery, Progress};
use postrinch_model::{Account, EmailAddress, ImapSettings, Provider, SignInReason};
use postrinch_store::Store;
use postrinch_sync::imap::{AccountCheck, Added, CheckFailure};
use postrinch_sync::{SyncError, SyncResult, check_address_free};

use crate::add_account::{Found, Step};

/// What the person is told after the address is looked up.
pub async fn look_up(
    store: &Store,
    address: &EmailAddress,
    progress: impl FnMut(Progress) + Send,
) -> Result<Discovery, String> {
    check_address_free(store, address, Provider::Imap)
        .await
        .map_err(|e| e.to_string())?;
    let discoverer =
        Discoverer::new().map_err(|e| format!("Postrinch could not look the servers up: {e}"))?;
    postrinch_discover::discover(&address.to_string(), &discoverer, progress)
        .await
        .map_err(|e| e.to_string())
}

/// How adding (or changing, or signing in again) went, for the dialog.
#[derive(Debug)]
pub enum Outcome {
    /// Stored and signed in: the dialog closes.
    Done(Box<Account>),
    /// Not done: nothing was kept. The person goes back to `step`, told
    /// `error`.
    Failed { step: Step, error: String },
    /// The person stopped the browser sign-in while it waited: nothing
    /// was kept, and there is nothing to tell them.
    Cancelled,
}

/// What adding came to. `found` says what discovery knew (for the app
/// password hint); `settings` are the servers that were checked; `from`
/// is where the person asked for the check (the found step or the
/// servers panel), where a refused password leaves them. Servers that
/// could not be reached or answered wrongly open the servers panel.
pub fn outcome(
    result: SyncResult<Added>,
    settings: &ImapSettings,
    found: &Found,
    from: Step,
) -> Outcome {
    match result {
        Ok(Added::Account(account)) => Outcome::Done(account),
        Ok(Added::CheckFailed(check)) => {
            let (error, servers_wrong) = check_text(&check, settings, found);
            let step = if servers_wrong { Step::Servers } else { from };
            Outcome::Failed { step, error }
        }
        Err(SyncError::SignInCancelled) => Outcome::Cancelled,
        Err(e) => {
            let step = match e {
                SyncError::AlreadyAdded { .. } => Step::Address,
                _ => from,
            };
            Outcome::Failed {
                step,
                error: error_text(&e),
            }
        }
    }
}

/// What a failed check says, server by server in the server's own words;
/// and whether it is the servers that look wrong (so the servers panel
/// opens) rather than the password.
pub fn check_text(check: &AccountCheck, settings: &ImapSettings, found: &Found) -> (String, bool) {
    let mut lines = Vec::new();
    let mut servers_wrong = false;
    let mut refused = false;
    let (imap, smtp) = (&settings.imap.host, &settings.smtp.host);
    // Both servers on one host are told apart by what they do, unless
    // they say the same thing (said once below).
    let alike = match (&check.imap, &check.smtp) {
        (Err(a), Err(b)) => failure_text(a, imap) == failure_text(b, smtp),
        _ => false,
    };
    let (imap_name, smtp_name) = if imap == smtp && !alike {
        (
            format!("The incoming server, {imap},"),
            format!("The outgoing server, {smtp},"),
        )
    } else {
        (imap.clone(), smtp.clone())
    };
    let failures = [
        (check.imap.as_ref().err(), &imap_name),
        (check.smtp.as_ref().err(), &smtp_name),
    ];
    for (failure, host) in failures {
        let Some(failure) = failure else { continue };
        lines.push(failure_text(failure, host));
        match failure {
            CheckFailure::SignIn(_) => refused = true,
            CheckFailure::Busy(_) => {}
            CheckFailure::Unreachable(_)
            | CheckFailure::Tls(_)
            | CheckFailure::Insecure
            | CheckFailure::Server(_) => servers_wrong = true,
        }
    }
    // Two servers that refused the same thing say it once.
    lines.dedup();
    let mut text = lines.join(" ");
    if refused && found.app_password_url.is_some() {
        let who = found.provider.as_deref().unwrap_or("This provider");
        text.push_str(&format!(" {who} needs an app password."));
    }
    (text, servers_wrong)
}

/// One server's failure, in a sentence that names it.
pub fn failure_text(failure: &CheckFailure, host: &str) -> String {
    match failure {
        CheckFailure::SignIn(SignInReason::CredentialRejected { detail }) if detail.is_empty() => {
            format!("{host} refused the password.")
        }
        CheckFailure::SignIn(SignInReason::CredentialRejected { detail }) => {
            format!("{host} refused the password: “{detail}”.")
        }
        CheckFailure::SignIn(reason) => format!("{host} refused the sign-in: {reason}."),
        CheckFailure::Unreachable(detail) => format!("{host} could not be reached: {detail}."),
        CheckFailure::Tls(detail) => {
            format!("The secure connection to {host} failed: {detail}.")
        }
        CheckFailure::Insecure => {
            format!("{host} offers no encryption, so no password was sent to it.")
        }
        CheckFailure::Busy(message) => {
            format!("{host} is busy just now; try again in a while: “{message}”.")
        }
        CheckFailure::Server(message) => format!("{host} answered: “{message}”."),
    }
}

/// What an error from adding says.
pub fn error_text(error: &SyncError) -> String {
    match error {
        SyncError::AlreadyAdded { .. } => format!("{error}."),
        SyncError::SignIn(detail) => format!("Signing in did not finish: {detail}."),
        SyncError::OAuthNotSetUp { provider, .. } => format!(
            "{provider} sign-in is not set up in this copy of Postrinch, so it cannot sign this account in."
        ),
        SyncError::Keychain(detail) => {
            format!("The sign-in could not be kept in the system keyring: {detail}.")
        }
        other => format!("The account was not added: {other}."),
    }
}

#[cfg(test)]
#[path = "add_account_flow_tests.rs"]
mod tests;
