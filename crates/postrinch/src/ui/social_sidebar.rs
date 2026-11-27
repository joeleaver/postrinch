//! Social mode's sidebar, after the Social mockup: "Social" at its head,
//! the built-in views (Home, Mentions, Messages, Drafts), "Your views",
//! a line, All channels, and "New view…" at the foot. Each row has its
//! mark, its name, its count (a badge where it calls for the person), and
//! ✎ on hover to open the view panel, as in Mail.

use postrinch_commands::SocialViewEntry;
use rinch::prelude::*;

use crate::social_shown::{GroupMark, Mark, row_count, view_mark};
use crate::state::State;
use crate::sync::thousands;
use crate::ui::icons::{Icon, icon};

/// Where a view stands in the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// Home, Mentions, Messages and Drafts.
    First,
    /// The person's own.
    Yours,
    /// All channels.
    Foot,
}

fn part_of(entry: &SocialViewEntry) -> Part {
    match entry.built_in {
        Some(postrinch_commands::BuiltInView::AllChannels) => Part::Foot,
        Some(_) => Part::First,
        None => Part::Yours,
    }
}

#[component]
pub fn social_sidebar(state: State) -> NodeHandle {
    let any_yours = Memo::new(move || {
        state
            .social
            .views
            .with(|v| v.iter().any(|e| part_of(e) == Part::Yours))
    });
    rsx! {
        nav {
            class: "sidebar social-sidebar",
            aria-label: "Social views",
            style: {move || format!("width: {}px", state.sidebar_width.get())},
            div { class: "folders",
                div { class: "social-side-head", "Social" }
                div { class: "folder-group",
                    {rows_in(__scope, state, Part::First)}
                    div {
                        class: "social-section",
                        style: {move || if any_yours.get() { "" } else { "display: none" }},
                        "Your views"
                    }
                    {rows_in(__scope, state, Part::Yours)}
                    div { class: "view-line", span { class: "view-rule" } }
                    {rows_in(__scope, state, Part::Foot)}
                    div {
                        class: "folder social-new",
                        role: "button",
                        onclick: move || state.new_social_view(),
                        span { class: "social-glyph", "+" }
                        span { class: "name-text", "New view\u{2026}" }
                    }
                }
            }
        }
    }
}

/// The views of one part. Keyed by what a row shows but its count, so a
/// row is built once and its count follows in place.
#[component]
fn rows_in(state: State, part: Part) -> NodeHandle {
    let listed = move || {
        let groups = state.social.groups.get();
        state.social.views.with(|views| {
            views
                .iter()
                .filter(|e| part_of(e) == part)
                .map(|e| {
                    let mark = view_mark(e, groups.get(&e.view.id).map_or(&[], Vec::as_slice));
                    (e.clone(), mark)
                })
                .collect::<Vec<_>>()
        })
    };
    rsx! {
        div { class: "view-group",
            for (entry, mark) in listed() {
                div { key: format!("{}|{}|{:?}|{mark:?}", entry.view.id, entry.view.name, entry.view.count),
                    {view_row(__scope, state, entry.clone(), mark.clone())}
                }
            }
        }
    }
}

#[component]
fn view_row(state: State, entry: SocialViewEntry, mark: Mark) -> NodeHandle {
    let id = entry.view.id.clone();
    let chosen = id.clone();
    let counted = entry.clone();
    let count = Memo::new(move || {
        state
            .social
            .tallies
            .with(|t| row_count(&counted, t.get(&counted.view.id)))
    });
    let editing = id.clone();
    let pressed = Memo::new(move || {
        state
            .social
            .edit
            .with(|e| e.as_ref().is_some_and(|e| e.is_of(&editing)))
    });
    let (opens, edits) = (id.clone(), id.clone());
    let title = format!("Edit {}", entry.view.name);
    let label = title.clone();
    rsx! {
        div {
            class: {move || {
                let mut class = String::from("folder view-row social-row");
                let making = state.social.edit.with(|e| e.as_ref().is_some_and(|e| e.is_new()));
                if state.social.open.get() == chosen && !making {
                    class.push_str(" chosen");
                }
                if count.get().is_some_and(|(_, loud)| loud) {
                    class.push_str(" unread");
                }
                class
            }},
            div { class: "row-open", onclick: move || state.open_social_view(&opens),
                span { class: "folder-name",
                    {mark_of(__scope, &mark)}
                    span { class: "name-text", {entry.view.name.clone()} }
                }
                span {
                    class: {move || if count.get().is_some_and(|(_, loud)| loud) { "social-badge" } else { "count" }},
                    style: {move || if count.get().is_some() { "" } else { "display: none" }},
                    {move || count.get().map(|(n, _)| thousands(n)).unwrap_or_default()}
                }
            }
            button {
                class: {move || if pressed.get() { "row-edit pressed" } else { "row-edit" }},
                title: {title.clone()},
                aria-label: {label.clone()},
                aria-pressed: {move || if pressed.get() { "true" } else { "false" }},
                onclick: move || state.toggle_social_edit(&edits),
                {icon(__scope, Icon::Write)}
            }
        }
    }
}

/// A view's mark, before its name.
fn mark_of(__scope: &mut RenderScope, mark: &Mark) -> NodeHandle {
    match mark {
        Mark::Glyph(glyph) => rsx! { span { class: "social-glyph", {*glyph} } },
        Mark::Group(group) => square(__scope, group, "social-square"),
        Mark::Stack(groups) => {
            let squares: Vec<NodeHandle> = groups
                .iter()
                .map(|g| square(__scope, g, "social-square stacked"))
                .collect();
            rsx! { span { class: "social-stack", {squares} } }
        }
    }
}

/// A group's square: its initial on its colour.
pub(crate) fn square(
    __scope: &mut RenderScope,
    group: &GroupMark,
    class: &'static str,
) -> NodeHandle {
    let style = format!("background: {}", group.color());
    let initial = group.initial();
    rsx! {
        span {
            class: class,
            style: style,
            aria-hidden: "true",
            {initial}
        }
    }
}

pub const CSS: &str = r#"
.social-side-head { padding: 2px 6px 8px 10px; font-weight: 700; font-size: 15px; color: var(--ink); }
.social-section {
    padding: 12px 10px 4px; font-size: 11.5px; font-weight: 600; color: var(--faint);
    text-transform: uppercase; letter-spacing: 0.05em;
}
.social-sidebar .view-line { margin: 8px 4px; height: 1px; }
.social-row { padding: 0 6px 0 10px; }
.social-row.unread .name-text { font-weight: 600; }
.social-glyph { width: 18px; flex-shrink: 0; text-align: center; color: var(--muted); font-size: 13px; font-weight: 400; }
.social-square {
    width: 18px; height: 18px; flex-shrink: 0; border-radius: 7px;
    display: flex; align-items: center; justify-content: center;
    color: #fff; font-size: 10px; font-weight: 700;
}
.social-stack { display: flex; flex-shrink: 0; }
.social-square.stacked { width: 16px; height: 16px; font-size: 8px; border: 1.5px solid var(--panel); margin-left: -5px; }
.social-square.stacked:first-child { margin-left: 0; }
.social-badge {
    padding: 0 6px; border-radius: 9px; background: var(--accent); color: #fff;
    font-size: 11px; font-weight: 600; line-height: 17px;
}
.view-row:hover .social-badge { visibility: hidden; }
.social-new { padding: 0 6px 0 10px; gap: 8px; justify-content: flex-start; color: var(--muted); }
"#;
