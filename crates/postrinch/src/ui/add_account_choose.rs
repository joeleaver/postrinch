//! The add-account dialog's first step (design/mockups/add-imap-account.html,
//! "Which kind"): Google, Microsoft, any other mail account, and, when
//! Thunderbird profiles are found on this computer, importing from them.

use rinch::prelude::*;

use crate::add_account::Kind;
use crate::state::State;
use crate::ui::add_account::head;

#[component]
pub fn choose(state: State) -> NodeHandle {
    let ui = state.adding;
    rsx! {
        div { class: "dialog",
            {head(__scope, "Add an account".into(), "Which kind of account is it?".into())}
            div { class: "add-choice", onclick: move || ui.add_as_google(state),
                span { class: "add-choice-icon", "G" }
                div {
                    span { class: "add-choice-name", "Google" }
                    span { class: "add-choice-about", "Gmail and Google Workspace. Signs in in your browser; mail, labels and calendars." }
                }
            }
            div { class: "add-choice", onclick: move || ui.choose(Kind::Microsoft),
                span { class: "add-choice-icon", "M" }
                div {
                    span { class: "add-choice-name", "Microsoft" }
                    span { class: "add-choice-about", "Outlook.com, Hotmail and Microsoft 365. Signs in in your browser. Mail only." }
                }
            }
            div { class: "add-choice", onclick: move || ui.choose(Kind::Other),
                span { class: "add-choice-icon", "@" }
                div {
                    span { class: "add-choice-name", "Other mail account" }
                    span { class: "add-choice-about", "Fastmail, iCloud, Yahoo, your work's server: any IMAP and SMTP account. Mail only." }
                }
            }
            // Offered once Thunderbird's profiles are found, which is
            // looked at as the dialog opens.
            for about in ui.thunderbird.get().map(|found| import_about(found.addable())) {
                div { key: "thunderbird", class: "add-choice", onclick: move || ui.open_import(),
                    span { class: "add-choice-icon", "⤓" }
                    div {
                        span { class: "add-choice-name", "Import from Thunderbird" }
                        span { class: "add-choice-about", {about.clone()} }
                    }
                }
            }
            div { class: "dialog-actions",
                Button { variant: "default", size: "sm", onclick: move || ui.close(), "Cancel" }
            }
        }
    }
}

/// What the import tile says of what was found.
pub fn import_about(addable: usize) -> String {
    match addable {
        0 => "Thunderbird on this computer has no accounts Postrinch can add.".to_owned(),
        1 => "Thunderbird on this computer has 1 account Postrinch can add.".to_owned(),
        n => format!("Thunderbird on this computer has {n} accounts Postrinch can add."),
    }
}
