use postrinch_model::EmailAddress;
use postrinch_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("no account {0}")]
    UnknownAccount(EmailAddress),
}

pub type CommandResult<T> = Result<T, CommandError>;
