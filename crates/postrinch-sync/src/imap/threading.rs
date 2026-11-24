//! Threading an IMAP account's mail, which Gmail does for itself and IMAP
//! leaves to the client (docs/imap.md, "Threading").
//!
//! A thread is named by its root message. A message already stored keeps
//! its thread when seen again (its second copy, in Sent and the inbox, or
//! every copy after a folder's UIDVALIDITY changed). A new message joins
//! the thread of its nearest ancestor already stored; failing that, its
//! thread is named by the oldest ancestor its headers name, or by itself.
//! Since the root's own id names the same thread, a reply that arrives
//! before what it answers still lands in the right thread once the root
//! arrives. Threads never merge: a message is threaded once, so a reply
//! naming only its parent (`In-Reply-To`, no `References`) that arrives
//! before the parent stays in a thread of its own. There is no threading
//! by subject: it joins unrelated "Invoice" mails.

use postrinch_model::{EmailAddress, ProviderMessageId, ProviderThreadId};

use super::ids::{bare, message_id, short_hash};

/// The message being threaded, as its own headers name it.
#[derive(Debug, Clone, Copy)]
pub enum Own<'a> {
    /// Its `Message-ID` header, as written.
    MessageId(&'a str),
    /// A message without a `Message-ID`: the provider id it is stored
    /// under ([`super::message_id_without_header`]).
    Headerless(&'a ProviderMessageId),
}

impl Own<'_> {
    /// The provider id the message is stored under in `account`.
    fn provider_id(self, account: &EmailAddress) -> ProviderMessageId {
        match self {
            Own::MessageId(header) => message_id(account, header),
            Own::Headerless(id) => id.clone(),
        }
    }
}

/// The thread of a message in `account`, from its headers: its
/// `References` (oldest first, as [`postrinch_model::MessageHeaders`] keeps
/// them), its `In-Reply-To`, and `own`, the message itself.
/// `stored_thread_of` gives the thread of a message already stored, by its
/// provider id.
///
/// A message already stored keeps its thread, so seeing it again never
/// moves it. Otherwise the nearest stored ancestor's thread wins: the
/// `References` entries from the last back, then `In-Reply-To`. With none
/// stored, the thread is `t` and a hash of the root: the first
/// `References` entry, else `In-Reply-To`, else the message's own
/// `Message-ID` (or, without one, its provider id).
pub fn thread_for(
    account: &EmailAddress,
    references: &[String],
    in_reply_to: Option<&str>,
    own: Own<'_>,
    stored_thread_of: impl Fn(&ProviderMessageId) -> Option<ProviderThreadId>,
) -> ProviderThreadId {
    if let Some(thread) = stored_thread_of(&own.provider_id(account)) {
        return thread;
    }
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
    let own_root = match own {
        Own::MessageId(header) => header,
        Own::Headerless(id) => id.0.as_str(),
    };
    let root = references
        .iter()
        .map(String::as_str)
        .chain(in_reply_to)
        .find(|id| !bare(id).is_empty())
        .unwrap_or(own_root);
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
            let thread = thread_for(
                &self.account,
                &references,
                in_reply_to,
                Own::MessageId(own),
                |id| self.threads.borrow().get(id).cloned(),
            );
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
        assert_eq!(
            thread_for(&ada, &references, None, Own::MessageId("b@x"), stored).0,
            "tada"
        );
        // Bob's reply to a@x does not find Ada's copy: a thread is an
        // account's, so his is rooted at a@x in his own account.
        assert_eq!(
            thread_for(&bob, &references, None, Own::MessageId("b@x"), stored),
            root_thread("a@x")
        );
    }

    #[test]
    fn a_message_seen_again_keeps_its_thread() {
        let mailbox = Mailbox::new();
        let a = mailbox.arrive("a@x", &[], None);
        // c names a and b; only a is stored, so c joins a's thread.
        assert_eq!(mailbox.arrive("c@x", &["a@x", "b@x"], Some("b@x")), a);
        // b arrives alone, its client having written no References.
        let b = mailbox.arrive("b@x", &[], None);
        assert_ne!(b, a);
        // c seen again (its copy in another folder, or after a new
        // UIDVALIDITY): b is now its nearest stored ancestor, but c stays.
        assert_eq!(mailbox.arrive("c@x", &["a@x", "b@x"], Some("b@x")), a);
    }

    #[test]
    fn threads_never_merge() {
        let mailbox = Mailbox::new();
        // A reply naming only its parent, before the parent arrives.
        let reply = mailbox.arrive("b@x", &[], Some("a@x"));
        let root = mailbox.arrive("a@x", &[], None);
        assert_eq!(reply, root, "a's own id names the thread b was put in");
        // But one whose parent is itself a reply is rooted at the parent,
        // and the grandparent arriving later does not join it.
        let late = mailbox.arrive("d@x", &[], Some("c@x"));
        let grandparent = mailbox.arrive("z@x", &[], None);
        assert_eq!(mailbox.arrive("c@x", &[], Some("z@x")), grandparent);
        assert_ne!(late, grandparent);
    }

    #[test]
    fn a_headerless_message_is_found_by_its_provider_id() {
        let mailbox = Mailbox::new();
        let id = ProviderMessageId("u0123456789abcdef01234567".into());
        mailbox
            .threads
            .borrow_mut()
            .insert(id.clone(), ProviderThreadId("tkept".into()));
        let stored = |found: &ProviderMessageId| mailbox.threads.borrow().get(found).cloned();
        let thread = thread_for(&mailbox.account, &[], None, Own::Headerless(&id), stored);
        assert_eq!(thread.0, "tkept");
        // Not stored, it is its own root, by its provider id.
        let other = ProviderMessageId("u1".into());
        let thread = thread_for(&mailbox.account, &[], None, Own::Headerless(&other), stored);
        assert_eq!(thread, root_thread("u1"));
    }
}
