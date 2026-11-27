//! The open view's header, after the Social mockup: its name, the line
//! saying what it gathers, Following on a channel, and, for a view read as
//! a stream with several places, the places as tabs after "All", each with
//! its unread count and, when the view spans groups, the group it is in.

use postrinch_commands::SocialPlaceInfo;
use rinch::prelude::*;

use crate::social_shown::{place_glyph, shows_tabs, sub_line, tabs_say_where};
use crate::social_ui::{is_place, place_ref};
use crate::state::State;

#[component]
pub fn social_head(state: State) -> NodeHandle {
    let entry = Memo::new(move || state.social.open_entry());
    let places = Memo::new(move || state.social.open_places());
    let name = Memo::new(move || entry.get().map(|e| e.view.name).unwrap_or_default());
    let sub = Memo::new(move || {
        entry
            .get()
            .map(|e| sub_line(&e, &places.get()))
            .unwrap_or_default()
    });
    let tabbed = Memo::new(move || entry.get().is_some_and(|e| shows_tabs(&e, &places.get())));
    // The channel the header is about, for Following; never a
    // conversation, which is not followed.
    let channel = Memo::new(move || {
        state
            .social
            .shown_place()
            .filter(|p| p.place.kind == postrinch_store::PlaceKind::Channel)
    });
    rsx! {
        div {
            class: {move || if tabbed.get() { "social-head tabbed" } else { "social-head" }},
            div { class: "social-head-row",
                h2 { class: "social-title",
                    span { class: "social-name", {move || name.get()} }
                    span { class: "social-sub", {move || sub.get()} }
                }
                for place in channel.get() {
                    // Keyed on whether it is followed, which the button says.
                    div { key: format!("{}|{}|{}", place.place.account, place.place.id, place.place.followed),
                        {follow_button(__scope, state, place.clone())}
                    }
                }
            }
            div {
                class: "social-tabs",
                role: "tablist",
                style: {move || if tabbed.get() { "" } else { "display: none" }},
                {all_tab(__scope, state)}
                for place in places.get() {
                    div {
                        // Keyed on all the tab says but its count, which follows in place.
                        key: format!(
                            "{}|{}|{}|{:?}|{}",
                            place.place.account,
                            place.place.id,
                            place.name,
                            place.group_name,
                            place_glyph(place)
                        ),
                        class: "social-tab-slot",
                        {tab(__scope, state, place.clone(), places)}
                    }
                }
            }
        }
    }
}

/// "All": every place of the view, merged.
fn all_tab(__scope: &mut RenderScope, state: State) -> NodeHandle {
    let on = Memo::new(move || state.social.open_tab().is_none());
    rsx! {
        button {
            class: {move || if on.get() { "social-tab on" } else { "social-tab" }},
            role: "tab",
            aria-selected: {move || if on.get() { "true" } else { "false" }},
            onclick: move || state.choose_social_tab(None),
            "All"
        }
    }
}

/// A place's tab: its glyph and name, its unread count, and the group it
/// is in when the view spans groups. The count follows in place.
fn tab(
    __scope: &mut RenderScope,
    state: State,
    place: SocialPlaceInfo,
    places: Memo<Vec<SocialPlaceInfo>>,
) -> NodeHandle {
    let r = place_ref(&place);
    let chosen = r.clone();
    let on = Memo::new(move || state.social.open_tab().as_ref() == Some(&chosen));
    let counted = r.clone();
    let unread = Memo::new(move || {
        places
            .get()
            .iter()
            .find(|p| is_place(p, &counted))
            .map_or(0, |p| p.unread)
    });
    let where_shown = Memo::new(move || tabs_say_where(&places.get()));
    let label = format!("{} {}", place_glyph(&place), place.name);
    let group = place.group_name.clone().unwrap_or_default();
    rsx! {
        button {
            class: {move || if on.get() { "social-tab on" } else { "social-tab" }},
            role: "tab",
            aria-selected: {move || if on.get() { "true" } else { "false" }},
            onclick: move || state.choose_social_tab(Some(r.clone())),
            span { {label.clone()} }
            span {
                class: "social-where",
                style: {move || if where_shown.get() { "" } else { "display: none" }},
                {group.clone()}
            }
            b {
                class: "social-tab-count",
                style: {move || if unread.get() > 0 { "" } else { "display: none" }},
                {move || unread.get().to_string()}
            }
        }
    }
}

/// Following ✓ on a channel the person follows, Follow on one they don't.
fn follow_button(__scope: &mut RenderScope, state: State, place: SocialPlaceInfo) -> NodeHandle {
    let followed = place.place.followed;
    rsx! {
        button {
            class: "social-ghost",
            aria-pressed: if followed { "true" } else { "false" },
            onclick: move || state.follow_social_place(&place),
            {if followed { "Following \u{2713}" } else { "Follow" }}
        }
    }
}

pub const CSS: &str = r#"
.social-head { padding: 10px 16px 10px; border-bottom: 1px solid var(--line-soft); }
.social-head.tabbed { padding-bottom: 0; }
.social-head-row { display: flex; align-items: center; gap: 10px; min-height: 30px; }
.social-title { flex: 1; min-width: 0; margin: 0; font-size: 16px; font-weight: 700; display: flex; align-items: baseline; overflow: hidden; }
.social-name { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.social-sub { margin-left: 8px; color: var(--muted); font-size: 12.5px; font-weight: 400; white-space: nowrap; }
.social-tabs { display: flex; gap: 2px; margin-top: 8px; overflow-x: auto; }
.social-tab-slot { display: contents; }
.social-tab {
    padding: 6px 10px; border: 0; border-bottom: 2px solid transparent; background: transparent;
    display: flex; align-items: baseline; gap: 4px;
    color: var(--ink-2); font: inherit; font-size: 13px; white-space: nowrap; cursor: pointer;
}
.social-tab:hover { color: var(--ink); }
.social-tab.on { border-bottom-color: var(--accent); color: var(--ink); font-weight: 600; }
.social-where { color: var(--faint); font-size: 11.5px; font-weight: 400; }
.social-tab-count { font-size: 11px; color: var(--accent); margin-left: -1px; }
.social-ghost {
    height: 30px; padding: 0 10px; flex-shrink: 0;
    border: 1px solid var(--field); border-radius: 6px; background: var(--surface);
    color: var(--ink-2); font: inherit; font-size: 12.5px; cursor: pointer;
}
.social-ghost:hover { background: var(--hover); }
"#;
