use postrinch_model::SavedView;
use rhypedb_query::ast::{Query, Source, Step};

use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreError, StoreResult};

const TYPE: &str = "SavedView";
const MESSAGE: &str = "Message";

/// Check that `query` is one a view can be: it parses, reads `Message`
/// objects, and only narrows, ranks or pages them. A view never writes and
/// never leaves the `Message` type.
fn check_query(query: &str) -> StoreResult<()> {
    let parsed: Query = rhypedb_query::parser::parse_query(query)
        .map_err(|e| StoreError::InvalidView(e.to_string()))?;
    let type_name = match &parsed.source {
        Source::All { type_name } | Source::Filter { type_name, .. } => type_name,
        Source::Get { .. } => {
            return Err(StoreError::InvalidView(
                "a view selects messages, not one object by id".into(),
            ));
        }
        Source::Create { .. } | Source::CreateBatch { .. } => {
            return Err(StoreError::InvalidView(
                "a view cannot create objects".into(),
            ));
        }
    };
    if type_name != MESSAGE {
        return Err(StoreError::InvalidView(format!(
            "a view selects {MESSAGE}, not {type_name}"
        )));
    }
    for step in &parsed.steps {
        match step {
            Step::Filter { .. }
            | Step::Matches { .. }
            | Step::Similar { .. }
            | Step::Limit { .. }
            | Step::Offset { .. } => {}
            Step::Traverse { field_name } => {
                return Err(StoreError::InvalidView(format!(
                    "a view cannot follow .{field_name} away from messages"
                )));
            }
            Step::Update { .. } | Step::Delete | Step::Link { .. } | Step::Unlink { .. } => {
                return Err(StoreError::InvalidView(
                    "a view cannot change anything".into(),
                ));
            }
        }
    }
    Ok(())
}

fn decode(fields: &Fields) -> StoreResult<SavedView> {
    SavedView::new(&fields.str("name")?, &fields.str("query")?)
        .map_err(|e| StoreError::corrupt(TYPE, fields.id(), e))
}

impl Store {
    /// Save a view, replacing the query of any view with the same name.
    pub fn put_saved_view(&self, view: &SavedView) -> StoreResult<()> {
        check_query(view.query())?;
        let create = FieldsBuilder::new()
            .str("name", view.name())
            .str("query", view.query());
        let update = FieldsBuilder::new().str("query", view.query());
        match self.find_views(view.name())?.first() {
            Some(existing) => self.db_update(TYPE, *existing, update.build())?,
            None => {
                self.upsert(TYPE, "name", view.name(), create.build(), update.build())?;
            }
        }
        Ok(())
    }

    /// The views named `name`, oldest first: found by name and, failing
    /// that, among all views, which are few (rhypedb#27: a name that is the
    /// start of another can be missed after a reopen).
    fn find_views(&self, name: &str) -> StoreResult<Vec<u64>> {
        if let Some(found) = self.find(TYPE, "name", name)? {
            return Ok(vec![found.id()]);
        }
        let mut ids: Vec<u64> = Vec::new();
        for object in self.db.scan_type_projected(TYPE, &["name"])? {
            let fields = Fields::new(TYPE, object);
            if fields.str("name")? == name {
                ids.push(fields.id());
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// Keep the oldest of each name's views, removing copies made before
    /// views were found despite rhypedb#27. Returns how many were removed.
    pub fn merge_duplicate_views(&self) -> StoreResult<usize> {
        let mut by_name: std::collections::HashMap<String, Vec<u64>> = Default::default();
        for object in self.db.scan_type_projected(TYPE, &["name"])? {
            let fields = Fields::new(TYPE, object);
            by_name
                .entry(fields.str("name")?)
                .or_default()
                .push(fields.id());
        }
        let mut removed = 0;
        for (_, mut copies) in by_name.into_iter().filter(|(_, c)| c.len() > 1) {
            copies.sort_unstable();
            for copy in copies.into_iter().skip(1) {
                self.db_delete(TYPE, copy)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Every saved view, sorted by name.
    pub fn saved_views(&self) -> StoreResult<Vec<SavedView>> {
        let mut views = self
            .db
            .scan_type(TYPE)?
            .into_iter()
            .map(|o| decode(&Fields::new(TYPE, o)))
            .collect::<StoreResult<Vec<_>>>()?;
        views.sort_by(|a, b| a.name().cmp(b.name()));
        Ok(views)
    }

    /// Remove a view by name. Removing one that is not there does nothing.
    pub fn remove_saved_view(&self, name: &str) -> StoreResult<()> {
        for view in self.find_views(name)? {
            self.db_delete(TYPE, view)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reads_of_messages() {
        for q in [
            "Message",
            "Message.filter(.unread == true)",
            "Message.filter(.starred == true).limit(50)",
            "Message.matches(.search_text, \"invoice\", k: 20)",
        ] {
            check_query(q).unwrap_or_else(|e| panic!("{q}: {e}"));
        }
    }

    #[test]
    fn refuses_writes_other_types_and_traversals() {
        for q in [
            "Message.filter(.unread == true).delete()",
            "Message.filter(.unread == true).update({ unread: false })",
            "Person.filter(.name == \"Ada\")",
            "Message.get(1)",
            "Message.from",
            "Message.filter(",
        ] {
            assert!(
                matches!(check_query(q), Err(StoreError::InvalidView(_))),
                "{q} should be refused"
            );
        }
    }

    #[test]
    fn put_replace_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let unread = SavedView::new("Unread", "Message.filter(.unread == true)").unwrap();
        let starred = SavedView::new("Starred", "Message.filter(.starred == true)").unwrap();
        store.put_saved_view(&unread).unwrap();
        store.put_saved_view(&starred).unwrap();
        let narrower =
            SavedView::new("Unread", "Message.filter(.unread == true).limit(10)").unwrap();
        store.put_saved_view(&narrower).unwrap();

        assert_eq!(
            store.saved_views().unwrap(),
            vec![starred.clone(), narrower]
        );
        store.remove_saved_view("Unread").unwrap();
        store.remove_saved_view("Unread").unwrap();
        assert_eq!(store.saved_views().unwrap(), vec![starred]);
    }

    #[test]
    fn copies_of_a_view_go_and_saving_finds_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let view = SavedView::new("Bills", "Message.filter(.starred == true)").unwrap();
        // A name another starts with is what rhypedb#27 misses after a
        // reopen.
        let longer = SavedView::new("Bills paid", "Message.filter(.unread == false)").unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            store.put_saved_view(&view).unwrap();
            store.put_saved_view(&longer).unwrap();
        }
        let store = (0..50)
            .find_map(|_| {
                Store::open(dir.path())
                    .inspect_err(|_| std::thread::sleep(std::time::Duration::from_millis(100)))
                    .ok()
            })
            .expect("the store reopens");
        let copy = FieldsBuilder::new()
            .str("name", "Bills")
            .str("query", "Message.filter(.starred == true)")
            .build();
        if store.db_create(TYPE, copy).is_err() {
            // The engine caught it: rhypedb#27 is fixed.
            return;
        }
        assert_eq!(store.merge_duplicate_views().unwrap(), 1);
        assert_eq!(store.saved_views().unwrap(), vec![view, longer.clone()]);
        // Saving finds the original even where the lookup by name misses.
        let changed = SavedView::new("Bills", "Message.filter(.unread == true)").unwrap();
        store.put_saved_view(&changed).unwrap();
        assert_eq!(store.saved_views().unwrap(), vec![changed, longer]);
    }
}
