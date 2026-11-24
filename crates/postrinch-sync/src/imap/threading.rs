//! Threading an IMAP account's mail, which Gmail does for itself and IMAP
//! leaves to the client (docs/imap.md, "Threading").
//!
//! A thread is named by its root message. A new message joins the thread
//! of its nearest ancestor already stored; failing that, its thread is
//! named by the oldest ancestor its headers name, or by itself. Since the
//! root's own id names the same thread, a reply that arrives before what it
//! answers still lands in the right thread once the root arrives. There is
//! no threading by subject: it joins unrelated "Invoice" mails.

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId};

use super::ids::{bare, message_id, short_hash};

/// The thread of a message in `account`, from its headers: its
/// `References` (oldest first, as [`postrinch_model::MessageHeaders`] keeps
/// them), its `In-Reply-To`, and `own`, its own `Message-ID` (or, for a
/// message without one, its provider id). `stored_thread_of` gives the
/// thread of a message already stored, by its provider id.
///
/// The nearest stored ancestor's thread wins: the `References` entries
/// from the last back, then `In-Reply-To`. With none stored, the thread is
/// `t` and a hash of the root: the first `References` entry, else
/// `In-Reply-To`, else `own`.
pub fn thread_for(
    account: &EmailAddress,
    references: &[String],
    in_reply_to: Option<&str>,
    own: &str,
    stored_thread_of: impl Fn(&ProviderMessageId) -> Option<ProviderThreadId>,
) -> ProviderThreadId {
    let ancestors = || {
        references
            .iter()
            .map(String::as_str)
            .rev()
            .chain(in_reply_to)
            .filter(|id| !bare(id).is_empty())
    };
    if let Some(thread) = ancestors().find_map(|id| stored_thread_of(&message_id(account, id))) {
        return thread;
    }
    let root = references
        .iter()
        .map(String::as_str)
        .chain(in_reply_to)
        .find(|id| !bare(id).is_empty())
        .unwrap_or(own);
    root_thread(root)
}

/// The thread a root message names: `t` and a hash of its `Message-ID`.
fn root_thread(root: &str) -> ProviderThreadId {
    ProviderThreadId(format!("t{}", short_hash(&[bare(root).as_bytes()])))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    /// A mailbox as threading sees it: the stored messages' threads.
    struct Mailbox {
        account: EmailAddress,
        threads: RefCell<HashMap<ProviderMessageId, ProviderThreadId>>,
    }

    impl Mailbox {
        fn new() -> Self {
            Self {
                account: EmailAddress::parse("ada@example.org").unwrap(),
                threads: RefCell::default(),
            }
        }

        /// Thread a message as it arrives and store it.
        fn arrive(&self, own: &str, references: &[&str], in_reply_to: Option<&str>) -> String {
            let references: Vec<String> = references.iter().map(|r| r.to_string()).collect();
            let thread = thread_for(&self.account, &references, in_reply_to, own, |id| {
                self.threads.borrow().get(id).cloned()
            });
            self.threads
                .borrow_mut()
                .insert(message_id(&self.account, own), thread.clone());
            thread.0
        }
    }

    #[test]
    fn a_message_alone_is_its_own_thread() {
        let mailbox = Mailbox::new();
        let thread = mailbox.arrive("a@x", &[], None);
        assert!(thread.starts_with('t'));
        assert_eq!(thread.len(), 25);
        assert_ne!(mailbox.arrive("b@x", &[], None), thread);
    }

    #[test]
    fn replies_in_order_join_the_root() {
        let mailbox = Mailbox::new();
        let root = mailbox.arrive("a@x", &[], None);
        assert_eq!(mailbox.arrive("b@x", &["a@x"], Some("a@x")), root);
        assert_eq!(mailbox.arrive("c@x", &["a@x", "b@x"], Some("b@x")), root);
        // A client that writes only In-Reply-To.
        assert_eq!(mailbox.arrive("d@x", &[], Some("<c@x>")), root);
    }

    #[test]
    fn a_reply_before_its_root_lands_in_the_roots_thread() {
        let mailbox = Mailbox::new();
        let reply = mailbox.arrive("c@x", &["a@x", "b@x"], Some("b@x"));
        let middle = mailbox.arrive("b@x", &["a@x"], Some("a@x"));
        let root = mailbox.arrive("a@x", &[], None);
        assert_eq!(reply, root);
        assert_eq!(middle, root);
    }

    #[test]
    fn the_nearest_stored_ancestor_wins() {
        let mailbox = Mailbox::new();
        // b was threaded on its own (its client wrote no References), and
        // a reply naming a, then b, joins b's thread.
        let b = mailbox.arrive("b@x", &[], None);
        assert_eq!(mailbox.arrive("c@x", &["a@x", "b@x"], None), b);
        // In-Reply-To counts when References names nothing stored.
        assert_eq!(mailbox.arrive("d@x", &["zz@x"], Some("b@x")), b);
    }

    #[test]
    fn the_root_is_the_first_reference_then_in_reply_to() {
        let mailbox = Mailbox::new();
        let by_references = mailbox.arrive("c@x", &["a@x", "b@x"], Some("b@x"));
        assert_eq!(by_references, root_thread("a@x").0);
        let by_reply = mailbox.arrive("e@x", &[], Some("<d@x>"));
        assert_eq!(by_reply, root_thread("d@x").0);
    }

    #[test]
    fn ancestors_are_looked_up_in_the_messages_account() {
        let ada = EmailAddress::parse("ada@example.org").unwrap();
        let bob = EmailAddress::parse("bob@example.org").unwrap();
        // Ada's copy of a@x is stored, in a thread of its own name.
        let stored = |id: &ProviderMessageId| {
            (*id == message_id(&ada, "a@x")).then(|| ProviderThreadId("tada".into()))
        };
        let references = ["a@x".to_owned()];
        assert_eq!(thread_for(&ada, &references, None, "b@x", stored).0, "tada");
        // Bob's reply to a@x does not find Ada's copy: a thread is an
        // account's, so his is rooted at a@x in his own account.
        assert_eq!(
            thread_for(&bob, &references, None, "b@x", stored),
            root_thread("a@x")
        );
    }
}
