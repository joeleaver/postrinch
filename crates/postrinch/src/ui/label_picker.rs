//! Labels a view looks for (Has: any of them, or all) or leaves out (Not),
//! chosen as senders and recipients are (chooser.rs), from every account's
//! labels, hidden ones and other apps' too: any of them can be what a
//! view looks for.

use postrinch_commands::{Filters, LabelNode, LabelPick};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::account_color;
use crate::ui::chooser::{Offer, Wiring, chooser};
use crate::ui::view_panel_mail::{change, filters};

#[component]
pub fn label_picker(state: State) -> NodeHandle {
    let chosen = move || {
        let f = filters(state);
        let mut all: Vec<(String, LabelPick)> =
            f.labels.into_iter().map(|l| (l, LabelPick::Has)).collect();
        all.extend(f.without.into_iter().map(|l| (l, LabelPick::Not)));
        all
    };
    let wiring = Wiring {
        chosen,
        offered: move |find: &str| offered(state, &find.to_lowercase()),
        asked: move |name: &str| filters(state).label(name),
        set: move |name: String, pick| change(state, move |f| f.pick_label(&name, pick)),
        finding: |_| {},
    };
    let several = move || filters(state).labels.len() > 1;
    let all = move || filters(state).all_labels;
    let mode = |__scope: &mut RenderScope, want: bool, name: &'static str| {
        rsx! {
            button {
                class: {move || if all() == want { "on" } else { "" }},
                role: "radio",
                aria-checked: {move || if all() == want { "true" } else { "false" }},
                onclick: move || change(state, move |f: &mut Filters| f.all_labels = want),
                {name}
            }
        }
    };
    let (any_of, all_of) = (
        mode(__scope, false, "Any of them"),
        mode(__scope, true, "All of them"),
    );
    let extra = rsx! {
        div {
            class: "chooser-mode",
            role: "radiogroup",
            aria-label: "How many of the labels it needs",
            style: {move || if several() { "" } else { "display: none" }},
            {any_of} {all_of}
        }
    };
    chooser(__scope, "Labels", "Add a label", wiring, Some(extra))
}

/// Every account's labels whose name has `find` in it, each account's in
/// its colour when there are several.
fn offered(state: State, find: &str) -> Vec<Offer> {
    state.sidebar.with(|s| {
        let accounts: Vec<_> = s.iter().flat_map(|s| &s.accounts).collect();
        let several = accounts.len() > 1;
        let mut offers: Vec<Offer> = Vec::new();
        for (at, a) in accounts.into_iter().enumerate() {
            let dot = several.then(|| format!("background: {}", account_color(at)));
            let mut every = Vec::new();
            walk(&a.labels.own, &mut every);
            for app in &a.labels.other_apps {
                walk(&app.labels, &mut every);
            }
            for (full, name) in every {
                let matches =
                    full.to_lowercase().contains(find) || name.to_lowercase().contains(find);
                // The same name in two accounts is one label to search by.
                if matches && !offers.iter().any(|o| o.value == full) {
                    let faint = full[..full.len() - name.len()].to_owned();
                    offers.push(Offer {
                        value: full,
                        shown: name,
                        faint,
                        dot: dot.clone(),
                    });
                }
            }
        }
        offers
    })
}

/// Each label under `nodes`: its whole name and its last part.
fn walk(nodes: &[LabelNode], out: &mut Vec<(String, String)>) {
    for node in nodes {
        if node.entry.is_some() && !node.full.is_empty() && node.full.ends_with(&node.name) {
            out.push((node.full.clone(), node.name.clone()));
        }
        walk(&node.children, out);
    }
}
