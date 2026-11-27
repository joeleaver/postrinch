//! The view panel's Places, after the Social mockup: a chip for each group
//! (every channel, now and later) and each place in the view, from any
//! account on any provider, with how it tells the person of new messages
//! (🔔 ▾, changed at once: notices belong to the place or group) and ✕ to
//! take it out of the view; then "+ Add a place", which finds the groups
//! and places known to add.

use postrinch_commands::{SocialGroupInfo, SocialPlaceInfo};
use postrinch_store::{GroupRef, Notices, PlaceKind, PlaceRef};
use rinch::prelude::*;

use crate::social_shown::{
    account_domain, notices_choices, notices_name, place_glyph, place_provider,
};
use crate::social_ui::{Choices, is_place, place_ref};
use crate::state::State;
use crate::ui::social_panel::read;

/// A group or a place, as a view names it.
#[derive(Debug, Clone, PartialEq)]
enum Chosen {
    Group(GroupRef),
    Place(PlaceRef),
}

/// A chip or an offer: what it names and what it says.
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    chosen: Chosen,
    /// Said faintly before the name: a channel's group.
    before: String,
    name: String,
    /// Said small after it: where it is, or what a group brings.
    detail: String,
    /// How it tells the person of new messages, for a channel or group.
    notices: Option<Option<Notices>>,
}

fn group_shown(info: &SocialGroupInfo) -> Shown {
    Shown {
        chosen: Chosen::Group(GroupRef {
            account: info.group.account.clone(),
            uri: info.group.uri.clone(),
        }),
        before: String::new(),
        name: info.name.clone(),
        detail: "every channel, now and later".to_owned(),
        notices: Some(info.group.notices),
    }
}

fn place_shown(info: &SocialPlaceInfo) -> Shown {
    let channel = info.place.kind == PlaceKind::Channel;
    Shown {
        chosen: Chosen::Place(place_ref(info)),
        before: info
            .group_name
            .as_ref()
            .map(|g| format!("{g} \u{203a} "))
            .unwrap_or_default(),
        name: format!("{} {}", place_glyph(info), info.name),
        detail: place_provider(info),
        notices: channel.then_some(info.place.notices),
    }
}

/// The view's groups and places, as the chips show them: as known, or by
/// what the view saved until what is known is loaded.
fn chips(edit_groups: &[GroupRef], edit_places: &[PlaceRef], choices: &Choices) -> Vec<Shown> {
    let groups = edit_groups.iter().map(|g| {
        choices
            .groups
            .iter()
            .find(|i| i.group.uri == g.uri && i.group.account.eq_ignore_ascii_case(&g.account))
            .map(group_shown)
            .unwrap_or_else(|| Shown {
                chosen: Chosen::Group(g.clone()),
                before: String::new(),
                name: g.uri.clone(),
                detail: account_domain(&g.account),
                notices: None,
            })
    });
    let places = edit_places.iter().map(|p| {
        choices
            .places
            .iter()
            .find(|i| is_place(i, p))
            .map(place_shown)
            .unwrap_or_else(|| Shown {
                chosen: Chosen::Place(p.clone()),
                before: String::new(),
                name: p.id.clone(),
                detail: account_domain(&p.account),
                notices: None,
            })
    });
    groups.chain(places).collect()
}

/// What "+ Add a place" offers: every group and place known that the view
/// has not, matching what is typed (in its name, group or provider).
fn offers(
    edit_groups: &[GroupRef],
    edit_places: &[PlaceRef],
    choices: &Choices,
    typed: &str,
) -> Vec<Shown> {
    let typed = typed.trim().to_lowercase();
    let matches = |s: &Shown| {
        typed.is_empty()
            || [&s.before, &s.name, &s.detail]
                .iter()
                .any(|t| t.to_lowercase().contains(&typed))
    };
    let groups = choices
        .groups
        .iter()
        .filter(|i| {
            !edit_groups
                .iter()
                .any(|g| g.uri == i.group.uri && g.account.eq_ignore_ascii_case(&i.group.account))
        })
        .map(|i| {
            let mut shown = group_shown(i);
            shown.detail = format!("every channel \u{b7} {}", i.group.provider);
            shown
        });
    let places = choices
        .places
        .iter()
        .filter(|i| !edit_places.iter().any(|p| is_place(i, p)))
        .map(place_shown);
    groups.chain(places).filter(matches).collect()
}

/// Places: the chips, then the finder while adding.
pub fn places_field(__scope: &mut RenderScope, state: State) -> NodeHandle {
    let adding = Signal::new(false);
    let find = Signal::new(String::new());
    let shown = Memo::new(move || {
        let choices = state.social.choices.get();
        read(state, |e| chips(&e.draft.groups, &e.draft.places, &choices))
    });
    let offered = Memo::new(move || {
        let choices = state.social.choices.get();
        let typed = find.get();
        read(state, |e| {
            offers(&e.draft.groups, &e.draft.places, &choices, &typed)
        })
    });
    rsx! {
        div { class: "social-field",
            span { class: "social-label", "Places" }
            div { class: "social-chips",
                for chip in shown.get() {
                    span { key: format!("{chip:?}"), class: "social-chip-slot",
                        {chip_of(__scope, state, chip.clone())}
                    }
                }
                button {
                    class: "social-chip add",
                    aria-expanded: {move || if adding.get() { "true" } else { "false" }},
                    onclick: move || adding.update(|a| *a = !*a),
                    "+ Add a place"
                }
            }
            div {
                class: "social-finder",
                style: {move || if adding.get() { "" } else { "display: none" }},
                TextInput {
                    spellcheck: false,
                    size: "sm",
                    placeholder: "Find a group or channel",
                    value_fn: move || find.get(),
                    oninput: move |v: String| find.set(v),
                }
                div { class: "social-offers",
                    for offer in offered.get() {
                        div { key: format!("{:?}", offer.chosen),
                            {offer_row(__scope, state, offer.clone())}
                        }
                    }
                }
            }
            p { class: "social-hint", "Channels and conversations from any account, on any provider. A group means all its channels, including ones made later." }
        }
    }
}

/// A chip: its name, where it is and its notices, and ✕.
fn chip_of(__scope: &mut RenderScope, state: State, chip: Shown) -> NodeHandle {
    let notices = chip
        .notices
        .map(|now| notices_menu(__scope, state, chip.chosen.clone(), now));
    let take_off = chip.chosen.clone();
    let label = format!("Take {}{} out of the view", chip.before, chip.name);
    let title = label.clone();
    let small = if notices.is_some() {
        format!("{} \u{b7}", chip.detail)
    } else {
        chip.detail.clone()
    };
    rsx! {
        span { class: "social-chip",
            span { {format!("{}{}", chip.before, chip.name)} }
            small { {small} }
            {notices}
            button {
                class: "social-chip-x",
                title: title.clone(),
                aria-label: label.clone(),
                onclick: move || state.change_social_edit(|e| match &take_off {
                    Chosen::Group(g) => e.remove_group(g),
                    Chosen::Place(p) => e.remove_place(p),
                }),
                "\u{2715}"
            }
        }
    }
}

/// 🔔 ▾ on a chip: how the channel or group tells the person of new
/// messages, changed at once.
fn notices_menu(
    __scope: &mut RenderScope,
    state: State,
    chosen: Chosen,
    now: Option<Notices>,
) -> NodeHandle {
    let of_group = matches!(chosen, Chosen::Group(_));
    let open = Signal::new(false);
    let items: Vec<NodeHandle> = notices_choices(of_group)
        .into_iter()
        .map(|(value, name)| {
            let chosen = chosen.clone();
            let on = value == now;
            rsx! {
                button {
                    class: if on { "rinch-dropdown-menu__item order-choice on" } else { "rinch-dropdown-menu__item order-choice" },
                    role: "menuitemradio",
                    aria-checked: if on { "true" } else { "false" },
                    onclick: move || {
                        open.set(false);
                        match &chosen {
                            Chosen::Group(g) => state.set_group_notices(g, value),
                            Chosen::Place(p) => state.set_place_notices(p, value),
                        }
                    },
                    span { class: "order-tick", {if on { "\u{2713}" } else { "" }} }
                    span { class: "rinch-dropdown-menu__item-label", {name} }
                }
            }
        })
        .collect();
    rsx! {
        DropdownMenu {
            opened_fn: move || open.get(),
            on_close: move || open.set(false),
            position: "bottom-start",
            width: "220px",
            DropdownMenuTarget {
                button {
                    class: "social-notices",
                    title: "How it tells you of new messages",
                    aria-haspopup: "menu",
                    aria-expanded: {move || if open.get() { "true" } else { "false" }},
                    onclick: move || open.update(|o| *o = !*o),
                    {format!("\u{1f514} {} \u{25be}", notices_name(now, of_group))}
                }
            }
            DropdownMenuDropdown {
                div { class: "order-menu", role: "menu", aria-label: "Notices", {items} }
            }
        }
    }
}

/// A group or place to add: its name, where it is, added on a click.
fn offer_row(__scope: &mut RenderScope, state: State, offer: Shown) -> NodeHandle {
    let chosen = offer.chosen.clone();
    rsx! {
        div {
            class: "chooser-row social-offer",
            role: "button",
            onclick: move || state.change_social_edit(|e| match &chosen {
                Chosen::Group(g) => e.add_group(g.clone()),
                Chosen::Place(p) => e.add_place(p.clone()),
            }),
            span { class: "name-text",
                span { class: "chooser-faint", {offer.before.clone()} }
                {offer.name.clone()}
            }
            small { class: "chooser-faint", {offer.detail.clone()} }
        }
    }
}

pub const CSS: &str = r#"
.social-chips { display: flex; flex-wrap: wrap; gap: 6px; border: 1px solid var(--field); border-radius: 6px; padding: 6px; }
.social-chip-slot { display: contents; }
.social-chip {
    display: flex; align-items: center; gap: 5px; flex-wrap: wrap;
    padding: 2px 4px 2px 8px; border: 0; border-radius: 12px;
    background: var(--hover); color: var(--ink); font: inherit; font-size: 12.5px;
}
.social-chip small { color: var(--faint); font-size: 11px; }
.social-chip.add { padding: 2px 8px; background: transparent; color: var(--accent); cursor: pointer; }
.social-chip .rinch-dropdown-menu, .social-chip .rinch-dropdown-menu__target { display: flex; }
.social-notices {
    padding: 0 2px; border: 0; border-radius: 4px; background: transparent;
    color: var(--faint); font: inherit; font-size: 11px; cursor: pointer;
}
.social-notices:hover { background: var(--field); color: var(--ink-2); }
.social-chip-x {
    width: 18px; height: 18px; padding: 0; border: 0; border-radius: 9px;
    background: transparent; color: var(--ink-2); font: inherit; font-size: 11px; cursor: pointer;
}
.social-chip-x:hover { background: var(--field); }
.social-finder { display: flex; flex-direction: column; gap: 6px; margin-top: 6px; }
.social-offers { display: flex; flex-direction: column; max-height: 200px; overflow-y: auto; padding-right: 12px; }
.social-offer { cursor: pointer; }
.social-offer small { flex-shrink: 0; font-size: 11.5px; }
"#;

#[cfg(test)]
#[path = "social_panel_places_tests.rs"]
mod tests;
