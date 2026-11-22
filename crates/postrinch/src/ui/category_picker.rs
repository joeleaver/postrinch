//! Gmail's categories a view looks for (Has: any of them) or leaves out
//! (Not), each with its Has and Not as a label's match row has them
//! (chooser.rs). Only five, so all five are listed, with no finding. Shown
//! only while an account has Gmail's categories.

use postrinch_commands::CATEGORY_NAMES;
use postrinch_model::Provider;
use rinch::prelude::*;

use crate::state::State;
use crate::ui::chooser::{Offer, row};
use crate::ui::view_panel_mail::{change, filters};

#[component]
pub fn category_picker(state: State) -> NodeHandle {
    let gmail = Memo::new(move || {
        state.sidebar.with(|s| {
            s.iter()
                .flat_map(|s| &s.accounts)
                .any(|a| a.account.provider == Provider::Google)
        })
    });
    let asked = move |name: &str| filters(state).category(name);
    let set = move |name: String, pick| change(state, move |f| f.pick_category(&name, pick));
    let rows: Vec<NodeHandle> = CATEGORY_NAMES
        .iter()
        .map(|name| {
            let mut shown = name.to_string();
            shown[..1].make_ascii_uppercase();
            let offer = Offer {
                value: (*name).to_owned(),
                shown,
                faint: String::new(),
                dot: None,
            };
            row(__scope, asked, set, offer)
        })
        .collect();
    rsx! {
        div {
            class: "panel-field chooser",
            style: {move || if gmail.get() { "" } else { "display: none" }},
            span { class: "panel-name", "Category" }
            div { class: "chooser-list categories", {rows} }
        }
    }
}

pub const CSS: &str = r#"
.chooser-list.categories { max-height: none; }
"#;
