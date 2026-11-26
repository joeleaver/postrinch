use serde_json::json;

use super::*;

#[test]
fn accounts_are_found_in_any_spelling() {
    let (_dir, store) = store();
    let ada = account("Ada@Groups.Example");
    store.put_social_account(&ada).unwrap();
    assert_eq!(store.social_account(ADA).unwrap(), Some(ada.clone()));
    store.put_social_account(&account(BOB)).unwrap();
    let actors: Vec<String> = store
        .social_accounts()
        .unwrap()
        .into_iter()
        .map(|a| a.actor)
        .collect();
    assert_eq!(actors, ["Ada@Groups.Example", BOB]);
    // Put again in another spelling: still one account.
    store.put_social_account(&account(ADA)).unwrap();
    assert_eq!(store.social_accounts().unwrap().len(), 2);
}

#[test]
fn accounts_are_listed_by_state_and_changed_as_they_are() {
    let (_dir, store) = store();
    store.put_social_account(&account(ADA)).unwrap();
    store.put_social_account(&account(BOB)).unwrap();
    let changed = store
        .change_social_account(BOB, |a| {
            a.state = SocialAccountState::NeedsSignIn;
            a.key_id = None;
        })
        .unwrap()
        .unwrap();
    assert_eq!(changed.state, SocialAccountState::NeedsSignIn);
    let needing = store
        .social_accounts_in(SocialAccountState::NeedsSignIn)
        .unwrap();
    assert_eq!(needing, [changed]);
    let active = store
        .social_accounts_in(SocialAccountState::Active)
        .unwrap();
    assert_eq!(active, [account(ADA)]);
    assert_eq!(
        store
            .change_social_account("nobody@x.example", |_| {})
            .unwrap(),
        None
    );
}

#[test]
fn providers_round_trip_by_domain() {
    let (_dir, store) = store();
    let provider = SocialProvider {
        domain: "Groups.Example".into(),
        discovery: json!({ "version": "1" }),
        capabilities: json!({ "limits": { "maxUploadBytes": 1000 } }),
        clock_offset_ms: -1500,
    };
    store.put_social_provider(&provider).unwrap();
    assert_eq!(
        store.social_provider("groups.example").unwrap(),
        Some(provider)
    );
    assert_eq!(store.social_provider("other.example").unwrap(), None);
}

#[test]
fn removing_an_account_removes_everything_it_holds() {
    let (_dir, store) = ada_with_general();
    store.put_social_account(&account(BOB)).unwrap();
    // Ada's things.
    store
        .put_social_message(&message(ADA, GENERAL, "m1", 1), "hello")
        .unwrap();
    store
        .queue_social_change(ADA, "c1", Some(GENERAL), &json!({}), &[], 1)
        .unwrap();
    store
        .put_social_notification(&notification(ADA, "n1", 1))
        .unwrap();
    store.put_social_media(&media(ADA, "a1")).unwrap();
    store
        .put_social_draft(&draft(ADA, "d1", Some(GENERAL)))
        .unwrap();
    // Bob's, which stay.
    store
        .put_social_place(&channel(BOB, GENERAL, None))
        .unwrap();
    store
        .put_social_message(&message(BOB, GENERAL, "m1", 1), "hello")
        .unwrap();
    store.put_social_draft(&draft(BOB, "d2", None)).unwrap();
    let both = |account: &str| PlaceRef {
        account: account.into(),
        id: GENERAL.into(),
    };
    let view = SocialView {
        id: "v1".into(),
        order: 1,
        name: "Both".into(),
        places: vec![both(ADA), both(BOB)],
        groups: vec![GroupRef {
            account: ADA.into(),
            uri: GROUP.into(),
        }],
        filters: SocialFilters::default(),
        read_as: ReadAs::Stream,
        count: SocialCount::Unread,
    };
    store.put_social_view(&view).unwrap();

    store.remove_social_account("ADA@groups.example").unwrap();

    assert_eq!(store.social_account(ADA).unwrap(), None);
    assert!(store.social_groups(ADA).unwrap().is_empty());
    assert!(store.social_places(ADA).unwrap().is_empty());
    assert_eq!(store.social_message(ADA, GENERAL, "m1").unwrap(), None);
    // Its content went with it: only Bob's is left.
    assert_eq!(store.db.count_type("SocialMessageContent").unwrap(), 1);
    assert!(store.social_outbox(ADA).unwrap().is_empty());
    assert!(store.social_notifications(ADA, 10).unwrap().is_empty());
    assert_eq!(
        store.social_media(ADA, "groups.example", "a1").unwrap(),
        None
    );
    assert_eq!(store.social_draft("d1").unwrap(), None);
    let view = store.social_view("v1").unwrap().unwrap();
    assert_eq!(view.places, [both(BOB)]);
    assert!(view.groups.is_empty());
    // Bob's are untouched.
    assert!(store.social_account(BOB).unwrap().is_some());
    assert!(store.social_message(BOB, GENERAL, "m1").unwrap().is_some());
    assert!(store.social_draft("d2").unwrap().is_some());
    // Again, and of an account never here: nothing to do.
    assert_eq!(store.db.count_type("SocialMessage").unwrap(), 1);
    store.remove_social_account(ADA).unwrap();
    store.remove_social_account("nobody@x.example").unwrap();
}

pub(super) fn notification(account: &str, id: &str, created: i64) -> SocialNotification {
    SocialNotification {
        account: account.into(),
        id: id.into(),
        created,
        read: false,
        notification: json!({ "id": id, "type": "mention" }),
    }
}

pub(super) fn media(account: &str, id: &str) -> SocialMedia {
    SocialMedia {
        account: account.into(),
        provider: "groups.example".into(),
        id: id.into(),
        url: format!("https://cdn.example/{id}"),
        blob: None,
        fetched: None,
        mime: Some("image/png".into()),
        size: Some(10),
        width: Some(2),
        height: Some(1),
        duration_ms: None,
        poster: None,
    }
}

pub(super) fn draft(account: &str, id: &str, place: Option<&str>) -> SocialDraft {
    SocialDraft {
        id: id.into(),
        account: account.into(),
        place: place.map(str::to_owned),
        reply_to: place.map(|_| "m1".to_owned()),
        updated: 10,
        kind: ContentKind::Article,
        title: Some("Title".into()),
        body: "# Hello\n\nWorld".into(),
        tags: vec!["rust".into()],
        cover: None,
        attachments: vec![DraftMedia::File {
            path: "/tmp/picture.png".into(),
            mime: Some("image/png".into()),
        }],
    }
}

#[test]
fn removing_an_account_removes_messages_of_places_never_kept() {
    let (_dir, store) = store();
    store.put_social_account(&account(ADA)).unwrap();
    // A private message that came before its conversation.
    store
        .put_social_message(&message(ADA, "dm_early", "m1", 1), "early")
        .unwrap();
    store
        .put_social_message(&message(BOB, "dm_early", "m1", 1), "early")
        .unwrap();
    store.remove_social_account(ADA).unwrap();
    assert_eq!(store.social_message(ADA, "dm_early", "m1").unwrap(), None);
    assert!(
        store
            .social_message(BOB, "dm_early", "m1")
            .unwrap()
            .is_some()
    );
    assert_eq!(store.db.count_type("SocialMessageContent").unwrap(), 1);
}
