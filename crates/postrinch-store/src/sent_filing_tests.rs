use postrinch_model::{
    Account, FolderRoles, ImapFolder, ImapPlace, Provider, Security, Server, SignInMethod,
};

use super::*;

fn setup() -> (tempfile::TempDir, Store, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let address = EmailAddress::parse("ada@example.org").unwrap();
    let server = |host: &str| Server {
        host: host.into(),
        port: 993,
        security: Security::Tls,
        username: "ada".into(),
    };
    let mut account = Account::new(address.clone(), Provider::Imap);
    account.imap = Some(ImapSettings {
        imap: server("imap.example.org"),
        smtp: server("smtp.example.org"),
        sign_in: SignInMethod::Password,
        roles: FolderRoles::default(),
        server_files_sent: true,
    });
    store.put_account(&account).unwrap();
    (dir, store, address)
}

fn settings(store: &Store, address: &EmailAddress) -> ImapSettings {
    store.account(address).unwrap().unwrap().imap.unwrap()
}

fn id(n: u32) -> ProviderMessageId {
    ProviderMessageId(format!("m{n}"))
}

const MINUTE: i64 = 60 * 1000;
const HOUR: i64 = 60 * MINUTE;
const DAY: i64 = 24 * HOUR;

/// `millis` after some moment.
fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(1_700_000_000_000 + millis)
}

fn files_sent(store: &Store, me: &EmailAddress, now: i64) -> bool {
    store
        .server_files_sent(me, Some(("SENT", 1)), &SentBackOff::default(), at(now))
        .unwrap()
}

fn filed(store: &Store, me: &EmailAddress, n: u32, missed: bool, now: i64) -> bool {
    store
        .sent_copy_filed(me, &id(n), missed, 3, at(now))
        .unwrap()
}

#[test]
fn off_only_after_misses_in_a_row() {
    let (_dir, store, me) = setup();
    assert!(!filed(&store, &me, 1, true, 0));
    assert!(!filed(&store, &me, 2, true, 0));
    // A copy found: the count starts again.
    store.sent_copy_found(&me).unwrap();
    assert!(!filed(&store, &me, 3, true, 0));
    assert!(!filed(&store, &me, 4, true, 0));
    assert!(settings(&store, &me).server_files_sent);
    assert!(filed(&store, &me, 5, true, 0));
    assert!(!settings(&store, &me).server_files_sent);
    assert!(!files_sent(&store, &me, 0));
    // Filed while off: not a miss, and nothing to turn off.
    assert!(!filed(&store, &me, 6, false, 0));
}

#[test]
fn the_back_off_doubles_up_to_its_longest() {
    let back_off = SentBackOff::default();
    let hours = |h: u64| std::time::Duration::from_secs(h * 60 * 60);
    assert_eq!(back_off.after(0), hours(0));
    assert_eq!(back_off.after(1), hours(1));
    assert_eq!(back_off.after(2), hours(2));
    assert_eq!(back_off.after(8), hours(128));
    assert_eq!(back_off.after(9), hours(7 * 24));
    assert_eq!(back_off.after(40), hours(7 * 24));
    assert_eq!(back_off.after(u32::MAX), hours(7 * 24));
}

#[test]
fn settings_saved_meanwhile_are_kept() {
    let (_dir, store, me) = setup();
    store.sent_copy_filed(&me, &id(1), true, 2, at(0)).unwrap();
    // The person changes a setting between two sends.
    let mut changed = settings(&store, &me);
    changed.roles.sent = Some("Sent Items".into());
    store.set_imap_settings(&me, &changed).unwrap();
    assert!(store.sent_copy_filed(&me, &id(2), true, 2, at(0)).unwrap());
    let now = settings(&store, &me);
    assert!(!now.server_files_sent);
    assert_eq!(now.roles.sent.as_deref(), Some("Sent Items"));
}

#[test]
fn an_account_without_imap_settings_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = EmailAddress::parse("ada@example.org").unwrap();
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    assert!(store.sent_copy_found(&me).is_err());
}

/// Keeps the folder SENT, and a copy of `message` in it at `uid` under
/// `uidvalidity`.
fn copy_in_sent(
    store: &Store,
    me: &EmailAddress,
    message: &ProviderMessageId,
    (uidvalidity, uid): (u32, u32),
) {
    store
        .put_imap_folder(me, &ImapFolder::new("SENT", "Sent"))
        .unwrap();
    let place = ImapPlace {
        folder: "SENT".into(),
        uidvalidity,
        uid,
        message: message.clone(),
        flags: vec!["\\Seen".into()],
        modseq: 0,
        arrived: None,
    };
    store.put_imap_place(me, &place).unwrap();
}

/// Plays one send at `now`, the provider filing it in Sent only after
/// the look gave up (when the setting is on, Postrinch filed it after a
/// miss) and sync seeing both copies; returns whether the setting was on
/// for the send.
fn send_filed_late(store: &Store, me: &EmailAddress, n: u32, now: i64) -> bool {
    let on = files_sent(store, me, now);
    filed(store, me, n, on, now);
    copy_in_sent(store, me, &id(n), (1, 2 * n));
    copy_in_sent(store, me, &id(n), (1, 2 * n + 1));
    on
}

#[test]
fn a_provider_late_only_during_an_outage_gets_its_setting_back() {
    let (_dir, store, me) = setup();
    // An outage: three sends in a row are filed only after the look.
    for n in 1..=3 {
        assert!(send_filed_late(&store, &me, n, 0), "{n}");
    }
    assert!(!settings(&store, &me).server_files_sent);
    // Within the hour, a second copy leaves it off.
    assert!(!send_filed_late(&store, &me, 4, 30 * MINUTE));
    // Once the hour has passed, a second copy turns it back on.
    assert!(files_sent(&store, &me, HOUR));
    // The provider files in time again: found, and the back-off starts
    // again from an hour.
    store.sent_copy_found(&me).unwrap();
    for n in 5..=7 {
        assert!(send_filed_late(&store, &me, n, 2 * HOUR), "{n}");
    }
    assert!(!send_filed_late(&store, &me, 8, 2 * HOUR + 30 * MINUTE));
    assert!(files_sent(&store, &me, 3 * HOUR));
}

#[test]
fn a_provider_always_late_gets_its_setting_back_ever_more_slowly() {
    let (_dir, store, me) = setup();
    // A send every two hours for ten weeks, each filed late.
    let step = 2 * HOUR;
    let (mut was_on, mut off_since, mut offs) = (true, 0, Vec::new());
    for n in 0..10 * 7 * 12 {
        let now = i64::from(n) * step;
        let on = send_filed_late(&store, &me, n + 1, now);
        if on && !was_on {
            offs.push(now - off_since);
        }
        if was_on && !settings(&store, &me).server_files_sent {
            off_since = now;
        }
        was_on = settings(&store, &me).server_files_sent;
    }
    // Off for an hour, then two, four... (to the next send), then a week
    // at most, however long it goes on.
    let mut expected = HOUR;
    for (flip, off) in offs.iter().enumerate() {
        let wait = expected.min(7 * DAY);
        assert!(*off >= wait && *off <= wait + step, "{flip}: {off}");
        expected *= 2;
    }
    // Eight doublings take it to a week; after that, about once a week.
    let weekly = offs.iter().filter(|off| **off >= 7 * DAY).count();
    assert!(weekly >= 3, "{offs:?}");
    assert!(offs.len() <= 8 + 10, "{offs:?}");
}

#[test]
fn a_clock_set_back_starts_the_wait_again() {
    let (_dir, store, me) = setup();
    for n in 1..=3 {
        send_filed_late(&store, &me, n, 10 * DAY);
    }
    // The clock goes back a day: the wait starts from then.
    assert!(!send_filed_late(&store, &me, 4, 9 * DAY));
    assert!(!files_sent(&store, &me, 9 * DAY + 30 * MINUTE));
    send_filed_late(&store, &me, 5, 9 * DAY + 30 * MINUTE);
    assert!(files_sent(&store, &me, 9 * DAY + HOUR));
}

#[test]
fn a_setting_the_person_turned_off_comes_back_at_once() {
    let (_dir, store, me) = setup();
    let mut off = settings(&store, &me);
    off.server_files_sent = false;
    store.set_imap_settings(&me, &off).unwrap();
    assert!(!send_filed_late(&store, &me, 1, 0));
    assert!(files_sent(&store, &me, 0));
}

#[test]
fn copies_under_an_old_uidvalidity_do_not_count() {
    let (_dir, store, me) = setup();
    let back_off = SentBackOff::default();
    let sent = |uidvalidity| {
        store
            .server_files_sent(&me, Some(("SENT", uidvalidity)), &back_off, at(HOUR))
            .unwrap()
    };
    for n in 1..=3 {
        filed(&store, &me, n, true, 0);
    }
    filed(&store, &me, 4, false, 0);
    assert!(!settings(&store, &me).server_files_sent);
    // One copy from before the folder's UIDVALIDITY changed, one now.
    copy_in_sent(&store, &me, &id(4), (1, 5));
    copy_in_sent(&store, &me, &id(4), (2, 1));
    assert!(!sent(2));
    copy_in_sent(&store, &me, &id(4), (2, 2));
    assert!(sent(2));
}
