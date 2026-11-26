//! The import in the dialog: the list opens on what was found, each
//! picked account takes its turn on the dialog's own steps, an add or a
//! skip moves on to the next, a failure or a cancelled browser stays on
//! its turn, and Stop importing ends it all.

use std::path::Path;

use super::*;
use crate::add_account::Way;
use crate::add_account_flow::Outcome;
use crate::import_thunderbird::Listing;

fn found() -> Listing {
    let home = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../postrinch-thunderbird/tests/fixtures/normal");
    let found = postrinch_thunderbird::find(&home);
    listing(&found, &[], |p| p == OAuthProvider::Microsoft).unwrap()
}

/// The dialog on the import's list.
fn on_the_list() -> AddUi {
    let ui = AddUi::new();
    ui.thunderbird.set(Some(found()));
    ui.open_import();
    assert_eq!(ui.step.get(), Some(Step::ImportList));
    ui
}

fn current(ui: AddUi) -> Option<String> {
    ui.import
        .get()
        .and_then(|i| i.current().map(|r| r.who.clone()))
}

#[test]
fn no_import_without_thunderbird() {
    let ui = AddUi::new();
    ui.go(Step::Choose);
    ui.open_import();
    assert_eq!(ui.step.get(), Some(Step::Choose));
    assert!(ui.import.get().is_none());
}

#[test]
fn each_picked_account_takes_its_turn() {
    let ui = on_the_list();
    ui.start_import();
    assert_eq!(ui.step.get(), Some(Step::ImportSignIn));
    let form = ui.form.get();
    assert_eq!(form.kind, Kind::Thunderbird);
    assert_eq!(form.address, "ada@fastmail.com");
    assert_eq!(form.way, Way::Password);

    // Gmail's turn: the form holds only its address, for the heading.
    ui.edit(|f| f.password = "typed".into());
    ui.skip_import();
    assert_eq!(current(ui).as_deref(), Some("ada.lovelace@gmail.com"));
    assert_eq!(ui.form.get().address, "ada.lovelace@gmail.com");
    assert!(ui.form.get().password.is_empty());

    ui.skip_import();
    ui.skip_import();
    assert_eq!(current(ui).as_deref(), Some("ada@outlook.com"));
    assert_eq!(ui.form.get().way, Way::Browser(OAuthProvider::Microsoft));
    ui.skip_import();
    assert_eq!(ui.step.get(), Some(Step::ImportDone));
    assert_eq!(ui.import.get().unwrap().ended.len(), 4);
}

#[test]
fn an_added_account_moves_on_and_another_address_does_not() {
    let ui = on_the_list();
    ui.start_import();
    let other = EmailAddress::parse("bo@example.org").unwrap();
    assert!(!ui.imported(&other));
    assert_eq!(current(ui).as_deref(), Some("ada@fastmail.com"));
    let fastmail = EmailAddress::parse("ada@fastmail.com").unwrap();
    assert!(ui.imported(&fastmail));
    assert_eq!(current(ui).as_deref(), Some("ada.lovelace@gmail.com"));
    let import = ui.import.get().unwrap();
    assert_eq!(import.ended[0].1, Ended::Added);
}

#[test]
fn a_failure_or_a_cancelled_browser_stays_on_the_turn() {
    let ui = on_the_list();
    ui.start_import();
    assert_eq!(ui.import_step(Step::Address), Step::ImportSignIn);
    assert_eq!(ui.import_step(Step::Found), Step::ImportSignIn);
    assert_eq!(ui.import_step(Step::Servers), Step::Servers);

    // Outlook's browser sign-in, cancelled.
    for _ in 0..3 {
        ui.skip_import();
    }
    let outlook = EmailAddress::parse("ada@outlook.com").unwrap();
    let run = ui
        .start(outlook, Step::ImportSignIn, Step::Browser, false)
        .unwrap();
    ui.cancel_browser();
    assert_eq!(ui.step.get(), Some(Step::ImportSignIn));
    assert_eq!(current(ui).as_deref(), Some("ada@outlook.com"));
    ui.answer(&run, Outcome::Cancelled);

    // Away from an import the steps are as they were.
    ui.close();
    ui.form.set(AddForm::new(Kind::Other));
    assert_eq!(ui.import_step(Step::Address), Step::Address);
}

#[test]
fn skip_and_stop_ask_nothing_while_a_check_runs() {
    let ui = on_the_list();
    ui.start_import();
    let fastmail = EmailAddress::parse("ada@fastmail.com").unwrap();
    ui.start(fastmail, Step::ImportSignIn, Step::Checking, false)
        .unwrap();
    ui.skip_import();
    ui.stop_import();
    assert_eq!(ui.step.get(), Some(Step::Checking));
    assert_eq!(current(ui).as_deref(), Some("ada@fastmail.com"));
}

#[test]
fn stop_importing_ends_it_all() {
    let ui = on_the_list();
    ui.start_import();
    ui.edit(|f| f.password = "typed".into());
    ui.stop_import();
    assert_eq!(ui.step.get(), Some(Step::ImportDone));
    let import = ui.import.get().unwrap();
    assert!(import.stopped());
    assert_eq!(import.added(), 0);
    assert!(ui.form.get().password.is_empty());
    // Closing forgets the import.
    ui.close();
    assert!(ui.import.get().is_none());
}

#[test]
fn ticking_counts_and_nothing_ticked_starts_nothing() {
    let ui = on_the_list();
    let picked = ui.import.get().unwrap().picked;
    for id in picked {
        ui.toggle_import(id);
    }
    ui.start_import();
    assert_eq!(ui.step.get(), Some(Step::ImportList));
}
