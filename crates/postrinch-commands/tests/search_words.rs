//! Words and people in an IMAP account's search, here and on its server:
//! a word or person left out leaves out what has it, and what the server
//! is asked for finds at least what search here finds, so the two answers
//! agree once what the server found is checked here.

use std::collections::HashSet;

use postrinch_commands::{Commands, Grouping};
use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, Mailbox, Message, MessageHeaders, Provider,
    ProviderMessageId, ProviderThreadId, SearchKey, SystemLabel, Timestamp,
};
use postrinch_store::Store;

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

/// A message in the inbox: downloaded with `body`, or stored by its
/// headers alone with none.
fn message(me: &EmailAddress, id: &str, at: i64, subject: &str, body: Option<&str>) -> Message {
    Message {
        account: me.clone(),
        id: ProviderMessageId(id.into()),
        thread: ProviderThreadId(format!("t-{id}")),
        headers: MessageHeaders {
            subject: Some(subject.into()),
            from: vec![Mailbox::new(None, address("mary@example.com"))],
            to: vec![Mailbox::new(None, me.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(at),
        labels: vec!["INBOX".into()],
        flags: Flags::default(),
        snippet: String::new(),
        body_text: body.unwrap_or_default().into(),
        downloaded: body.is_some(),
        attachments: Vec::new(),
        size: 0,
    }
}

struct Imap {
    _dir: tempfile::TempDir,
    commands: Commands,
    me: EmailAddress,
}

fn imap(mail: &[(&str, &str, Option<&str>)]) -> Imap {
    let me = address("ada@example.org");
    let messages = mail
        .iter()
        .enumerate()
        .map(|(i, (id, subject, body))| message(&me, id, 100 * (i as i64 + 1), subject, *body))
        .collect();
    imap_of(me, messages)
}

/// An IMAP account `me` with these messages in its inbox.
fn imap_of(me: EmailAddress, messages: Vec<Message>) -> Imap {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .put_account(&Account::new(me.clone(), Provider::Imap))
        .unwrap();
    store
        .put_label(
            &me,
            &Label::new("INBOX", "Inbox", LabelKind::System(SystemLabel::Inbox)),
        )
        .unwrap();
    for message in &messages {
        store.put_message(message, None).unwrap();
    }
    Imap {
        _dir: dir,
        commands: Commands::new(store),
        me,
    }
}

impl Imap {
    /// The messages search here finds, by id, sorted.
    fn here(&self, query: &str) -> Vec<String> {
        let mut ids: Vec<String> = self
            .commands
            .search(query, 50)
            .unwrap()
            .threads
            .iter()
            .map(|r| r.summary.id.0.trim_start_matches("t-").to_owned())
            .collect();
        ids.sort();
        ids
    }

    /// Of `found` (as a server would answer), what is shown once checked
    /// here, by id, sorted.
    fn checked(&self, query: &str, found: &[&str]) -> Vec<String> {
        let found: Vec<ProviderMessageId> = found
            .iter()
            .map(|id| ProviderMessageId((*id).into()))
            .collect();
        let mut ids: Vec<String> = self
            .commands
            .remote_rows(query, &self.me, &found, &HashSet::new(), Grouping::Messages)
            .unwrap()
            .iter()
            .map(|r| r.summary.id.0.trim_start_matches("t-").to_owned())
            .collect();
        ids.sort();
        ids
    }

    fn key(&self, query: &str) -> SearchKey {
        let search = self.commands.imap_search(query, &self.me).unwrap();
        assert_eq!(search.alternatives.len(), 1, "{query}");
        search.alternatives[0].key.clone()
    }
}

#[test]
fn a_word_left_out_leaves_out_what_has_it_here() {
    let f = imap(&[
        ("weekly", "Rota", Some("the weekly rota")),
        ("drafted", "Rota", Some("a rota, drafted")),
        ("lunch", "Lunch", Some("noodles")),
    ]);
    // Not "a rota that must say draft": the word is left out.
    assert_eq!(f.here("rota -draft"), ["weekly"]);
    assert_eq!(f.here("-draft"), ["lunch", "weekly"]);
    // A stop word leaves nothing out.
    assert_eq!(f.here("rota -the"), ["drafted", "weekly"]);
}

#[test]
fn a_word_left_out_is_checked_here_not_asked() {
    let f = imap(&[
        ("concat", "Rota", Some("concatenate the rota")),
        ("cat", "Rota", Some("the cat's rota")),
        ("subject", "Cat rota", None),
        ("body", "Rota", None),
    ]);
    // The server is asked for the rota alone: NOT TEXT "cat" would leave
    // out "concatenate", which search here keeps.
    assert_eq!(f.key("rota -cat"), SearchKey::Text("rota".into()));
    // Here, "body" is found by its subject.
    assert_eq!(f.here("rota -cat"), ["body", "concat"]);
    // Whatever the server finds, what is shown agrees with search here:
    // what has the word, in its body or (stored by its headers) in its
    // subject, is left out.
    assert_eq!(
        f.checked("rota -cat", &["concat", "cat", "subject", "body"]),
        ["body", "concat"]
    );
}

#[test]
fn what_the_server_finds_by_a_stem_is_checked_here() {
    let f = imap(&[
        ("shipped", "Order", Some("it shipped today")),
        ("relationship", "Us", Some("our relationship")),
        ("waiting", "Order", None),
    ]);
    // `shipping` is `ship` here, as `shipped` is: the server is asked for
    // the text both have.
    assert_eq!(f.key("shipping"), SearchKey::Text("ship".into()));
    assert_eq!(f.here("shipping"), ["shipped"]);
    // "relationship" has the text but not the word: its body is here, so
    // the check drops it. The one stored by its headers is the server's.
    assert_eq!(
        f.checked("shipping", &["shipped", "relationship", "waiting"]),
        ["shipped", "waiting"]
    );
}

#[test]
fn a_hyphenated_word_or_phrase_is_its_terms() {
    let f = imap(&[
        ("joined", "Invoice-4471", Some("attached")),
        ("apart", "Invoice", Some("the 4471 one")),
        ("wrapped", "Weekly", Some("the weekly\r\nreport")),
    ]);
    assert_eq!(
        f.key("invoice-4471"),
        SearchKey::All(vec![
            SearchKey::Text("invoic".into()),
            SearchKey::Text("4471".into())
        ])
    );
    // Search here finds both, as the server asked so does.
    assert_eq!(f.here("invoice-4471"), ["apart", "joined"]);
    assert_eq!(
        f.checked("invoice-4471", &["apart", "joined"]),
        ["apart", "joined"]
    );
    // A phrase across a line break is still the phrase here; the server
    // is asked for its terms, which do not need to be side by side.
    assert_eq!(f.here(r#""weekly report""#), ["wrapped"]);
    assert_eq!(
        f.key(r#""weekly report""#),
        SearchKey::All(vec![
            SearchKey::Text("week".into()),
            SearchKey::Text("report".into())
        ])
    );
}

#[test]
fn people_left_out_are_checked_here_not_asked() {
    let me = address("ada@example.org");
    // A rota from (or copied to) each of these, downloaded or stored by its
    // headers alone.
    let people = [
        ("jose", None, "jose@example.com", Some("the rota")),
        ("accented", Some("José Pérez"), "jp@example.com", None),
        ("mary", None, "mary@example.com", Some("the rota")),
        ("joseph", None, "joseph@example.com", None),
    ];
    let mail = |to_too: bool| {
        let messages: Vec<Message> = people
            .iter()
            .enumerate()
            .map(|(i, (id, name, who, body))| {
                let mut m = message(&me, id, 100 * (i as i64 + 1), "Rota", *body);
                let who = Mailbox::new(name.map(str::to_owned), address(who));
                if to_too {
                    m.headers.cc = vec![who];
                } else {
                    m.headers.from = vec![who];
                }
                m
            })
            .collect();
        imap_of(me.clone(), messages)
    };
    let found = ["jose", "accented", "mary", "joseph"];
    for (op, to_too) in [("from", false), ("to", true), ("cc", true), ("bcc", true)] {
        let f = mail(to_too);
        let query = format!("rota -{op}:jose");
        // The server is asked for the rota alone: a server that folds
        // accents would leave out "José" for NOT FROM "jose", where search
        // here may keep it.
        assert_eq!(f.key(&query), SearchKey::Text("rota".into()), "{query}");
        // The check here leaves out what search here leaves out, and the
        // one it leaves out is gone, whatever the server found.
        let here = f.here(&query);
        assert!(!here.contains(&"jose".to_owned()), "{query}: {here:?}");
        assert!(here.contains(&"mary".to_owned()), "{query}: {here:?}");
        assert_eq!(f.checked(&query, &found), here, "{query}");
    }
}
