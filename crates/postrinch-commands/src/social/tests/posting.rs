//! Drafts, and posting, editing, deleting and reacting through the outbox.

use postrinch_store::{
    ContentKind, DraftMedia, ReactionSum, SocialChange, SocialContent, SocialProvider,
};

use super::*;

fn content(body: &str) -> SocialContent {
    SocialContent {
        body: body.into(),
        ..SocialContent::default()
    }
}

#[test]
fn drafts_are_kept_until_sent_and_survive_a_restart() {
    let world = world();
    let mut draft = world
        .commands
        .start_social_draft(ADA, Some(GENERAL), None, ContentKind::Memo)
        .unwrap();
    assert_eq!(draft.account, ADA);
    draft.body = "half written".into();
    let saved = world.commands.save_social_draft(&draft).unwrap();
    assert!(saved.updated >= draft.updated);
    let empty_id = SocialDraft {
        id: String::new(),
        ..saved.clone()
    };
    let other = world.commands.save_social_draft(&empty_id).unwrap();
    assert!(!other.id.is_empty() && other.id != saved.id);
    assert!(world.commands.remove_social_draft(&other.id).unwrap());
    assert!(!world.commands.remove_social_draft(&other.id).unwrap());

    let World {
        _dir: dir,
        store,
        commands,
    } = world;
    drop((store, commands));
    let commands = Commands::new(Store::open(dir.path()).unwrap());
    assert_eq!(
        commands.social_draft(&saved.id).unwrap(),
        Some(saved.clone())
    );
    assert_eq!(commands.social_drafts().unwrap(), [saved]);
}

#[test]
fn a_draft_names_a_place_and_message_it_knows() {
    let world = world();
    assert!(
        refused(world.commands.start_social_draft(
            "nobody@x.example",
            None,
            None,
            ContentKind::Chat
        ))
        .contains("no social account")
    );
    assert!(
        refused(
            world
                .commands
                .start_social_draft(ADA, Some("nowhere"), None, ContentKind::Chat)
        )
        .contains("knows no place")
    );
    assert!(
        refused(
            world
                .commands
                .start_social_draft(ADA, None, Some("m1"), ContentKind::Chat)
        )
        .contains("needs the place")
    );
    assert!(
        refused(world.commands.start_social_draft(
            ADA,
            Some(GENERAL),
            Some("m1"),
            ContentKind::Chat
        ))
        .contains("no message m1")
    );
    let queued = world
        .commands
        .send_social(&world.draft(GENERAL, None, ContentKind::Chat, "hi"))
        .unwrap();
    assert!(
        refused(world.commands.start_social_draft(
            ADA,
            Some(GENERAL),
            Some(&queued.message.id),
            ContentKind::Chat
        ))
        .contains("still being sent")
    );
    // A draft with no place yet is kept, but not sent.
    let unplaced = world
        .commands
        .start_social_draft(ADA, None, None, ContentKind::Chat)
        .unwrap();
    assert!(refused(world.commands.send_social(&unplaced)).contains("where it goes"));
}

#[test]
fn sending_queues_a_post_and_drops_the_draft() {
    let world = world();
    let mut draft = world.draft(GENERAL, None, ContentKind::Article, "  The body  \n");
    draft.title = Some(" A title ".into());
    draft.tags = vec!["news".into(), " News ".into(), "".into(), "rust".into()];
    world.commands.save_social_draft(&draft).unwrap();
    let item = world.commands.send_social(&draft).unwrap();
    assert_eq!(world.commands.social_draft(&draft.id).unwrap(), None);
    assert_eq!(item.message.kind, "article");
    assert_eq!(item.message.message["title"], "A title");
    assert_eq!(item.message.message["tags"], json!(["news", "rust"]));
    assert_eq!(item.message.message["content"]["text"], "  The body");

    // The draft's id is its client id: sending it again queues nothing.
    assert_eq!(item.message.id, draft.id);
    let again = world.commands.send_social(&draft).unwrap();
    assert_eq!(again.message.id, draft.id);
    let outbox = world.commands.social_outbox(ADA).unwrap();
    assert_eq!(outbox.len(), 1);
    let SocialChange::Post(post) = &outbox[0].change else {
        panic!("{:?}", outbox[0].change);
    };
    assert_eq!(post.kind, ContentKind::Article);
    assert_eq!(post.content.title.as_deref(), Some("A title"));
    assert_eq!(outbox[0].place.as_deref(), Some(GENERAL));
    assert!(
        world
            .commands
            .give_up_social_change(ADA, &outbox[0].client_id)
            .unwrap()
    );
    assert!(world.commands.social_outbox(ADA).unwrap().is_empty());
}

#[test]
fn what_a_post_carries_is_checked() {
    let world = world();
    let send = |kind, body: &str, f: &dyn Fn(&mut SocialDraft)| {
        let mut draft = world.draft(GENERAL, None, kind, body);
        f(&mut draft);
        world.commands.send_social(&draft)
    };
    assert!(refused(send(ContentKind::Chat, "  ", &|_| {})).contains("nothing to send"));
    assert!(
        refused(send(ContentKind::Memo, "x", &|d| d.title = Some("T".into())))
            .contains("Only an article")
    );
    let missing = DraftMedia::File {
        path: "/no/such/file.png".into(),
        mime: None,
    };
    let attach = missing.clone();
    assert!(
        refused(send(ContentKind::Chat, "x", &move |d| d.attachments =
            vec![attach.clone()]))
        .contains("no file")
    );
    let gone = DraftMedia::Kept {
        blob: "0".repeat(64),
        name: "paste.png".into(),
        mime: None,
    };
    assert!(
        refused(send(ContentKind::Chat, "x", &move |d| d.attachments =
            vec![gone.clone()]))
        .contains("no longer kept")
    );
    let doc = world
        .store
        .put_social_media_bytes(b"not a picture")
        .unwrap();
    let cover = DraftMedia::Kept {
        blob: doc.clone(),
        name: "notes.txt".into(),
        mime: None,
    };
    assert!(
        refused(send(ContentKind::Article, "x", &move |d| d.cover =
            Some(cover.clone())))
        .contains("must be a picture")
    );

    // The home provider's upload limit (§5.8).
    world
        .store
        .put_social_provider(&SocialProvider {
            domain: "groups.example".into(),
            discovery: json!({}),
            capabilities: json!({ "limits": { "maxUploadBytes": 4 } }),
            clock_offset_ms: 0,
        })
        .unwrap();
    let big = DraftMedia::Kept {
        blob: doc,
        name: "notes.txt".into(),
        mime: None,
    };
    assert!(
        refused(send(ContentKind::Chat, "x", &move |d| d.attachments =
            vec![big.clone()]))
        .contains("more than groups.example takes")
    );
    let small = world.store.put_social_media_bytes(b"ok").unwrap();
    let fits = DraftMedia::Kept {
        blob: small,
        name: "ok.txt".into(),
        mime: None,
    };
    let item = send(ContentKind::Chat, "", &move |d| {
        d.attachments = vec![fits.clone()]
    })
    .unwrap();
    assert!(!item.posted);
}

#[test]
fn the_group_and_channel_say_who_may_post_what() {
    let world = world();
    let chat = |place| {
        world
            .commands
            .send_social(&world.draft(place, None, ContentKind::Chat, "hi"))
    };
    // A conversation takes chat only.
    assert!(chat(DM).is_ok());
    let memo = world.draft(DM, None, ContentKind::Memo, "hi");
    assert!(refused(world.commands.send_social(&memo)).contains("chat messages only"));

    // The group's post roles, then the channel's own for a kind.
    world.group_permissions(GROUP, Some("guest"), json!({}));
    assert!(refused(chat(GENERAL)).contains("can't post chat messages"));
    world.channel_permissions(GENERAL, json!({ "post:message": ["guest"] }));
    assert!(chat(GENERAL).is_ok());
    let info = world.commands.social_place(ADA, GENERAL).unwrap();
    assert_eq!(info.may_post, [ContentKind::Chat]);

    // Reply only, and only to some types (§5.2.1).
    world.bob(GENERAL, "chat", 1, "a chat line");
    let mut memo = message(ADA, GENERAL, "memo", 2, BOB, "a memo");
    memo.kind = "memo".into();
    world.put(&memo);
    world.channel_permissions(
        GENERAL,
        json!({ "replyOnly": ["guest"], "replyOnlyTo": ["memo"], "post:message": ["guest"] }),
    );
    assert!(refused(chat(GENERAL)).contains("only reply"));
    let to_chat = world.draft(GENERAL, Some("chat"), ContentKind::Chat, "re");
    assert!(refused(world.commands.send_social(&to_chat)).contains("reply here to memos"));
    let to_memo = world.draft(GENERAL, Some("memo"), ContentKind::Chat, "re");
    assert!(world.commands.send_social(&to_memo).is_ok());
    assert!(
        world
            .commands
            .social_place(ADA, GENERAL)
            .unwrap()
            .may_post
            .is_empty()
    );

    // The owner may do everything.
    world.group_permissions(GROUP, None, json!({ "post": [] }));
    assert!(refused(chat(GENERAL)).contains("can't post"));
    let mut group = world.store.social_group(ADA, GROUP).unwrap().unwrap();
    group.group["owner"] = ADA.to_uppercase().into();
    world.store.put_social_group(&group).unwrap();
    assert!(chat(GENERAL).is_ok());
}

#[test]
fn edits_are_the_authors_while_their_time_lasts() {
    let world = world();
    world.bob(GENERAL, "bobs", 1, "Bob's");
    assert!(
        refused(
            world
                .commands
                .edit_social_message(ADA, GENERAL, "bobs", &content("x"))
        )
        .contains("only what you wrote")
    );
    let mut mine = message(ADA, GENERAL, "mine", 2, ADA, "first");
    world.put(&mine);
    let edited = world
        .commands
        .edit_social_message(ADA, GENERAL, "mine", &content("second"))
        .unwrap();
    assert_eq!(edited.message.message["content"]["text"], "second");
    assert!(edited.pending.as_ref().unwrap().content.is_some());
    let page = world
        .commands
        .social_place_page(ADA, GENERAL, None, 10)
        .unwrap();
    assert_eq!(page.items[0].message.message["content"]["text"], "second");

    mine.message["permissions"] = json!({ "editUntil": "2001-01-01T00:00:00Z" });
    world.put(&mine);
    assert!(
        refused(
            world
                .commands
                .edit_social_message(ADA, GENERAL, "mine", &content("third"))
        )
        .contains("time for edits is over")
    );
    let queued = world
        .commands
        .send_social(&world.draft(GENERAL, None, ContentKind::Chat, "q"))
        .unwrap();
    assert!(
        refused(world.commands.edit_social_message(
            ADA,
            GENERAL,
            &queued.message.id,
            &content("x")
        ))
        .contains("still being sent")
    );
}

#[test]
fn deleting_takes_back_a_post_not_yet_tried() {
    let world = world();
    let queued = world
        .commands
        .send_social(&world.draft(GENERAL, None, ContentKind::Chat, "oops"))
        .unwrap();
    let id = queued.message.id.clone();
    assert_eq!(
        world
            .commands
            .delete_social_message(ADA, GENERAL, &id)
            .unwrap(),
        None
    );
    assert!(world.commands.social_outbox(ADA).unwrap().is_empty());

    let tried = world
        .commands
        .send_social(&world.draft(GENERAL, None, ContentKind::Chat, "oops"))
        .unwrap();
    world
        .store
        .social_change_tried(ADA, &tried.message.id)
        .unwrap();
    assert!(
        refused(
            world
                .commands
                .delete_social_message(ADA, GENERAL, &tried.message.id)
        )
        .contains("may have reached")
    );
}

#[test]
fn deleting_a_kept_message_is_the_authors_or_a_moderators() {
    let world = world();
    world.put(&message(ADA, GENERAL, "mine", 1, ADA, "mine"));
    world.bob(GENERAL, "bobs", 2, "Bob's");
    let deleted = world
        .commands
        .delete_social_message(ADA, GENERAL, "mine")
        .unwrap()
        .unwrap();
    assert!(deleted.message.deleted);
    assert!(
        refused(world.commands.delete_social_message(ADA, GENERAL, "mine")).contains("is deleted")
    );
    assert!(
        refused(world.commands.delete_social_message(ADA, GENERAL, "bobs"))
            .contains("only what you wrote")
    );
    world.group_permissions(GROUP, Some("member"), json!({ "moderate": ["member"] }));
    assert!(
        world
            .commands
            .delete_social_message(ADA, GENERAL, "bobs")
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_reaction_is_given_or_taken_back_once() {
    let world = world();
    let mut liked = world.bob(GENERAL, "m1", 1, "hi");
    let shown = world
        .commands
        .react_social(ADA, GENERAL, "m1", "👍", true)
        .unwrap();
    assert_eq!(
        shown.message.reactions,
        [ReactionSum {
            key: "👍".into(),
            count: 1,
            mine: true,
        }]
    );
    // Asking again changes nothing.
    world
        .commands
        .react_social(ADA, GENERAL, "m1", " 👍 ", true)
        .unwrap();
    assert_eq!(world.commands.social_outbox(ADA).unwrap().len(), 1);

    liked.reactions = vec![ReactionSum {
        key: "🎉".into(),
        count: 2,
        mine: true,
    }];
    world.put(&liked);
    let shown = world
        .commands
        .react_social(ADA, GENERAL, "m1", "🎉", false)
        .unwrap();
    assert!(
        shown
            .message
            .reactions
            .iter()
            .any(|r| r.key == "🎉" && r.count == 1 && !r.mine)
    );
    assert!(refused(world.commands.react_social(ADA, GENERAL, "m1", " ", true)).contains("emoji"));

    world.channel_permissions(GENERAL, json!({ "react": ["admin"] }));
    assert!(
        refused(world.commands.react_social(ADA, GENERAL, "m1", "😀", true))
            .contains("can't react")
    );
    assert!(!world.commands.social_place(ADA, GENERAL).unwrap().may_react);
}
