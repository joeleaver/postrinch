use postrinch_model::{
    Account, EmailAddress, Flags, Label, LabelKind, ListOrder, Mailbox, Message, MessageHeaders,
    Provider, ProviderMessageId, ProviderThreadId, SavedView, SystemLabel, Timestamp, ViewCount,
};
use postrinch_store::Store;

use crate::{Commands, EVERYTHING_ELSE, Place};

fn address(s: &str) -> EmailAddress {
    EmailAddress::parse(s).unwrap()
}

fn message(me: &EmailAddress, thread: &str, at: i64, unread: bool, labels: &[&str]) -> Message {
    Message {
        account: me.clone(),
        id: ProviderMessageId(format!("{thread}-{at}")),
        thread: ProviderThreadId(thread.into()),
        headers: MessageHeaders {
            subject: Some(format!("About {thread}")),
            from: vec![Mailbox::new(None, address("ann@example.org"))],
            to: vec![Mailbox::new(None, me.clone())],
            ..Default::default()
        },
        received: Timestamp::from_millis(at),
        labels: labels.iter().map(|l| l.to_string()).collect(),
        flags: Flags {
            unread,
            starred: false,
        },
        snippet: String::new(),
        body_text: String::new(),
        downloaded: true,
        attachments: Vec::new(),
        size: 0,
    }
}

/// An inbox of three conversations: a Primary one, an Updates one, and a
/// client's (labelled, in Primary too) that I answered last.
fn setup() -> (tempfile::TempDir, Commands, EmailAddress) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let me = address("me@example.org");
    store
        .put_account(&Account::new(me.clone(), Provider::Google))
        .unwrap();
    let personal = LabelKind::System(SystemLabel::CategoryPersonal);
    let updates = LabelKind::System(SystemLabel::CategoryUpdates);
    for (id, name, kind) in [
        ("INBOX", "INBOX", LabelKind::System(SystemLabel::Inbox)),
        ("SENT", "SENT", LabelKind::System(SystemLabel::Sent)),
        ("CATEGORY_PERSONAL", "CATEGORY_PERSONAL", personal),
        ("CATEGORY_UPDATES", "CATEGORY_UPDATES", updates),
        ("Label_1", "Clients", LabelKind::User),
    ] {
        store.put_label(&me, &Label::new(id, name, kind)).unwrap();
    }
    let inbox = ["INBOX", "CATEGORY_PERSONAL"];
    store
        .put_message(&message(&me, "dinner", 300, true, &inbox), None)
        .unwrap();
    let update = ["INBOX", "CATEGORY_UPDATES"];
    store
        .put_message(&message(&me, "invoice", 200, false, &update), None)
        .unwrap();
    let client = ["INBOX", "CATEGORY_PERSONAL", "Label_1"];
    store
        .put_message(&message(&me, "acme", 100, false, &client), None)
        .unwrap();
    let mut reply = message(&me, "acme", 150, false, &["SENT"]);
    reply.headers.from = vec![Mailbox::new(None, me.clone())];
    reply.headers.to = vec![Mailbox::new(None, address("ann@example.org"))];
    reply.headers.in_reply_to = Some("acme-100@example.org".into());
    store.put_message(&reply, None).unwrap();
    (dir, Commands::new(store), me)
}

fn listed(commands: &Commands, query: &str) -> Vec<String> {
    let view = commands.new_view("Test", query).unwrap();
    let page = commands
        .list_view(&view, ListOrder::NewestFirst, 0, 50)
        .unwrap();
    page.threads
        .iter()
        .map(|r| r.summary.id.0.clone())
        .collect()
}

#[test]
fn built_in_views_come_first_and_last() {
    let (_dir, commands, _) = setup();
    let keys: Vec<String> = commands
        .views()
        .unwrap()
        .into_iter()
        .map(|e| e.view.key)
        .collect();
    assert_eq!(keys[..2], ["inbox", "updates"], "Primary is the inbox");
    assert_eq!(keys.last().unwrap(), "drafts");
    let clients = commands.new_view("Clients", "label:clients").unwrap();
    commands.save_view(&clients).unwrap();
    let entries = commands.views().unwrap();
    let mine = entries.iter().find(|e| e.view.key == clients.key).unwrap();
    assert_eq!((mine.place, mine.original.is_none()), (Place::Yours, true));
    assert_eq!(
        entries[5].view.name, "Clients",
        "after the five inbox views"
    );
}

#[test]
fn each_account_has_an_inbox_under_inbox_when_there_are_several() {
    let (_dir, commands, me) = setup();
    let nested = |commands: &Commands| -> Vec<String> {
        let entries = commands.views().unwrap();
        assert_eq!(entries[0].view.key, "inbox");
        entries
            .iter()
            .skip(1)
            .take_while(|e| e.nested)
            .map(|e| e.view.name.clone())
            .collect()
    };
    assert!(
        nested(&commands).is_empty(),
        "one account: no inbox of its own"
    );

    let work = address("me@work.example");
    commands
        .store
        .put_account(&Account::new(work.clone(), Provider::Google))
        .unwrap();
    assert_eq!(nested(&commands), [me.to_string(), work.to_string()]);
    let mine = commands
        .views()
        .unwrap()
        .into_iter()
        .find(|e| e.view.name == me.to_string())
        .unwrap();
    assert_eq!(listed_view(&commands, &mine.view), ["dinner", "acme"]);

    // They follow Inbox: changed there, changed in each.
    let mut inbox = commands.views().unwrap()[0].view.clone();
    inbox.query = "in:inbox".into();
    commands.save_view(&inbox).unwrap();
    let mine = commands
        .views()
        .unwrap()
        .into_iter()
        .find(|e| e.view.key == mine.view.key)
        .unwrap();
    assert_eq!(mine.view.query, "in:inbox");
    assert_eq!(
        listed_view(&commands, &mine.view),
        ["dinner", "invoice", "acme"]
    );
    commands.remove_view("inbox").unwrap();

    // Hidden, it stays an account inbox, not a view of the person's own.
    let mut hidden = mine.view.clone();
    hidden.hidden = true;
    commands.save_view(&hidden).unwrap();
    let entries = commands.views().unwrap();
    let again = entries.iter().find(|e| e.view.key == hidden.key).unwrap();
    assert!(again.nested && again.view.hidden && again.original.is_some());
    assert!(entries.iter().all(|e| e.place != Place::Yours));
}

fn listed_view(commands: &Commands, view: &postrinch_model::SavedView) -> Vec<String> {
    let page = commands
        .list_view(view, ListOrder::NewestFirst, 0, 50)
        .unwrap();
    page.threads
        .iter()
        .map(|r| r.summary.id.0.clone())
        .collect()
}

#[test]
fn labels_alone_and_searches_list_the_same() {
    let (_dir, commands, _) = setup();
    assert_eq!(listed(&commands, "in:inbox"), ["dinner", "invoice", "acme"]);
    assert_eq!(
        listed(&commands, "in:inbox category:primary"),
        ["dinner", "acme"]
    );
    // The same, the slow way: a flag makes it a search.
    assert_eq!(
        listed(&commands, "in:inbox category:primary is:read"),
        ["acme"]
    );
    assert_eq!(
        listed(&commands, "label:clients OR category:updates"),
        ["invoice", "acme"]
    );
    assert_eq!(listed(&commands, "in:inbox last:me"), ["acme"]);
    // Every inbox message is Ann's; leaving her out leaves nothing, and
    // any of two senders finds hers.
    assert!(listed(&commands, "in:inbox -from:ann").is_empty());
    assert_eq!(
        listed(&commands, "in:inbox {from:ann from:nobody}"),
        ["dinner", "invoice", "acme"]
    );
    assert_eq!(
        listed(&commands, "in:inbox last:them"),
        ["dinner", "invoice"]
    );
}

#[test]
fn counts_and_everything_else() {
    let (_dir, commands, me) = setup();
    let mut clients = commands.new_view("Clients", "label:clients").unwrap();
    clients.count = ViewCount::All;
    commands.save_view(&clients).unwrap();
    let views: Vec<_> = commands
        .views()
        .unwrap()
        .into_iter()
        .map(|e| e.view)
        .collect();
    let counts = commands.view_counts(&views).unwrap();
    let count = |key: &str| {
        let view = views.iter().find(|v| v.key == key).unwrap();
        counts
            .iter()
            .find(|t| t.key == key)
            .and_then(|t| t.shown(view.count))
    };
    assert_eq!(count("inbox"), Some(1));
    assert_eq!(count(&clients.key), Some(1));
    assert_eq!(count("promotions"), None, "counts nothing");
    // Every inbox conversation is in a tab, so nothing else is left.
    let everything_else = views.iter().find(|v| v.key == EVERYTHING_ELSE).unwrap();
    let page = commands
        .list_view(everything_else, ListOrder::NewestFirst, 0, 50)
        .unwrap();
    assert_eq!(page.total, 0);
    // Hide Updates, and its conversation is left for Everything else.
    let mut updates = views.iter().find(|v| v.key == "updates").unwrap().clone();
    updates.hidden = true;
    commands.save_view(&updates).unwrap();
    let page = commands
        .list_view(everything_else, ListOrder::NewestFirst, 0, 50)
        .unwrap();
    assert_eq!(page.threads[0].summary.id.0, "invoice");
    // One account's view of another's mail is empty.
    let mut elsewhere = commands.new_view("Other", "in:inbox").unwrap();
    elsewhere.accounts = vec![me];
    assert_eq!(
        commands
            .list_view(&elsewhere, ListOrder::NewestFirst, 0, 50)
            .unwrap()
            .total,
        3
    );
}

#[test]
fn saving_checks_and_removing_puts_built_ins_back() {
    let (_dir, commands, _) = setup();
    let mut bad = commands.new_view("Bad", "label:clients").unwrap();
    bad.query = "is:sleepy".into();
    assert!(commands.save_view(&bad).is_err());
    bad.query = "in:inbox".into();
    bad.accounts = vec![address("stranger@example.org")];
    assert!(commands.save_view(&bad).is_err());

    let mut updates = commands.views().unwrap()[1].view.clone();
    updates.query = "in:inbox category:updates is:unread".into();
    commands.save_view(&updates).unwrap();
    let entry = commands.views().unwrap()[1].clone();
    assert_eq!(entry.view.query, updates.query);
    assert_eq!(entry.original.unwrap().query, "in:inbox category:updates");
    commands.remove_view("updates").unwrap();
    assert_eq!(
        commands.views().unwrap()[1].view.query,
        "in:inbox category:updates"
    );

    // A Primary saved before it was the inbox is passed over, not made
    // one of the person's own.
    let mut old = updates;
    old.key = "primary".into();
    commands.store.put_view(&old).unwrap();
    assert!(
        commands
            .views()
            .unwrap()
            .iter()
            .all(|e| e.view.key != "primary")
    );
}

#[test]
fn an_agent_of_some_accounts_changes_only_views_of_those() {
    let (_dir, commands, me) = setup();
    let work = address("me@work.example");
    commands
        .store
        .put_account(&Account::new(work.clone(), Provider::Google))
        .unwrap();
    let at = Timestamp::from_millis(1);
    let mut theirs = commands.new_view("Work", "is:unread").unwrap();
    theirs.accounts = vec![work.clone()];
    commands.save_view(&theirs).unwrap();
    let mut every = SavedView::new("Every", "is:starred", Timestamp::from_millis(2)).unwrap();
    commands.save_view(&every).unwrap();

    let agent = commands.limited_to(std::slice::from_ref(&me));
    let keys: Vec<String> = agent
        .views()
        .unwrap()
        .into_iter()
        .map(|e| e.view.key)
        .collect();
    assert!(
        !keys.contains(&theirs.key),
        "another account's view is not there"
    );
    assert!(keys.contains(&every.key) && keys.contains(&"inbox".to_owned()));

    // A view of every account, built in or not, is the person's.
    every.name = "Changed".into();
    assert!(agent.save_view(&every).is_err());
    assert!(agent.remove_view("inbox").is_err());
    let mut wider = SavedView::new("Mine", "is:unread", at).unwrap();
    assert!(agent.save_view(&wider).is_err());
    wider.accounts = vec![me.clone()];
    agent.save_view(&wider).unwrap();
    // Nor can it take over another account's view by its key.
    let mut taken = theirs.clone();
    taken.accounts = vec![me.clone()];
    assert!(agent.save_view(&taken).is_err());
    assert!(agent.remove_view(&theirs.key).is_err());
    agent.remove_view(&wider.key).unwrap();
    // The window changes any.
    commands.save_view(&every).unwrap();
}
