//! Finding profiles and their accounts in the fixture homes under
//! `tests/fixtures`: a usual Thunderbird with several profiles (one with
//! a damaged prefs.js, one whose folder is gone), the Snap's and the
//! Flatpak's layouts, Windows', all three Linux ones at once, and a profile at an absolute
//! path. And that nothing but the settings files is ever asked for.

mod support;

use std::path::{Path, PathBuf};

use postrinch_thunderbird::{
    AccountKind, Install, Problem, Security, SignIn, Signature, find, find_with,
};
use support::{Recording, copy_tree, fixture};

#[test]
fn a_usual_thunderbird_with_several_profiles() {
    let home = fixture("normal");
    let found = find(&home);
    assert!(found.unreadable.is_empty());
    let names: Vec<&str> = found.profiles.iter().map(|p| p.name.as_str()).collect();
    // The default first; the profile whose folder is gone is left out.
    assert_eq!(names, vec!["default-release", "Broken", "Work"]);
    assert!(found.profiles[0].default);
    assert!(found.profiles.iter().skip(1).all(|p| !p.default));
    assert!(found.profiles.iter().all(|p| p.install == Install::Native));

    let main = &found.profiles[0];
    assert_eq!(main.problems, vec![]);
    assert_eq!(
        main.dir,
        home.join(".thunderbird/x1.default-release"),
        "a relative path is under profiles.ini's folder"
    );
    let kinds: Vec<&AccountKind> = main.accounts.iter().map(|a| &a.kind).collect();
    assert_eq!(
        kinds,
        vec![
            &AccountKind::Imap,
            &AccountKind::Imap,
            &AccountKind::Pop,
            &AccountKind::Feeds,
            &AccountKind::LocalFolders,
        ]
    );

    // A password IMAP account, with its own outgoing server.
    let fastmail = &main.accounts[0];
    let identity = fastmail.identity.as_ref().unwrap();
    assert_eq!(identity.address.as_deref(), Some("ada@fastmail.com"));
    // user.js wins over prefs.js.
    assert_eq!(identity.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(
        identity.signature,
        Some(Signature {
            text: "Ada Lovelace\n\"Analytical\" Engines".into(),
            html: false,
        })
    );
    let imap = fastmail.incoming.as_ref().unwrap();
    assert_eq!(
        (imap.host.as_str(), imap.port, imap.security, imap.sign_in),
        ("imap.fastmail.com", 993, Security::Tls, SignIn::Password)
    );
    assert_eq!(imap.username.as_deref(), Some("ada@fastmail.com"));
    let smtp = fastmail.outgoing.as_ref().unwrap();
    assert_eq!(
        (smtp.host.as_str(), smtp.port, smtp.security),
        ("smtp.fastmail.com", 465, Security::Tls)
    );

    // Gmail over OAuth, its signature in a file (so not taken).
    let gmail = &main.accounts[1];
    assert_eq!(gmail.incoming.as_ref().unwrap().sign_in, SignIn::OAuth2);
    assert_eq!(gmail.outgoing.as_ref().unwrap().host, "smtp.gmail.com");
    assert_eq!(gmail.identity.as_ref().unwrap().signature, None);

    // POP, feeds and Local Folders are listed, with their names.
    assert_eq!(
        main.accounts[2].incoming.as_ref().unwrap().host,
        "mail.btinternet.com"
    );
    assert_eq!(
        main.accounts[3].label.as_deref(),
        Some("Blogs & News Feeds")
    );
    assert_eq!(main.accounts[4].label.as_deref(), Some("Local Folders"));
    assert_eq!(main.accounts[4].outgoing, None);
}

#[test]
fn microsoft_over_oauth_and_news_in_the_work_profile() {
    let found = find(&fixture("normal"));
    let work = found.profiles.iter().find(|p| p.name == "Work").unwrap();
    let outlook = &work.accounts[0];
    let imap = outlook.incoming.as_ref().unwrap();
    assert_eq!(
        (imap.host.as_str(), imap.sign_in),
        ("outlook.office365.com", SignIn::OAuth2)
    );
    let smtp = outlook.outgoing.as_ref().unwrap();
    assert_eq!(
        (smtp.host.as_str(), smtp.port, smtp.security, smtp.sign_in),
        (
            "smtp.office365.com",
            587,
            Security::StartTls,
            SignIn::OAuth2
        )
    );
    assert_eq!(
        outlook.identity.as_ref().unwrap().signature,
        Some(Signature {
            text: "<b>Ada</b> &mdash; Work".into(),
            html: true,
        })
    );
    assert_eq!(work.accounts[1].kind, AccountKind::News);
}

#[test]
fn a_damaged_prefs_js_keeps_what_parses_and_says_so() {
    let found = find(&fixture("normal"));
    let broken = found.profiles.iter().find(|p| p.name == "Broken").unwrap();
    assert_eq!(
        broken.problems,
        vec![Problem::Malformed {
            file: "prefs.js",
            prefs: 5,
            first_line: 6,
        }]
    );
    let account = &broken.accounts[0];
    assert_eq!(account.kind, AccountKind::Imap);
    let identity = account.identity.as_ref().unwrap();
    assert_eq!(identity.address.as_deref(), Some("sam@acme.co"));
    assert_eq!(identity.name, None);
    let imap = account.incoming.as_ref().unwrap();
    // The port and security that did not parse are Thunderbird's usual.
    assert_eq!(
        (imap.host.as_str(), imap.port, imap.security),
        ("mail.acme.co", 143, Security::Plain)
    );
    assert_eq!(imap.username.as_deref(), Some("sam"));
}

#[test]
fn the_snap_layout() {
    let home = fixture("snap");
    let found = find(&home);
    assert_eq!(found.profiles.len(), 1);
    let profile = &found.profiles[0];
    assert_eq!(profile.install, Install::Snap);
    assert!(profile.default, "Default=1 with no Install section");
    assert_eq!(
        profile.dir,
        home.join("snap/thunderbird/common/.thunderbird/s1.default")
    );
    let account = &profile.accounts[0];
    let imap = account.incoming.as_ref().unwrap();
    assert_eq!(imap.host, "mail.acme.co");
    assert_eq!(imap.username.as_deref(), Some("sam.jones"));
    // No identity names an outgoing server: the default one, on
    // Thunderbird's usual port for it (no port is set).
    let smtp = account.outgoing.as_ref().unwrap();
    assert_eq!(
        (smtp.host.as_str(), smtp.port, smtp.security),
        ("smtp.acme.co", 587, Security::StartTls)
    );
}

#[test]
fn the_windows_layout() {
    let home = fixture("windows");
    let found = find(&home);
    assert_eq!(found.profiles.len(), 1);
    let profile = &found.profiles[0];
    assert_eq!(profile.install, Install::Native);
    assert!(profile.default, "named by its Install section");
    assert_eq!(
        profile.dir,
        home.join("AppData/Roaming/Thunderbird")
            .join("Profiles/w1.default-release")
    );
    let imap = profile.accounts[0].incoming.as_ref().unwrap();
    assert_eq!(
        (imap.host.as_str(), imap.port, imap.security),
        ("imap.acme.co", 993, Security::Tls)
    );
}

#[test]
fn the_flatpak_layout() {
    let found = find(&fixture("flatpak"));
    assert_eq!(found.profiles.len(), 1);
    let profile = &found.profiles[0];
    assert_eq!(profile.install, Install::Flatpak);
    assert!(profile.default, "named by its Install section");
    assert_eq!(
        profile.accounts[0].incoming.as_ref().unwrap().host,
        "imap.mail.me.com"
    );
}

#[test]
fn all_three_installations_at_once_in_order() {
    let home = tempfile::tempdir().unwrap();
    for (from, under) in [
        ("normal", ".thunderbird"),
        ("snap", "snap"),
        ("flatpak", ".var"),
    ] {
        copy_tree(&fixture(from).join(under), &home.path().join(under));
    }
    let found = find(home.path());
    let installs: Vec<Install> = found.profiles.iter().map(|p| p.install).collect();
    assert_eq!(
        installs,
        vec![
            Install::Native,
            Install::Native,
            Install::Native,
            Install::Snap,
            Install::Flatpak,
        ]
    );
}

#[test]
fn a_profile_at_an_absolute_path() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let root = home.path().join(".thunderbird");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("profiles.ini"),
        format!(
            "[Profile0]\nName=Elsewhere\nIsRelative=0\nPath={}\nDefault=1\n",
            elsewhere.path().display()
        ),
    )
    .unwrap();
    copy_tree(
        &fixture("flatpak").join(".var/app/org.mozilla.Thunderbird/.thunderbird/f1.default-esr"),
        elsewhere.path(),
    );
    let found = find(home.path());
    assert_eq!(found.profiles.len(), 1);
    assert_eq!(found.profiles[0].dir, elsewhere.path());
    assert_eq!(found.profiles[0].accounts.len(), 1);
}

#[test]
fn no_thunderbird_finds_nothing() {
    let home = tempfile::tempdir().unwrap();
    assert_eq!(find(home.path()), Default::default());
}

#[test]
fn an_unreadable_profiles_ini_is_told() {
    let home = tempfile::tempdir().unwrap();
    // A folder where the file should be cannot be read as one.
    std::fs::create_dir_all(home.path().join(".thunderbird/profiles.ini")).unwrap();
    let found = find(home.path());
    assert!(found.profiles.is_empty());
    assert_eq!(found.unreadable.len(), 1);
}

/// A home with one profile, "one", holding `prefs.js` and, if given,
/// `user.js`.
fn one_profile(prefs: &str, user: Option<&str>) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let root = home.path().join(".thunderbird");
    std::fs::create_dir_all(root.join("one")).unwrap();
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=one\nIsRelative=1\nPath=one\nDefault=1\n",
    )
    .unwrap();
    std::fs::write(root.join("one/prefs.js"), prefs).unwrap();
    if let Some(user) = user {
        std::fs::write(root.join("one/user.js"), user).unwrap();
    }
    home
}

#[test]
fn each_problem_names_its_file() {
    let home = one_profile(
        "user_pref(\"a\", 1);\nuser_pref(\"b\" 2);\n",
        Some("user_pref(\"c\", 3);\nuser_pref(\"d\", );\nuser_pref(\"e\" 5);\n"),
    );
    let found = find(home.path());
    assert_eq!(
        found.profiles[0].problems,
        vec![
            Problem::Malformed {
                file: "prefs.js",
                prefs: 1,
                first_line: 2,
            },
            Problem::Malformed {
                file: "user.js",
                prefs: 2,
                first_line: 2,
            },
        ]
    );
}

#[test]
fn a_linked_user_js_is_refused_and_the_profile_says_so() {
    let home = one_profile("user_pref(\"a\", 1);\n", None);
    let dir = home.path().join(".thunderbird/one");
    std::fs::write(dir.join("logins.json"), "{}").unwrap();
    support::link_file(&dir.join("logins.json"), &dir.join("user.js"));
    let found = find(home.path());
    assert_eq!(
        found.profiles[0].problems,
        vec![Problem::Refused {
            file: "user.js",
            why: "user.js is a link, so Postrinch didn't read it".into(),
        }]
    );
}

#[test]
fn a_linked_profiles_ini_is_refused() {
    let home = one_profile("user_pref(\"a\", 1);\n", None);
    let root = home.path().join(".thunderbird");
    std::fs::rename(root.join("profiles.ini"), root.join("real.ini")).unwrap();
    support::link_file(&root.join("real.ini"), &root.join("profiles.ini"));
    let found = find(home.path());
    assert!(found.profiles.is_empty());
    assert_eq!(
        found.unreadable,
        vec![(
            root.join("profiles.ini"),
            "profiles.ini is a link, so Postrinch didn't read it".to_owned()
        )]
    );
}

/// The files a search of `home` asks for.
fn asked_for(home: &Path) -> Vec<PathBuf> {
    let files = Recording::default();
    find_with(home, &files);
    files.asked()
}

#[test]
fn only_settings_files_are_ever_asked_for() {
    let normal = fixture("normal");
    // The decoys are there to be found, next to the prefs.js read.
    for decoy in [
        "logins.json",
        "key4.db",
        "key3.db",
        "signons.sqlite",
        "cert9.db",
    ] {
        assert!(
            normal
                .join(".thunderbird/x1.default-release")
                .join(decoy)
                .exists()
        );
    }
    for home in ["normal", "snap", "flatpak", "windows"] {
        let asked = asked_for(&fixture(home));
        assert!(!asked.is_empty());
        for path in &asked {
            let name = path.file_name().unwrap().to_str().unwrap();
            assert!(
                ["profiles.ini", "prefs.js", "user.js"].contains(&name),
                "{} was asked for",
                path.display()
            );
        }
    }
    // Exactly: each installation's profiles.ini, then each profile's
    // prefs.js and (when it has one) user.js.
    let tb = normal.join(".thunderbird");
    assert_eq!(
        asked_for(&normal),
        vec![
            tb.join("profiles.ini"),
            tb.join("x1.default-release/prefs.js"),
            tb.join("x1.default-release/user.js"),
            tb.join("g4.gone/prefs.js"),
            tb.join("b3.broken/prefs.js"),
            tb.join("b3.broken/user.js"),
            tb.join("w2.work/prefs.js"),
            tb.join("w2.work/user.js"),
            normal.join("AppData/Roaming/Thunderbird/profiles.ini"),
            normal.join("snap/thunderbird/common/.thunderbird/profiles.ini"),
            normal.join(".var/app/org.mozilla.Thunderbird/.thunderbird/profiles.ini"),
        ]
    );
}

#[test]
fn the_crate_reads_the_disk_in_one_place_only() {
    // Every read goes through files.rs, which refuses anything but the
    // settings files: no other source file may touch the file system.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        if name == "files.rs" {
            continue;
        }
        for banned in [
            "std::fs",
            "File::",
            "read_dir",
            "metadata",
            "canonicalize",
            ".exists()",
            ".is_file()",
            ".is_dir()",
        ] {
            assert!(!text.contains(banned), "{name} uses {banned}");
        }
    }
}
