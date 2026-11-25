//! What an agent asks of the account's provider itself rather than of
//! what is kept here: making, renaming and deleting labels (an IMAP
//! account's folders), and changing Google accounts' calendars. These are
//! done at once, so the agent's next call can use what it made; the app
//! gives the server a [`Remote`] when sync runs. Calendars are Google
//! accounts' only: a calendar call for an IMAP account is refused before
//! it gets here ([`Commands::check_calendars`]).

use postrinch_commands::Commands;
use postrinch_model::EmailAddress;
use serde_json::{Value, json};

use crate::views::say;
use crate::wire::Call;

/// The provider's calls, for Google and IMAP accounts alike, each
/// answering in a sentence when it fails.
pub trait Remote: Send + Sync {
    /// Make a label; returns its id.
    fn create_label(&self, account: &EmailAddress, name: &str) -> Result<String, String>;
    fn rename_label(&self, account: &EmailAddress, id: &str, name: &str) -> Result<(), String>;
    fn delete_label(&self, account: &EmailAddress, id: &str) -> Result<(), String>;
    /// Make an event (`event` none) or change one; returns its id. Its
    /// guests are told: only called for changes that tell nobody, or that
    /// the person made.
    fn save_event(
        &self,
        account: &EmailAddress,
        event: Option<&str>,
        draft: &postrinch_model::EventDraft,
    ) -> Result<String, String>;
    fn delete_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        event: &str,
        tell: bool,
    ) -> Result<(), String>;
    /// Who one of the account's own calendars is shared with.
    fn calendar_shares(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> Result<Vec<postrinch_model::CalendarShare>, String>;
    /// Add a calendar shared with the account to its calendars.
    fn add_calendar(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> Result<postrinch_model::CalendarInfo, String>;
}

const NO_REMOTE: &str =
    "Postrinch can't reach the account's mail server now (sync is off), so it can't change labels.";

pub(crate) fn run(
    commands: &Commands,
    call: Call,
    remote: Option<&dyn Remote>,
) -> Result<Value, String> {
    match call {
        Call::CreateLabel { account, name } => {
            let name = label_name(&name)?;
            commands.check(&account).map_err(say)?;
            // Asking twice makes one label.
            if let Some(same) = find(commands, &account, &name)? {
                return Ok(json!({ "label": same.0, "name": same.1, "made": false }));
            }
            let id = remote.ok_or(NO_REMOTE)?.create_label(&account, &name)?;
            Ok(json!({ "label": id, "name": name, "made": true }))
        }
        Call::RenameLabel {
            account,
            label,
            name,
        } => {
            let name = label_name(&name)?;
            let (id, _) = user_label(commands, &account, &label)?;
            if let Some((other, _)) = find(commands, &account, &name)?
                && other != id
            {
                return Err(format!("{account} already has a label named {name:?}."));
            }
            remote
                .ok_or(NO_REMOTE)?
                .rename_label(&account, &id, &name)?;
            Ok(json!({ "label": id, "name": name }))
        }
        Call::DeleteLabel { account, label } => {
            let (id, name) = user_label(commands, &account, &label)?;
            let page = commands.rules_page(&account).map_err(say)?;
            if let Some(rule) = page.rules.iter().find(|r| uses(r, &id)) {
                return Err(format!(
                    "Rule {} ({:?}) uses the label {name:?}: change or delete the rule first.",
                    rule.id, rule.name
                ));
            }
            remote.ok_or(NO_REMOTE)?.delete_label(&account, &id)?;
            Ok(json!({ "deleted": id, "name": name }))
        }
        _ => unreachable!("only label calls come here"),
    }
}

/// A label's name as the provider takes it: `Parent/Child` nests.
fn label_name(name: &str) -> Result<String, String> {
    let parts: Vec<&str> = name.split('/').map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty()) {
        return Err(format!(
            "{name:?} is not a label name: give it a name (`Parent/Child` puts it under another)."
        ));
    }
    Ok(parts.join("/"))
}

/// The person's label with this id or name, as (id, name).
fn find(
    commands: &Commands,
    account: &EmailAddress,
    label: &str,
) -> Result<Option<(String, String)>, String> {
    let labels = commands.user_labels(account).map_err(say)?;
    Ok(labels
        .iter()
        .find(|l| l.provider_id == label)
        .or_else(|| labels.iter().find(|l| l.name.eq_ignore_ascii_case(label)))
        .map(|l| (l.provider_id.clone(), l.name.clone())))
}

fn user_label(
    commands: &Commands,
    account: &EmailAddress,
    label: &str,
) -> Result<(String, String), String> {
    commands.check(account).map_err(say)?;
    find(commands, account, label)?.ok_or_else(|| {
        format!(
            "{account} has no label {label:?} of the person's. list_labels shows the ones it has."
        )
    })
}

fn uses(rule: &postrinch_rules::Rule, id: &str) -> bool {
    use postrinch_rules::{Action, Test};
    rule.actions
        .iter()
        .any(|a| matches!(a, Action::AddLabel(l) if l == id))
        || rule
            .conditions
            .iter()
            .any(|c| matches!(&c.test, Test::HasLabel(l) if l == id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_names_nest_and_are_trimmed() {
        assert_eq!(label_name(" Personal ").unwrap(), "Personal");
        assert_eq!(label_name("Work / Clients").unwrap(), "Work/Clients");
        assert!(label_name("").is_err());
        assert!(label_name("Work/").is_err());
    }
}
