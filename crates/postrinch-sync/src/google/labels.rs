//! Gmail's labels, mapped onto the model's.
//!
//! Gmail models read and starred as the `UNREAD` and `STARRED` labels; the
//! model has them as flags. Gmail's other system labels that the model has
//! no place for (`CHAT` and the coloured stars) are not stored, and are
//! dropped from the messages that carry them.

use std::collections::HashSet;

use postrinch_model::{Flags, Label, LabelColor, LabelKind, LabelVisibility, SystemLabel};

#[cfg(test)]
use super::api::LabelColorResource;
use super::api::LabelResource;

const UNREAD: &str = "UNREAD";
const STARRED: &str = "STARRED";

fn system_label(id: &str) -> Option<SystemLabel> {
    Some(match id {
        "INBOX" => SystemLabel::Inbox,
        "SENT" => SystemLabel::Sent,
        "DRAFT" => SystemLabel::Drafts,
        "SPAM" => SystemLabel::Spam,
        "TRASH" => SystemLabel::Trash,
        "IMPORTANT" => SystemLabel::Important,
        "CATEGORY_PERSONAL" => SystemLabel::CategoryPersonal,
        "CATEGORY_SOCIAL" => SystemLabel::CategorySocial,
        "CATEGORY_PROMOTIONS" => SystemLabel::CategoryPromotions,
        "CATEGORY_UPDATES" => SystemLabel::CategoryUpdates,
        "CATEGORY_FORUMS" => SystemLabel::CategoryForums,
        _ => return None,
    })
}

/// The model's label for a Gmail label, or `None` for one it does not keep.
pub(crate) fn to_label(resource: &LabelResource) -> Option<Label> {
    let kind = if resource.kind == "user" {
        LabelKind::User
    } else {
        LabelKind::System(system_label(&resource.id)?)
    };
    let mut label = Label::new(resource.id.clone(), resource.name.clone(), kind);
    label.visibility = match resource.list_visibility.as_deref() {
        Some("labelHide") => LabelVisibility::Hide,
        Some("labelShowIfUnread") => LabelVisibility::ShowIfUnread,
        _ => LabelVisibility::Show,
    };
    label.color = resource.color.as_ref().map(|c| LabelColor {
        background: c.background_color.clone(),
        text: c.text_color.clone(),
    });
    Some(label)
}

/// Split a message's Gmail label ids into the labels the store keeps and
/// the flags. `stored` is the ids of the account's stored labels.
pub(crate) fn split_label_ids(ids: &[String], stored: &HashSet<String>) -> (Vec<String>, Flags) {
    let flags = Flags {
        unread: ids.iter().any(|id| id == UNREAD),
        starred: ids.iter().any(|id| id == STARRED),
    };
    let labels = ids
        .iter()
        .filter(|id| stored.contains(*id))
        .cloned()
        .collect();
    (labels, flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::api::LabelColorResource;

    fn resource(id: &str, name: &str, kind: &str) -> LabelResource {
        LabelResource {
            id: id.into(),
            name: name.into(),
            kind: kind.into(),
            list_visibility: None,
            color: None,
        }
    }

    #[test]
    fn keeps_visibility_and_colour() {
        let mut hidden = resource("Label_3", "[Superhuman]/Muted", "user");
        hidden.list_visibility = Some("labelHide".into());
        hidden.color = Some(LabelColorResource {
            text_color: "#ffffff".into(),
            background_color: "#16a766".into(),
        });
        let label = to_label(&hidden).unwrap();
        assert_eq!(label.visibility, LabelVisibility::Hide);
        assert_eq!(label.color.unwrap().background, "#16a766");

        let mut unread = resource("Label_4", "Later", "user");
        unread.list_visibility = Some("labelShowIfUnread".into());
        assert_eq!(
            to_label(&unread).unwrap().visibility,
            LabelVisibility::ShowIfUnread
        );
        assert_eq!(
            to_label(&resource("INBOX", "INBOX", "system"))
                .unwrap()
                .visibility,
            LabelVisibility::Show
        );
    }

    #[test]
    fn maps_system_user_and_skipped_labels() {
        let inbox = to_label(&resource("INBOX", "INBOX", "system")).unwrap();
        assert_eq!(inbox.kind, LabelKind::System(SystemLabel::Inbox));
        let user = to_label(&resource("Label_3", "Work/Clients", "user")).unwrap();
        assert_eq!(user.kind, LabelKind::User);
        assert_eq!(to_label(&resource("CHAT", "CHAT", "system")), None);
        assert_eq!(to_label(&resource("UNREAD", "UNREAD", "system")), None);
    }

    #[test]
    fn flags_come_out_of_labels() {
        let stored: HashSet<String> = ["INBOX", "Label_3"].map(String::from).into();
        let ids = ["INBOX", "UNREAD", "STARRED", "CHAT", "Label_3"].map(String::from);
        let (labels, flags) = split_label_ids(&ids, &stored);
        assert_eq!(labels, ["INBOX", "Label_3"]);
        assert_eq!(
            flags,
            Flags {
                unread: true,
                starred: true
            }
        );
    }
}
