use postrinch_model::EmailAddress;
use postrinch_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no account {0}")]
    UnknownAccount(EmailAddress),
    #[error("not a colour: {0:?} (expected #rrggbb)")]
    BadColor(String),
}

pub type CommandResult<T> = Result<T, CommandError>;
