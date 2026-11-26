//! Importing from Thunderbird in the window: finding its profiles off the
//! window's thread when the dialog opens, and taking the picked accounts
//! one at a time through the dialog's own steps. An IMAP account fills in
//! the form and is added as one added by hand (`AddUi::add`, with its
//! check, its outcomes and its servers panel); a Gmail account goes to
//! Google's own sign-in. The list and the run are `import_thunderbird`.

use postrinch_model::EmailAddress;
use postrinch_sync::imap::OAuthProvider;

use crate::actions::sync;
use crate::add_account::{AddForm, Kind, Step};
use crate::add_account_run::AddUi;
use crate::import_thunderbird::{Ended, Import, Route, listing};
use crate::state::State;

impl AddUi {
    /// Looks for Thunderbird's profiles under the home folder, off the
    /// window's thread; the chooser offers the import once they are found.
    /// Only `profiles.ini`, `prefs.js` and `user.js` are read.
    pub fn find_thunderbird(self) {
        let Some(sync) = sync() else {
            return;
        };
        let Some(home) = dirs::home_dir() else {
            return;
        };
        let ready: Vec<OAuthProvider> = OAuthProvider::ALL
            .iter()
            .copied()
            .filter(|p| sync.imap_oauth.is_set_up(*p))
            .collect();
        let found = self.thunderbird;
        let runtime = sync.runtime.clone();
        runtime.spawn_blocking(move || {
            let profiles = postrinch_thunderbird::find(&home);
            // Without the accounts here, every account would look new:
            // better to offer nothing than to offer wrongly.
            let Ok(accounts) = sync.store.accounts() else {
                return;
            };
            let here: Vec<EmailAddress> = accounts.into_iter().map(|a| a.address).collect();
            found.send(listing(&profiles, &here, |p| ready.contains(&p)));
        });
    }

    /// "Import from Thunderbird": the list, every account that can be
    /// added ticked.
    pub fn open_import(self) {
        let Some(listing) = self.thunderbird.get() else {
            return;
        };
        self.import.set(Some(Import::new(listing)));
        self.go(Step::ImportList);
    }

    /// Ticks or clears a row of the list.
    pub fn toggle_import(self, id: usize) {
        self.import.update(|i| {
            if let Some(import) = i {
                import.toggle(id);
            }
        });
    }

    /// "Import N accounts": the first picked account's turn.
    pub fn start_import(self) {
        let mut started = false;
        self.import.update(|i| {
            if let Some(import) = i {
                started = import.start();
            }
        });
        if started {
            self.next_import();
        }
    }

    /// The account whose turn it is, or the summary once none is left.
    fn next_import(self) {
        let Some(row) = self.import.get().and_then(|i| i.current().cloned()) else {
            return self.go(Step::ImportDone);
        };
        let form = match row.route {
            Some(Route::Imap(form)) => *form,
            Some(Route::Google(address)) => {
                let mut form = AddForm::new(Kind::Thunderbird);
                form.address = address.to_string();
                form
            }
            // Only rows that can be added are picked.
            None => return self.end_import_turn(Ended::Skipped),
        };
        self.changing.set(None);
        self.form.set(form);
        self.go(Step::ImportSignIn);
    }

    fn end_import_turn(self, ended: Ended) {
        self.form.update(AddForm::forget_passwords);
        self.import.update(|i| {
            if let Some(import) = i {
                import.end_current(ended);
            }
        });
        self.next_import();
    }

    /// "Skip this account".
    pub fn skip_import(self) {
        if !self.working() {
            self.end_import_turn(Ended::Skipped);
        }
    }

    /// "Stop importing": the accounts still to come are not added.
    pub fn stop_import(self) {
        if self.working() {
            return;
        }
        self.form.update(AddForm::forget_passwords);
        self.import.update(|i| {
            if let Some(import) = i {
                import.stop();
            }
        });
        self.go(Step::ImportDone);
    }

    /// "Sign in with Google" on a Gmail account: Google's own sign-in,
    /// whose notice says how it went; the next account's turn meanwhile.
    pub fn import_google(self, state: State) {
        let Some(Route::Google(address)) = self
            .import
            .get()
            .and_then(|i| i.current().and_then(|r| r.route.clone()))
        else {
            return;
        };
        state.sign_in(Some(address));
        self.end_import_turn(Ended::ToGoogle);
    }

    /// The account whose turn it is was added: the next one's turn. False
    /// when `address` is not an import's (the dialog then closes on it as
    /// after any add).
    pub fn imported(self, address: &EmailAddress) -> bool {
        let current = self.import.get().and_then(|i| i.current().cloned());
        let ours = current.is_some_and(|row| {
            matches!(row.route, Some(Route::Imap(form)) if form.parsed_address().ok().as_ref() == Some(address))
        });
        if ours {
            self.end_import_turn(Ended::Added);
        }
        ours
    }

    /// Where a failed or cancelled add of an imported account leaves the
    /// person: its own turn, rather than an address step it never had.
    pub fn import_step(self, step: Step) -> Step {
        if self.form.get().kind == Kind::Thunderbird && matches!(step, Step::Address | Step::Found)
        {
            Step::ImportSignIn
        } else {
            step
        }
    }
}

#[cfg(test)]
#[path = "add_account_import_tests.rs"]
mod tests;
