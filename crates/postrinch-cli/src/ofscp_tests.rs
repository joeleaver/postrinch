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
    let social = Social::new(store, transport, Arc::new(MemorySecrets::default()));
    Fixture {
        provider,
        social,
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
