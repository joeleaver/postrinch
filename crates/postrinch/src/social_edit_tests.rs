use postrinch_commands::{BuiltInView, SocialViewEntry};
use postrinch_store::{ContentKind, GroupRef, PlaceRef, ReadAs};

use super::*;

fn own(name: &str) -> SocialViewEntry {
    let mut view = SocialEdit::new_view().draft;
    view.id = "v1".into();
    view.name = name.into();
    SocialViewEntry {
        view,
        built_in: None,
        original: None,
    }
}

fn home() -> SocialViewEntry {
    let original = BuiltInView::Home.original();
    SocialViewEntry {
        view: original.clone(),
        built_in: Some(BuiltInView::Home),
        original: Some(original),
    }
}

fn place(account: &str, id: &str) -> PlaceRef {
    PlaceRef {
        account: account.into(),
        id: id.into(),
    }
}

#[test]
fn a_view_opens_as_it_is_and_nothing_is_changed_yet() {
    let edit = SocialEdit::of(&own("Rust Audio"));
    assert!(!edit.changed());
    assert!(edit.can_save());
    assert!(edit.is_of("v1"));
    assert!(!edit.is_new());
}

#[test]
fn a_new_view_needs_a_name_to_be_saved() {
    let mut edit = SocialEdit::new_view();
    assert!(edit.is_new());
    assert!(!edit.is_of(""));
    assert!(!edit.can_save());
    edit.draft.name = "   ".into();
    assert!(!edit.can_save());
    edit.draft.name = "Spec work".into();
    assert!(edit.can_save());
    assert!(edit.changed());
}

#[test]
fn every_kind_shows_until_one_is_unticked() {
    let mut edit = SocialEdit::of(&own("v"));
    assert!(KINDS.iter().all(|&k| edit.shows(k)));
    edit.toggle_kind(ContentKind::Chat);
    assert_eq!(
        edit.draft.filters.kinds,
        [ContentKind::Memo, ContentKind::Article]
    );
    assert!(!edit.shows(ContentKind::Chat));
}

#[test]
fn ticking_every_kind_again_names_none_so_new_kinds_show_too() {
    let mut edit = SocialEdit::of(&own("v"));
    edit.toggle_kind(ContentKind::Memo);
    edit.toggle_kind(ContentKind::Memo);
    assert!(edit.draft.filters.kinds.is_empty());
    assert!(!edit.changed());
}

#[test]
fn the_last_kind_ticked_stays_ticked() {
    let mut edit = SocialEdit::of(&own("v"));
    edit.toggle_kind(ContentKind::Chat);
    edit.toggle_kind(ContentKind::Memo);
    assert_eq!(edit.draft.filters.kinds, [ContentKind::Article]);
    edit.toggle_kind(ContentKind::Article);
    assert_eq!(edit.draft.filters.kinds, [ContentKind::Article]);
}

#[test]
fn kinds_are_kept_in_shows_order_whatever_order_they_are_ticked_in() {
    let mut edit = SocialEdit::of(&own("v"));
    edit.toggle_kind(ContentKind::Chat);
    edit.toggle_kind(ContentKind::Memo);
    edit.toggle_kind(ContentKind::Chat);
    assert_eq!(
        edit.draft.filters.kinds,
        [ContentKind::Chat, ContentKind::Article]
    );
}

#[test]
fn a_place_or_group_is_added_once_whatever_the_accounts_spelling() {
    let mut edit = SocialEdit::of(&own("v"));
    edit.add_place(place("ada@groups.example", "c1"));
    edit.add_place(place("Ada@Groups.Example", "c1"));
    assert_eq!(edit.draft.places.len(), 1);
    let group = GroupRef {
        account: "ada@groups.example".into(),
        uri: "https://groups.example/api/groups/g1".into(),
    };
    edit.add_group(group.clone());
    edit.add_group(group.clone());
    assert_eq!(edit.draft.groups.len(), 1);
    assert!(edit.has_group(&group));
    edit.remove_group(&group);
    edit.remove_place(&place("ADA@groups.example", "c1"));
    assert!(edit.draft.places.is_empty() && edit.draft.groups.is_empty());
}

#[test]
fn the_persons_view_is_deleted_and_a_built_in_one_put_back_only_when_changed() {
    assert_eq!(SocialEdit::of(&own("v")).removal(), Some(Removal::Delete));
    assert_eq!(SocialEdit::new_view().removal(), None);
    assert_eq!(SocialEdit::of(&home()).removal(), None);
    let mut changed = home();
    changed.view.read_as = ReadAs::Stream;
    assert_eq!(SocialEdit::of(&changed).removal(), Some(Removal::PutBack));
    assert_eq!(Removal::Delete.label(), "Delete view");
    assert_eq!(Removal::PutBack.label(), "Back to the original");
}

#[test]
fn filters_narrow_only_views_that_show_messages() {
    assert!(SocialEdit::of(&own("v")).filters_apply());
    assert!(SocialEdit::of(&home()).filters_apply());
    let drafts = BuiltInView::Drafts.original();
    let edit = SocialEdit::of(&SocialViewEntry {
        view: drafts.clone(),
        built_in: Some(BuiltInView::Drafts),
        original: Some(drafts),
    });
    assert!(!edit.filters_apply());
}
