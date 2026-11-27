//! Social mode, after the Social mockup (design/mockups/ofscp-social.html):
//! the sidebar of views beside the open view, its header and tabs at the
//! top, and the view panel over the view while it is open. What a view's
//! places hold (streams, lists and the reader) fills the view below its
//! header (docs/ofscp.md, "Reading").

use rinch::prelude::*;

use crate::state::State;
use crate::ui::{social_head, social_panel, social_sidebar, splitter};

#[component]
pub fn social(state: State) -> NodeHandle {
    let editing = Memo::new(move || state.social.edit.with(Option::is_some));
    rsx! {
        div { class: "panes",
            {social_sidebar::social_sidebar(__scope, state)}
            {splitter::splitter(__scope, state, splitter::Pane::Sidebar)}
            section { class: "social-main", aria-label: "View",
                {social_head::social_head(__scope, state)}
                div { class: "social-view" }
                if editing.get() {
                    {social_panel::social_panel(__scope, state)}
                }
            }
        }
    }
}

pub const CSS: &str = r#"
.social-main { position: relative; flex: 1; min-width: 0; display: flex; flex-direction: column; background: var(--surface); }
.social-view { flex: 1; min-height: 0; }
"#;
