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
        self.upsert(TYPE, "name", view.name(), create.build(), update.build())?;
        Ok(())
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
        if let Some(view) = self.find(TYPE, "name", name)? {
            self.db_delete(TYPE, view.id())?;
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
}
