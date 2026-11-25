//! Waiting outbox entries in runs that can go to the provider as one, for
//! every provider's outbox sender (Gmail's `messages.batchModify`, IMAP's
//! commands over a set of UIDs).

use postrinch_store::OutboxEntry;

/// Waiting entries in order, in runs that make the same change, each run
/// naming at most `batch` messages. Nothing different falls between the
/// entries of a run, so sending a run as one keeps the changes in the
/// order they were made.
pub(crate) fn runs(waiting: Vec<OutboxEntry>, batch: usize) -> Vec<Vec<OutboxEntry>> {
    let mut all: Vec<Vec<OutboxEntry>> = Vec::new();
    for entry in waiting {
        match all.last_mut() {
            Some(run)
                if run[0].change == entry.change
                    && run.iter().map(|e| e.messages.len()).sum::<usize>()
                        + entry.messages.len()
                        <= batch =>
            {
                run.push(entry);
            }
            _ => all.push(vec![entry]),
        }
    }
    all
}

/// A run of entries as one: their messages, each once, and their change.
pub(crate) fn merged(run: &[OutboxEntry]) -> OutboxEntry {
    let mut one = run[0].clone();
    one.messages.clear();
    for entry in run {
        for m in &entry.messages {
            if !one.messages.contains(m) {
                one.messages.push(m.clone());
            }
        }
    }
    one
}

#[cfg(test)]
mod tests {
    use postrinch_model::{EmailAddress, MessageChange, ProviderMessageId, Timestamp};

    use super::*;

    fn entry(id: u64, change: MessageChange, messages: &[&str]) -> OutboxEntry {
        OutboxEntry {
            id,
            account: EmailAddress::parse("ada@example.org").unwrap(),
            seq: id,
            messages: messages
                .iter()
                .map(|m| ProviderMessageId((*m).into()))
                .collect(),
            change,
            created: Timestamp::from_millis(0),
            attempts: 0,
            error: None,
            sent: None,
        }
    }

    #[test]
    fn runs_join_the_same_change_up_to_the_batch() {
        let archive = MessageChange::remove_label("INBOX");
        let waiting = vec![
            entry(1, archive.clone(), &["a", "b"]),
            entry(2, archive.clone(), &["b", "c"]),
            entry(3, MessageChange::read(), &["a"]),
            entry(4, archive.clone(), &["d", "e", "f"]),
            entry(5, archive.clone(), &["g"]),
        ];
        let runs = runs(waiting, 4);
        let ids: Vec<Vec<u64>> = runs
            .iter()
            .map(|r| r.iter().map(|e| e.id).collect())
            .collect();
        assert_eq!(ids, [vec![1, 2], vec![3], vec![4, 5]]);
        let one = merged(&runs[0]);
        let names: Vec<&str> = one.messages.iter().map(|m| m.0.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }
}
