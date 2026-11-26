//! Posts, replies and messages being written, kept as the person types so
//! they survive a restart.

use super::{DraftMedia, KEY, RECORD, SocialDraft, scoped, social_account_key};
use crate::codec::{Fields, FieldsBuilder};
use crate::{Store, StoreResult};

const DRAFT: &str = "SocialDraft";

impl Store {
    /// Keep a draft, replacing the one with its id.
    pub fn put_social_draft(&self, draft: &SocialDraft) -> StoreResult<()> {
        let place = draft.place.as_ref().map(|p| scoped(&draft.account, p));
        let fields = FieldsBuilder::new()
            .str("account", social_account_key(&draft.account))
            .opt_str("place", place)
            .datetime("updated", draft.updated)
            .json(RECORD, draft);
        self.social_put(DRAFT, KEY, &draft.id, fields)?;
        Ok(())
    }

    /// The draft with this id.
    pub fn social_draft(&self, id: &str) -> StoreResult<Option<SocialDraft>> {
        self.social_record(DRAFT, KEY, id)
    }

    /// Every draft of every account, the most recently changed first.
    pub fn social_drafts(&self) -> StoreResult<Vec<SocialDraft>> {
        let drafts = self
            .db
            .scan_type_projected(DRAFT, &[RECORD])?
            .into_iter()
            .map(|o| Fields::new(DRAFT, o).json(RECORD))
            .collect::<StoreResult<Vec<SocialDraft>>>()?;
        Ok(newest_first(drafts))
    }

    /// The drafts written for one place, the most recently changed first.
    pub fn social_drafts_for(&self, account: &str, place: &str) -> StoreResult<Vec<SocialDraft>> {
        let drafts = self.social_records_where(DRAFT, "place", &scoped(account, place))?;
        Ok(newest_first(drafts))
    }

    /// Delete a draft. Returns whether it was here.
    pub fn remove_social_draft(&self, id: &str) -> StoreResult<bool> {
        self.social_remove(DRAFT, KEY, id)
    }

    /// Delete every draft of `account`.
    pub(super) fn remove_social_drafts(&self, account: &str) -> StoreResult<usize> {
        self.social_remove_where(DRAFT, "account", &social_account_key(account))
    }

    /// Take the place (and the message answered there) off the drafts
    /// written for the place whose key is `place`, which is going.
    pub(super) fn unplace_social_drafts(&self, place: &str) -> StoreResult<()> {
        for found in self.social_where(DRAFT, "place", place)? {
            let mut draft: SocialDraft = found.json(RECORD)?;
            draft.place = None;
            draft.reply_to = None;
            let fields = FieldsBuilder::new()
                .null("place")
                .json(RECORD, &draft)
                .build();
            self.db_update(DRAFT, found.id(), fields)?;
        }
        Ok(())
    }

    /// Every blob a draft's pictures and files are kept in.
    pub(crate) fn social_draft_blobs(&self) -> StoreResult<Vec<String>> {
        let mut blobs = Vec::new();
        for object in self.db.scan_type_projected(DRAFT, &[RECORD])? {
            let draft: SocialDraft = Fields::new(DRAFT, object).json(RECORD)?;
            for media in draft.cover.iter().chain(&draft.attachments) {
                if let DraftMedia::Kept { blob, .. } = media {
                    blobs.push(blob.clone());
                }
            }
        }
        Ok(blobs)
    }
}

/// Sorted by when they changed, newest first, then by id.
fn newest_first(mut drafts: Vec<SocialDraft>) -> Vec<SocialDraft> {
    drafts.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
    drafts
}
