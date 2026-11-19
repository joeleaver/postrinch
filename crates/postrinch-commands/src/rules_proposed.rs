//! Rules agents propose. An agent may save a rule that labels, files or
//! flags mail, but one that forwards mail out of the account or asks an
//! agent waits here, on this computer, until the person saves or discards
//! it in the rule editor.

use postrinch_model::EmailAddress;
use postrinch_rules::{Proposed, Rule};
use serde::{Deserialize, Serialize};

use crate::{CommandError, CommandResult, Commands};

/// A proposed rule and its account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleProposal {
    pub account: EmailAddress,
    pub proposed: Proposed,
}

impl Commands {
    /// Keep `rule` for the person to check. Returns the proposal's id.
    pub fn propose_rule(
        &self,
        account: &EmailAddress,
        rule: Rule,
        agent: &str,
        reason: Option<String>,
    ) -> CommandResult<u32> {
        self.check(account)?;
        rule.check()
            .map_err(|e| CommandError::Invalid(e.to_string()))?;
        Ok(self.store.update_rules(account, |_, local| {
            let id = local.proposed.iter().map(|p| p.id + 1).max().unwrap_or(1);
            local.proposed.push(Proposed {
                id,
                rule,
                agent: agent.to_owned(),
                reason,
            });
            id
        })?)
    }

    /// Every proposal waiting, account by account, oldest first.
    pub fn rule_proposals(&self) -> CommandResult<Vec<RuleProposal>> {
        let mut all = Vec::new();
        for account in self.visible_accounts()? {
            let (_, local) = self.store.rules(&account.address)?;
            all.extend(local.proposed.into_iter().map(|proposed| RuleProposal {
                account: account.address.clone(),
                proposed,
            }));
        }
        Ok(all)
    }

    /// The proposal is saved or discarded: it goes.
    pub fn settle_rule_proposal(&self, account: &EmailAddress, id: u32) -> CommandResult<()> {
        self.check(account)?;
        self.store
            .update_rules(account, |_, local| local.proposed.retain(|p| p.id != id))?;
        Ok(())
    }
}
