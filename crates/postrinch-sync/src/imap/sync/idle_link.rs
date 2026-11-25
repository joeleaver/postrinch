//! What an account's IDLE loop and its other connections tell each other
//! (docs/imap.md, "Sync"):
//!
//! - **Let go.** When the account's connections are let go
//!   ([`ImapSync::disconnect`](super::ImapSync::disconnect)), its IDLE
//!   connection is to go too.
//! - **Give way.** When the server refuses one of the account's other
//!   connections because the account has too many open, the IDLE
//!   connection, the one it can do without, signs out so that one can
//!   sign in in its place ([`Links::give_way`]).
//! - **Holding.** Whether the IDLE loop holds a signed-in connection now,
//!   so that a refused connection knows whether there is one to ask for.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use postrinch_model::EmailAddress;
use tokio::sync::watch;

/// One account's signals.
struct Link {
    /// Raised each time the account's connections are let go.
    released: watch::Sender<u64>,
    /// Raised each time another connection asks the IDLE to give way.
    give_way: watch::Sender<u64>,
    /// How many signed-in IDLE connections the account has (one at
    /// most, in the ordinary run of things).
    holding: watch::Sender<usize>,
}

impl Default for Link {
    fn default() -> Self {
        Self {
            released: watch::channel(0).0,
            give_way: watch::channel(0).0,
            holding: watch::channel(0).0,
        }
    }
}

/// Every account's [`Link`]. Cheap to clone; clones share them.
#[derive(Clone, Default)]
pub(super) struct Links(Arc<Mutex<HashMap<String, Arc<Link>>>>);

impl Links {
    fn link(&self, account: &EmailAddress) -> Arc<Link> {
        let mut links = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(links.entry(account.key()).or_default())
    }

    /// A count that rises each time the account's connections are let go.
    pub(super) fn released(&self, account: &EmailAddress) -> watch::Receiver<u64> {
        self.link(account).released.subscribe()
    }

    /// A count that rises each time the IDLE connection is asked to give
    /// way.
    pub(super) fn give_way_asked(&self, account: &EmailAddress) -> watch::Receiver<u64> {
        self.link(account).give_way.subscribe()
    }

    /// Tells the account's IDLE loop, if one watches, to let go.
    pub(super) fn release(&self, account: &EmailAddress) {
        self.link(account).released.send_modify(|n| *n += 1);
    }

    /// Marks a signed-in IDLE connection as held until the returned
    /// guard is dropped, with the connection.
    pub(super) fn hold(&self, account: &EmailAddress) -> Holding {
        let link = self.link(account);
        link.holding.send_modify(|n| *n += 1);
        Holding(link)
    }

    /// When the account holds an IDLE connection, asks it to give way and
    /// waits, at most `within`, until it has signed out. Returns whether
    /// it did: only then is a connection refused for the account's
    /// connection limit worth trying again at once.
    pub(super) async fn give_way(&self, account: &EmailAddress, within: Duration) -> bool {
        let link = self.link(account);
        let mut holding = link.holding.subscribe();
        if *holding.borrow_and_update() == 0 {
            return false;
        }
        link.give_way.send_modify(|n| *n += 1);
        // The sender lives in `link`, held here: waiting cannot fail.
        matches!(
            tokio::time::timeout(within, holding.wait_for(|n| *n == 0)).await,
            Ok(Ok(_))
        )
    }
}

/// An IDLE connection held: dropped with it.
pub(super) struct Holding(Arc<Link>);

impl Drop for Holding {
    fn drop(&mut self) {
        self.0.holding.send_modify(|n| *n = n.saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ada() -> EmailAddress {
        EmailAddress::parse("ada@example.org").unwrap()
    }

    #[tokio::test]
    async fn nothing_to_give_way_without_an_idle_connection() {
        let links = Links::default();
        let asked = links.give_way_asked(&ada());
        assert!(!links.give_way(&ada(), Duration::from_secs(5)).await);
        assert!(!asked.has_changed().unwrap(), "nobody was asked");
    }

    #[tokio::test]
    async fn the_idle_connection_gives_way_once_it_is_dropped() {
        let (links, me) = (Links::default(), ada());
        let mut asked = links.give_way_asked(&me);
        let held = links.hold(&me);
        let idle = async move {
            asked.changed().await.unwrap();
            drop(held);
        };
        let (gave_way, ()) = tokio::join!(links.give_way(&me, Duration::from_secs(5)), idle);
        assert!(gave_way);
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_connection_that_does_not_go_is_waited_for_no_longer_than_asked() {
        let links = Links::default();
        let _held = links.hold(&ada());
        assert!(!links.give_way(&ada(), Duration::from_secs(5)).await);
    }

    #[test]
    fn each_account_has_its_own() {
        let links = Links::default();
        let other = EmailAddress::parse("bob@example.org").unwrap();
        let released = links.released(&other);
        links.release(&ada());
        assert!(!released.has_changed().unwrap());
        links.release(&other);
        assert!(released.has_changed().unwrap());
    }
}
