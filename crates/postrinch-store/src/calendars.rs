//! An account's calendars and their events, as calendar sync keeps them.
//! Events are found by their calendar's key, which starts theirs.

use postrinch_model::{CalendarInfo, EmailAddress, EventRecord};

use crate::codec::{Fields, FieldsBuilder};
use crate::keys::SEP;
use crate::{Store, StoreResult};

const CALENDAR: &str = "Calendar";
const EVENT: &str = "Event";

fn calendar_key(account: &EmailAddress, calendar: &str) -> String {
    format!("{}{SEP}{calendar}", account.key())
}

impl Store {
    /// The account's calendars, as last synced.
    pub fn calendars(&self, account: &EmailAddress) -> StoreResult<Vec<CalendarInfo>> {
        let key = account.key();
        let mut found = Vec::new();
        for object in self
            .db
            .scan_type_projected(CALENDAR, &["account", "info"])?
        {
            let calendar = Fields::new(CALENDAR, object);
            if calendar.str("account")? == key {
                found.push(calendar.json("info")?);
            }
        }
        Ok(found)
    }

    /// Keep a calendar as the account lists it; where its sync got to stays.
    pub fn put_calendar(&self, account: &EmailAddress, info: &CalendarInfo) -> StoreResult<()> {
        let key = calendar_key(account, &info.id);
        let update = FieldsBuilder::new().json("info", info).build();
        let create = FieldsBuilder::new()
            .str("key", key.clone())
            .str("account", account.key())
            .json("info", info)
            .null("sync_token")
            .build();
        self.upsert(CALENDAR, "key", &key, create, update)?;
        Ok(())
    }

    /// Forget a calendar the account no longer lists, and its events.
    pub fn remove_calendar(&self, account: &EmailAddress, calendar: &str) -> StoreResult<()> {
        self.clear_events(account, calendar)?;
        if let Some(found) = self.find(CALENDAR, "key", &calendar_key(account, calendar))? {
            self.db_delete(CALENDAR, found.id())?;
        }
        Ok(())
    }

    /// Forget every calendar of the account, and their events.
    pub fn remove_calendars(&self, account: &EmailAddress) -> StoreResult<()> {
        for info in self.calendars(account)? {
            self.remove_calendar(account, &info.id)?;
        }
        Ok(())
    }

    /// Where the sync of a calendar's events got to.
    pub fn calendar_sync_token(
        &self,
        account: &EmailAddress,
        calendar: &str,
    ) -> StoreResult<Option<String>> {
        match self.find(CALENDAR, "key", &calendar_key(account, calendar))? {
            Some(found) => found.opt_str("sync_token"),
            None => Ok(None),
        }
    }

    /// Record where the sync of a calendar's events got to, or `None` to
    /// start again. The calendar must be kept already.
    pub fn set_calendar_sync_token(
        &self,
        account: &EmailAddress,
        calendar: &str,
        token: Option<&str>,
    ) -> StoreResult<()> {
        if let Some(found) = self.find(CALENDAR, "key", &calendar_key(account, calendar))? {
            let fields = match token {
                Some(token) => FieldsBuilder::new().str("sync_token", token),
                None => FieldsBuilder::new().null("sync_token"),
            };
            self.db_update(CALENDAR, found.id(), fields.build())?;
        }
        Ok(())
    }

    /// Keep an event as its calendar has it.
    pub fn put_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        record: &EventRecord,
    ) -> StoreResult<()> {
        let calendar = calendar_key(account, calendar);
        let key = format!("{calendar}{SEP}{}", record.event.id);
        let update = FieldsBuilder::new().json("record", record).build();
        let create = FieldsBuilder::new()
            .str("key", key.clone())
            .str("calendar", calendar)
            .json("record", record)
            .build();
        self.upsert(EVENT, "key", &key, create, update)?;
        Ok(())
    }

    /// Forget an event its calendar deleted.
    pub fn remove_event(
        &self,
        account: &EmailAddress,
        calendar: &str,
        id: &str,
    ) -> StoreResult<()> {
        let key = format!("{}{SEP}{id}", calendar_key(account, calendar));
        if let Some(found) = self.find(EVENT, "key", &key)? {
            self.db_delete(EVENT, found.id())?;
        }
        Ok(())
    }

    /// Forget every event of a calendar, to sync it again from the start.
    pub fn clear_events(&self, account: &EmailAddress, calendar: &str) -> StoreResult<()> {
        let calendar = calendar_key(account, calendar);
        let mut ids = Vec::new();
        for object in self.db.scan_type_projected(EVENT, &["calendar"])? {
            let event = Fields::new(EVENT, object);
            if event.str("calendar")? == calendar {
                ids.push(event.id());
            }
        }
        for id in ids {
            self.db_delete(EVENT, id)?;
        }
        Ok(())
    }

    /// Every event of the account's calendars, with its calendar's id.
    pub fn events(&self, account: &EmailAddress) -> StoreResult<Vec<(String, EventRecord)>> {
        let prefix = format!("{}{SEP}", account.key());
        let mut found = Vec::new();
        for object in self
            .db
            .scan_type_projected(EVENT, &["calendar", "record"])?
        {
            let event = Fields::new(EVENT, object);
            let calendar = event.str("calendar")?;
            if let Some(id) = calendar.strip_prefix(&prefix) {
                found.push((id.to_owned(), event.json("record")?));
            }
        }
        Ok(found)
    }
}

#[cfg(test)]
#[path = "calendars_tests.rs"]
mod tests;
