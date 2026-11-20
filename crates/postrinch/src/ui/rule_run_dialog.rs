//! Settings › Rules' "Run now" dialog, after its mockup: the last three
//! months' mail, or all of it.

use rinch::prelude::*;

use crate::state::State;

#[component]
pub fn run_dialog(state: State) -> NodeHandle {
    let run = state.rules.run;
    rsx! {
        Modal {
            opened_fn: move || run.asking.get().is_some(),
            onclose: move || run.asking.set(None),
            title: "",
            size: "sm",
            // Keyed on the rule, so choosing between the two keeps the
            // dialog as it is.
            for (account, id, name) in run.asking.get().map(|a| (a.account.clone(), a.rule.id, a.rule.name.clone())) {
                div { key: format!("{}|{id}", account.key()), class: "dialog",
                    h2 { class: "dialog-title", {format!("Run \u{201c}{name}\u{201d} now")} }
                    div { class: "run-choices",
                        Radio {
                            name: "run-over",
                            label: "The last 3 months",
                            description: {move || run.recent_text()},
                            checked_fn: move || run.asking.with(|a| a.as_ref().is_some_and(|a| !a.all)),
                            onchange: move || run.choose(false),
                        }
                        Radio {
                            name: "run-over",
                            label: "All mail",
                            description: "Every message is checked, so a big account can take a while. It carries on in the background.",
                            checked_fn: move || run.asking.with(|a| a.as_ref().is_some_and(|a| a.all)),
                            onchange: move || run.choose(true),
                        }
                    }
                    p { class: "dialog-hint",
                        "It changes mail as the rule does. It forwards nothing and asks no agent: those are only for new mail."
                    }
                    div { class: "dialog-actions",
                        Button { variant: "default", size: "sm", onclick: move || run.asking.set(None), "Cancel" }
                        Button { variant: "filled", size: "sm", onclick: move || run.run(), "Run" }
                    }
                }
            }
        }
    }
}

pub const CSS: &str = r#"
.run-choices { display: flex; flex-direction: column; gap: 12px; margin: 4px 0 6px; }
.rule-run {
    height: 26px; padding: 0 10px; border: 1px solid var(--field); border-radius: 6px;
    background: transparent; color: var(--ink-2); font-size: 12px; cursor: pointer;
}
.rule-run:hover { background: var(--hover); }
.rule-run:disabled { opacity: 0.5; cursor: default; }
.rule-ran { font-size: 12px; color: var(--muted); }
.rule-ran.done { color: var(--ink-2); }
"#;
