//! The add-account dialog in the window: its signals, and the slow work
//! (`add_account_flow`) run on sync's runtime, reporting back by
//! `send`. Each run carries the dialog's generation: an answer for a
//! dialog since closed or gone back from is dropped.

use std::cell::RefCell;

use postrinch_discover::{Progress, Step as LookStep, StepState};
use postrinch_model::{EmailAddress, ImapSettings};
use postrinch_sync::imap::{NewSignIn, OAuthProvider, SignInCanceller, sign_in_cancel};
use rinch::prelude::*;

use crate::actions::{Notice, sync};
use crate::add_account::{AddForm, Kind, ServerForm, Step, Way};
use crate::add_account_flow::{Outcome, look_up, outcome};
use crate::state::{SettingsSection, State, View};

thread_local! {
    /// Stops the browser sign-in the dialog waits on.
    static CANCEL: RefCell<Option<SignInCanceller>> = const { RefCell::new(None) };
}

/// The add-account dialog's state.
#[derive(Clone, Copy)]
pub struct AddUi {
    /// Where the dialog is; none when it is closed.
    pub step: Signal<Option<Step>>,
    pub form: Signal<AddForm>,
    /// The step the servers panel came from, where its Back goes.
    pub servers_back: Signal<Step>,
    /// Discovery's steps so far, as they report.
    pub looking: Signal<Vec<(LookStep, StepState)>>,
    /// What went wrong, shown on the step the person is back on.
    pub error: Signal<Option<String>>,
    /// The page the browser was asked to open, for "Open the page again".
    pub browser_page: Signal<Option<String>>,
    /// Bumped whenever the dialog opens, closes or goes back, so answers
    /// for what was left are dropped.
    pub generation: Signal<u64>,
    /// The account whose servers the servers panel changes ("Server
    /// settings…"); none while adding one.
    pub changing: Signal<Option<EmailAddress>>,
}

impl AddUi {
    pub fn new() -> Self {
        Self {
            step: Signal::new(None),
            form: Signal::new(AddForm::new(Kind::Other)),
            servers_back: Signal::new(Step::Address),
            looking: Signal::new(Vec::new()),
            error: Signal::new(None),
            browser_page: Signal::new(None),
            generation: Signal::new(0),
            changing: Signal::new(None),
        }
    }

    /// Opens the dialog on the chooser.
    pub fn open(self) {
        self.changing.set(None);
        self.form.set(AddForm::new(Kind::Other));
        self.go(Step::Choose);
    }

    /// "Server settings…" on an IMAP account: the servers panel, filled
    /// in with the servers it has.
    pub fn change_servers_of(self, account: EmailAddress, settings: &ImapSettings) {
        let mut form = AddForm::new(Kind::Other);
        form.address = account.to_string();
        form.imap = ServerForm::from_server(&settings.imap);
        form.smtp = ServerForm::from_server(&settings.smtp);
        self.form.set(form);
        self.changing.set(Some(account));
        self.servers(Step::Servers);
    }

    /// "Check and save" on an account's servers: both are checked with
    /// the sign-in it keeps, and changed only once both pass.
    pub fn save_servers(self, state: State) {
        let Some(account) = self.changing.get() else {
            return;
        };
        let form = self.form.get();
        let servers = form
            .imap
            .server("incoming")
            .and_then(|imap| Ok((imap, form.smtp.server("outgoing")?)));
        let (imap, smtp) = match servers {
            Ok(servers) => servers,
            Err(error) => return self.error.set(Some(error)),
        };
        let Some(sync) = sync() else {
            return self.error.set(Some(sync_off()));
        };
        let stored = match sync.store.account(&account) {
            Ok(Some(stored)) => stored.imap,
            Ok(None) => None,
            Err(e) => {
                return self
                    .error
                    .set(Some(format!("The account could not be read: {e}")));
            }
        };
        let Some(mut settings) = stored else {
            return self
                .error
                .set(Some(format!("{account} is not an IMAP account.")));
        };
        settings.imap = imap;
        settings.smtp = smtp;
        self.go(Step::Checking);
        let generation = self.generation.get();
        let ui = self;
        let runtime = sync.runtime.clone();
        runtime.spawn(async move {
            let result = sync
                .mail()
                .imap()
                .change_servers(&account, settings.clone())
                .await;
            let done = outcome(result, &settings, &form.found, Step::Servers);
            if matches!(done, Outcome::Done(_)) {
                sync.accounts_changed();
            }
            ui.on_main(generation, move |ui| ui.finish(state, done, Step::Servers));
        });
    }

    /// Closes the dialog, forgetting what was typed and stopping a
    /// browser sign-in it waits on.
    pub fn close(self) {
        cancel_browser();
        self.generation.update(|n| *n += 1);
        self.form.update(AddForm::forget_passwords);
        self.step.set(None);
        self.error.set(None);
    }

    /// Goes to `step`, leaving whatever was running behind.
    pub fn go(self, step: Step) {
        self.generation.update(|n| *n += 1);
        self.error.set(None);
        self.step.set(Some(step));
    }

    /// The servers panel, from `back`.
    pub fn servers(self, back: Step) {
        self.servers_back.set(back);
        self.go(Step::Servers);
    }

    /// A chooser tile: the address next.
    pub fn choose(self, kind: Kind) {
        self.form.set(AddForm::new(kind));
        self.go(Step::Address);
    }

    /// Changes the form; any error shown is about the form as it was.
    pub fn edit(self, change: impl FnOnce(&mut AddForm)) {
        self.form.update(change);
    }

    /// "Continue" on the address: checks the address is not an account
    /// here already, then looks its servers up.
    pub fn look_up(self) {
        let form = self.form.get();
        let address = match form.parsed_address() {
            Ok(address) => address,
            Err(error) => return self.error.set(Some(error)),
        };
        let Some(sync) = sync() else {
            return self.error.set(Some(sync_off()));
        };
        self.go(Step::LookingUp);
        self.looking.set(Vec::new());
        let generation = self.generation.get();
        let ui = self;
        // The browser sign-ins this copy has set up.
        let ready: Vec<OAuthProvider> = OAuthProvider::ALL
            .iter()
            .copied()
            .filter(|p| sync.imap_oauth.is_set_up(*p))
            .collect();
        let runtime = sync.runtime.clone();
        runtime.spawn(async move {
            let looking = ui.looking;
            let progress = move |p: Progress| {
                looking.update_send(move |steps| note_progress(steps, p));
            };
            let found = look_up(&sync.store, &address, progress).await;
            ui.on_main(generation, move |ui| match found {
                Ok(discovery) => {
                    ui.form
                        .update(|f| f.take_discovery(&discovery, |p| ready.contains(&p)));
                    let form = ui.form.get();
                    let next = form.step_after_discovery();
                    if next == Step::Servers {
                        // The panel's head says these are guesses; it is
                        // not an error.
                        ui.servers(Step::Address);
                    } else {
                        ui.go(next);
                        ui.error.set(form.no_way_in());
                    }
                }
                Err(error) => {
                    ui.go(Step::Address);
                    ui.error.set(Some(error));
                }
            });
        });
    }

    /// "Add account", "Sign in with…" or "Check and add": checks the
    /// form, then signs in and checks both servers off the window's
    /// thread, and adds the account once both took it.
    pub fn add(self, state: State) {
        let from = self.step.get().unwrap_or(Step::Found);
        let form = self.form.get();
        let (new, secret) = match form.to_add() {
            Ok(add) => add,
            Err(error) => return self.error.set(Some(error)),
        };
        let Some(sync) = sync() else {
            return self.error.set(Some(sync_off()));
        };
        let browser = match form.way {
            Way::Browser(provider) => Some(provider),
            Way::Password => None,
        };
        self.go(if browser.is_some() {
            Step::Browser
        } else {
            Step::Checking
        });
        let generation = self.generation.get();
        let ui = self;
        let sign_in = match (browser, secret) {
            (Some(provider), _) => {
                let (canceller, cancel) = sign_in_cancel();
                CANCEL.with(|c| *c.borrow_mut() = Some(canceller));
                let page = self.browser_page;
                NewSignIn::OAuth {
                    provider,
                    open_browser: Box::new(move |url: &str| {
                        page.send(Some(url.to_owned()));
                        open::that_detached(url)
                    }),
                    cancel: Some(cancel),
                }
            }
            (None, Some(secret)) => NewSignIn::Password(secret),
            (None, None) => return self.error.set(Some("Give the password.".into())),
        };
        let settings = new.settings.clone();
        let found = form.found.clone();
        let runtime = sync.runtime.clone();
        runtime.spawn(async move {
            let result = sync
                .mail()
                .imap()
                .add_account(&sync.imap_oauth, new, sign_in)
                .await;
            let done = outcome(result, &settings, &found, from);
            if matches!(done, Outcome::Done(_)) {
                sync.accounts_changed();
            }
            ui.on_main(generation, move |ui| ui.finish(state, done, from));
        });
    }

    /// "Cancel" while the browser is open: stops waiting for it.
    pub fn cancel_browser(self) {
        cancel_browser();
        let back = if self.form.get().found.source.is_some() {
            Step::Found
        } else {
            Step::Servers
        };
        self.go(back);
    }

    /// "Add as a Google account" on Google's mail: Google's own sign-in,
    /// with the address as its hint.
    pub fn add_as_google(self, state: State) {
        let address = self.form.get().parsed_address().ok();
        self.close();
        state.sign_in(address);
    }

    /// The add's answer, for the dialog that asked from step `from`.
    fn finish(self, state: State, done: Outcome, from: Step) {
        CANCEL.with(|c| c.borrow_mut().take());
        match done {
            Outcome::Done(account) => {
                let text = if self.changing.get().is_some() {
                    format!("{}'s servers are changed.", account.address)
                } else {
                    format!("{} is added. Its mail is on the way.", account.address)
                };
                self.close();
                // The account's detail, wherever the dialog was opened.
                state.view.set(View::Settings);
                state.settings_section.set(SettingsSection::Accounts);
                state.account_shown.set(Some(account.address.clone()));
                state.notice.set(Some(Notice {
                    text,
                    trouble: false,
                }));
                state.revision.update(|n| *n += 1);
            }
            Outcome::Failed { step, error } => {
                // The servers panel opened from the found step goes back
                // there; one the person was already on keeps its Back.
                if step == Step::Servers && from != Step::Servers {
                    self.servers(from);
                } else {
                    self.go(step);
                }
                self.error.set(Some(error));
            }
        }
    }

    /// Runs `then` on the window's thread, unless the dialog has moved
    /// on since generation `generation`.
    fn on_main(self, generation: u64, then: impl FnOnce(AddUi) + Send + 'static) {
        rinch::run_on_main_thread(move || {
            if self.generation.get() == generation {
                then(self);
            }
        });
    }
}

/// Stops a browser sign-in the dialog is waiting on, if one is.
fn cancel_browser() {
    if let Some(canceller) = CANCEL.with(|c| c.borrow_mut().take()) {
        canceller.cancel();
    }
}

/// Discovery's report of `progress`: a step's state replaces what it
/// said before.
pub fn note_progress(steps: &mut Vec<(LookStep, StepState)>, progress: Progress) {
    match steps.iter_mut().find(|(step, _)| *step == progress.step) {
        Some(seen) => seen.1 = progress.state,
        None => steps.push((progress.step, progress.state)),
    }
}

/// What a discovery step says, as the looking-up step lists it.
pub fn look_line(domain: &str, step: LookStep, state: StepState) -> String {
    let what = match step {
        LookStep::DomainSettings => format!("{domain}'s own settings"),
        LookStep::ProviderList => "Thunderbird's list of providers".to_owned(),
        LookStep::DnsRecords => "The domain's DNS records".to_owned(),
        LookStep::CommonNames => "Common server names".to_owned(),
    };
    match state {
        StepState::Running => format!("{what}…"),
        StepState::Found => format!("{what}: found"),
        StepState::NothingFound => format!("{what}: nothing found"),
    }
}

/// The steps discovery takes, in order, for the looking-up list.
pub const LOOK_STEPS: [LookStep; 4] = [
    LookStep::DomainSettings,
    LookStep::ProviderList,
    LookStep::DnsRecords,
    LookStep::CommonNames,
];

fn sync_off() -> String {
    "Sync is off, so accounts cannot be added. The sidebar says why.".to_owned()
}

/// The address the dialog is adding, for its subtitle.
pub fn adding_address(form: &AddForm) -> String {
    match EmailAddress::parse(form.address.trim()) {
        Ok(address) => address.to_string(),
        Err(_) => form.address.trim().to_owned(),
    }
}
