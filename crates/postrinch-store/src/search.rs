use std::collections::HashMap;

use postrinch_model::Message;

use crate::codec::Fields;
use crate::messages::{CONTENT, TYPE};
use crate::{Store, StoreResult};

/// A message that matched a search, with its BM25 score (higher is better).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub message: Message,
    pub score: f32,
}

impl Store {
    /// Full-text search over every account's messages: subject, names and
    /// addresses, and body. `query` takes `+required` terms, `"phrases"` and
    /// `prefix*` terms. Returns the best `limit` hits, best first.
    pub fn search(&self, query: &str, limit: usize) -> StoreResult<Vec<SearchHit>> {
        let result = self
            .db
            .fulltext_search(CONTENT, "search_text", query, limit, None, None)?;
        // Each hit is a message's content; find the message it belongs to.
        let mut message_of = HashMap::new();
        for hit in &result.hits {
            if let Some(message) = self.link(CONTENT, hit.object_id, "message")? {
                message_of.insert(hit.object_id, message);
            }
        }
        let ids: Vec<u64> = message_of.values().copied().collect();
        // get_many returns objects in id order and leaves out any deleted
        // since the search ran; match them back to the ranked hits by id.
        let objects: HashMap<u64, Fields> = self
            .get_many(TYPE, &ids)?
            .into_iter()
            .map(|f| (f.id(), f))
            .collect();
        result
            .hits
            .iter()
            .filter_map(|hit| {
                let message = message_of.get(&hit.object_id)?;
                objects.get(message).map(|f| (hit, f))
            })
            .map(|(hit, fields)| {
                Ok(SearchHit {
                    message: self.decode_message(fields)?,
                    score: hit.score,
                })
            })
            .collect()
    }
}
