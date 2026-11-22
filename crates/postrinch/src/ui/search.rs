//! Search's own parts of the window, after the Search mockup: the lines
//! under the results' heading about Gmail's part and about a query that
//! does not read. A search is refined in the view panel, from the pencil
//! on its row in the sidebar.

use rinch::prelude::*;

use crate::remote_search::remote_line;
use crate::state::State;

/// The lines under the results' heading.
#[component]
pub fn search_notes(state: State) -> NodeHandle {
    rsx! {
        div { class: "search-notes",
            div {
                class: "search-note",
                style: {move || {
                    let line = remote_line(state.remote.get().as_ref());
                    if line.is_empty() || state.search_error.get().is_some() { "display: none".to_owned() } else { String::new() }
                }},
                {move || remote_line(state.remote.get().as_ref())}
            }
            for why in state.search_error.get() {
                div { key: why.clone(), class: "search-error", {why.clone()} }
            }
        }
    }
}

pub const CSS: &str = r#"
.title-search { display: flex; align-items: center; gap: 6px; }
.title-search > :first-child { flex: 1; min-width: 0; }
.search-notes { display: flex; flex-direction: column; }
.search-note {
    padding: 8px 16px;
    border-bottom: 1px solid var(--line-soft);
    background: var(--panel);
    font-size: 13px;
    color: var(--ink-2);
}
.search-error { padding: 8px 16px; border-bottom: 1px solid var(--line-soft); background: var(--warn-bg); color: var(--trouble); font-size: 13px; }
.found-by {
    flex-shrink: 0;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--warn-bg);
    color: var(--trouble);
    font-size: 11px;
    font-family: 'IBM Plex Mono', monospace;
}
"#;
