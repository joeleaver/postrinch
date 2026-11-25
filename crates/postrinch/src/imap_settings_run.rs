//! An IMAP account's settings in the window: the folders it offers for
//! each role and the roles the person picks, the person's say on whether
//! the provider files sent mail, and the password dialog for signing in
//! again. The slow work runs on sync's runtime and reports back by
//! `send`; the servers panel is the add-account dialog's
//! (`AddUi::change_servers_of`).

use std::sync::Arc;

use postrinch_model::{EmailAddress, FolderRole, FolderRoles, SignInMethod, SystemLabel};
use postrinch_sync::imap::{AccountFolder, ImapSecret, SignInAgain};
use rinch::prelude::*;

use crate::actions::{Notice, sync};
use crate::add_account::Found;
use crate::add_account_flow::{Outcome, outcome};
use crate::state::State;

/// A role the person can give one of an account's folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Sent,
    Drafts,
    Archive,
    Trash,
    Junk,
}

impl Role {
    pub const ALL: [Role; 5] = [
        Role::Sent,
        Role::Drafts,
        Role::Archive,
        Role::Trash,
        Role::Junk,
    ];

    /// The role's name, as the Folders section labels its choice.
    pub fn name(self) -> &'static str {
        match self {
            Role::Sent => "Sent",
            Role::Drafts => "Drafts",
            Role::Archive => "Archive",
            Role::Trash => "Trash",
            Role::Junk => "Junk",
        }
    }

    /// The role sync gives the folder that has it.
    pub fn folder_role(self) -> FolderRole {
        match self {
            Role::Sent => FolderRole::System(SystemLabel::Sent),
            Role::Drafts => FolderRole::System(SystemLabel::Drafts),
            Role::Archive => FolderRole::Archive,
            Role::Trash => FolderRole::System(SystemLabel::Trash),
            Role::Junk => FolderRole::System(SystemLabel::Spam),
        }
    }

    fn slot(self, roles: &mut FolderRoles) -> &mut Option<String> {
        match self {
            Role::Sent => &mut roles.sent,
            Role::Drafts => &mut roles.drafts,
            Role::Archive => &mut roles.archive,
            Role::Trash => &mut roles.trash,
            Role::Junk => &mut roles.junk,
        }
    }

    /// The folder the person chose for the role, if they chose one.
    pub fn chosen(self, roles: &FolderRoles) -> Option<String> {
        match self {
            Role::Sent => roles.sent.clone(),
            Role::Drafts => roles.drafts.clone(),
            Role::Archive => roles.archive.clone(),
            Role::Trash => roles.trash.clone(),
            Role::Junk => roles.junk.clone(),
        }
    }

    /// Gives the role to `folder`, or back to the server's choice.
    pub fn choose(self, roles: &mut FolderRoles, folder: Option<String>) {
        *self.slot(roles) = folder;
    }
}

/// The value of a role's "as the server says" choice: no folder chosen.
/// Not empty, as a select shows an empty value as nothing chosen.
pub const SERVER_SAYS: &str = "server";

/// The prefix of a folder's value in a role's select, so no folder's name
/// can be taken for [`SERVER_SAYS`].
const FOLDER: &str = "folder:";

/// The select's value for a role given to `folder`, or the server's
/// choice for none.
pub fn role_value(folder: Option<&str>) -> String {
    folder.map_or_else(|| SERVER_SAYS.to_owned(), |name| format!("{FOLDER}{name}"))
}

/// The folder a select's value chooses; `None` for the server's choice.
pub fn chosen_folder(value: &str) -> Option<String> {
    value.strip_prefix(FOLDER).map(str::to_owned)
}

/// A role's choices for its select: the server's own first, then each
/// folder by its shown name, valued by its name on the server
/// ([`role_value`]). The server's choice names the folder that has the
/// role now, while the person has not chosen one.
pub fn role_choices(
    role: Role,
    folders: &[AccountFolder],
    roles: &FolderRoles,
) -> Vec<SelectOption> {
    let server = if role.chosen(roles).is_some() {
        "As the server says".to_owned()
    } else {
        let has = folders.iter().find(|f| f.role == Some(role.folder_role()));
        let name = has.map_or("None", |f| f.shown.as_str());
        format!("{name} (the server says)")
    };
    let mut choices = vec![SelectOption::new(SERVER_SAYS, server)];
    choices.extend(
        folders
            .iter()
            .map(|f| SelectOption::new(role_value(Some(&f.name)), f.shown.clone())),
    );
    choices
}

/// The account settings' state that is not the account itself.
#[derive(Clone, Copy)]
pub struct ImapUi {
    /// The account the folders are for, and its folders (no inbox).
    pub folders: Signal<Option<(EmailAddress, Vec<AccountFolder>)>>,
    /// The password dialog: the account signing in again, and why.
    pub again: Signal<Option<(EmailAddress, String)>>,
    /// The account takes a second password for its outgoing server.
    pub again_separate: Signal<bool>,
    pub password: Signal<String>,
    pub smtp_password: Signal<String>,
    pub again_error: Signal<Option<String>>,
    /// The servers are checking the password.
    pub again_busy: Signal<bool>,
    /// Bumped when the dialog opens or closes, so a late answer is dropped.
    generation: Signal<u64>,
}

impl ImapUi {
    pub fn new() -> Self {
        Self {
            folders: Signal::new(None),
            again: Signal::new(None),
            again_separate: Signal::new(false),
            password: Signal::new(String::new()),
            smtp_password: Signal::new(String::new()),
            again_error: Signal::new(None),
            again_busy: Signal::new(false),
            generation: Signal::new(0),
        }
    }

    /// Reads `account`'s folders, for the Folders section's choices.
    pub fn load_folders(self, state: State, account: EmailAddress) {
        let Some(sync) = sync() else {
            return;
        };
        let folders = self.folders;
        sync.runtime.clone().spawn(async move {
            match sync.mail().imap().account_folders(&account).await {
                Ok(found) => folders.send(Some((account, found))),
                Err(e) => state.notice.send(Some(Notice {
                    text: format!("{account}'s folders could not be read: {e}"),
                    trouble: true,
                })),
            }
        });
    }

    /// Gives `role` to `folder` (none: back to the server's choice),
    /// changing only that role as the account is stored now, so a quick
    /// second choice does not undo the first. The folders take their
    /// roles at the next pass, which starts now.
    pub fn set_role(self, state: State, account: EmailAddress, role: Role, folder: Option<String>) {
        let Some(sync) = sync() else {
            return state.notice.set(Some(sync_off()));
        };
        let ui = self;
        sync.runtime.clone().spawn(async move {
            let set = sync
                .mail()
                .imap()
                .change_folder_roles(&account, move |roles| role.choose(roles, folder))
                .await;
            match set {
                Ok(_) => {
                    state.revision.update_send(|n| *n += 1);
                    // The roles move at the next pass; run it now, and
                    // the choices follow it.
                    rinch::run_on_main_thread(move || ui.reload_after_pass(state, account));
                }
                Err(e) => state.notice.send(Some(Notice {
                    text: format!("The folder could not be set: {e}"),
                    trouble: true,
                })),
            }
        });
    }

    /// Reads the folders again once the pass the role change started has
    /// moved them.
    fn reload_after_pass(self, state: State, account: EmailAddress) {
        let Some(sync) = sync() else {
            return;
        };
        let folders = self.folders;
        sync.runtime.clone().spawn(async move {
            // The pass waits its turn behind sync's own on the account.
            let pass = sync.mail().sync(&account, Arc::new(|_| {})).await;
            if let Err(e) = pass {
                tracing::info!("IMAP: the pass after a role change did not finish: {e}");
            }
            state.revision.update_send(|n| *n += 1);
            match sync.mail().imap().account_folders(&account).await {
                Ok(found) => folders.send(Some((account, found))),
                Err(e) => state.notice.send(Some(Notice {
                    text: format!("{account}'s folders could not be read: {e}"),
                    trouble: true,
                })),
            }
        });
    }

    /// "The server files sent mail in Sent itself": `Some` is the
    /// person's say, `None` hands it back to what sync learns.
    pub fn set_files_sent(self, state: State, account: EmailAddress, files: Option<bool>) {
        let Some(sync) = sync() else {
            return state.notice.set(Some(sync_off()));
        };
        sync.runtime.clone().spawn(async move {
            match sync
                .mail()
                .imap()
                .set_server_files_sent(&account, files)
                .await
            {
                Ok(_) => {
                    sync.accounts_changed();
                    state.revision.update_send(|n| *n += 1);
                }
                Err(e) => state.notice.send(Some(Notice {
                    text: format!("The setting could not be kept: {e}"),
                    trouble: true,
                })),
            }
        });
    }

    /// Opens the password dialog for `account`, saying `reason`.
    pub fn open_sign_in_again(self, account: EmailAddress, reason: String, sign_in: &SignInMethod) {
        self.generation.update(|n| *n += 1);
        self.again_separate
            .set(*sign_in == SignInMethod::SeparateSmtpPassword);
        self.password.set(String::new());
        self.smtp_password.set(String::new());
        self.again_error.set(None);
        self.again_busy.set(false);
        self.again.set(Some((account, reason)));
    }

    /// Closes the dialog, forgetting what was typed.
    pub fn close_sign_in_again(self) {
        self.generation.update(|n| *n += 1);
        self.password.set(String::new());
        self.smtp_password.set(String::new());
        self.again_busy.set(false);
        self.again.set(None);
    }

    /// "Sign in": both servers check the password, and it is kept only
    /// once both took it.
    pub fn sign_in_again(self, state: State) {
        let Some((account, secret)) = self.ready_to_sign_in() else {
            return;
        };
        let Some(sync) = sync() else {
            return self.again_error.set(Some(sync_off().text));
        };
        let settings = match sync.store.account(&account) {
            Ok(Some(stored)) => stored.imap,
            _ => None,
        };
        let Some(settings) = settings else {
            return self
                .again_error
                .set(Some(format!("{account} is not an IMAP account.")));
        };
        self.again_error.set(None);
        self.again_busy.set(true);
        let generation = self.generation.get();
        let ui = self;
        sync.runtime.clone().spawn(async move {
            let imap = sync.mail().imap();
            let how = SignInAgain::Password(secret);
            let result = imap.sign_in_again(&sync.imap_oauth, &account, how).await;
            let done = outcome(
                result,
                &settings,
                &Found::default(),
                crate::add_account::Step::Found,
            );
            if matches!(done, Outcome::Done(_)) {
                sync.accounts_changed();
            }
            rinch::run_on_main_thread(move || {
                if ui.generation.get() == generation {
                    ui.signed_in(state, done);
                }
            });
        });
    }

    /// The account and the secret to check: none while a check is under
    /// way (Enter and the button both ask), or when the dialog is closed
    /// or lacks a password, which its error then says.
    pub fn ready_to_sign_in(self) -> Option<(EmailAddress, ImapSecret)> {
        if self.again_busy.get() {
            return None;
        }
        let (account, _) = self.again.get()?;
        match again_secret(
            self.password.get(),
            self.again_separate.get().then(|| self.smtp_password.get()),
        ) {
            Ok(secret) => Some((account, secret)),
            Err(error) => {
                self.again_error.set(Some(error));
                None
            }
        }
    }

    fn signed_in(self, state: State, done: Outcome) {
        match done {
            Outcome::Done(account) => {
                self.close_sign_in_again();
                state.notice.set(Some(Notice {
                    text: format!("{} is signed in. Its mail is on the way.", account.address),
                    trouble: false,
                }));
                state.revision.update(|n| *n += 1);
            }
            Outcome::Failed { error, .. } => {
                self.again_busy.set(false);
                self.again_error.set(Some(error));
            }
            // A password sign-in has no browser to cancel.
            Outcome::Cancelled => self.again_busy.set(false),
        }
    }
}

/// The secret the dialog's passwords make, or what is missing.
pub fn again_secret(password: String, smtp_password: Option<String>) -> Result<ImapSecret, String> {
    if password.is_empty() {
        return Err("Give the password.".into());
    }
    match smtp_password {
        None => Ok(ImapSecret::password(password)),
        Some(smtp) if smtp.is_empty() => Err("Give the outgoing server's password.".into()),
        Some(smtp) => Ok(ImapSecret::Password {
            password,
            smtp_password: Some(smtp),
        }),
    }
}

fn sync_off() -> Notice {
    Notice {
        text: "Sync is off, so accounts cannot be changed. The sidebar says why.".into(),
        trouble: true,
    }
}

#[cfg(test)]
#[path = "imap_settings_run_tests.rs"]
mod tests;
