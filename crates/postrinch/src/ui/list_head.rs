//! The row above the list, after the list header mockup: what can be done
//! to the open conversation (archive, trash, mark unread, its labels, the
//! star). The list's order is in the view's header (view_head.rs).

use rinch::prelude::*;

use crate::state::State;
use crate::ui::thread_actions::toolbar;

#[component]
pub fn list_actions(state: State) -> NodeHandle {
    rsx! {
        div { class: "list-actions",
            div {
                // Nothing to act on until a conversation is open.
                class: {move || if state.selected.get().is_some() { "list-tools" } else { "list-tools idle" }},
                {toolbar(__scope, state)}
            }
        }
    }
}

/// The row's styles.
pub const CSS: &str = r#"
.list-actions {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 12px 14px 8px;
}
.list-actions .icon-button { width: 30px; height: 30px; }
.list-tools.idle { opacity: 0.4; pointer-events: none; }
"#;
