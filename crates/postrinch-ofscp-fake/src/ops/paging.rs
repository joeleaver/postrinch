//! §7.2 paging over a list that is not a timeline (members, reactions,
//! conversations, notifications): cursors are positions in the list,
//! tagged with the list they belong to.

use ofscp_core::wire::{Page, Paged};

use crate::problem::{Outcome, Problem};
use crate::state::timeline::PageQuery;

/// One page of `items`, which are in ascending order (oldest first):
/// `backward` (the default) walks from the end.
pub(crate) fn page<T: Clone>(items: &[T], query: &PageQuery, tag: &str) -> Outcome<Paged<T>> {
    let at = match &query.cursor {
        None => None,
        Some(c) => Some(
            parse(c, tag)
                .ok_or_else(|| Problem::bad_request("the cursor is not from this list"))?,
        ),
    };
    let positions: Vec<usize> = if query.forward {
        let start = at.map_or(0, |a| a + 1);
        (start..items.len()).collect()
    } else {
        let end = at.map_or(items.len(), |a| a.min(items.len()));
        (0..end).rev().collect()
    };
    let more = positions.len() > query.limit;
    let chosen = &positions[..positions.len().min(query.limit)];
    Ok(Paged {
        items: chosen.iter().map(|&i| items[i].clone()).collect(),
        page: Page {
            next_cursor: if more {
                chosen.last().map(|&i| format!("{tag}.{i}"))
            } else {
                None
            },
            prev_cursor: chosen.first().map(|&i| format!("{tag}.{i}")),
        },
    })
}

fn parse(cursor: &str, tag: &str) -> Option<usize> {
    let (t, i) = cursor.rsplit_once('.')?;
    (t == tag).then(|| i.parse().ok())?
}
