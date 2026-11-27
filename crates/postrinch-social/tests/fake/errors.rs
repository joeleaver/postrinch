//! What goes wrong on the way: a busy provider, a clock that is off, a
//! key revoked from another device.

use postrinch_ofscp::ofscp_core::ObjectUri;
use postrinch_ofscp::ofscp_core::wire::GroupPointerRequest;
use postrinch_ofscp_fake::FakeProvider;
use postrinch_social::SocialError;
use postrinch_store::SocialAccountState;

use crate::common::{PASSWORD, channel_uri, group_uri, home, requests_to};

#[tokio::test]
async fn a_503_during_sync_is_retried() {
    let h = home().await;
    h.add().await;
    h.provider.faults().fail_next(2, None);
    h.social
        .sync(&h.account())
        .await
        .expect("the retries answer");
    let statuses: Vec<u16> = requests_to(&h.provider, "/api/me")
        .iter()
        .map(|r| r.status)
        .collect();
    assert_eq!(statuses[statuses.len() - 3..], [503, 503, 200]);
}

#[tokio::test]
async fn a_provider_too_busy_fails_the_sync_but_not_the_account() {
    let h = home().await;
    h.add().await;
    h.provider.faults().fail_next(10, None);
    let error = h.social.sync(&h.account()).await.unwrap_err();
    assert!(matches!(error, SocialError::Transport(_)), "{error}");
    let account = h
        .social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::Active);
}

#[tokio::test]
async fn a_provider_clock_off_by_minutes_is_learned_and_kept() {
    let h = home().await;
    h.provider.faults().set_clock_offset(400);
    let added = h.add().await;
    assert_eq!(added.account.state, SocialAccountState::Active);
    let provider = h
        .social
        .store()
        .social_provider(h.provider.authority().as_str())
        .unwrap()
        .unwrap();
    let off = provider.clock_offset_ms;
    assert!((395_000..=405_000).contains(&off), "{off}");
}

#[tokio::test]
async fn a_revoked_key_puts_the_account_in_need_of_sign_in() {
    let h = home().await;
    let key_id = h.add().await.account.key_id.unwrap();
    h.provider.faults().revoke_key(&key_id);
    let error = h.social.sync(&h.account()).await.unwrap_err();
    assert!(
        matches!(
            error,
            SocialError::Unusable {
                state: SocialAccountState::NeedsSignIn,
                ..
            }
        ),
        "{error}"
    );
    let account = h
        .social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::NeedsSignIn);

    // Refused from then on without asking the provider.
    let before = h.provider.requests().len();
    assert!(h.social.sync(&h.account()).await.is_err());
    assert_eq!(h.provider.requests().len(), before);
}

#[tokio::test]
async fn an_account_whose_clock_was_off_syncs_again_and_is_active() {
    let h = home().await;
    h.add().await;
    h.social
        .store()
        .change_social_account(&h.account(), |a| a.state = SocialAccountState::ClockSkewed)
        .unwrap();
    h.social.sync(&h.account()).await.expect("tried again");
    let account = h
        .social
        .store()
        .social_account(&h.account())
        .unwrap()
        .unwrap();
    assert_eq!(account.state, SocialAccountState::Active);
}

#[tokio::test]
async fn another_provider_refusing_the_key_leaves_the_account_and_its_group_alone() {
    let h = home().await;
    let away = FakeProvider::start().await.unwrap();
    let dan = away.add_user("dan", PASSWORD);
    let group = away.create_group(&dan, "Away");
    let channel = away.create_channel(&group, "general");
    away.add_member(&group, &h.ada, "member");
    away.post_as(&dan, &group, &channel, "first");
    let uri = ObjectUri::canonicalize(&group_uri(&away, &group)).unwrap();
    h.phone
        .put_group_pointer(&uri, &GroupPointerRequest { role: None })
        .await
        .unwrap();
    // The other provider cannot check Ada's signatures (it cannot reach
    // her home for her keys, say); home can.
    away.faults().refuse_signatures(true);

    let added = h.add().await;
    assert_eq!(added.account.state, SocialAccountState::Active);
    assert_eq!(added.synced.skipped, 1, "the group is left out");
    let store = h.social.store();
    assert!(store.social_groups(&h.account()).unwrap().is_empty());
    // The pointer stays at home: nothing said the group is gone.
    assert_eq!(h.phone.my_groups().await.unwrap().groups.len(), 1);
    let place = channel_uri(&away, &group, &channel);
    assert!(store.social_place(&h.account(), &place).unwrap().is_none());

    // Once it can, the group comes in; when it cannot again, what is kept
    // of it stays as it was.
    away.faults().refuse_signatures(false);
    let synced = h.social.sync(&h.account()).await.unwrap();
    assert_eq!((synced.groups, synced.places), (1, 1));
    away.faults().refuse_signatures(true);
    h.social.sync(&h.account()).await.expect("still syncs");
    assert_eq!(store.social_groups(&h.account()).unwrap().len(), 1);
    assert_eq!(h.messages(&place).len(), 1);
    let account = store.social_account(&h.account()).unwrap().unwrap();
    assert_eq!(account.state, SocialAccountState::Active);
}

#[tokio::test]
async fn a_refused_channel_list_keeps_the_channels_and_where_they_were_read() {
    let h = home().await;
    let away = FakeProvider::start().await.unwrap();
    let dan = away.add_user("dan", PASSWORD);
    let group = away.create_group(&dan, "Away");
    let channel = away.create_channel(&group, "general");
    away.add_member(&group, &h.ada, "member");
    away.post_as(&dan, &group, &channel, "first");
    let uri = ObjectUri::canonicalize(&group_uri(&away, &group)).unwrap();
    h.phone
        .put_group_pointer(&uri, &GroupPointerRequest { role: None })
        .await
        .unwrap();
    h.add().await;
    let store = h.social.store();
    let place = channel_uri(&away, &group, &channel);
    let kept = store.social_place(&h.account(), &place).unwrap().unwrap();
    assert!(
        kept.read_marker.is_some(),
        "a remote place reads by its own marker"
    );

    // The group answers; its channel list is refused.
    away.faults().refuse_signatures_ending("/channels");
    let synced = h.social.sync(&h.account()).await.expect("still syncs");
    assert_eq!(synced.skipped, 1);
    assert_eq!(store.social_groups(&h.account()).unwrap().len(), 1);
    let still = store
        .social_place(&h.account(), &place)
        .unwrap()
        .expect("the channel is kept");
    assert_eq!(still.read_marker, kept.read_marker);
    assert_eq!(still.cursors, kept.cursors);
    assert_eq!(h.messages(&place).len(), 1);
}
