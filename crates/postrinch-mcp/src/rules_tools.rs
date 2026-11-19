//! The rules tools: what Settings › Rules shows and does, for agents. A
//! rule that forwards mail or asks an agent is proposed to the person
//! rather than saved.

use postrinch_agents::Call;
use postrinch_agents::rule_shape::RuleShape;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::tools::{AccountArgs, Postrinch, address};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveRuleArgs {
    /// The account's address.
    pub account: String,
    /// To change a rule: its id, as list_rules gives it. Leave it out to
    /// make a new rule, which goes last.
    #[serde(default)]
    pub id: Option<u32>,
    /// The rule. Conditions (`test`, `value`, `not`): from, from_domain,
    /// from_words, sent_to, to, cc, subject_words, subject_phrase, words,
    /// phrase, has_attachment, attachment_type, attachment_name,
    /// larger_than_mb, smaller_than_mb, unread, starred, label, in_inbox,
    /// older_than_days (only with when `waited`), gmail_search. Actions
    /// (`action`, `value`, `prompt`): add_label, skip_inbox, mark_read,
    /// star, mark_important, never_important, category, never_spam, trash,
    /// forward, ask_agent. Labels by name or id, as list_labels gives them.
    pub rule: RuleShape,
    /// Also do it now to the matching mail of the last three months. Default
    /// false.
    #[serde(default)]
    pub apply_to_existing: bool,
    /// Why you made it, shown to the person if the rule waits for them.
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuleArgs {
    /// The account's address.
    pub account: String,
    /// The rule's id, as list_rules gives it.
    pub id: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RuleOnArgs {
    /// The account's address.
    pub account: String,
    /// The rule's id, as list_rules gives it.
    pub id: u32,
    /// true to switch it on, false to switch it off (kept, not run).
    pub on: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoveRuleArgs {
    /// The account's address.
    pub account: String,
    /// The rule's id, as list_rules gives it.
    pub id: u32,
    /// Its place in the order rules run, 0 first.
    pub to: usize,
}

#[tool_router(router = rules_router, vis = "pub(crate)")]
impl Postrinch {
    /// An account's mail rules, in the order they run: each one's id,
    /// whether it is on, a summary in a line, whether Gmail runs it (even
    /// while Postrinch is closed) or Postrinch does, and the rule itself in
    /// the shape save_rule takes. Also the filters made in Gmail's own
    /// settings, the addresses Gmail lets rules forward to, and rules
    /// waiting for the person.
    #[tool(name = "list_rules")]
    async fn list_rules(
        &self,
        Parameters(args): Parameters<AccountArgs>,
    ) -> Result<String, String> {
        self.call(Call::Rules {
            account: address(&args.account)?,
        })
        .await
    }

    /// Make a mail rule, or change one: it runs on mail as it arrives (or,
    /// with when `waited`, on mail as it ages), from now on, in this account.
    /// A rule that labels, files or flags mail is saved at once; the person
    /// can change it, switch it off or delete it in Settings › Rules. A rule
    /// that forwards mail out of the account or asks an agent is the
    /// person's to save: it waits for them in Postrinch with your reason,
    /// and nothing happens until they save it.
    #[tool(name = "save_rule")]
    async fn save_rule(
        &self,
        Parameters(args): Parameters<SaveRuleArgs>,
    ) -> Result<String, String> {
        self.call(Call::SaveRule {
            account: address(&args.account)?,
            rule_id: args.id,
            rule: args.rule,
            apply_to_existing: args.apply_to_existing,
            reason: args.reason,
        })
        .await
    }

    /// Delete a rule. Mail it already changed stays as it is.
    #[tool(name = "delete_rule")]
    async fn delete_rule(&self, Parameters(args): Parameters<RuleArgs>) -> Result<String, String> {
        self.call(Call::DeleteRule {
            account: address(&args.account)?,
            rule_id: args.id,
        })
        .await
    }

    /// Switch a rule on or off. An off rule is kept but does nothing.
    #[tool(name = "set_rule_on")]
    async fn set_rule_on(
        &self,
        Parameters(args): Parameters<RuleOnArgs>,
    ) -> Result<String, String> {
        self.call(Call::SetRuleOn {
            account: address(&args.account)?,
            rule_id: args.id,
            on: args.on,
        })
        .await
    }

    /// Move a rule in the order rules run; a rule that says stop keeps the
    /// ones after it from seeing the mail it matched. Answers with the
    /// rules in their new order.
    #[tool(name = "move_rule")]
    async fn move_rule(
        &self,
        Parameters(args): Parameters<MoveRuleArgs>,
    ) -> Result<String, String> {
        self.call(Call::MoveRule {
            account: address(&args.account)?,
            rule_id: args.id,
            to: args.to,
        })
        .await
    }
}
