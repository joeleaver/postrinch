//! Finding a system label among an account's labels, as searches, views and
//! lists name it.

use postrinch_model::{Label, LabelKind, SystemLabel};

/// The label `system` names among one account's `labels`, if any.
///
/// Gmail sorts its inbox into categories; an account without them (an IMAP
/// account) has its whole inbox as its Primary, so there
/// [`SystemLabel::CategoryPersonal`] names the inbox and `in:inbox
/// category:primary` is the whole inbox (docs/imap.md, "One model, two
/// providers"). The other categories name nothing in such an account.
pub(crate) fn system_label_in(labels: &[Label], system: SystemLabel) -> Option<&Label> {
    let find = |s: SystemLabel| labels.iter().find(|l| l.kind == LabelKind::System(s));
    find(system).or_else(|| {
        let uncategorised = system == SystemLabel::CategoryPersonal
            && !labels
                .iter()
                .any(|l| matches!(l.kind, LabelKind::System(k) if k.is_category()));
        if uncategorised {
            find(SystemLabel::Inbox)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(label: SystemLabel) -> Label {
        Label::new(
            label.provider_id(),
            label.provider_id(),
            LabelKind::System(label),
        )
    }

    #[test]
    fn primary_is_the_category_where_there_are_categories() {
        let labels = [
            system(SystemLabel::Inbox),
            system(SystemLabel::CategoryPersonal),
            system(SystemLabel::CategoryUpdates),
        ];
        let found = system_label_in(&labels, SystemLabel::CategoryPersonal).unwrap();
        assert_eq!(found.provider_id, "CATEGORY_PERSONAL");
    }

    #[test]
    fn primary_is_the_inbox_without_categories() {
        let labels = [
            system(SystemLabel::Inbox),
            system(SystemLabel::Sent),
            system(SystemLabel::Trash),
        ];
        let found = system_label_in(&labels, SystemLabel::CategoryPersonal).unwrap();
        assert_eq!(found.provider_id, "INBOX");
        assert_eq!(system_label_in(&labels, SystemLabel::CategoryUpdates), None);
        assert_eq!(
            system_label_in(&labels, SystemLabel::Sent)
                .unwrap()
                .provider_id,
            "SENT"
        );
    }

    #[test]
    fn a_missing_primary_beside_other_categories_names_nothing() {
        let labels = [
            system(SystemLabel::Inbox),
            system(SystemLabel::CategoryUpdates),
        ];
        assert_eq!(
            system_label_in(&labels, SystemLabel::CategoryPersonal),
            None
        );
    }
}
