//! The checks on the attachments of a message a provider stores (§5.8
//! *Checks on attachments in a message*).

use ofscp_core::wire::Attachment;
use ofscp_core::{Actor, Authority};

use crate::problem::{Outcome, Problem};
use crate::state::State;

/// The attachments as the message will store them: a hosted one as this
/// provider's own record, another one with any `expiresAt` dropped.
pub(crate) fn check(
    state: &State,
    authority: &Authority,
    author: &Actor,
    given: Vec<Attachment>,
) -> Outcome<Vec<Attachment>> {
    given
        .into_iter()
        .map(|a| match state.uploads.get(&a.id) {
            Some(upload) => {
                if upload.uploader != *author {
                    return Err(Problem::bad_request(format!(
                        "{} was uploaded by someone else",
                        a.id
                    )));
                }
                if upload.target != *authority {
                    return Err(Problem::bad_request(format!(
                        "{} was uploaded for another provider",
                        a.id
                    )));
                }
                let stored = &upload.attachment;
                if a.mime != stored.mime || a.size != stored.size || a.hash != stored.hash {
                    return Err(Problem::bad_request(format!(
                        "{} does not match the stored upload",
                        a.id
                    )));
                }
                Ok(stored.clone())
            }
            None if author.is_local_to(authority) => Err(Problem::bad_request(format!(
                "{} is not an upload of this provider",
                a.id
            ))),
            None => foreign(a),
        })
        .collect()
}

/// An attachment hosted elsewhere, from a remote author.
fn foreign(mut a: Attachment) -> Outcome<Attachment> {
    if !a.url.starts_with("https://") {
        return Err(Problem::bad_request("an attachment url is not https"));
    }
    a.expires_at = None;
    if let Some(t) = &mut a.thumbnail {
        if !t.url.starts_with("https://") {
            return Err(Problem::bad_request("a thumbnail url is not https"));
        }
        t.expires_at = None;
    }
    Ok(a)
}
