//! The OFSCP commands against `postrinch-ofscp-fake`, with device keys in
//! memory: nothing touches the keychain or a real provider.

use std::sync::Arc;
use std::time::Duration;

use postrinch_ofscp::{Options, RetryPolicy, Tls, Transport};
use postrinch_ofscp_fake::{FakeProvider, root_certificate_der};
use postrinch_store::Store;
use postrinch_sync_core::MemorySecrets;

use super::*;

const PASSWORD: &str = "a-test-password-0123";

/// Passwords given in order, as a person typing them would.
struct Given(Vec<&'static str>);

impl Passwords for Given {
    fn password(&mut self, _: &str) -> Result<String> {
        Ok(self.0.remove(0).to_owned())
    }
}

struct Fixture {
    provider: FakeProvider,
    social: Social,
    transport: Transport,
    actor: String,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let provider = FakeProvider::start().await.unwrap();
    let actor = provider.add_user("ada", PASSWORD);
    let group = provider.create_group(&actor, "Ada's group");
    let channel = provider.create_channel(&group, "general");
    provider.post_as(&actor, &group, &channel, "hello");
    let options = Options {
        retry: RetryPolicy {
            first_backoff: Duration::from_millis(20),
            ..RetryPolicy::default()
        },
        ..Options::default()
    };
    let tls = Tls::with_extra_roots(vec![root_certificate_der().to_vec()]).unwrap();
    let transport = Transport::new(tls, options).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let social = Social::new(store, transport.clone(), Arc::new(MemorySecrets::default()));
    Fixture {
        provider,
        social,
        transport,
        actor: actor.to_string(),
        _dir: dir,
    }
}

#[tokio::test]
async fn add_sync_and_remove_from_the_command_line() {
    let f = fixture().await;
    add(&f.social, &f.actor, &mut Given(vec![PASSWORD]))
        .await
        .expect("add-ofscp");
    let accounts = f.social.store().social_accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].state, SocialAccountState::Active);

    run(&f.social, OfscpCommand::Accounts, &mut Given(vec![]))
        .await
        .expect("ofscp accounts");
    run(
        &f.social,
        OfscpCommand::Sync { actor: None },
        &mut Given(vec![]),
    )
    .await
    .expect("ofscp sync");
    let sign_in = OfscpCommand::SignIn {
        actor: f.actor.clone(),
        password_stdin: false,
    };
    run(&f.social, sign_in, &mut Given(vec![PASSWORD]))
        .await
        .expect("ofscp sign-in");

    let remove = OfscpCommand::Remove {
        actor: f.actor.clone(),
    };
    run(&f.social, remove, &mut Given(vec![]))
        .await
        .expect("ofscp remove");
    assert!(f.social.store().social_accounts().unwrap().is_empty());
    assert!(f.provider.requests().iter().any(|r| r.status == 200));
}

#[tokio::test]
async fn a_wrong_password_adds_nothing() {
    let f = fixture().await;
    let error = add(&f.social, &f.actor, &mut Given(vec!["not it"]))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("password"), "{error}");
    assert!(f.social.store().social_accounts().unwrap().is_empty());
}

#[tokio::test]
async fn sync_with_no_accounts_says_how_to_add_one() {
    let f = fixture().await;
    let error = run(
        &f.social,
        OfscpCommand::Sync { actor: None },
        &mut Given(vec![]),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("add-ofscp"), "{error}");
}

#[tokio::test]
async fn send_and_avatar_from_the_command_line() {
    use postrinch_ofscp::Endpoint;
    use postrinch_ofscp::http::{MediaUpload, Source};
    use postrinch_ofscp::ofscp_core::wire::{Patch, ProfileUpdate};
    use postrinch_store::{ContentKind, DraftMedia, SocialChange, SocialContent, SocialPost};

    let f = fixture().await;
    add(&f.social, &f.actor, &mut Given(vec![PASSWORD]))
        .await
        .expect("add-ofscp");
    let place = f
        .social
        .store()
        .social_places(&f.actor)
        .unwrap()
        .into_iter()
        .next()
        .expect("Ada's channel")
        .id;
    let dir = tempfile::tempdir().unwrap();
    let picture = dir.path().join("picture.png");
    std::fs::write(&picture, b"a picture").unwrap();
    let post = SocialChange::Post(SocialPost {
        kind: ContentKind::Chat,
        reply_to: None,
        content: SocialContent {
            body: "a picture".into(),
            attachments: vec![DraftMedia::File {
                path: picture,
                mime: None,
            }],
            ..SocialContent::default()
        },
    });
    f.social
        .store()
        .queue_social_change(&f.actor, "c1", Some(&place), &post, 1)
        .unwrap();
    let send = OfscpCommand::Send {
        actor: f.actor.clone(),
    };
    run(&f.social, send, &mut Given(vec![]))
        .await
        .expect("ofscp send");
    assert!(f.social.store().social_outbox(&f.actor).unwrap().is_empty());

    // Bob sets an avatar; Ada fetches it, then again from the cache.
    f.provider.add_user("bob", PASSWORD);
    let transport = &f.transport;
    let endpoint = Endpoint::new(f.provider.authority());
    let bob = transport
        .provider(endpoint.clone())
        .sign_in("bob", PASSWORD, "bob's")
        .await
        .unwrap();
    let bob = transport.client(Arc::new(bob), endpoint);
    let upload = MediaUpload::new(
        Source::Bytes(b"bob's face".to_vec().into()),
        "bob.png",
        "image/png",
    );
    let face = bob.upload(&upload, None).await.unwrap();
    let update = ProfileUpdate {
        avatar: Patch::Set(face.url),
        ..ProfileUpdate::default()
    };
    bob.update_profile(&update).await.unwrap();
    let out = dir.path().join("bob.png");
    for _ in 0..2 {
        let avatar = OfscpCommand::Avatar {
            actor: f.actor.clone(),
            user: f.provider.actor("bob").to_string(),
            out: out.clone(),
        };
        run(&f.social, avatar, &mut Given(vec![]))
            .await
            .expect("ofscp avatar");
        assert_eq!(std::fs::read(&out).unwrap(), b"bob's face");
    }
    let fetches = f
        .provider
        .requests()
        .iter()
        .filter(|r| r.path.starts_with("/media/"))
        .count();
    assert_eq!(fetches, 1);
}
