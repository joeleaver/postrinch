use std::collections::HashMap;

use postrinch_model::Message;

use crate::codec::Fields;
use crate::messages::TYPE;
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
            .fulltext_search(TYPE, "search_text", query, limit, None, None)?;
        let ids: Vec<u64> = result.hits.iter().map(|h| h.object_id).collect();
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
            .filter_map(|hit| objects.get(&hit.object_id).map(|f| (hit, f)))
            .map(|(hit, fields)| {
                Ok(SearchHit {
                    message: self.decode_message(fields)?,
                    score: hit.score,
                })
            })
            .collect()
    }
}
