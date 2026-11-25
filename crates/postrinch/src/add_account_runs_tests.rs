//! The adds the dialog starts: one per address, the browser's wait the
//! only part that can be cancelled, and each answer reaching the person
//! whether or not they are still in the dialog.

use postrinch_discover::{Progress, Step as LookStep, StepState};
use postrinch_model::{Account, Provider};
use postrinch_sync::imap::sign_in_cancel;

use super::*;

fn ada() -> EmailAddress {
    EmailAddress::parse("ada@example.org").unwrap()
}

fn added() -> Outcome {
    Outcome::Done(Box::new(Account::new(ada(), Provider::Imap)))
}

fn refused() -> Outcome {
    Outcome::Failed {
        step: Step::Found,
        error: "imap.example.org refused the password.".into(),
    }
}

/// Whether a canceller is kept for the add at `generation`.
fn has_canceller(generation: u64) -> bool {
    CANCEL.with(|c| c.borrow().as_ref().is_some_and(|(g, _)| *g == generation))
}

/// An add of ada's account from the found step, waiting on the browser.
fn waiting_on_the_browser(ui: AddUi) -> Run {
    ui.go(Step::Found);
    let run = ui.start(ada(), Step::Found, Step::Browser, false).unwrap();
    keep_canceller(run.generation, sign_in_cancel().0);
    run
}

#[test]
fn one_add_at_a_time_for_an_address() {
    let ui = AddUi::new();
    let run = waiting_on_the_browser(ui);
    assert_eq!(ui.step.get(), Some(Step::Browser));
    assert_eq!(ui.under_way.get(), [ada()]);

    // The person cancels, and asks again while the first is ending.
    ui.cancel_browser();
    assert_eq!(ui.step.get(), Some(Step::Servers), "nothing was found");
    let again = ui.start(ada(), Step::Found, Step::Browser, false);
    assert!(
        again
            .unwrap_err()
            .contains("still busy with ada@example.org")
    );
    assert_eq!(ui.step.get(), Some(Step::Servers), "the dialog stays put");
    let other = EmailAddress::parse("bo@example.org").unwrap();
    assert!(ui.start(other, Step::Found, Step::Checking, false).is_ok());

    ui.answer(&run, Outcome::Cancelled);
    assert!(ui.start(ada(), Step::Found, Step::Checking, false).is_ok());
}

#[test]
fn once_the_browser_is_back_the_dialog_is_checking_without_cancel() {
    let ui = AddUi::new();
    let run = waiting_on_the_browser(ui);
    assert!(has_canceller(run.generation));
    ui.granted(&run);
    assert_eq!(ui.step.get(), Some(Step::Checking));
    assert!(!has_canceller(run.generation), "nothing left to cancel");
    assert_eq!(ui.generation.get(), run.generation, "still this add's");

    // Cancel is not offered now, and asks nothing if it comes anyway.
    ui.cancel_browser();
    assert_eq!(ui.step.get(), Some(Step::Checking));
    assert!(ui.working());
    assert!(matches!(ui.answer(&run, added()), Answer::Dialog(_)));
}

#[test]
fn a_browser_coming_back_after_the_person_left_moves_nothing() {
    let ui = AddUi::new();
    let run = waiting_on_the_browser(ui);
    ui.close();
    ui.granted(&run);
    assert_eq!(ui.step.get(), None, "the dialog stays closed");
}

#[test]
fn an_add_that_outlives_the_dialog_still_says_how_it_went() {
    let ui = AddUi::new();
    let run = ui.start(ada(), Step::Found, Step::Checking, false).unwrap();
    ui.close();
    let Answer::Away(Some(notice)) = ui.answer(&run, added()) else {
        panic!("no notice");
    };
    assert_eq!(
        notice.text,
        "ada@example.org is added. Its mail is on the way."
    );
    assert!(!notice.trouble);
    assert!(ui.under_way.get().is_empty());

    let run = ui.start(ada(), Step::Found, Step::Checking, false).unwrap();
    ui.go(Step::Address);
    let Answer::Away(Some(notice)) = ui.answer(&run, refused()) else {
        panic!("no notice");
    };
    assert_eq!(
        notice.text,
        "ada@example.org was not added. imap.example.org refused the password."
    );
    assert!(notice.trouble);
}

#[test]
fn servers_changed_after_the_dialog_closed_say_so() {
    let ui = AddUi::new();
    let run = ui
        .start(ada(), Step::Servers, Step::Checking, true)
        .unwrap();
    ui.close();
    let Answer::Away(Some(notice)) = ui.answer(&run, refused()) else {
        panic!("no notice");
    };
    assert!(
        notice
            .text
            .starts_with("ada@example.org's servers were not changed.")
    );
    assert_eq!(done_text(&run), "ada@example.org's servers are changed.");
}

#[test]
fn a_sign_in_the_person_stopped_tells_nothing() {
    let ui = AddUi::new();
    let run = waiting_on_the_browser(ui);
    ui.close();
    assert!(
        !has_canceller(run.generation),
        "closing stopped the browser"
    );
    assert!(matches!(
        ui.answer(&run, Outcome::Cancelled),
        Answer::Away(None)
    ));
}

#[test]
fn an_add_ending_leaves_a_newer_adds_canceller_alone() {
    let ui = AddUi::new();
    let first = waiting_on_the_browser(ui);
    ui.cancel_browser();
    let other = EmailAddress::parse("bo@example.org").unwrap();
    let second = ui.start(other, Step::Found, Step::Browser, false).unwrap();
    keep_canceller(second.generation, sign_in_cancel().0);
    ui.answer(&first, Outcome::Cancelled);
    assert!(has_canceller(second.generation));
    cancel_browser();
}

#[test]
fn a_second_enter_asks_nothing_while_the_dialog_works() {
    let ui = AddUi::new();
    ui.go(Step::LookingUp);
    let generation = ui.generation.get();
    // "Continue" again while discovery runs: it is not started over.
    ui.look_up();
    assert_eq!(ui.generation.get(), generation);
    assert_eq!(ui.step.get(), Some(Step::LookingUp));
    for (step, working) in [
        (Step::Address, false),
        (Step::Found, false),
        (Step::Servers, false),
        (Step::Browser, true),
        (Step::Checking, true),
    ] {
        ui.go(step);
        assert_eq!(ui.working(), working, "{step:?}");
    }
}

#[test]
fn an_abandoned_lookups_progress_stays_out_of_a_new_one() {
    let ui = AddUi::new();
    ui.go(Step::LookingUp);
    let old = ui.generation.get();
    ui.go(Step::Address);
    ui.go(Step::LookingUp);
    let new = ui.generation.get();
    let found = Progress {
        step: LookStep::DomainSettings,
        state: StepState::Found,
    };
    ui.noted(old, found);
    assert!(ui.looking.get().is_empty());
    ui.noted(new, found);
    assert_eq!(
        ui.looking.get(),
        [(LookStep::DomainSettings, StepState::Found)]
    );
}
