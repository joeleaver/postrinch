use postrinch_model::{EmailAddress, SystemLabel};
use postrinch_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no account {0}")]
    UnknownAccount(EmailAddress),
    /// A social account (by its actor, `handle@domain`) the store does not
    /// have, or this caller does not see.
    #[error("no social account {0}")]
    UnknownSocialAccount(String),
    /// What the person or agent asked for cannot be done as asked: one
    /// sentence saying why.
    #[error("{0}")]
    Invalid(String),
    #[error("not a colour: {0:?} (expected #rrggbb)")]
    BadColor(String),
    #[error("account {account} has no {label:?} label")]
    NoSystemLabel {
        account: EmailAddress,
        label: SystemLabel,
    },
}

pub type CommandResult<T> = Result<T, CommandError>;
