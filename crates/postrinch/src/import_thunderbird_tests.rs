//! What the import lists from the Thunderbird fixtures of
//! `postrinch-thunderbird` (by profile, each account with how it comes in
//! or why it cannot), the form an IMAP account fills in, and the import's
//! run through the picked accounts.

use std::path::{Path, PathBuf};

use postrinch_model::{EmailAddress, Security};
use postrinch_sync::imap::OAuthProvider;

use super::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../postrinch-thunderbird/tests/fixtures")
        .join(name)
}

fn address(a: &str) -> EmailAddress {
    EmailAddress::parse(a).unwrap()
}

/// The normal fixture's list, with Microsoft's sign-in set up or not and
/// `here` already in Postrinch.
fn normal(microsoft: bool, here: &[&str]) -> Listing {
    let found = postrinch_thunderbird::find(&fixture("normal"));
    let here: Vec<EmailAddress> = here.iter().map(|a| address(a)).collect();
    listing(&found, &here, |p| {
        microsoft && p == OAuthProvider::Microsoft
    })
    .unwrap()
}

fn imap_form(row: &Row) -> &AddForm {
    match &row.route {
        Some(Route::Imap(form)) => form,
        _ => panic!("{} is not added over IMAP", row.who),
    }
}

#[test]
fn every_account_is_listed_by_profile_with_how_it_comes_in() {
    let listing = normal(true, &["joe@example.com"]);
    let labels: Vec<&str> = listing.groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "Profile “default-release”",
            "Profile “Broken”",
            "Profile “Work”"
        ]
    );
    let rows: Vec<(&str, &str, &str)> = listing
        .rows()
        .map(|r| (r.who.as_str(), r.server.as_str(), r.how.as_str()))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("ada@fastmail.com", "imap.fastmail.com", "App password"),
            (
                "ada.lovelace@gmail.com",
                "imap.gmail.com",
                "Sign in with Google"
            ),
            (
                "ada@btinternet.com",
                "mail.btinternet.com · POP",
                "POP accounts aren't supported"
            ),
            ("Blogs & News Feeds", "", "Feeds aren't supported"),
            ("Local Folders", "", "Local Folders aren't supported"),
            ("sam@acme.co", "mail.acme.co", "Password"),
            (
                "ada@outlook.com",
                "outlook.office365.com",
                "Sign in with Microsoft"
            ),
            // A news account goes by its identity's address, like mail.
            (
                "ada@outlook.com",
                "news.example.org · News",
                "Newsgroups aren't supported"
            ),
        ]
    );
    assert_eq!(listing.addable(), 4);
    // The damaged profile says so, and lists what it could read.
    assert!(
        listing.groups[1]
            .problem
            .as_deref()
            .unwrap()
            .contains("5 of this profile's settings could not be read")
    );
}

#[test]
fn accounts_here_already_cannot_be_picked() {
    let listing = normal(true, &["ada@fastmail.com", "ada@outlook.com"]);
    let here: Vec<&str> = listing
        .rows()
        .filter(|r| r.how == "Already in Postrinch")
        .map(|r| r.who.as_str())
        .collect();
    assert_eq!(here, vec!["ada@fastmail.com", "ada@outlook.com"]);
    assert_eq!(listing.addable(), 2);
}

#[test]
fn microsoft_without_its_sign_in_cannot_be_picked() {
    let listing = normal(false, &[]);
    let outlook = listing.rows().find(|r| r.who == "ada@outlook.com").unwrap();
    assert!(outlook.route.is_none());
    assert!(outlook.how.contains("isn't set up"), "{}", outlook.how);
}

#[test]
fn an_imap_accounts_form_has_thunderbirds_servers_and_postrinchs_sign_in() {
    let listing = normal(true, &[]);
    let fastmail = imap_form(listing.rows().next().unwrap());
    assert_eq!(fastmail.kind, Kind::Thunderbird);
    assert_eq!(fastmail.address, "ada@fastmail.com");
    assert_eq!(fastmail.name, "Ada Lovelace");
    assert_eq!(fastmail.imap.line(), "imap.fastmail.com · 993 · TLS");
    assert_eq!(fastmail.smtp.line(), "smtp.fastmail.com · 465 · TLS");
    assert_eq!(fastmail.imap.username, "ada@fastmail.com");
    assert_eq!(fastmail.way, Way::Password);
    assert_eq!(fastmail.found.provider.as_deref(), Some("Fastmail"));
    assert_eq!(fastmail.found.source, Some(FROM_THUNDERBIRD));
    assert!(fastmail.found.app_password_url.is_some());
    assert!(
        fastmail.password.is_empty(),
        "no password comes from Thunderbird"
    );

    let outlook = imap_form(listing.rows().find(|r| r.who == "ada@outlook.com").unwrap());
    assert_eq!(outlook.way, Way::Browser(OAuthProvider::Microsoft));
    assert!(!outlook.found.takes_passwords);
    assert_eq!(outlook.smtp.line(), "smtp.office365.com · 587 · STARTTLS");

    // A server Thunderbird reaches without encryption is tried with
    // STARTTLS; a server without a user name signs in with the address.
    let sam = imap_form(listing.rows().find(|r| r.who == "sam@acme.co").unwrap());
    assert_eq!(sam.imap.security, Security::StartTls);
    assert_eq!(sam.imap.port, "143");
    assert!(
        sam.smtp.host.is_empty(),
        "the profile has no outgoing server"
    );
    assert_eq!(sam.smtp.line(), "Not set");
    assert_eq!(
        sam.to_add().err().as_deref(),
        Some("Give the outgoing server's name.")
    );
}

#[test]
fn gmail_comes_in_as_a_google_account() {
    let listing = normal(true, &[]);
    let gmail = listing.rows().nth(1).unwrap();
    assert!(matches!(
        &gmail.route,
        Some(Route::Google(a)) if *a == address("ada.lovelace@gmail.com")
    ));
}

#[test]
fn installations_are_named_when_there_are_several() {
    let home = tempfile::tempdir().unwrap();
    for (from, under) in [("snap", "snap"), ("flatpak", ".var")] {
        copy(&fixture(from).join(under), &home.path().join(under));
    }
    let found = postrinch_thunderbird::find(home.path());
    let listing = listing(&found, &[], |_| false).unwrap();
    let labels: Vec<&str> = listing.groups.iter().map(|g| g.label.as_str()).collect();
    assert_eq!(
        labels,
        vec![
            "Profile “default” (Snap)",
            "Profile “default-esr” (Flatpak)"
        ]
    );
}

#[test]
fn no_thunderbird_lists_nothing() {
    let home = tempfile::tempdir().unwrap();
    let found = postrinch_thunderbird::find(home.path());
    assert!(listing(&found, &[], |_| true).is_none());
}

#[test]
fn an_address_in_two_profiles_is_picked_once() {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".thunderbird");
    for dir in ["one", "two"] {
        copy(
            &fixture("flatpak")
                .join(".var/app/org.mozilla.Thunderbird/.thunderbird/f1.default-esr"),
            &root.join(dir),
        );
    }
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=one\nIsRelative=1\nPath=one\nDefault=1\n[Profile1]\nName=two\nIsRelative=1\nPath=two\n",
    )
    .unwrap();
    let found = postrinch_thunderbird::find(home.path());
    let listing = listing(&found, &[], |_| false).unwrap();
    let hows: Vec<&str> = listing.rows().map(|r| r.how.as_str()).collect();
    assert_eq!(hows, vec!["App password", "Listed above"]);
}

#[test]
fn the_import_takes_the_picked_accounts_in_turn() {
    let mut import = Import::new(normal(true, &[]));
    assert_eq!(import.picked.len(), 4, "every account that can be added");
    // A row that cannot be added is never picked.
    import.toggle(2);
    assert_eq!(import.picked.len(), 4);
    // The person leaves Gmail out.
    import.toggle(1);
    assert!(import.start());
    assert_eq!(import.current().unwrap().who, "ada@fastmail.com");
    assert_eq!(import.position(), "1 of 3");
    import.end_current(Ended::Added);
    assert_eq!(import.current().unwrap().who, "sam@acme.co");
    assert_eq!(import.position(), "2 of 3");
    import.end_current(Ended::Skipped);
    assert_eq!(import.current().unwrap().who, "ada@outlook.com");
    import.end_current(Ended::Added);
    assert!(import.current().is_none());
    assert_eq!(import.added(), 2);
    assert!(!import.stopped());
}

#[test]
fn stopping_marks_the_rest_not_added() {
    let mut import = Import::new(normal(true, &[]));
    assert!(import.start());
    import.end_current(Ended::Added);
    import.stop();
    let ended: Vec<Ended> = import.ended.iter().map(|(_, e)| *e).collect();
    assert_eq!(
        ended,
        vec![
            Ended::Added,
            Ended::NotAdded,
            Ended::NotAdded,
            Ended::NotAdded
        ]
    );
    assert!(import.stopped());
    assert_eq!(import.added(), 1);
}

#[test]
fn nothing_picked_starts_nothing() {
    let mut import = Import::new(normal(true, &[]));
    for id in import.picked.clone() {
        import.toggle(id);
    }
    assert!(!import.start());
    assert!(import.current().is_none());
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}
