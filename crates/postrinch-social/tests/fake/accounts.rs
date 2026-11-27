//! Adding an account, signing it in again, and removing it.

use postrinch_ofscp::ofscp_core::ObjectUri;
use postrinch_ofscp::ofscp_core::wire::GroupPointerRequest;
use postrinch_ofscp_fake::{Authentication, FakeProvider};
use postrinch_social::{Revocation, SocialError};
use postrinch_store::SocialAccountState;

use crate::common::{DEVICE, PASSWORD, group_uri, home};

#[tokio::test]
async fn adding_registers_a_key_and_signs_every_request_with_it() {
    let h = home().await;
    let added = h.add().await;
    assert_eq!(added.account.state, SocialAccountState::Active);
    let key_id = added.account.key_id.clone().expect("the key's id is kept");
    let entry = h.key_entry().expect("the key is in the keychain");
    assert!(entry.contains(&key_id), "{entry}");

    let keys = h.device_keys().await;
    let ours = keys
        .iter()
        .find(|k| k.key_id == key_id)
        .expect("registered");
    assert_eq!(ours.device_name, DEVICE);

    // Every request of the first sync is signed with the new key, and the
    // fake checked each signature.
    let signed: Vec<_> = h
        .provider
        .requests()
        .into_iter()
        .filter(|r| r.path.starts_with("/api/me"))
        .collect();
    assert!(!signed.is_empty());
    for request in signed {
        assert_eq!(
            request.authentication,
            Authentication::Verified {
                actor: h.ada.clone(),
                key_id: key_id.clone(),
            },
            "{}",
            request.path
        );
    }
}

#[tokio::test]
async fn a_wrong_password_keeps_nothing() {
    let h = home().await;
    let found = h.social.look_up(&h.account()).await.unwrap();
    let error = h.social.add(&found, "not the password").await.unwrap_err();
    assert!(
        matches!(error, SocialError::WrongPassword { .. }),
        "{error}"
    );
    assert!(h.social.store().social_accounts().unwrap().is_empty());
    assert!(h.key_entry().is_none());
    assert_eq!(h.device_keys().await.len(), 1, "only the phone's");
}

#[tokio::test]
async fn an_account_is_added_once() {
    let h = home().await;
    h.add().await;
    let error = h.social.look_up(&h.account()).await.unwrap_err();
    assert!(matches!(error, SocialError::AlreadyAdded(_)), "{error}");
}

#[tokio::test]
async fn a_failed_first_sync_keeps_nothing_and_revokes_the_key() {
    let h = home().await;
    // A group on a provider that is gone by the time Ada adds her account.
    let away = FakeProvider::start().await.unwrap();
    let group = away.create_group(&h.ada, "Away");
    let uri = ObjectUri::canonicalize(&group_uri(&away, &group)).unwrap();
    h.phone
        .put_group_pointer(&uri, &GroupPointerRequest { role: None })
        .await
        .expect("the pointer");
    drop(away);

    let found = h.social.look_up(&h.account()).await.unwrap();
    let error = h.social.add(&found, PASSWORD).await.unwrap_err();
    assert!(matches!(error, SocialError::Transport(_)), "{error}");
    let store = h.social.store();
    assert!(store.social_accounts().unwrap().is_empty());
    assert!(store.social_places(&h.account()).unwrap().is_empty());
    assert!(store.social_groups(&h.account()).unwrap().is_empty());
    assert!(h.key_entry().is_none());
    assert_eq!(h.device_keys().await.len(), 1, "the new key was revoked");
}

#[tokio::test]
async fn signing_in_again_replaces_the_key_and_revokes_the_old_one() {
    let h = home().await;
    let old = h.add().await.account.key_id.unwrap();
    h.provider.faults().revoke_key(&old);
    let error = h.social.sync(&h.account()).await.unwrap_err();
    assert!(matches!(error, SocialError::Unusable { .. }), "{error}");

    let account = h
        .social
        .sign_in_again(&h.account(), PASSWORD)
        .await
        .expect("signed in again");
    assert_eq!(account.state, SocialAccountState::Active);
    let new = account.key_id.unwrap();
    assert_ne!(new, old);
    assert!(h.key_entry().unwrap().contains(&new));
    let keys = h.device_keys().await;
    assert!(keys.iter().any(|k| k.key_id == new));
    assert!(!keys.iter().any(|k| k.key_id == old));
    h.social.sync(&h.account()).await.expect("syncs again");
}

#[tokio::test]
async fn signing_in_again_with_a_working_key_revokes_it() {
    let h = home().await;
    let old = h.add().await.account.key_id.unwrap();
    let account = h
        .social
        .sign_in_again(&h.account(), PASSWORD)
        .await
        .unwrap();
    let keys = h.device_keys().await;
    assert!(!keys.iter().any(|k| k.key_id == old), "the old key revoked");
    assert!(
        keys.iter()
            .any(|k| Some(&k.key_id) == account.key_id.as_ref())
    );
    assert_eq!(keys.len(), 2, "the phone's and the new one");
}

#[tokio::test]
async fn removing_signs_out_and_forgets_everything() {
    let h = home().await;
    let (bob, group, channel) = h.bob_with_group();
    h.provider.post_as(&bob, &group, &channel, "hello");
    let key_id = h.add().await.account.key_id.unwrap();
    let store = h.social.store();
    assert!(!store.social_places(&h.account()).unwrap().is_empty());

    let removed = h.social.remove(&h.account()).await.expect("removed");
    assert_eq!(removed.revocation, Revocation::Revoked);
    assert!(store.social_accounts().unwrap().is_empty());
    assert!(store.social_places(&h.account()).unwrap().is_empty());
    assert!(store.social_groups(&h.account()).unwrap().is_empty());
    let place = crate::common::channel_uri(&h.provider, &group, &channel);
    assert!(h.messages(&place).is_empty());
    assert!(h.key_entry().is_none());
    assert!(!h.device_keys().await.iter().any(|k| k.key_id == key_id));
}

#[tokio::test]
async fn removing_with_a_key_already_revoked_still_forgets_it() {
    let h = home().await;
    let key_id = h.add().await.account.key_id.unwrap();
    h.provider.faults().revoke_key(&key_id);
    let removed = h.social.remove(&h.account()).await.expect("removed");
    assert_eq!(removed.revocation, Revocation::AlreadyGone);
    assert!(h.social.store().social_accounts().unwrap().is_empty());
    assert!(h.key_entry().is_none());
}

#[tokio::test]
async fn an_unknown_account_is_refused() {
    let h = home().await;
    let error = h.social.sync(&h.account()).await.unwrap_err();
    assert!(matches!(error, SocialError::NoAccount(_)), "{error}");
    let error = h.social.sync("not an actor").await.unwrap_err();
    assert!(matches!(error, SocialError::NotAnActor { .. }), "{error}");
}
