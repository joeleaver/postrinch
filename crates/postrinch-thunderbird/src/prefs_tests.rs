use super::*;

fn s(v: &str) -> PrefValue {
    PrefValue::String(v.to_owned())
}

#[test]
fn reads_strings_ints_and_bools() {
    let prefs = Prefs::parse(
        r#"// Mozilla User Preferences
user_pref("mail.server.server1.hostname", "imap.example.org");
user_pref("mail.server.server1.port", 993);
user_pref("mail.server.server1.check_new_mail", true);
user_pref("mail.server.server1.download_on_biff", false);
user_pref("negative", -12);
user_pref("plus", +7);
"#,
    );
    assert!(prefs.errors.is_empty(), "{:?}", prefs.errors);
    assert_eq!(
        prefs.string("mail.server.server1.hostname"),
        Some("imap.example.org")
    );
    assert_eq!(prefs.int("mail.server.server1.port"), Some(993));
    assert_eq!(prefs.bool("mail.server.server1.check_new_mail"), Some(true));
    assert_eq!(
        prefs.bool("mail.server.server1.download_on_biff"),
        Some(false)
    );
    assert_eq!(prefs.int("negative"), Some(-12));
    assert_eq!(prefs.int("plus"), Some(7));
    // The wrong kind is none, not a guess.
    assert_eq!(prefs.int("mail.server.server1.hostname"), None);
    assert_eq!(prefs.string("mail.server.server1.port"), None);
}

#[test]
fn reads_every_escape() {
    let prefs = Prefs::parse(
        r#"user_pref("a", "say \"hi\" \\ it's");
user_pref("b", 'single \'quoted\' "double"');
user_pref("c", "line\nnext\rback");
user_pref("d", "caf\xe9 é ☃");
user_pref("e", "😀");
"#,
    );
    assert!(prefs.errors.is_empty(), "{:?}", prefs.errors);
    assert_eq!(prefs.get("a"), Some(&s(r#"say "hi" \ it's"#)));
    assert_eq!(prefs.get("b"), Some(&s(r#"single 'quoted' "double""#)));
    assert_eq!(prefs.get("c"), Some(&s("line\nnext\rback")));
    assert_eq!(prefs.get("d"), Some(&s("café é ☃")));
    assert_eq!(prefs.get("e"), Some(&s("😀")));
}

#[test]
fn a_string_may_hold_raw_utf8_and_newlines() {
    let prefs = Prefs::parse("user_pref(\"sig\", \"Ada — Lovelace\nLondon\");");
    assert_eq!(prefs.string("sig"), Some("Ada — Lovelace\nLondon"));
}

#[test]
fn skips_comments_of_every_kind() {
    let prefs = Prefs::parse(
        r#"# a hash comment user_pref("no", 1);
/* a block
   user_pref("no", 2); */
user_pref("yes", /* inline */ 3); // trailing
pref("default", 4);
sticky_pref("sticky", 5);
user_pref("locked", 6, locked);
user_pref("both", 7, sticky, locked);
"#,
    );
    assert!(prefs.errors.is_empty(), "{:?}", prefs.errors);
    assert_eq!(prefs.get("no"), None);
    assert_eq!(prefs.int("yes"), Some(3));
    assert_eq!(prefs.int("default"), Some(4));
    assert_eq!(prefs.int("sticky"), Some(5));
    assert_eq!(prefs.int("locked"), Some(6));
    assert_eq!(prefs.int("both"), Some(7));
}

#[test]
fn many_prefs_on_one_line_and_one_pref_over_many() {
    let prefs =
        Prefs::parse("user_pref(\"a\",1);user_pref(\"b\",2);\nuser_pref(\n  \"c\",\n  3\n);");
    assert!(prefs.errors.is_empty(), "{:?}", prefs.errors);
    assert_eq!(prefs.int("a"), Some(1));
    assert_eq!(prefs.int("b"), Some(2));
    assert_eq!(prefs.int("c"), Some(3));
}

#[test]
fn a_later_value_replaces_an_earlier_one() {
    let mut prefs = Prefs::parse("user_pref(\"a\", 1);\nuser_pref(\"a\", 2);");
    assert_eq!(prefs.int("a"), Some(2));
    // As user.js does prefs.js's.
    prefs.read("user_pref(\"a\", \"three\");");
    assert_eq!(prefs.string("a"), Some("three"));
}

#[test]
fn a_bad_pref_is_skipped_and_the_rest_kept() {
    let prefs = Prefs::parse(
        r#"user_pref("good1", 1);
user_pref("bad1", nope);
user_pref("good2", 2);
user_pref("bad2", "\q");
user_pref("bad3", 99999999999);
user_pref("bad4" 4);
garbage here;
user_pref("bad5", "\ud83d");
user_pref("bad6", 12abc);
user_pref("bad7", 1, frozen);
user_pref("bad8", "\x00");
user_pref("bad9", 9;
user_pref("good3", 3);
"#,
    );
    assert_eq!(prefs.int("good1"), Some(1));
    assert_eq!(prefs.int("good2"), Some(2));
    assert_eq!(prefs.int("good3"), Some(3));
    let lines: Vec<usize> = prefs.errors.iter().map(|e| e.line).collect();
    assert_eq!(lines, vec![2, 4, 5, 6, 7, 8, 9, 10, 11, 12]);
    for name in [
        "bad1", "bad2", "bad3", "bad4", "bad5", "bad6", "bad7", "bad8", "bad9",
    ] {
        assert_eq!(prefs.get(name), None, "{name}");
    }
}

#[test]
fn errors_never_quote_what_a_pref_held() {
    let prefs = Prefs::parse("user_pref(\"secret\", \"hunter2\\q\");");
    assert_eq!(prefs.errors.len(), 1);
    assert!(!format!("{:?}", prefs.errors).contains("hunter2"));
}

#[test]
fn a_file_cut_short_keeps_what_came_before() {
    let prefs = Prefs::parse("user_pref(\"a\", 1);\nuser_pref(\"b\", \"never closed");
    assert_eq!(prefs.int("a"), Some(1));
    assert_eq!(prefs.errors.len(), 1);
    let unclosed = Prefs::parse("user_pref(\"a\", 1);\n/* never closed");
    assert_eq!(unclosed.int("a"), Some(1));
    assert_eq!(unclosed.errors.len(), 1);
}

#[test]
fn an_empty_file_has_nothing() {
    let prefs = Prefs::parse("");
    assert!(prefs.errors.is_empty());
    assert_eq!(prefs.names().count(), 0);
}
