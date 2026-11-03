//! The sidebar's labels: the person's own as a tree, then other apps'
//! under the app that made them, collapsed. Hidden labels come back with
//! "Show all labels".

use std::collections::HashSet;

use postrinch_commands::{AppLabels, Folder, LabelNode, Sidebar};
use postrinch_model::LabelPrefs;
use rinch::prelude::*;

use crate::state::State;
use crate::sync::thousands;
use crate::ui::LabelEdit;
use crate::ui::label_edit::{EditMode, label_menu};

/// One line of the labels section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRow {
    /// Stable across reloads: where in the tree this row is.
    pub id: String,
    pub kind: RowKind,
    pub depth: usize,
    pub name: String,
    pub folder: Option<Folder>,
    pub unread: usize,
    pub color: Option<String>,
    /// `Some(open)` for a row with something under it.
    pub open: Option<bool>,
    /// For a row with a label of its own: what the person changed about it,
    /// whether it shows by itself, and its name at the provider.
    pub editable: Option<(LabelPrefs, bool, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// An account's name, when there are several.
    Account,
    /// "From other apps".
    Heading,
    /// An app, under "From other apps".
    App,
    Label,
    /// "Show all labels" or "Hide unused labels".
    ShowAll,
}

impl LabelRow {
    /// Changes whenever anything the row shows changes, so the row is
    /// drawn again.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{:?}|{}|{:?}|{:?}",
            self.id, self.name, self.open, self.unread, self.color, self.editable
        )
    }
}

/// The rows to show. `toggled` holds the ids of the rows the person opened
/// or closed: the person's own labels start open, other apps' closed.
pub fn label_rows(sidebar: &Sidebar, show_all: bool, toggled: &HashSet<String>) -> Vec<LabelRow> {
    let several = sidebar.accounts.len() > 1;
    let mut rows = Vec::new();
    let mut hidden = 0;
    for account in &sidebar.accounts {
        let key = account.account.address.key();
        let tree = &account.labels;
        let start = rows.len();
        for node in &tree.own {
            push_node(
                &mut rows,
                &mut hidden,
                &format!("{key}/own"),
                node,
                0,
                true,
                show_all,
                toggled,
            );
        }
        let apps: Vec<&AppLabels> = tree
            .other_apps
            .iter()
            .filter(|a| {
                let shows = show_all || a.shown;
                if !shows {
                    hidden += usize::from(a.entry.is_some())
                        + a.labels.iter().map(labels_in).sum::<usize>();
                }
                shows
            })
            .collect();
        if !apps.is_empty() {
            let id = format!("{key}/apps");
            let open = toggled.contains(&id);
            rows.push(row(
                &id,
                RowKind::Heading,
                0,
                "From other apps",
                None,
                0,
                Some(open),
            ));
            if open {
                for app in apps {
                    push_app(&mut rows, &mut hidden, &key, app, show_all, toggled);
                }
            }
        }
        if several && rows.len() > start {
            let name = account.account.address.to_string();
            rows.insert(start, row(&key, RowKind::Account, 0, &name, None, 0, None));
        }
    }
    if hidden > 0 || show_all {
        let name = if show_all {
            "Hide unused labels".to_owned()
        } else {
            format!("Show all labels ({} hidden)", thousands(hidden))
        };
        rows.push(row("show-all", RowKind::ShowAll, 0, &name, None, 0, None));
    }
    rows
}

#[allow(clippy::too_many_arguments)]
fn push_node(
    rows: &mut Vec<LabelRow>,
    hidden: &mut usize,
    parent: &str,
    node: &LabelNode,
    depth: usize,
    open_by_default: bool,
    show_all: bool,
    toggled: &HashSet<String>,
) {
    if !show_all && !node.shown {
        *hidden += labels_in(node);
        return;
    }
    let id = format!("{parent}/{}", node.segment);
    let open = (!node.children.is_empty()).then(|| open_by_default != toggled.contains(&id));
    let folder = node.entry.as_ref().map(|e| e.folder.clone());
    let unread = node.entry.as_ref().map_or(0, |e| e.unread);
    rows.push(row(
        &id,
        RowKind::Label,
        depth,
        &node.name,
        folder,
        unread,
        open,
    ));
    if let Some(last) = rows.last_mut() {
        last.color = node.color.clone();
        last.editable = node
            .entry
            .is_some()
            .then(|| (node.prefs.clone(), node.shown, node.segment.clone()));
    }
    if open == Some(true) {
        for child in &node.children {
            push_node(
                rows,
                hidden,
                &id,
                child,
                depth + 1,
                open_by_default,
                show_all,
                toggled,
            );
        }
    }
}

/// How many labels a node and everything under it stand for.
fn labels_in(node: &LabelNode) -> usize {
    usize::from(node.entry.is_some()) + node.children.iter().map(labels_in).sum::<usize>()
}

fn push_app(
    rows: &mut Vec<LabelRow>,
    hidden: &mut usize,
    account_key: &str,
    app: &AppLabels,
    show_all: bool,
    toggled: &HashSet<String>,
) {
    let id = format!("{account_key}/app/{}", app.app);
    let open = (!app.labels.is_empty()).then(|| toggled.contains(&id));
    let folder = app.entry.as_ref().map(|e| e.folder.clone());
    let unread = app.entry.as_ref().map_or(0, |e| e.unread);
    rows.push(row(&id, RowKind::App, 1, &app.app, folder, unread, open));
    if open == Some(true) {
        for node in &app.labels {
            push_node(rows, hidden, &id, node, 2, false, show_all, toggled);
        }
    }
}

fn row(
    id: &str,
    kind: RowKind,
    depth: usize,
    name: &str,
    folder: Option<Folder>,
    unread: usize,
    open: Option<bool>,
) -> LabelRow {
    LabelRow {
        id: id.to_owned(),
        kind,
        depth,
        name: name.to_owned(),
        folder,
        unread,
        color: None,
        open,
        editable: None,
    }
}

#[component]
pub fn labels_section(state: State) -> NodeHandle {
    rsx! {
        div { class: "folder-group",
            div { class: "section-title", "Labels" }
            for line in label_rows_now(state) {
                div { key: line.key(),
                    {label_line(__scope, state, line.clone())}
                }
            }
        }
    }
}

fn label_rows_now(state: State) -> Vec<LabelRow> {
    match state.sidebar.get() {
        Some(sidebar) => label_rows(&sidebar, state.show_all.get(), &state.toggled.get()),
        None => Vec::new(),
    }
}

#[component]
fn label_line(state: State, line: LabelRow) -> NodeHandle {
    let indent = match line.kind {
        RowKind::ShowAll => String::new(),
        _ => format!("padding-left: {}px", 2 + 14 * line.depth),
    };
    let class_for = match line.kind {
        RowKind::Account => "label-account",
        RowKind::Heading => "folder heading",
        RowKind::App => "folder from-app",
        RowKind::Label => "folder",
        RowKind::ShowAll => "show-all",
    };
    let chosen = line.folder.clone();
    // The chevron opens and closes; the name opens the label, or does what
    // the chevron does when there is no label to open.
    let id = line.id.clone();
    let toggle = move || {
        let id = id.clone();
        state.toggled.update(move |set| {
            if !set.remove(&id) {
                set.insert(id);
            }
        });
    };
    let chevron = match line.open {
        Some(open) => {
            let toggle = toggle.clone();
            let glyph = if open { "▾" } else { "▸" };
            rsx! { span { class: "chevron", onclick: toggle, {glyph} } }
        }
        None => rsx! { span { class: "chevron" } },
    };
    let folder = line.folder.clone();
    let (kind, has_children) = (line.kind, line.open.is_some());
    let open_it = move || match (kind, folder.clone()) {
        (RowKind::ShowAll, _) => state.show_all.update(|on| *on = !*on),
        (_, Some(folder)) => state.open_folder(folder),
        _ if has_children => toggle(),
        _ => {}
    };
    let dot = line.color.clone().map(|c| {
        rsx! { span { class: "dot", style: {format!("background: {c}")} } }
    });
    let unread = (line.unread > 0).then(|| {
        let n = thousands(line.unread);
        rsx! { span { class: "count", {n} } }
    });
    let menu = match (&line.folder, &line.editable) {
        (Some(Folder::Label { account, label }), Some((prefs, shown, provider_name))) => {
            let edit = LabelEdit {
                account: account.clone(),
                label: label.clone(),
                provider_name: provider_name.clone(),
                prefs: prefs.clone(),
                mode: EditMode::Rename,
            };
            Some(label_menu(__scope, state, edit, *shown))
        }
        _ => None,
    };
    let body = rsx! {
        div {
            class: {move || {
                let here = chosen.as_ref().is_some_and(|f| *f == state.folder.get());
                if here { format!("{class_for} chosen") } else { class_for.to_owned() }
            }},
            style: {indent.clone()},
            {chevron}
            span { class: "folder-name", onclick: open_it,
                {dot}
                {line.name.clone()}
            }
            {unread}
        }
    };
    match menu {
        Some(menu) => rsx! {
            ContextMenu {
                ContextMenuTarget { {body} }
                {menu}
            }
        },
        None => body,
    }
}
