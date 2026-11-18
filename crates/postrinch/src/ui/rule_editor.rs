//! The rule editor, after its mockup: name, when, what to look for, what
//! to do, and beside them what the rule matches among the mail already
//! here. Saving a rule that forwards asks once.

use postrinch_rules::{Category, Match, When};
use rinch::prelude::*;

use crate::dates;
use crate::rule_draft::{
    DOES, Does, Draft, FIELDS, Looks, does_named, field_named, how_named, hows, needs_value,
};
use crate::rules_ui::RulesUi;
use crate::state::State;

fn options(names: &[&str]) -> Vec<SelectOption> {
    names.iter().map(|n| SelectOption::new(*n, *n)).collect()
}

fn draft_get<T>(ui: RulesUi, read: impl FnOnce(&Draft) -> T, or: T) -> T {
    ui.draft.with(|d| d.as_ref().map(read)).unwrap_or(or)
}

#[component]
pub fn rule_editor(state: State) -> NodeHandle {
    let ui = state.rules;
    rsx! {
        div { class: "rule-dialogs",
        Modal {
            opened_fn: move || ui.draft.get().is_some() && !ui.confirm_forward.get(),
            onclose: move || ui.close(),
            title: "",
            size: "xl",
            for _open in ui.draft.get().map(|d| d.id) {
                div { key: format!("{_open}"), class: "rule-editor",
                    {editor_body(__scope, state)}
                }
            }
        }
        Modal {
            opened_fn: move || ui.confirm_forward.get(),
            onclose: move || ui.confirm_forward.set(false),
            title: "",
            size: "sm",
            div { class: "dialog",
                h2 { class: "dialog-title", "This rule sends mail out of your account" }
                p { class: "dialog-hint", {move || forward_text(ui)} }
                p { class: "dialog-hint", "Turning the rule off or deleting it stops the forwarding." }
                div { class: "dialog-actions",
                    Button { variant: "default", size: "sm", onclick: move || ui.confirm_forward.set(false), "Cancel" }
                    Button { variant: "filled", size: "sm", onclick: move || ui.save(true), "Save and forward" }
                }
            }
        }
        }
    }
}

fn forward_text(ui: RulesUi) -> String {
    let rule = draft_get(ui, Draft::rule, postrinch_rules::Rule::new(0, ""));
    let to: Vec<String> = rule
        .actions
        .iter()
        .filter_map(|a| match a {
            postrinch_rules::Action::Forward(to) => Some(to.clone()),
            _ => None,
        })
        .collect();
    let line = summarize_conditions(&rule);
    format!(
        "Every message {line} will be forwarded to {} as it arrives, without asking each time.",
        to.join(" and ")
    )
}

fn summarize_conditions(rule: &postrinch_rules::Rule) -> String {
    let whole = postrinch_rules::summarize(rule, &|l| l.to_owned(), &|a| a.to_owned());
    let conditions = whole.split(" \u{2192} ").next().unwrap_or("").to_owned();
    let mut chars = conditions.chars();
    match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[component]
fn editor_body(state: State) -> NodeHandle {
    let ui = state.rules;
    let name = Signal::new(draft_get(ui, |d| d.name.clone(), String::new()));
    let is_new = draft_get(ui, |d| d.id == 0, true);
    rsx! {
        div { class: "rule-editor-body",
            div { class: "rule-editor-head",
                h2 { class: "dialog-title", {if is_new { "New rule" } else { "Edit rule" }} }
                span { class: "spacer" }
                span { class: "rule-editor-account", {move || ui.editing_account.get().map(|a| a.to_string()).unwrap_or_default()} }
            }
            div { class: "rule-editor-columns",
                div { class: "rule-editor-form",
                    div { class: "rule-form-row",
                        span { class: "rule-form-label", "Name" }
                        TextInput {
                            value_fn: move || name.get(),
                            oninput: move |v: String| {
                                name.set(v.clone());
                                ui.draft.update(|d| if let Some(d) = d { d.name = v });
                            },
                        }
                    }
                    div { class: "rule-form-row",
                        span { class: "rule-form-label", "When" }
                        div { class: "rule-segs", role: "group", aria-label: "When the rule runs",
                            {when_chip(__scope, ui, When::Arrives, "Mail arrives")}
                            {when_chip(__scope, ui, When::Waited, "Mail has been in the inbox a while")}
                        }
                    }
                    div { class: "rule-section",
                        div { class: "rule-form-row",
                            span { "If" }
                            Select {
                                data: options(&["all", "any"]),
                                value_fn: move || if draft_get(ui, |d| d.matching == Match::Any, false) { "any".into() } else { "all".into() },
                                onchange: move |v: String| ui.change(|d| d.matching = if v == "any" { Match::Any } else { Match::All }),
                            }
                            span { "of these are true" }
                        }
                        for row in draft_get(ui, |d| d.looks.clone(), Vec::new()) {
                            div { key: format!("{}|{}|{}", row.id, row.field, row.how),
                                {looks_row(__scope, state, row.clone())}
                            }
                        }
                        button {
                            class: "rule-add",
                            onclick: move || ui.change(|d| d.add_looks("From", "is", String::new())),
                            "+ Add a condition"
                        }
                    }
                    div { class: "rule-section",
                        span { class: "rule-form-label", "Then" }
                        for row in draft_get(ui, |d| d.does.clone(), Vec::new()) {
                            div { key: format!("{}|{}", row.id, row.what),
                                {does_row(__scope, state, row.clone())}
                            }
                        }
                        button {
                            class: "rule-add",
                            onclick: move || ui.change(|d| d.add_does("Add the label", String::new())),
                            "+ Add an action"
                        }
                    }
                    Checkbox {
                        label: "Stop here: later rules don\u{2019}t see mail this rule matched",
                        checked_fn: move || draft_get(ui, |d| d.stop, false),
                        onchange: move || ui.change(|d| d.stop = !d.stop),
                    }
                }
                {already_here(__scope, state)}
            }
            for error in ui.error.get() {
                p { key: error.clone(), class: "rule-error", {error.clone()} }
            }
            div { class: "dialog-actions rule-editor-foot",
                if !is_new {
                    Button { variant: "subtle", color: "red", size: "sm", onclick: move || ui.delete(), "Delete rule" }
                }
                span { class: "spacer" }
                Button { variant: "default", size: "sm", onclick: move || ui.close(), "Cancel" }
                Button { variant: "filled", size: "sm", onclick: move || ui.save(false), "Save" }
            }
        }
    }
}

fn when_chip(
    __scope: &mut RenderScope,
    ui: RulesUi,
    which: When,
    label: &'static str,
) -> NodeHandle {
    rsx! {
        button {
            class: {move || if draft_get(ui, |d| d.when == which, false) { "rule-seg chosen" } else { "rule-seg" }},
            aria-pressed: {move || if draft_get(ui, |d| d.when == which, false) { "true" } else { "false" }},
            onclick: move || ui.change(|d| d.when = which),
            {label}
        }
    }
}

#[component]
fn looks_row(state: State, row: Looks) -> NodeHandle {
    let ui = state.rules;
    let id = row.id;
    let value = Signal::new(row.value.clone());
    let set = move |v: String| {
        value.set(v.clone());
        ui.change(|d| {
            if let Some(r) = d.looks.iter_mut().find(|r| r.id == id) {
                r.value = v;
            }
        });
    };
    let field_names: Vec<&str> = FIELDS.iter().map(|(f, _)| *f).collect();
    let field = row.field;
    let input: NodeHandle = if row.how == "labelled" {
        label_select(__scope, ui, value, set)
    } else if needs_value(row.how) {
        let hint = placeholder(row.field, row.how);
        rsx! { TextInput { placeholder: hint, value_fn: move || value.get(), oninput: set } }
    } else {
        rsx! { span { class: "rule-value-none" } }
    };
    rsx! {
        div { class: "rule-line-row",
            div { class: "rule-pick",
                Select {
                    data: options(&field_names),
                    value_fn: move || field.to_owned(),
                    onchange: move |v: String| ui.change(|d| {
                        if let Some(r) = d.looks.iter_mut().find(|r| r.id == id) {
                            r.field = field_named(&v);
                            r.how = hows(r.field)[0];
                        }
                    }),
                }
            }
            div { class: "rule-pick wide",
                Select {
                    data: options(hows(row.field)),
                    value_fn: move || row.how.to_owned(),
                    onchange: move |v: String| ui.change(|d| {
                        if let Some(r) = d.looks.iter_mut().find(|r| r.id == id) {
                            r.how = how_named(r.field, &v);
                        }
                    }),
                }
            }
            div { class: "rule-value", {input} }
            button {
                class: "rule-remove",
                aria-label: "Remove this condition",
                onclick: move || ui.change(|d| d.looks.retain(|r| r.id != id)),
                "\u{2715}"
            }
        }
    }
}

fn placeholder(field: &str, how: &str) -> &'static str {
    match (field, how) {
        ("From", "domain is") => "example.com",
        ("From" | "Sent to", "has the words") => "Words in the name",
        ("From" | "Sent to", _) => "someone@example.com",
        ("Attachment", "file type is") => "pdf",
        ("Attachment", _) => "report.pdf",
        ("Size", _) => "5",
        ("Mail is", _) => "7",
        ("Gmail search", _) => "Gmail\u{2019}s search words",
        _ => "Words",
    }
}

fn label_select(
    __scope: &mut RenderScope,
    ui: RulesUi,
    value: Signal<String>,
    set: impl Fn(String) + 'static,
) -> NodeHandle {
    let labels: Vec<SelectOption> = ui
        .labels
        .get()
        .into_iter()
        .map(|l| SelectOption::new(l.label, l.name))
        .collect();
    rsx! {
        Select { data: labels, placeholder: "Pick a label", value_fn: move || value.get(), onchange: move |v: String| set(v) }
    }
}

#[component]
fn does_row(state: State, row: Does) -> NodeHandle {
    let ui = state.rules;
    let id = row.id;
    let value = Signal::new(row.value.clone());
    let prompt = Signal::new(row.prompt.clone());
    let set = move |v: String| {
        value.set(v.clone());
        ui.change(|d| {
            if let Some(r) = d.does.iter_mut().find(|r| r.id == id) {
                r.value = v;
            }
        });
    };
    let extra: NodeHandle = match row.what {
        "Add the label" => label_select(__scope, ui, value, set),
        "Put in the category" => {
            let items: Vec<SelectOption> = Category::ALL
                .iter()
                .map(|c| SelectOption::new(c.name(), c.name()))
                .collect();
            rsx! { Select { data: items, placeholder: "Pick a category", value_fn: move || value.get(), onchange: move |v: String| set(v) } }
        }
        "Forward to" => forward_pick(__scope, ui, value, set),
        "Ask an agent" => {
            let agents: Vec<SelectOption> = ui
                .agents
                .get()
                .into_iter()
                .map(|(id, name)| SelectOption::new(id, name))
                .collect();
            let inner = if agents.is_empty() {
                rsx! { span { class: "rule-hint", "Connect Claude Code in Settings \u{203a} Agents first." } }
            } else {
                rsx! {
                    div { class: "rule-agent",
                        Select { data: agents, placeholder: "Pick an agent", value_fn: move || value.get(), onchange: move |v: String| set(v) }
                        TextInput {
                            placeholder: "What to do, such as \u{201c}draft a reply saying I\u{2019}ll be there\u{201d}",
                            value_fn: move || prompt.get(),
                            oninput: move |v: String| {
                                prompt.set(v.clone());
                                ui.change(|d| {
                                    if let Some(r) = d.does.iter_mut().find(|r| r.id == id) {
                                        r.prompt = v;
                                    }
                                });
                            },
                        }
                    }
                }
            };
            rsx! { div { class: "rule-agent", {inner} } }
        }
        _ => rsx! { span { class: "rule-value-none" } },
    };
    rsx! {
        div { class: "rule-line-row",
            div { class: "rule-pick",
                Select {
                    data: options(DOES),
                    value_fn: move || row.what.to_owned(),
                    onchange: move |v: String| ui.change(|d| {
                        if let Some(r) = d.does.iter_mut().find(|r| r.id == id) {
                            r.what = does_named(&v);
                            r.value.clear();
                        }
                    }),
                }
            }
            div { class: "rule-value", {extra} }
            button {
                class: "rule-remove",
                aria-label: "Remove this action",
                onclick: move || ui.change(|d| d.does.retain(|r| r.id != id)),
                "\u{2715}"
            }
        }
    }
}

fn forward_pick(
    __scope: &mut RenderScope,
    ui: RulesUi,
    value: Signal<String>,
    set: impl Fn(String) + 'static,
) -> NodeHandle {
    let addresses: Vec<SelectOption> = ui
        .page
        .get()
        .map(|p| p.forwarding)
        .unwrap_or_default()
        .into_iter()
        .map(|a| SelectOption::new(a.clone(), a))
        .collect();
    let pick: Option<NodeHandle> = (!addresses.is_empty()).then(|| {
        rsx! { Select { data: addresses, placeholder: "Pick an address", value_fn: move || value.get(), onchange: move |v: String| set(v) } }
    });
    rsx! {
        div { class: "rule-agent",
            {pick}
            span { class: "rule-hint",
                "Only addresses Gmail has verified for forwarding. Add one in Gmail\u{2019}s settings, under Forwarding."
            }
        }
    }
}

#[component]
fn already_here(state: State) -> NodeHandle {
    let ui = state.rules;
    rsx! {
        aside { class: "rule-here",
            div { class: "rule-here-head",
                span { class: "rule-here-title", {move || here_title(ui)} }
                span { class: "rule-hint", "From the last 3 months. The rule acts on new mail." }
            }
            div { class: "rule-here-list",
                for hit in ui.here.get().map(|h| h.threads).unwrap_or_default().into_iter().take(8) {
                    div { key: hit.thread.0.clone(), class: "rule-hit",
                        div { class: "rule-hit-top",
                            span { class: "rule-hit-from", {hit.from.clone()} }
                            span { class: "when", {dates::short(hit.received, &dates::now())} }
                        }
                        span { class: "rule-hit-subject", {hit.subject.clone()} }
                    }
                }
            }
            if ui.here.get().is_some_and(|h| h.messages > 0) {
                div { class: "rule-here-foot",
                    Checkbox {
                        label: {move || existing_label(ui)},
                        checked_fn: move || draft_get(ui, |d| d.existing, false),
                        onchange: move || ui.change(|d| d.existing = !d.existing),
                    }
                }
            }
        }
    }
}

fn here_title(ui: RulesUi) -> String {
    match ui.here.get() {
        None => "Already here".into(),
        Some(h) if h.threads.is_empty() => "Already here: nothing matches".into(),
        Some(h) if h.threads.len() == 1 => "Already here: 1 conversation matches".into(),
        Some(h) => format!("Already here: {} conversations match", h.threads.len()),
    }
}

fn existing_label(ui: RulesUi) -> String {
    let n = ui.here.get().map_or(0, |h| h.threads.len());
    let these = if n == 1 {
        "this one".to_owned()
    } else {
        format!("these {n}")
    };
    let agent = draft_get(
        ui,
        |d| d.does.iter().any(|r| r.what == "Ask an agent"),
        false,
    );
    if agent {
        format!("Also do this to {these} now (agents are asked only about new mail)")
    } else {
        format!("Also do this to {these} now")
    }
}

pub const CSS: &str = r#"
.rule-editor-body { display: flex; flex-direction: column; gap: 12px; }
.rule-editor-head { display: flex; align-items: center; gap: 12px; }
.rule-editor-account { font-size: 13px; color: var(--muted); }
.rule-editor-columns { display: flex; gap: 16px; min-height: 0; }
.rule-editor-form { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 14px; }
.rule-form-row { display: flex; align-items: center; gap: 8px; font-size: 13px; color: var(--ink-2); }
.rule-form-label { width: 56px; font-size: 13px; color: var(--muted); }
.rule-form-row .rinch-text-input { width: 320px; }
.rule-segs { display: flex; gap: 2px; padding: 2px; border: 1px solid var(--field); border-radius: 7px; }
.rule-seg { height: 28px; padding: 0 12px; border: none; border-radius: 5px; background: transparent; color: var(--ink-2); font-size: 13px; cursor: pointer; }
.rule-seg.chosen { background: var(--accent-tint); color: var(--accent-ink); }
.rule-section { display: flex; flex-direction: column; gap: 8px; }
.rule-line-row { display: flex; align-items: flex-start; gap: 8px; }
.rule-pick { width: 150px; flex-shrink: 0; }
.rule-pick.wide { width: 190px; }
.rule-value { flex: 1; min-width: 0; }
.rule-value-none { display: block; }
.rule-agent { display: flex; flex-direction: column; gap: 6px; }
.rule-remove { width: 28px; height: 30px; border: none; border-radius: 5px; background: transparent; color: var(--muted); cursor: pointer; }
.rule-remove:hover { background: var(--hover); }
.rule-add { align-self: flex-start; border: none; background: transparent; color: var(--accent); font-size: 13px; padding: 4px 0; cursor: pointer; }
.rule-hint { font-size: 12px; color: var(--muted); line-height: 1.4; }
.rule-error { margin: 0; font-size: 13px; color: var(--danger); }
.rule-here { width: 300px; flex-shrink: 0; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); display: flex; flex-direction: column; }
.rule-here-head { padding: 12px 14px 8px; display: flex; flex-direction: column; gap: 3px; }
.rule-here-title { font-size: 13px; font-weight: 600; }
.rule-here-list { border-top: 1px solid var(--line-soft); display: flex; flex-direction: column; }
.rule-hit { padding: 8px 12px; border-bottom: 1px solid var(--line-soft); display: flex; flex-direction: column; gap: 2px; font-size: 13px; }
.rule-hit-top { display: flex; gap: 8px; }
.rule-hit-from { flex: 1; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.rule-hit-subject { color: var(--muted); overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.rule-here-foot { margin-top: auto; padding: 12px 14px; border-top: 1px solid var(--line-soft); font-size: 13px; }
.rule-editor-foot { align-items: center; }
"#;
