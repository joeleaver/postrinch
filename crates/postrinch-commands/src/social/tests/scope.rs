//! An agent limited to some social accounts sees and changes only theirs.

use postrinch_store::{ContentKind, PlaceRef, ReadAs, SocialCount, SocialFilters, SocialView};

use super::*;
use crate::{CommandError, HOME};

#[test]
fn an_agent_sees_only_the_accounts_it_was_given() {
    let world = world();
    world.bob(GENERAL, "g1", 1, "hello");
    world.put(&message(CARL, CARL_CHANNEL, "c1", 2, BOB, "hello"));
    let agent = world.commands.limited(&[], &[CARL]);

    let accounts: Vec<String> = agent
        .social_accounts()
        .unwrap()
        .into_iter()
        .map(|a| a.actor)
        .collect();
    assert_eq!(accounts, [CARL]);
    assert!(matches!(
        agent.social_account(ADA),
        Err(CommandError::UnknownSocialAccount(_))
    ));
    assert!(matches!(
        agent.social_place_page(ADA, GENERAL, None, 5),
        Err(CommandError::UnknownSocialAccount(_))
    ));
    let places: Vec<String> = agent
        .social_places()
        .unwrap()
        .into_iter()
        .map(|p| p.place.id)
        .collect();
    assert_eq!(places, [CARL_CHANNEL]);
    let page = agent.social_view_page(HOME, None, None, 10).unwrap();
    assert!(page.items.is_empty(), "Home reads memos and articles");
    let found = agent.search_social("hello", 10).unwrap();
    assert_eq!(ids(&found), ["c1"]);

    // Ada's drafts and views are hidden from it.
    let draft = world.draft(GENERAL, None, ContentKind::Chat, "x");
    world.commands.save_social_draft(&draft).unwrap();
    assert!(agent.social_drafts().unwrap().is_empty());
    assert_eq!(agent.social_draft(&draft.id).unwrap(), None);
    assert!(!agent.remove_social_draft(&draft.id).unwrap());
    let mut stolen = draft.clone();
    stolen.account = CARL.into();
    stolen.place = Some(CARL_CHANNEL.into());
    assert!(matches!(
        agent.send_social(&stolen),
        Err(CommandError::UnknownSocialAccount(_))
    ));
    assert!(world.commands.social_draft(&draft.id).unwrap().is_some());
}

#[test]
fn an_agent_changes_only_views_of_its_accounts_alone() {
    let world = world();
    let agent = world.commands.limited(&[], &[CARL]);
    let view = |account: &str, id: &str| SocialView {
        id: String::new(),
        order: 0,
        name: "Mine".into(),
        places: vec![PlaceRef {
            account: account.into(),
            id: id.into(),
        }],
        groups: vec![],
        filters: SocialFilters::default(),
        read_as: ReadAs::Stream,
        count: SocialCount::Unread,
    };
    assert!(matches!(
        agent.save_social_view(&view(ADA, GENERAL)),
        Err(CommandError::UnknownSocialAccount(_))
    ));
    let adas = world
        .commands
        .save_social_view(&view(ADA, GENERAL))
        .unwrap();
    let ids: Vec<String> = agent
        .social_views()
        .unwrap()
        .into_iter()
        .map(|e| e.view.id)
        .collect();
    assert!(!ids.contains(&adas.id));
    assert!(agent.remove_social_view(&adas.id).is_err());

    let carls = agent.save_social_view(&view(CARL, CARL_CHANNEL)).unwrap();
    agent
        .order_social_views(std::slice::from_ref(&carls.id))
        .unwrap();
    let mut home = agent.social_view(HOME).unwrap().view;
    home.name = "Renamed".into();
    assert!(refused(agent.save_social_view(&home)).contains("only views of your own"));
    agent.remove_social_view(&carls.id).unwrap();
}

#[test]
fn an_agent_orders_its_views_among_their_places_only() {
    let world = world();
    let view = |account: &str, id: &str, name: &str| SocialView {
        id: String::new(),
        order: 0,
        name: name.into(),
        places: vec![PlaceRef {
            account: account.into(),
            id: id.into(),
        }],
        groups: vec![],
        filters: SocialFilters::default(),
        read_as: ReadAs::Stream,
        count: SocialCount::Unread,
    };
    let c1 = world
        .commands
        .save_social_view(&view(CARL, CARL_CHANNEL, "C1"))
        .unwrap();
    let a = world
        .commands
        .save_social_view(&view(ADA, GENERAL, "A"))
        .unwrap();
    let c2 = world
        .commands
        .save_social_view(&view(CARL, CARL_CHANNEL, "C2"))
        .unwrap();
    let agent = world.commands.limited(&[], &[CARL]);
    agent
        .order_social_views(std::slice::from_ref(&c2.id))
        .unwrap();
    let order: Vec<String> = world
        .commands
        .social_views()
        .unwrap()
        .into_iter()
        .filter(|e| e.built_in.is_none())
        .map(|e| e.view.name)
        .collect();
    assert_eq!(order, ["C2", "A", "C1"]);
    assert_eq!(
        world.commands.social_view(&a.id).unwrap().view.order,
        a.order
    );
    assert_ne!(c1.order, a.order);
}

#[test]
fn an_agent_proposes_what_speaks_for_the_person_and_changes_the_rest() {
    let world = world();
    let agent = world.commands.limited(&[], &[ADA]);
    world.bob(GENERAL, "m1", 1, "hello");
    world.put(&message(ADA, GENERAL, "mine", 2, ADA, "mine"));

    // Drafts are local: an agent writes them.
    let mut draft = agent
        .start_social_draft(ADA, Some(GENERAL), None, ContentKind::Chat)
        .unwrap();
    draft.body = "hi".into();
    let draft = agent.save_social_draft(&draft).unwrap();
    assert!(says(agent.send_social(&draft)));
    assert!(says(agent.edit_social_message(
        ADA,
        GENERAL,
        "mine",
        &postrinch_store::SocialContent {
            body: "x".into(),
            ..Default::default()
        }
    )));
    assert!(says(agent.delete_social_message(ADA, GENERAL, "mine")));
    assert!(says(agent.react_social(ADA, GENERAL, "m1", "👍", true)));
    assert!(says(agent.follow_social_place(ADA, NEWS, true)));
    let name = postrinch_store::ProfileChange {
        display_name: Some("Agent".into()),
        ..Default::default()
    };
    assert!(says(agent.change_social_profile(ADA, &name)));
    assert!(world.commands.social_outbox(ADA).unwrap().is_empty());

    // Read state is the person's, and local: an agent marks it.
    assert_eq!(agent.mark_social_read(ADA, GENERAL, None).unwrap(), 1);
}

/// Whether `result` is the refusal an agent gets for what it may only
/// propose.
fn says<T: std::fmt::Debug>(result: crate::CommandResult<T>) -> bool {
    refused(result).contains("propose it")
}
