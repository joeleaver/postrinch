//! Making a label, after its mockup: from the "+" by Labels in the
//! sidebar, or "New label inside…" on a label's menu. It is made at the
//! account's provider at once (a folder, for an IMAP account), so it shows
//! on the person's phone and the web too.

use postrinch_agents::Remote;
use postrinch_model::EmailAddress;
use rinch::prelude::*;

use crate::actions::{commands, sync};
use crate::remote_mail::RemoteMail;
use crate::state::State;

/// The label being made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLabel {
    pub account: EmailAddress,
    /// Gmail's name of the label to put it under; empty for none.
    pub parent: String,
    pub name: String,
    pub error: Option<String>,
    pub busy: bool,
}

impl NewLabel {
    pub fn new(account: EmailAddress, parent: String) -> Self {
        Self {
            account,
            parent,
            name: String::new(),
            error: None,
            busy: false,
        }
    }

    /// The whole name Gmail is given, or why there is none.
    fn full_name(&self) -> Result<String, String> {
        let name = self.name.trim().trim_matches('/').trim();
        if name.is_empty() {
            return Err("Give the label a name.".into());
        }
        Ok(if self.parent.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.parent)
        })
    }
}

fn change(state: State, edit: impl FnOnce(&mut NewLabel)) {
    state.making.update(|m| {
        if let Some(m) = m {
            edit(m);
            m.error = None;
        }
    });
}

/// Make the label in Gmail, off the window's thread, and close the dialog
/// once it is made.
fn create(state: State) {
    let Some(making) = state.making.get().filter(|m| !m.busy) else {
        return;
    };
    let full = match making.full_name() {
        Ok(full) => full,
        Err(e) => {
            return state
                .making
                .update(|m| m.iter_mut().for_each(|m| m.error = Some(e.clone())));
        }
    };
    let taken = commands()
        .and_then(|c| c.user_labels(&making.account).ok())
        .is_some_and(|labels| labels.iter().any(|l| l.name.eq_ignore_ascii_case(&full)));
    let refuse = |why: String| {
        state
            .making
            .update(|m| m.iter_mut().for_each(|m| m.error = Some(why.clone())))
    };
    if taken {
        return refuse(format!(
            "There is already a label called \u{201c}{full}\u{201d}."
        ));
    }
    let Some(sync) = sync() else {
        return refuse(
            "Postrinch can\u{2019}t reach the account\u{2019}s mail server now (sync is off), so it can\u{2019}t make labels."
                .into(),
        );
    };
    state
        .making
        .update(|m| m.iter_mut().for_each(|m| m.busy = true));
    let (making_signal, revision) = (state.making, state.revision);
    std::thread::spawn(
        move || match RemoteMail::of(&sync).create_label(&making.account, &full) {
            Ok(_) => {
                making_signal.send(None);
                revision.update_send(|n| *n += 1);
            }
            Err(e) => making_signal.update_send(move |m| {
                if let Some(m) = m {
                    m.busy = false;
                    m.error = Some(e);
                }
            }),
        },
    );
}

/// The labels a new one can go under, as (Gmail's name, the name shown).
fn parents(account: &EmailAddress) -> Vec<SelectOption> {
    let Some(commands) = commands() else {
        return Vec::new();
    };
    let names = commands.user_labels(account).unwrap_or_default();
    // An empty value shows the placeholder, "Nothing".
    let mut options = vec![SelectOption::new("", "Nothing")];
    for choice in commands.label_choices(account).unwrap_or_default() {
        if let Some(label) = names.iter().find(|l| l.provider_id == choice.label) {
            options.push(SelectOption::new(label.name.clone(), choice.name));
        }
    }
    options
}

#[component]
pub fn new_label_dialog(state: State) -> NodeHandle {
    rsx! {
        Modal {
            opened_fn: move || state.making.get().is_some(),
            onclose: move || state.making.set(None),
            title: "",
            size: "sm",
            // Keyed on the account, so its labels are the ones to put the
            // new one under.
            for account in state.making.get().map(|m| m.account) {
                div { key: account.key(), class: "dialog new-label",
                    {body(__scope, state, account.clone())}
                }
            }
        }
    }
}

#[component]
fn body(state: State, account: EmailAddress) -> NodeHandle {
    let read = move |f: fn(&NewLabel) -> String| {
        state.making.with(|m| m.as_ref().map(f).unwrap_or_default())
    };
    let accounts: Vec<SelectOption> = crate::ui::labels::sidebar_accounts(state)
        .into_iter()
        .map(|a| SelectOption::new(a.to_string(), a.to_string()))
        .collect();
    let several = accounts.len() > 1;
    let under = parents(&account);
    rsx! {
        div { class: "new-label-fields",
            h2 { class: "dialog-title", "New label" }
            TextInput {
                spellcheck: false,
                label: "Name",
                value_fn: move || read(|m| m.name.clone()),
                oninput: move |v: String| change(state, |m| m.name = v),
                onsubmit: move || create(state),
            }
            Select {
                label: "Put it under",
                placeholder: "Nothing",
                data: under,
                value_fn: move || read(|m| m.parent.clone()),
                onchange: move |v: String| change(state, |m| m.parent = v),
            }
            if several {
                Select {
                    label: "Account",
                    data: accounts.clone(),
                    value_fn: move || read(|m| m.account.to_string()),
                    onchange: move |v: String| {
                        if let Ok(account) = EmailAddress::parse(&v) {
                            change(state, |m| {
                                m.account = account;
                                m.parent = String::new();
                            });
                        }
                    },
                }
            }
            p { class: "dialog-hint", "Made in Gmail too, so it shows on your phone and the web." }
            // A keyed loop, not `if let` (rinch#2105).
            for error in state.making.get().and_then(|m| m.error) {
                p { key: error.clone(), class: "dialog-trouble", {error.clone()} }
            }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || state.making.set(None), "Cancel" }
                Button {
                    variant: "filled",
                    size: "sm",
                    disabled: {move || state.making.get().is_some_and(|m| m.busy)},
                    onclick: move || create(state),
                    {move || if state.making.get().is_some_and(|m| m.busy) { "Creating\u{2026}" } else { "Create" }}
                }
            }
        }
    }
}

pub const CSS: &str = r#"
.new-label-fields { display: flex; flex-direction: column; gap: 12px; }
.dialog-trouble { margin: 0; font-size: 13px; color: var(--trouble); }
.labels-head { display: flex; align-items: center; }
.labels-head .section-title { flex: 1; }
.label-add {
    width: 22px; height: 22px; margin-right: 4px; border: none; border-radius: 5px;
    background: transparent; color: var(--muted); font-size: 16px; line-height: 1;
    display: flex; align-items: center; justify-content: center; cursor: pointer;
}
.label-add:hover { background: var(--hover); color: var(--ink-2); }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_label_is_named_under_its_parent() {
        let me = EmailAddress::parse("me@example.org").unwrap();
        let mut label = NewLabel::new(me, "Work".into());
        assert!(label.full_name().is_err());
        label.name = " /Contracts/ ".into();
        assert_eq!(label.full_name().unwrap(), "Work/Contracts");
        label.parent.clear();
        assert_eq!(label.full_name().unwrap(), "Contracts");
    }
}
