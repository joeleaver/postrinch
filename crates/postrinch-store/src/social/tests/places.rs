use super::*;

const RANDOM: &str = "https://groups.example/api/groups/g1/channels/random";
const DM: &str = "dm_abc";

#[test]
fn groups_round_trip_and_list_by_account_and_provider() {
    let (_dir, store) = ada_with_general();
    let other = "https://groups.example/api/groups/g0";
    store.put_social_group(&group(ADA, other)).unwrap();
    store.put_social_group(&group(BOB, GROUP)).unwrap();
    assert_eq!(
        store.social_group(ADA, GROUP).unwrap(),
        Some(group(ADA, GROUP))
    );
    let uris: Vec<String> = store
        .social_groups(ADA)
        .unwrap()
        .into_iter()
        .map(|g| g.uri)
        .collect();
    assert_eq!(uris, [other, GROUP]);
    assert_eq!(store.social_groups_on("Groups.Example").unwrap().len(), 3);
}

#[test]
fn removing_a_group_removes_its_channels_and_their_messages() {
    let (_dir, store) = ada_with_general();
    store.put_social_place(&channel(ADA, RANDOM, None)).unwrap();
    let mut dm = channel(ADA, DM, None);
    dm.kind = PlaceKind::Conversation;
    dm.group = None;
    store.put_social_place(&dm).unwrap();
    // Bob is in the same group: his channels stay.
    store
        .put_social_place(&channel(BOB, GENERAL, None))
        .unwrap();
    store
        .put_social_message(&message(ADA, GENERAL, "m1", 1), "hi")
        .unwrap();
    let channels = store.social_places_of_group(ADA, GROUP).unwrap();
    assert_eq!(channels.len(), 2);

    store.remove_social_group(ADA, GROUP).unwrap();
    assert_eq!(store.social_group(ADA, GROUP).unwrap(), None);
    let ids: Vec<String> = store
        .social_places(ADA)
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, [DM]);
    assert_eq!(store.social_message(ADA, GENERAL, "m1").unwrap(), None);
    assert!(store.social_place(BOB, GENERAL).unwrap().is_some());
}

#[test]
fn places_change_as_they_are_and_followed_ones_list_by_activity() {
    let (_dir, store) = ada_with_general();
    store
        .put_social_place(&channel(ADA, RANDOM, Some(20)))
        .unwrap();
    let mut quiet = channel(ADA, DM, Some(30));
    quiet.followed = false;
    store.put_social_place(&quiet).unwrap();
    let changed = store
        .change_social_place(ADA, GENERAL, |p| {
            p.last_activity = Some(10);
            p.cursors.live = Some("c9".into());
            p.read_marker = Some("m9".into());
        })
        .unwrap()
        .unwrap();
    assert_eq!(store.social_place(ADA, GENERAL).unwrap(), Some(changed));
    let followed: Vec<String> = store
        .social_followed_places()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(followed, [RANDOM, GENERAL]);
    assert_eq!(
        store.change_social_place(ADA, "nowhere", |_| {}).unwrap(),
        None
    );
}

#[test]
fn keeping_a_place_merges_with_what_is_here_or_keeps_it_new() {
    let (_dir, store) = ada_with_general();
    store
        .change_social_place(ADA, GENERAL, |p| p.read_marker = Some("m9".into()))
        .unwrap();
    let mut fresh = channel(ADA, GENERAL, Some(20));
    fresh.object = json!({ "id": GENERAL, "name": "renamed" });
    let kept = store
        .keep_social_place(fresh, |kept, fresh| SocialPlace {
            read_marker: kept.read_marker,
            ..fresh
        })
        .unwrap();
    assert_eq!(kept.read_marker.as_deref(), Some("m9"));
    assert_eq!(kept.object["name"], "renamed");
    assert_eq!(store.social_place(ADA, GENERAL).unwrap(), Some(kept));

    // A place not here yet is kept as it came, without asking `keep`.
    let new = channel(ADA, RANDOM, None);
    let kept = store
        .keep_social_place(new.clone(), |_, _| unreachable!("nothing to merge"))
        .unwrap();
    assert_eq!(kept, new);
    assert_eq!(store.social_place(ADA, RANDOM).unwrap(), Some(new));
}

#[test]
fn removing_a_place_keeps_its_drafts_without_it() {
    let (_dir, store) = ada_with_general();
    store
        .put_social_message(&message(ADA, GENERAL, "m1", 1), "hi")
        .unwrap();
    store
        .queue_social_change(ADA, "c1", Some(GENERAL), &read_change("m1"), 1)
        .unwrap();
    store
        .queue_social_change(ADA, "c2", None, &read_change("m2"), 2)
        .unwrap();
    store
        .put_social_draft(&accounts::draft(ADA, "d1", Some(GENERAL)))
        .unwrap();

    store.remove_social_place(ADA, GENERAL).unwrap();
    assert_eq!(store.social_place(ADA, GENERAL).unwrap(), None);
    assert_eq!(store.social_message(ADA, GENERAL, "m1").unwrap(), None);
    let left: Vec<String> = store
        .social_outbox(ADA)
        .unwrap()
        .into_iter()
        .map(|e| e.client_id)
        .collect();
    assert_eq!(left, ["c2"]);
    let draft = store.social_draft("d1").unwrap().unwrap();
    assert_eq!((draft.place, draft.reply_to), (None, None));
    assert!(store.social_drafts_for(ADA, GENERAL).unwrap().is_empty());
}

#[test]
fn views_keep_the_persons_order() {
    let (_dir, store) = store();
    let view = |id: &str, order| SocialView {
        id: id.into(),
        order,
        name: id.to_uppercase(),
        places: vec![PlaceRef {
            account: ADA.into(),
            id: GENERAL.into(),
        }],
        groups: vec![],
        filters: SocialFilters {
            kinds: vec![ContentKind::Article, ContentKind::Memo],
            search: "from:bob tag:rust".into(),
        },
        read_as: ReadAs::List,
        count: SocialCount::Mentions,
    };
    for (id, order) in [("b", 2), ("a", 2), ("c", 1)] {
        store.put_social_view(&view(id, order)).unwrap();
    }
    let ids = |store: &Store| -> Vec<String> {
        store
            .social_views()
            .unwrap()
            .into_iter()
            .map(|v| v.id)
            .collect()
    };
    assert_eq!(ids(&store), ["c", "a", "b"]);
    store.put_social_view(&view("c", 3)).unwrap();
    assert_eq!(ids(&store), ["a", "b", "c"]);
    assert_eq!(store.social_view("a").unwrap(), Some(view("a", 2)));
    assert!(store.remove_social_view("a").unwrap());
    assert!(!store.remove_social_view("a").unwrap());
    assert_eq!(ids(&store), ["b", "c"]);
}
