//! The Linux side of the keychain: making sure the Secret Service has a
//! keyring to write to.
//!
//! GNOME Keyring and KDE Wallet both serve the Secret Service, and both
//! normally have a default keyring (GNOME's "Login", KDE's "kdewallet").
//! Some desktops have a Secret Service with none: GNOME Keyring started
//! outside a GNOME session, for one, holds only an in-memory session
//! keyring. The `keyring` crate does not create one then; it fails. So
//! before a secret is stored, Postrinch asks the service to create a
//! default keyring, which makes the service ask the person for a password
//! for it, as a browser does.

use secret_service::EncryptionType;
use secret_service::blocking::SecretService;

use crate::{SyncError, SyncResult};

/// The label the created keyring gets, matching GNOME's own name for it.
const LABEL: &str = "Login";

pub(crate) fn ensure_default_collection() -> SyncResult<()> {
    let service = SecretService::connect(EncryptionType::Dh).map_err(describe)?;
    match service.get_default_collection() {
        Ok(_) => Ok(()),
        Err(secret_service::Error::NoResult) => {
            tracing::info!("the Secret Service has no default keyring; asking it to create one");
            service
                .create_collection(LABEL, "default")
                .map(drop)
                .map_err(describe)
        }
        Err(e) => Err(describe(e)),
    }
}

/// Say what went wrong in terms a person can act on.
fn describe(e: secret_service::Error) -> SyncError {
    SyncError::Keychain(match e {
        secret_service::Error::Unavailable => {
            "no password store is running; Postrinch keeps sign-ins in the desktop's \
             Secret Service (GNOME Keyring or KDE Wallet)"
                .into()
        }
        secret_service::Error::Prompt => "creating a keyring for Postrinch was cancelled".into(),
        secret_service::Error::Locked => "the keyring is locked; unlock it and try again".into(),
        other => format!("the Secret Service: {other}"),
    })
}
