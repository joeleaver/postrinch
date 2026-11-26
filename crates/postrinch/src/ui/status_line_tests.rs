use super::*;

fn sync(address: &str, how: SyncHow, progress: Option<(usize, usize)>) -> AccountSync {
    AccountSync {
        account: EmailAddress::parse(address).unwrap(),
        line: String::new(),
        progress,
        how,
    }
}

fn idle(address: &str) -> AccountSync {
    sync(address, SyncHow::Idle, None)
}

#[test]
fn all_well_is_one_quiet_line() {
    let all = [idle("a@example.org"), idle("b@example.org")];
    let s = summary(&all, None);
    assert_eq!(s.text, "Up to date");
    assert_eq!(s.tone, Tone::Quiet);
    assert_eq!(s.progress, None);
}

#[test]
fn no_accounts_says_so_quietly() {
    let s = summary(&[], None);
    assert_eq!(s.text, "No accounts syncing");
    assert_eq!(s.tone, Tone::Quiet);
}

#[test]
fn accounts_at_work_are_counted_with_their_downloads_together() {
    let all = [
        sync("a@example.org", SyncHow::Working, Some((300, 1000))),
        sync("b@example.org", SyncHow::Working, None),
        sync("c@example.org", SyncHow::Working, Some((200, 1000))),
        idle("d@example.org"),
    ];
    let s = summary(&all, None);
    assert_eq!(s.text, "Syncing 3 of 4 accounts\u{2026}");
    assert_eq!(s.tone, Tone::Working);
    assert_eq!(s.progress, Some((500, 2000)));
    assert_eq!(percent(s.progress.unwrap()), 25);
}

#[test]
fn one_account_at_work_needs_no_count() {
    let s = summary(&[sync("a@example.org", SyncHow::Working, None)], None);
    assert_eq!(s.text, "Syncing\u{2026}");
    assert_eq!(s.progress, None);
}

#[test]
fn sign_in_comes_before_any_other_word() {
    let all = [
        sync("a@example.org", SyncHow::NeedsSignIn, None),
        sync("b@example.org", SyncHow::Trouble, None),
        sync("c@example.org", SyncHow::Working, Some((1, 2))),
    ];
    let s = summary(&all, Some("Sync is off"));
    assert_eq!(s.text, "1 account needs sign-in");
    assert_eq!(s.tone, Tone::Trouble);
    let two = [
        sync("a@example.org", SyncHow::NeedsSignIn, None),
        sync("b@example.org", SyncHow::NeedsSignIn, None),
    ];
    assert_eq!(summary(&two, None).text, "2 accounts need sign-in");
}

#[test]
fn other_trouble_comes_before_sync_being_off() {
    let all = [
        sync("a@example.org", SyncHow::Trouble, None),
        idle("b@example.org"),
    ];
    assert_eq!(
        summary(&all, Some("Sync is off")).text,
        "Trouble with 1 account"
    );
}

#[test]
fn sync_off_is_said_in_its_own_words_even_with_accounts_well() {
    let off = "Google sign-in isn\u{2019}t set up, so Google accounts don\u{2019}t sync";
    let s = summary(&[idle("a@example.org")], Some(off));
    assert_eq!(s.text, off);
    assert_eq!(s.tone, Tone::Trouble);
}

#[test]
fn a_percentage_never_divides_by_nothing_or_overshoots() {
    assert_eq!(percent((0, 0)), 100);
    assert_eq!(percent((5, 4)), 100);
    assert_eq!(percent((1, 3)), 33);
}

#[test]
fn only_sign_in_and_trouble_are_trouble() {
    assert!(SyncHow::NeedsSignIn.trouble());
    assert!(SyncHow::Trouble.trouble());
    assert!(!SyncHow::Working.trouble());
    assert!(!SyncHow::Idle.trouble());
}

fn account(address: &str, signing: bool) -> Account {
    let mut a = Account::new(
        EmailAddress::parse(address).unwrap(),
        postrinch_model::Provider::Google,
    );
    if signing {
        a.state = AccountState::NeedsSignIn(postrinch_model::SignInReason::NoCredential);
    }
    a
}

#[test]
fn an_account_the_store_says_must_sign_in_shows_so_though_sync_said_nothing() {
    // After a restart sync passes it by in silence.
    let accounts = [
        account("a@example.org", false),
        account("b@example.org", true),
    ];
    let all = standing(&accounts, &[idle("a@example.org")]);
    assert_eq!(all.len(), 2);
    assert_eq!(all[1].how, SyncHow::NeedsSignIn);
    assert_eq!(all[1].line, NEEDS_SIGN_IN);
    assert_eq!(summary(&all, None).text, "1 account needs sign-in");
}

#[test]
fn the_stores_sign_in_outweighs_an_older_word_from_sync() {
    let accounts = [account("a@example.org", true)];
    let all = standing(
        &accounts,
        &[sync("a@example.org", SyncHow::Working, Some((1, 9)))],
    );
    assert_eq!(all.len(), 1);
    assert_eq!((all[0].how, all[0].progress), (SyncHow::NeedsSignIn, None));
}

#[test]
fn an_account_signed_in_again_shows_what_sync_says() {
    let accounts = [account("a@example.org", false)];
    let reports = [sync("a@example.org", SyncHow::Working, None)];
    assert_eq!(standing(&accounts, &reports), reports);
    // Before the sidebar knows the accounts, sync's word stands alone.
    assert_eq!(standing(&[], &reports), reports);
}
