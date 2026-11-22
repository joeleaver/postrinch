//! The filters, read from and written into the query text.

use super::*;

#[test]
fn archived_mail_is_in_inbox_or_not() {
    let read = |q: &str| Filters::read(q).archived;
    assert_eq!(read("label:clients"), Archived::Include);
    assert_eq!(read("in:inbox category:primary"), Archived::LeaveOut);
    assert_eq!(read("label:clients -in:inbox"), Archived::Only);
    let mut f = Filters::read("in:inbox category:primary");
    f.archived = Archived::Only;
    assert_eq!(
        f.apply("in:inbox category:primary"),
        "-in:inbox category:primary"
    );
    f.archived = Archived::Include;
    assert_eq!(f.apply("in:inbox category:primary"), "category:primary");
    // Other folders stay the person's own.
    assert_eq!(Filters::read("in:sent").archived, Archived::Include);
    assert_eq!(Filters::default().apply("in:sent"), "in:sent");
}

#[test]
fn filters_round_trip_through_the_text() {
    let query = r#"cover shift from:"Ada Lovelace" is:unread in:inbox"#;
    let mut filters = Filters::read(query);
    assert_eq!(filters.from, ["Ada Lovelace"]);
    assert!(filters.unread);
    filters.unread = false;
    filters.within = Some("1m".into());
    filters.labels = vec!["Work/Clients".into()];
    assert_eq!(
        filters.apply(query),
        r#"cover shift newer_than:1m from:"Ada Lovelace" in:inbox label:Work/Clients"#
    );
    assert_eq!(Filters::default().apply(query), "cover shift");
}

#[test]
fn labels_are_any_of_and_last_is_kept() {
    let mut filters = Filters::default();
    filters.pick_label("Clients/Acme", Some(LabelPick::Has));
    filters.pick_label("Receipts 2024", Some(LabelPick::Has));
    filters.last = Some(Last::Me);
    filters.attachments = true;
    let query = filters.apply("has:attachment");
    assert_eq!(
        query,
        r#"{label:Clients/Acme label:"Receipts 2024"} has:attachment last:me"#
    );
    let read = Filters::read(&query);
    assert_eq!(read.labels, ["Clients/Acme", "Receipts 2024"]);
    assert!(read.attachments && read.label("clients-acme") == Some(LabelPick::Has));
    assert!(!read.all_labels);
    assert_eq!(read.last, Some(Last::Me));
    filters.pick_label("clients/acme", None);
    assert_eq!(filters.labels, ["Receipts 2024"]);
    // A group of the person's own stays.
    let theirs = "{from:ann label:y} label:x";
    assert_eq!(Filters::default().apply(theirs), "{from:ann label:y}");
}

#[test]
fn senders_and_recipients_any_of_or_not() {
    let query = "invoice {from:ann from:\"Bob Smith\"} -from:noreply to:me@x.org";
    let mut f = Filters::read(query);
    assert_eq!(f.from, ["ann", "Bob Smith"]);
    assert_eq!(
        (f.not_from.as_slice(), f.to.as_slice()),
        (&["noreply".to_owned()][..], &["me@x.org".to_owned()][..])
    );
    assert_eq!(f.apply(query), query);
    f.from.retain(|p| p != "ann");
    f.not_to.push("list@x.org".into());
    assert_eq!(
        f.apply(query),
        "invoice from:\"Bob Smith\" -from:noreply to:me@x.org -to:list@x.org"
    );
    // Groups of each kind are read side by side.
    let both = Filters::read("{label:a label:b} {to:c to:d}");
    assert_eq!((both.labels.len(), both.to.len()), (2, 2));
    assert_eq!(
        Filters::default().apply("{label:a label:b} {to:c to:d} word"),
        "word"
    );
}

#[test]
fn labels_all_of_and_left_out() {
    let query = "invoice label:Clients label:Receipts -label:Newsletters";
    let mut filters = Filters::read(query);
    assert_eq!(filters.labels, ["Clients", "Receipts"]);
    assert!(filters.all_labels);
    assert_eq!(filters.label("newsletters"), Some(LabelPick::Not));
    assert_eq!(filters.apply(query), query);
    filters.all_labels = false;
    filters.pick_label("Receipts", Some(LabelPick::Not));
    assert_eq!(
        filters.apply(query),
        "invoice label:Clients -label:Newsletters -label:Receipts"
    );
    filters.pick_label("Old", Some(LabelPick::Has));
    assert_eq!(
        filters.apply(query),
        "invoice {label:Clients label:Old} -label:Newsletters -label:Receipts"
    );
    // A label beside a group is the person's own, and stays.
    let theirs = "{label:a label:b} label:c -label:d";
    let read = Filters::read(theirs);
    assert_eq!((read.labels.len(), read.without.len()), (2, 1));
    assert_eq!(Filters::default().apply(theirs), "label:c");
}

#[test]
fn categories_any_of_or_not() {
    let query = "in:inbox category:personal -category:promotions";
    let mut f = Filters::read(query);
    assert_eq!(f.categories, ["primary"]);
    assert_eq!(f.category("promotions"), Some(LabelPick::Not));
    assert_eq!(
        f.apply(query),
        "in:inbox category:primary -category:promotions"
    );
    f.pick_category("updates", Some(LabelPick::Has));
    f.pick_category("promotions", None);
    let written = f.apply(query);
    assert_eq!(written, "in:inbox {category:primary category:updates}");
    assert_eq!(Filters::read(&written).categories, ["primary", "updates"]);
}
