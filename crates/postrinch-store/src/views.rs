//! Views the person made or changed, each a `SavedView` object: its
//! `name` field holds the view's key (unique), `settings` the whole view.
//! A built-in view is stored only once changed. The commands check that a
//! view's query reads before it gets here.

use postrinch_model::SavedView;

use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

const TYPE: &str = "SavedView";
/// The field that holds a view's key.
const KEY: &str = "name";

impl Store {
    /// Save a view, replacing the one with the same key.
    pub fn put_view(&self, view: &SavedView) -> StoreResult<()> {
        let fields = || {
            FieldsBuilder::new()
                .str(KEY, &view.key)
                .str("query", &view.query)
                .json("settings", view)
        };
        match self.find_views(&view.key)?.first() {
            Some(existing) => self.db_update(TYPE, *existing, fields().build())?,
            None => {
                self.upsert(TYPE, KEY, &view.key, fields().build(), fields().build())?;
            }
        }
        Ok(())
    }

    /// The views with this key, oldest first: found by key and, failing
    /// that, among all views, which are few (rhypedb#27: a key that is the
    /// start of another can be missed after a reopen).
    fn find_views(&self, key: &str) -> StoreResult<Vec<u64>> {
        if let Some(found) = self.find(TYPE, KEY, key)? {
            return Ok(vec![found.id()]);
        }
        let mut ids: Vec<u64> = Vec::new();
        for object in self.db.scan_type_projected(TYPE, &[KEY])? {
            let fields = Fields::new(TYPE, object);
            if fields.str(KEY)? == key {
                ids.push(fields.id());
            }
        }
        ids.sort_unstable();
        Ok(ids)
    }

    /// Keep the oldest of each key's views, removing copies made before
    /// views were found despite rhypedb#27. Returns how many were removed.
    pub fn merge_duplicate_views(&self) -> StoreResult<usize> {
        let mut by_key: std::collections::HashMap<String, Vec<u64>> = Default::default();
        for object in self.db.scan_type_projected(TYPE, &[KEY])? {
            let fields = Fields::new(TYPE, object);
            by_key
                .entry(fields.str(KEY)?)
                .or_default()
                .push(fields.id());
        }
        let mut removed = 0;
        for (_, mut copies) in by_key.into_iter().filter(|(_, c)| c.len() > 1) {
            copies.sort_unstable();
            for copy in copies.into_iter().skip(1) {
                self.db_delete(TYPE, copy)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Every stored view, in the order they were first saved. Objects from
    /// before views had settings (an earlier query language) are left out.
    pub fn views(&self) -> StoreResult<Vec<SavedView>> {
        let mut objects: Vec<Fields> = self
            .db
            .scan_type(TYPE)?
            .into_iter()
            .map(|o| Fields::new(TYPE, o))
            .collect();
        objects.sort_by_key(Fields::id);
        let mut views = Vec::new();
        for fields in objects {
            if let Some(view) = fields.opt_json::<SavedView>("settings")? {
                views.push(view);
            }
        }
        Ok(views)
    }

    /// Remove a view by key. Removing one that is not there does nothing.
    pub fn remove_view(&self, key: &str) -> StoreResult<()> {
        for view in self.find_views(key)? {
            self.db_delete(TYPE, view)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use postrinch_model::Timestamp;

    fn view(name: &str, query: &str, made: i64) -> SavedView {
        SavedView::new(name, query, Timestamp::from_millis(made)).unwrap()
    }

    #[test]
    fn put_replace_list_remove() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let clients = view("Clients", "label:clients", 1);
        let receipts = view("Receipts", "label:receipts", 2);
        store.put_view(&clients).unwrap();
        store.put_view(&receipts).unwrap();
        let mut renamed = clients.clone();
        renamed.rename("Customers").unwrap();
        store.put_view(&renamed).unwrap();

        assert_eq!(store.views().unwrap(), vec![renamed, receipts.clone()]);
        store.remove_view(&clients.key).unwrap();
        store.remove_view(&clients.key).unwrap();
        assert_eq!(store.views().unwrap(), vec![receipts]);
    }

    #[test]
    fn copies_of_a_view_go_and_saving_finds_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let first = view("Bills", "label:bills", 1);
        // A key another starts with is what rhypedb#27 misses after a
        // reopen.
        let longer = view("Bills paid", "label:paid", 10);
        {
            let store = Store::open(dir.path()).unwrap();
            store.put_view(&first).unwrap();
            store.put_view(&longer).unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        let copy = FieldsBuilder::new()
            .str(KEY, &first.key)
            .str("query", "label:bills")
            .json("settings", &first)
            .build();
        if store.db_create(TYPE, copy).is_err() {
            // The engine caught it: rhypedb#27 is fixed.
            return;
        }
        assert_eq!(store.merge_duplicate_views().unwrap(), 1);
        assert_eq!(store.views().unwrap(), vec![first.clone(), longer.clone()]);
        let mut changed = first;
        changed.set_query("label:bills is:unread").unwrap();
        store.put_view(&changed).unwrap();
        assert_eq!(store.views().unwrap(), vec![changed, longer]);
    }

    #[test]
    fn views_from_before_settings_are_left_out() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let old = FieldsBuilder::new()
            .str(KEY, "Unread")
            .str("query", "Message.filter(.unread == true)")
            .build();
        store.db_create(TYPE, old).unwrap();
        assert!(store.views().unwrap().is_empty());
    }
}
