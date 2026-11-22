//! The list's header: the name of what is shown, and nothing more. It only
//! names; changing a view is the panel's, from the pencil on its row.

use rinch::prelude::*;

use crate::state::State;
use crate::view_edit::Editing;

#[component]
pub fn view_head(state: State) -> NodeHandle {
    let title = Memo::new(move || {
        if state.search.get().is_some() {
            return "Search".to_owned();
        }
        let new = state
            .view_edit
            .with(|e| e.as_ref().is_some_and(|e| e.kind == Editing::New));
        if new {
            return "New view".to_owned();
        }
        state.saved_view().map(|e| e.view.name).unwrap_or_default()
    });
    rsx! {
        div { class: "view-head",
            h1 { class: "view-title", {move || title.get()} }
        }
    }
}

/// The header's styles.
pub const CSS: &str = r#"
.view-head { flex-shrink: 0; display: flex; flex-direction: column; padding: 12px 20px 10px; border-bottom: 1px solid var(--line-soft); min-width: 0; }
.view-title { margin: 0; font-size: 18px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
"#;
