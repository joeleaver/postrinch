//! Two providers: a foreign actor's signature is checked against its home.

use ofscp_core::wire::TimelineItem;
use postrinch_ofscp::http::{MediaUpload, PageRequest, Source};
use postrinch_ofscp::{Endpoint, Error};
use postrinch_ofscp_fake::{Authentication, FakeProvider};

use crate::common::{requests_to, signed_in, transport};

#[tokio::test]
async fn a_foreign_member_signs_with_the_key_from_home() {
    let home = FakeProvider::start().await.unwrap();
    let away = FakeProvider::start().await.unwrap();
    let transport = transport();
    let alice = signed_in(&transport, &home, "alice").await;
    let actor = alice.identity().actor().clone();

    let bob = away.add_user("bob", crate::common::PASSWORD);
    let group = away.create_group(&bob, "Away");
    let channel = away.create_channel(&group, "general");
    away.add_member(&group, &actor, "member");
    let posted = away.post_as(&bob, &group, &channel, "welcome");

    let at_away = transport.client(alice.identity().clone(), Endpoint::new(away.authority()));
    let group_seen = at_away.group(&group).await.expect("the group");
    assert_eq!(group_seen.id, group);
    let page = at_away
        .messages(&group, &channel, &PageRequest::newest())
        .await
        .expect("the history");
    assert!(matches!(
        &page.items[..],
        [TimelineItem::Message(m)] if m.id == posted.id
    ));

    let path = format!("/api/groups/{group}");
    let [request] = &requests_to(&away, &path)[..] else {
        panic!("one GET {path}");
    };
    assert_eq!(
        request.authentication,
        Authentication::Verified {
            actor: actor.clone(),
            key_id: alice.identity().key_id().to_owned(),
        }
    );
    let keys = format!("/.well-known/ofscp/users/{}/keys", actor.handle());
    assert!(
        !requests_to(&home, &keys).is_empty(),
        "away fetched the key"
    );

    // Uploads go to the home provider only.
    let upload = MediaUpload::new(
        Source::Bytes(vec![1, 2, 3].into()),
        "a.bin",
        "application/octet-stream",
    );
    assert!(matches!(
        at_away.upload(&upload, None).await,
        Err(Error::NotHome { .. })
    ));
}
