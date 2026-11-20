//! The person's labels made, renamed and deleted in Gmail at once, and
//! kept in the store, against a mock of the Gmail API.

mod common;

use common::*;
use serde_json::json;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, ResponseTemplate};

#[tokio::test]
async fn labels_are_made_renamed_and_deleted_in_gmail_and_here() {
    let f = fixture().await;
    Mock::given(method("POST"))
        .and(path("/gmail/labels"))
        .and(body_partial_json(
            json!({"name": "Personal", "labelListVisibility": "labelShow"}),
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "Label_7", "name": "Personal", "type": "user"})),
        )
        .expect(1)
        .mount(&f.server)
        .await;
    Mock::given(method("PATCH"))
        .and(path("/gmail/labels/Label_7"))
        .and(body_partial_json(json!({"name": "Home"})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id": "Label_7", "name": "Home", "type": "user"})),
        )
        .expect(1)
        .mount(&f.server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/gmail/labels/Label_7"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&f.server)
        .await;

    let made = f.sync.create_label(&f.me, "Personal").await.unwrap();
    assert_eq!(made.provider_id, "Label_7");
    let names = |store: &postrinch_store::Store| -> Vec<String> {
        store
            .labels(&f.me)
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect()
    };
    assert_eq!(names(&f.store), ["Personal"]);

    f.sync.rename_label(&f.me, "Label_7", "Home").await.unwrap();
    assert_eq!(names(&f.store), ["Home"]);

    f.sync.delete_label(&f.me, "Label_7").await.unwrap();
    assert!(names(&f.store).is_empty());
}
