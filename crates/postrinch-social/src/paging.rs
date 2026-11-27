//! Walking a paged list (§7.2) to its end.

use postrinch_ofscp::http::{Direction, PageRequest};
use postrinch_ofscp::ofscp_core::wire::Paged;

/// The page after `page`, if `found` says there is one. A provider that
/// gives back the cursor it was asked with has no more.
pub(crate) fn next_page<T>(found: &Paged<T>, page: &PageRequest) -> Option<PageRequest> {
    let cursor = found.page.next_cursor.as_ref()?;
    if page.cursor.as_ref() == Some(cursor) {
        return None;
    }
    Some(PageRequest::after(cursor.clone(), Direction::Backward))
}
