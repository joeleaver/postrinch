//! Settings › Rules' state, and what its buttons do: through the commands,
//! then waking sync so Gmail gets the rules and the runner so they act.

use postrinch_commands::{AlreadyHere, LabelChoice, RuleProposal, RulesPage};
use postrinch_model::{EmailAddress, Timestamp};
use rinch::prelude::*;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::actions::{commands, sync};

/// Counts drafts looked up, so an answer for an older one is dropped.
static ASKED: AtomicU64 = AtomicU64::new(0);
use crate::rule_draft::{Draft, FromAgent};

/// The rules page's signals. `Copy`, like the window's state.
#[derive(Clone, Copy)]
pub struct RulesUi {
    /// The account whose rules show.
    pub account: Signal<Option<EmailAddress>>,
    pub page: Signal<Option<RulesPage>>,
    /// The account's labels, for the editor.
    pub labels: Signal<Vec<LabelChoice>>,
    /// Agents a rule can ask, by id with their names.
    pub agents: Signal<Vec<(String, String)>>,
    /// The rule being edited, and in which account.
    pub draft: Signal<Option<Draft>>,
    pub editing_account: Signal<Option<EmailAddress>>,
    /// Why the draft cannot be saved yet.
    pub error: Signal<Option<String>>,
    /// What the draft matches among the mail already here.
    pub here: Signal<Option<AlreadyHere>>,
    /// Asking before a rule that forwards is saved.
    pub confirm_forward: Signal<bool>,
    /// The rule being dragged, and the row it is over.
    pub dragging: Signal<Option<u32>>,
    pub over: Signal<Option<usize>>,
    /// Rules agents proposed, waiting for the person, in every account.
    pub proposals: Signal<Vec<RuleProposal>>,
    /// Running a saved rule now.
    pub run: crate::rule_run_now::RunNow,
}

impl RulesUi {
    pub fn new() -> Self {
        Self {
            account: Signal::new(None),
            page: Signal::new(None),
            labels: Signal::new(Vec::new()),
            agents: Signal::new(Vec::new()),
            draft: Signal::new(None),
            editing_account: Signal::new(None),
            error: Signal::new(None),
            here: Signal::new(None),
            confirm_forward: Signal::new(false),
            dragging: Signal::new(None),
            over: Signal::new(None),
            proposals: Signal::new(Vec::new()),
            run: crate::rule_run_now::RunNow::new(),
        }
    }

    /// Read the shown account's rules again.
    pub fn load(self, accounts: &[EmailAddress]) {
        let Some(commands) = commands() else {
            return;
        };
        let account = match self.account.get() {
            Some(a) if accounts.contains(&a) => a,
            _ => match accounts.first() {
                Some(a) => {
                    self.account.set(Some(a.clone()));
                    a.clone()
                }
                None => return,
            },
        };
        match commands.rules_page(&account) {
            Ok(page) => self.page.set(Some(page)),
            Err(e) => tracing::warn!("{account}: reading its rules: {e}"),
        }
        self.labels
            .set(commands.label_choices(&account).unwrap_or_default());
        self.agents.set(crate::rules::askable());
    }

    fn changed(self, account: &EmailAddress) {
        if let Some(sync) = sync() {
            sync.wake.notify_one();
        }
        crate::rules::due_now(account.clone());
        let accounts: Vec<EmailAddress> = self.account.get().into_iter().collect();
        self.load(&accounts);
    }

    /// Open the editor on `draft`, for `account`.
    pub fn edit(self, account: EmailAddress, draft: Draft) {
        if self.account.get().as_ref() != Some(&account) {
            self.account.set(Some(account.clone()));
            self.load(std::slice::from_ref(&account));
        }
        self.editing_account.set(Some(account));
        self.error.set(None);
        self.here.set(None);
        self.draft.set(Some(draft));
        self.look_here();
    }

    pub fn close(self) {
        self.draft.set(None);
        self.confirm_forward.set(false);
    }

    /// Change the draft and look again at what it matches.
    pub fn change(self, change: impl FnOnce(&mut Draft)) {
        self.draft.update(|d| {
            if let Some(d) = d {
                change(d);
            }
        });
        self.error.set(None);
        self.look_here();
    }

    /// Find what the draft matches, on a thread of its own: it reads up to
    /// thousands of messages.
    fn look_here(self) {
        let (Some(draft), Some(account), Some(commands)) =
            (self.draft.get(), self.editing_account.get(), commands())
        else {
            return;
        };
        let rule = draft.rule();
        let mine = ASKED.fetch_add(1, Ordering::SeqCst) + 1;
        let here = self.here;
        if rule.conditions.is_empty() {
            here.set(None);
            return;
        }
        let commands = (*commands).clone();
        std::thread::spawn(move || {
            let found = commands.already_here(&account, &rule, now());
            if ASKED.load(Ordering::SeqCst) == mine {
                here.send(found.ok());
            }
        });
    }

    /// Save the draft, asking first if it forwards and that wasn't asked.
    pub fn save(self, forward_asked: bool) {
        let (Some(draft), Some(account), Some(commands)) =
            (self.draft.get(), self.editing_account.get(), commands())
        else {
            return;
        };
        if let Some(trouble) = draft.trouble() {
            self.error.set(Some(trouble));
            return;
        }
        let rule = draft.rule();
        let was_forwarding = self
            .page
            .get()
            .and_then(|p| p.rules.into_iter().find(|r| r.id == rule.id))
            .is_some_and(|r| r == rule);
        if rule.sends_mail() && !forward_asked && !was_forwarding {
            self.confirm_forward.set(true);
            return;
        }
        match commands.save_rule(&account, rule.clone()) {
            Ok(id) => {
                if let Some(proposal) = &draft.proposal {
                    self.settle(&account, proposal.id);
                }
                if draft.existing {
                    let commands = (*commands).clone();
                    let saved = postrinch_rules::Rule { id, ..rule };
                    let target = account.clone();
                    std::thread::spawn(move || {
                        if let Err(e) = commands.apply_to_existing(&target, &saved, now()) {
                            tracing::warn!("{target}: applying a rule to existing mail: {e}");
                        }
                        if let Some(sync) = sync_sender() {
                            sync.notify_one();
                        }
                    });
                }
                self.close();
                self.changed(&account);
            }
            Err(e) => self.error.set(Some(e.to_string())),
        }
    }

    /// Open an agent's proposed rule in the editor.
    pub fn open_proposal(self, proposal: &RuleProposal) {
        let p = &proposal.proposed;
        let mut draft = Draft::of(&p.rule);
        draft.proposal = Some(FromAgent {
            id: p.id,
            agent: p.agent.clone(),
            reason: p.reason.clone(),
        });
        self.edit(proposal.account.clone(), draft);
    }

    /// Discard the proposal the editor has open.
    pub fn discard(self) {
        let (Some(draft), Some(account)) = (self.draft.get(), self.editing_account.get()) else {
            return;
        };
        if let Some(proposal) = &draft.proposal {
            self.settle(&account, proposal.id);
        }
        self.close();
    }

    /// The proposal is saved or discarded.
    fn settle(self, account: &EmailAddress, id: u32) {
        if let Some(commands) = commands()
            && let Err(e) = commands.settle_rule_proposal(account, id)
        {
            tracing::warn!("{account}: settling a proposed rule: {e}");
        }
        self.proposals
            .update(|all| all.retain(|p| !(&p.account == account && p.proposed.id == id)));
    }

    pub fn delete(self) {
        let (Some(draft), Some(account), Some(commands)) =
            (self.draft.get(), self.editing_account.get(), commands())
        else {
            return;
        };
        if draft.id != 0
            && let Err(e) = commands.delete_rule(&account, draft.id)
        {
            self.error.set(Some(e.to_string()));
            return;
        }
        self.close();
        self.changed(&account);
    }

    pub fn set_on(self, id: u32, on: bool) {
        self.with_account(|commands, account| commands.set_rule_on(account, id, on));
    }

    /// Move the dragged rule to where it was dropped.
    pub fn drop_at(self, to: usize) {
        let dragged = self.dragging.get();
        self.dragging.set(None);
        self.over.set(None);
        if let Some(id) = dragged {
            self.with_account(|commands, account| commands.move_rule(account, id, to));
        }
    }

    pub fn import(self, filter: String) {
        self.with_account(|commands, account| commands.import_filter(account, &filter).map(drop));
    }

    fn with_account(
        self,
        act: impl FnOnce(
            &postrinch_commands::Commands,
            &EmailAddress,
        ) -> postrinch_commands::CommandResult<()>,
    ) {
        let (Some(account), Some(commands)) = (self.account.get(), commands()) else {
            return;
        };
        if let Err(e) = act(&commands, &account) {
            tracing::warn!("{account}: changing its rules: {e}");
        }
        self.changed(&account);
    }
}

impl Default for RulesUi {
    fn default() -> Self {
        Self::new()
    }
}

/// Sync's sender, from another thread.
pub fn sync_sender() -> Option<std::sync::Arc<tokio::sync::Notify>> {
    SENDER.get().cloned()
}

static SENDER: std::sync::OnceLock<std::sync::Arc<tokio::sync::Notify>> =
    std::sync::OnceLock::new();

/// Remember sync's sender for work that finishes off the window's thread.
pub fn set_sender(send: std::sync::Arc<tokio::sync::Notify>) {
    let _ = SENDER.set(send);
}

pub fn now() -> Timestamp {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    Timestamp::from_millis(millis)
}
