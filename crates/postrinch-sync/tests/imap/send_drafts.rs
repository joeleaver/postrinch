//! Keeping drafts in an IMAP account's Drafts folder: kept, kept again in
//! place of the old one, and deleted when the draft is sent.

use postrinch_mailtest::{ImapFault, NewMessage};
use postrinch_store::OutgoingKind;
use postrinch_sync::imap::message_id;
use postrinch_sync::outgoing_message_id;

use crate::send_support::{
    SendFixture, header_of, message_id_header, send_fixture, send_fixture_without_drafts,
};

/// Keeps `subject` as a draft, in place of `replaces`.
async fn keep(f: &SendFixture, subject: &str, replaces: Option<&str>) {
    let mut draft = f.draft(subject);
    draft.replaces = replaces.map(|h| message_id(&f.me, h));
    f.queue(&draft, OutgoingKind::Keep);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1, "{subject}");
}

/// The `Message-ID` of the one draft in Drafts.
fn only_draft(f: &SendFixture) -> String {
    let drafts = f.folder("Drafts");
    assert_eq!(drafts.len(), 1, "{drafts:?}");
    assert!(drafts[0].has_flag("\\Draft") && drafts[0].has_flag("\\Seen"));
    header_of(&drafts[0])
}

#[tokio::test]
async fn a_draft_is_kept_in_drafts() {
    for uidplus in [true, false] {
        let f = send_fixture(uidplus, false).await;
        keep(&f, "Thursday", None).await;
        let header = only_draft(&f);
        let id = message_id(&f.me, &header);
        assert_eq!(f.places(&id), usize::from(uidplus), "uidplus {uidplus}");
        assert!(f.smtp.messages().is_empty(), "a draft is not sent");
        f.sync().await;
        assert_eq!(f.places(&id), 1, "uidplus {uidplus}");
        assert!(f.outgoing().is_empty());
    }
}

#[tokio::test]
async fn keeping_a_draft_again_deletes_the_old_one() {
    for uidplus in [true, false] {
        let f = send_fixture(uidplus, false).await;
        keep(&f, "Thursday", None).await;
        let old = only_draft(&f);
        if !uidplus {
            // Its place is left to sync, which the window has seen it by.
            f.sync().await;
        }
        keep(&f, "Thursday, later", Some(&old)).await;
        let new = only_draft(&f);
        assert_ne!(new, old, "uidplus {uidplus}");
        let old_id = message_id(&f.me, &old);
        assert_eq!(f.places(&old_id), 0, "uidplus {uidplus}");
        f.sync().await;
        assert!(f.store.message(&f.me, &old_id).unwrap().is_none());
        assert_eq!(f.places(&message_id(&f.me, &new)), 1);
    }
}

#[tokio::test]
async fn without_uidplus_another_deleted_message_is_left_for_its_owner() {
    let f = send_fixture(false, false).await;
    let theirs = NewMessage::new(&b"Subject: theirs\r\n\r\nBody\r\n"[..]).flags(&["\\Deleted"]);
    f.user.append("Drafts", theirs).unwrap();
    keep(&f, "Thursday", None).await;
    f.sync().await;
    let old = header_of(f.folder("Drafts").last().unwrap());
    keep(&f, "Thursday, later", Some(&old)).await;
    // EXPUNGE would take theirs too: ours is only marked, which counts
    // as gone.
    assert_eq!(f.imap_commands("EXPUNGE"), 0);
    let drafts = f.folder("Drafts");
    assert_eq!(drafts.len(), 3);
    let ours = drafts
        .iter()
        .find(|m| message_id_header(m).as_ref() == Some(&old));
    assert!(ours.unwrap().has_flag("\\Deleted"));
    let old_id = message_id(&f.me, &old);
    let kept = f.store.imap_message_places(&f.me, &old_id).unwrap();
    assert_eq!(kept.len(), 1);
    assert!(kept[0].flags.iter().any(|flag| flag == "\\Deleted"));
    // Gone from the store, as a message sync sees marked.
    f.sync().await;
    assert!(f.store.message(&f.me, &old_id).unwrap().is_none());
    // Its owner expunges theirs: ours, still known as ours, go with the
    // next draft's expunge.
    f.user.expunge("Drafts", drafts[0].uid).unwrap();
    let later = header_of(f.folder("Drafts").last().unwrap());
    keep(&f, "Thursday, later still", Some(&later)).await;
    assert_eq!(f.imap_commands("EXPUNGE"), 1);
    assert_eq!(f.folder("Drafts").len(), 1);
}

#[tokio::test]
async fn sending_a_draft_deletes_it() {
    for uidplus in [true, false] {
        let f = send_fixture(uidplus, false).await;
        keep(&f, "Thursday", None).await;
        let old = only_draft(&f);
        f.sync().await;
        let mut draft = f.draft("Thursday");
        draft.replaces = Some(message_id(&f.me, &old));
        f.queue(&draft, OutgoingKind::Send);
        assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
        assert!(f.folder("Drafts").is_empty(), "uidplus {uidplus}");
        assert_eq!(f.folder("Sent").len(), 1, "uidplus {uidplus}");
        assert_eq!(f.smtp.messages().len(), 1, "uidplus {uidplus}");
    }
}

#[tokio::test]
async fn a_keep_the_server_refused_does_not_strand_the_draft_before_it() {
    let f = send_fixture(true, false).await;
    keep(&f, "Thursday", None).await;
    let first = only_draft(&f);
    f.sync().await;

    // Kept again, but the server is full: it waits for the person.
    let mut draft = f.draft("Thursday, later");
    draft.replaces = Some(message_id(&f.me, &first));
    f.queue(&draft, OutgoingKind::Keep);
    let full = ImapFault::No("[OVERQUOTA] Mailbox is full".into());
    f.imap.fail_next("APPEND", 1, full);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let refused = f.waiting();
    assert!(refused.refused);
    assert_eq!(only_draft(&f), first);

    // Kept once more, in place of the one the window last kept, which
    // never got there: the one that did is deleted.
    let never_kept = outgoing_message_id(&refused);
    keep(&f, "Thursday, last", Some(&never_kept)).await;
    let last = only_draft(&f);
    assert!(last != first && last != never_kept);
    assert!(f.outgoing().is_empty(), "the refused Keep is out of date");
}

#[tokio::test]
async fn sending_in_place_of_a_refused_keep_deletes_the_draft_before_it() {
    let f = send_fixture(true, false).await;
    keep(&f, "Thursday", None).await;
    let first = only_draft(&f);
    f.sync().await;
    let mut draft = f.draft("Thursday, later");
    draft.replaces = Some(message_id(&f.me, &first));
    f.queue(&draft, OutgoingKind::Keep);
    f.imap.fail_next(
        "APPEND",
        1,
        ImapFault::No("[OVERQUOTA] Mailbox is full".into()),
    );
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let never_kept = outgoing_message_id(&f.waiting());

    let mut send = f.draft("Thursday, sent");
    send.replaces = Some(message_id(&f.me, &never_kept));
    f.queue(&send, OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert!(f.folder("Drafts").is_empty());
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty());
}

#[tokio::test]
async fn sending_in_place_of_a_keep_refused_for_want_of_drafts_retires_it() {
    let f = send_fixture_without_drafts().await;
    f.queue(&f.draft("Thursday"), OutgoingKind::Keep);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 0);
    let refused = f.waiting();
    assert!(refused.refused);
    assert!(
        refused
            .error
            .as_deref()
            .unwrap()
            .contains("no Drafts folder")
    );

    let mut send = f.draft("Thursday, sent");
    send.replaces = Some(message_id(&f.me, &outgoing_message_id(&refused)));
    f.queue(&send, OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.folder("Sent").len(), 1);
    assert!(f.outgoing().is_empty(), "the refused Keep is out of date");
}

#[tokio::test]
async fn a_drafts_folder_deleted_on_the_server_does_not_stop_the_outbox() {
    let f = send_fixture(true, false).await;
    // Deleted there since sync last saw it.
    f.user.delete_folder("Drafts").unwrap();
    f.queue(&f.draft("Thursday"), OutgoingKind::Keep);
    f.queue(&f.draft("Friday"), OutgoingKind::Send);
    assert_eq!(f.sync.send_outgoing(&f.me).await.unwrap(), 1);
    assert_eq!(f.smtp.messages().len(), 1, "the next message went");
    let waiting = f.waiting();
    assert!(waiting.refused);
    let error = waiting.error.unwrap();
    assert!(error.contains("would not keep the draft"), "{error}");
}
