//! What goes wrong on the way: a busy provider, a clock that is off, a
//! key revoked from another device.

use postrinch_social::SocialError;
use postrinch_store::SocialAccountState;

use crate::common::{home, requests_to};

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
