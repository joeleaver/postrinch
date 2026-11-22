//! Agents' answers to invitations, kept until the person gives or drops
//! them.

use postrinch_model::{EmailAddress, ProposedAnswer, ProviderMessageId};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::SEP;
use crate::{Store, StoreResult};

const TYPE: &str = "ProposedAnswer";

fn key(account: &EmailAddress, message: &ProviderMessageId) -> String {
    format!("{}{SEP}{}", account.key(), message.0)
}

impl Store {
    /// Keep an agent's answer to the invitation in `proposal.message`, in
    /// place of any earlier one.
    pub fn put_proposed_answer(
        &self,
        account: &EmailAddress,
        proposal: &ProposedAnswer,
    ) -> StoreResult<()> {
        let key = key(account, &proposal.message);
        let update = FieldsBuilder::new().json("proposal", proposal).build();
        let create = FieldsBuilder::new()
            .str("key", key.clone())
            .str("account", account.key())
            .json("proposal", proposal)
            .build();
        self.upsert(TYPE, "key", &key, create, update)?;
        Ok(())
    }

    /// The account's waiting answers, oldest first.
    pub fn proposed_answers(&self, account: &EmailAddress) -> StoreResult<Vec<ProposedAnswer>> {
        let wanted = account.key();
        let mut found: Vec<ProposedAnswer> = Vec::new();
        for object in self
            .db
            .scan_type_projected(TYPE, &["account", "proposal"])?
        {
            let fields = Fields::new(TYPE, object);
            if fields.str("account")? == wanted {
                found.push(fields.json("proposal")?);
            }
        }
        found.sort_by_key(|p| p.made);
        Ok(found)
    }

    /// The answer is given or dropped: it goes.
    pub fn remove_proposed_answer(
        &self,
        account: &EmailAddress,
        message: &ProviderMessageId,
    ) -> StoreResult<()> {
        if let Some(found) = self.find(TYPE, "key", &key(account, message))? {
            self.db_delete(TYPE, found.id())?;
        }
        Ok(())
    }
}

impl Store {
    /// Drop every answer waiting in the account.
    pub(crate) fn remove_proposed_answers(&self, account: &EmailAddress) -> StoreResult<()> {
        for p in self.proposed_answers(account)? {
            self.remove_proposed_answer(account, &p.message)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use postrinch_model::{Answer, ProviderThreadId, Timestamp};

    use super::*;

    #[test]
    fn a_later_answer_replaces_the_earlier() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let me = EmailAddress::parse("me@example.com").unwrap();
        let mut p = ProposedAnswer {
            message: ProviderMessageId("m1".into()),
            thread: ProviderThreadId("t1".into()),
            answer: Answer::Yes,
            agent: "Claude".into(),
            reason: None,
            summary: Some("Plans".into()),
            made: Timestamp::from_millis(1),
        };
        store.put_proposed_answer(&me, &p).unwrap();
        p.answer = Answer::No;
        store.put_proposed_answer(&me, &p).unwrap();
        let waiting = store.proposed_answers(&me).unwrap();
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting[0].answer, Answer::No);
        store.remove_proposed_answer(&me, &p.message).unwrap();
        assert!(store.proposed_answers(&me).unwrap().is_empty());
    }
}
