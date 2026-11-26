use postrinch_model::EmailAddress;
use rhypedb_engine::EngineError;

pub type StoreResult<T> = Result<T, StoreError>;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("the store schema does not parse: {0}")]
    Schema(String),
    #[error("database: {0}")]
    Engine(#[from] EngineError),
    #[error("blob store: {0}")]
    Io(#[from] std::io::Error),
    #[error("no account {0} in the store")]
    UnknownAccount(EmailAddress),
    #[error("no message {id} in account {account}")]
    UnknownMessage { account: EmailAddress, id: String },
    /// A message carries a label the store does not have: put labels before
    /// the messages that use them.
    #[error("no label {id} in account {account}")]
    UnknownLabel { account: EmailAddress, id: String },
    /// A place names an IMAP folder the store does not have: put folders
    /// before the places in them.
    #[error("no IMAP folder {id} in account {account}")]
    UnknownImapFolder { account: EmailAddress, id: String },
    /// The store was written in a format too old to upgrade (see
    /// `format.rs`). The fix is to delete `dir` and add the accounts again.
    #[error(
        "the store at {} was written by another build of Postrinch ({found}); \
         delete that directory and add your accounts again (raw messages are kept)",
        dir.display()
    )]
    OldFormat {
        dir: std::path::PathBuf,
        found: String,
    },
    /// The store was written by a newer Postrinch, in a format this one
    /// does not know. It is left as it is: the newer Postrinch opens it.
    #[error(
        "the store at {} was written by a newer Postrinch (format {found}; this one \
         reads format {current} and older): update Postrinch to open it",
        dir.display()
    )]
    NewerFormat {
        dir: std::path::PathBuf,
        found: u32,
        current: u32,
    },
    /// Upgrading the store from an older format failed part-way. What was
    /// done stays done, and opening the store again carries on from there.
    #[error("upgrading the store at {} from format {from} failed: {cause}", dir.display())]
    Upgrade {
        dir: std::path::PathBuf,
        from: u32,
        cause: Box<StoreError>,
    },
    /// A stored object does not decode to what the schema promises: a bug,
    /// or a database written by a different version of Postrinch.
    #[error("stored {type_name} {object_id} is malformed: {detail}")]
    Corrupt {
        type_name: &'static str,
        object_id: u64,
        detail: String,
    },
}

impl StoreError {
    pub(crate) fn corrupt(type_name: &'static str, object_id: u64, detail: impl ToString) -> Self {
        Self::Corrupt {
            type_name,
            object_id,
            detail: detail.to_string(),
        }
    }
}
