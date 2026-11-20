//! Running a saved rule now, from Settings › Rules: over the last three
//! months' mail, or over all of it. A run over all mail reads every
//! message, so it runs off the window's thread and says how it went on
//! the rule's row.

use postrinch_model::EmailAddress;
use postrinch_rules::Rule;
use rinch::prelude::*;

use crate::actions::commands;
use crate::rules_ui::now;
use crate::sync::thousands;

/// The rule the dialog asks about, and what it would run over.
#[derive(Debug, Clone, PartialEq)]
pub struct Asking {
    pub account: EmailAddress,
    pub rule: Rule,
    pub all: bool,
}

/// What a rule's row says about its last run.
#[derive(Debug, Clone, PartialEq)]
pub struct Said {
    pub account: EmailAddress,
    pub rule: u32,
    pub running: bool,
    pub text: String,
}

#[derive(Clone, Copy)]
pub struct RunNow {
    pub asking: Signal<Option<Asking>>,
    /// The conversations the asked rule matches in the last three months,
    /// by rule, once counted.
    pub recent: Signal<Option<(u32, usize)>>,
    pub said: Signal<Vec<Said>>,
}

impl RunNow {
    pub fn new() -> Self {
        Self {
            asking: Signal::new(None),
            recent: Signal::new(None),
            said: Signal::new(Vec::new()),
        }
    }

    /// Open the dialog for `rule`, and count what it matches recently.
    pub fn ask(self, account: EmailAddress, rule: Rule) {
        let Some(commands) = commands() else {
            return;
        };
        self.recent.set(None);
        let (recent, commands) = (self.recent, (*commands).clone());
        let (target, looked) = (account.clone(), rule.clone());
        std::thread::spawn(
            move || match commands.already_here(&target, &looked, now()) {
                Ok(here) => recent.send(Some((looked.id, here.threads.len()))),
                Err(e) => tracing::warn!("{target}: counting what a rule matches: {e}"),
            },
        );
        self.asking.set(Some(Asking {
            account,
            rule,
            all: false,
        }));
    }

    pub fn choose(self, all: bool) {
        self.asking.update(|a| {
            if let Some(a) = a {
                a.all = all;
            }
        });
    }

    /// What the dialog says the last three months hold.
    pub fn recent_text(self) -> String {
        let id = self.asking.with(|a| a.as_ref().map(|a| a.rule.id));
        match self.recent.get() {
            Some((rule, n)) if Some(rule) == id => match n {
                0 => "Nothing matches".into(),
                1 => "1 conversation matches".into(),
                n => format!("{} conversations match", thousands(n)),
            },
            _ => "Counting\u{2026}".into(),
        }
    }

    pub fn running(self, account: &EmailAddress, rule: u32) -> bool {
        self.said.with(|all| {
            all.iter()
                .any(|s| s.running && &s.account == account && s.rule == rule)
        })
    }

    /// Run the asked rule, closing the dialog; its row says how it went.
    pub fn run(self) {
        let (Some(asking), Some(commands)) = (self.asking.get(), commands()) else {
            return;
        };
        self.asking.set(None);
        let Asking { account, rule, all } = asking;
        if self.running(&account, rule.id) {
            return;
        }
        let over = if all { "all mail" } else { "the last 3 months" };
        self.say(Said {
            account: account.clone(),
            rule: rule.id,
            running: true,
            text: format!("Running over {over}\u{2026}"),
        });
        let (said, commands) = (self.said, (*commands).clone());
        std::thread::spawn(move || {
            let run = if all {
                commands.apply_to_all_existing(&account, &rule, now())
            } else {
                commands.apply_to_existing(&account, &rule, now())
            };
            let text = match run {
                Ok(0) => format!("Ran over {over}: nothing needed changing."),
                Ok(1) => format!("Ran over {over}: 1 message changed."),
                Ok(n) => format!("Ran over {over}: {} messages changed.", thousands(n)),
                Err(e) => {
                    tracing::warn!("{account}: running a rule over {over}: {e}");
                    format!("Couldn\u{2019}t run over {over}: {e}")
                }
            };
            let done = Said {
                account: account.clone(),
                rule: rule.id,
                running: false,
                text,
            };
            said.update_send(move |all| replace(all, done));
            if let Some(sync) = crate::rules_ui::sync_sender() {
                sync.notify_one();
            }
        });
    }

    fn say(self, said: Said) {
        self.said.update(|all| replace(all, said));
    }

    /// What the row of `rule` says, if anything.
    pub fn said_of(self, account: Option<&EmailAddress>, rule: u32) -> Option<Said> {
        let account = account?;
        self.said.with(|all| {
            all.iter()
                .find(|s| &s.account == account && s.rule == rule)
                .cloned()
        })
    }
}

impl Default for RunNow {
    fn default() -> Self {
        Self::new()
    }
}

fn replace(all: &mut Vec<Said>, said: Said) {
    all.retain(|s| !(s.account == said.account && s.rule == said.rule));
    all.push(said);
}
