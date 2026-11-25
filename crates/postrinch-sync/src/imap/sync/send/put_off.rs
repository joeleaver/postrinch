//! Recipients the server put off (docs/imap.md, "Writing and sending"):
//! a 4xx at RCPT for one recipient, as `452 4.2.2 Mailbox full`. The
//! message goes to the others at once, and to them on a later attempt,
//! each recipient once. A recipient put off on too many attempts, or for
//! too long, is left to the person, naming them and the server's words.

use std::time::Duration;

use postrinch_model::{EmailAddress, Timestamp};
use postrinch_store::PutOff;

use super::Delivery;
use super::sent::list;
use super::transmit::partly;
use crate::SyncResult;
use crate::imap::sync::ImapSync;

/// How many attempts a recipient may be put off in before the message
/// waits for the person.
pub(super) const PUT_OFF_ATTEMPTS: u32 = 6;

/// How long after a recipient was first put off the message waits for
/// the person, however few attempts there were.
pub(super) const PUT_OFF_FOR: Duration = Duration::from_secs(24 * 60 * 60);

impl ImapSync {
    /// The server on `server` put off `put_off` (with its words) in an
    /// attempt that delivered the message to everyone else it is for
    /// (`delivered` so far): records it, and says what becomes of the
    /// message. Tried again later ([`Delivery::PutOff`]) while each is
    /// within the limits; otherwise it waits for the person.
    pub(super) async fn put_off(
        &self,
        id: u64,
        server: &str,
        put_off: Vec<(EmailAddress, String)>,
        delivered: &[EmailAddress],
    ) -> SyncResult<Delivery> {
        let now = super::super::places::now();
        let standing = self
            .with_store(move |s| s.outgoing_put_off(id, &put_off, now))
            .await?;
        if let Some(why) = given_up(&standing, now) {
            return Ok(Delivery::Refused(partly(delivered, &why)));
        }
        let who: Vec<EmailAddress> = standing.iter().map(|p| p.address.clone()).collect();
        let words = standing
            .iter()
            .map(|p| format!("{}: {}", p.address, p.words))
            .collect::<Vec<_>>()
            .join("; ");
        Ok(Delivery::PutOff {
            server: server.to_owned(),
            why: format!(
                "{server} put off {} for now, to try again later ({words})",
                list(&who)
            ),
        })
    }
}

/// Why the message waits for the person now, if any of `standing` was
/// put off on too many attempts or for too long by `now`.
fn given_up(standing: &[PutOff], now: Timestamp) -> Option<String> {
    let longest = i64::try_from(PUT_OFF_FOR.as_millis()).unwrap_or(i64::MAX);
    let over = |p: &&PutOff| {
        p.times >= PUT_OFF_ATTEMPTS
            || now.as_millis().saturating_sub(p.since.as_millis()) >= longest
    };
    if !standing.iter().any(|p| over(&p)) {
        return None;
    }
    let each = standing
        .iter()
        .map(|p| {
            format!(
                "the server put it off for {} {} time{}, saying: {}",
                p.address,
                p.times,
                if p.times == 1 { "" } else { "s" },
                p.words
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(each)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_off(times: u32, since: i64) -> PutOff {
        PutOff {
            address: EmailAddress::parse("bob@example.net").unwrap(),
            words: "Mailbox full".into(),
            times,
            since: Timestamp::from_millis(since),
        }
    }

    #[test]
    fn a_recipient_is_left_to_the_person_after_so_many_attempts_or_so_long() {
        let day = i64::try_from(PUT_OFF_FOR.as_millis()).unwrap();
        let now = Timestamp::from_millis(10 * day);
        assert_eq!(given_up(&[put_off(1, 10 * day)], now), None);
        assert_eq!(
            given_up(&[put_off(PUT_OFF_ATTEMPTS - 1, 9 * day + 1)], now),
            None
        );
        let why = given_up(&[put_off(PUT_OFF_ATTEMPTS, 10 * day)], now).unwrap();
        assert!(
            why.contains("bob@example.net") && why.contains("Mailbox full"),
            "{why}"
        );
        let why = given_up(&[put_off(2, 9 * day)], now).unwrap();
        assert!(why.contains("2 times"), "{why}");
    }
}
