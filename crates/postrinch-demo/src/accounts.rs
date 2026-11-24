//! The two accounts and their labels, as Gmail lists them: its system
//! labels (ids and names alike, `INBOX`, `CATEGORY_SOCIAL`…) and each
//! account's own, ids as Gmail makes them (`Label_12`). Labels nest by
//! name (`Home/Bills`); a few carry a colour, as tags do.

use postrinch_model::{Account, Label, LabelColor, LabelKind, Provider, SystemLabel};
use postrinch_store::Store;

use crate::DemoResult;
use crate::people::{Person, SAM_HOME, SAM_WORK};

pub const INBOX: &str = "INBOX";
pub const SENT: &str = "SENT";
pub const IMPORTANT: &str = "IMPORTANT";
pub const PRIMARY: &str = "CATEGORY_PERSONAL";
pub const UPDATES: &str = "CATEGORY_UPDATES";
pub const PROMOTIONS: &str = "CATEGORY_PROMOTIONS";
pub const SOCIAL: &str = "CATEGORY_SOCIAL";
pub const FORUMS: &str = "CATEGORY_FORUMS";

const SYSTEM: [(&str, SystemLabel); 11] = [
    (INBOX, SystemLabel::Inbox),
    (SENT, SystemLabel::Sent),
    ("DRAFT", SystemLabel::Drafts),
    ("SPAM", SystemLabel::Spam),
    ("TRASH", SystemLabel::Trash),
    (IMPORTANT, SystemLabel::Important),
    (PRIMARY, SystemLabel::CategoryPersonal),
    (SOCIAL, SystemLabel::CategorySocial),
    (PROMOTIONS, SystemLabel::CategoryPromotions),
    (UPDATES, SystemLabel::CategoryUpdates),
    (FORUMS, SystemLabel::CategoryForums),
];

/// Whether a label id is one of Gmail's inbox categories.
pub fn is_category(id: &str) -> bool {
    id.starts_with("CATEGORY_")
}

// sam@example.com's own labels.
pub const HOME: &str = "Label_11";
pub const BILLS: &str = "Label_12";
pub const GARDEN: &str = "Label_13";
pub const TRAVEL: &str = "Label_14";
pub const LISBON: &str = "Label_15";
pub const RECEIPTS: &str = "Label_16";
pub const FOLLOW_UP: &str = "Label_17";
pub const READ_LATER: &str = "Label_18";

// sam@lindenworks.example's own labels.
pub const CLIENTS: &str = "Label_21";
pub const HARBOR: &str = "Label_22";
pub const FERNHILL: &str = "Label_23";
pub const STUDIO: &str = "Label_24";
pub const HIRING: &str = "Label_25";
pub const INVOICES: &str = "Label_26";
pub const WAITING: &str = "Label_27";
pub const TO_REVIEW: &str = "Label_28";

/// A label's id, name and tag colour (background, text), if it has one.
type Own = (
    &'static str,
    &'static str,
    Option<(&'static str, &'static str)>,
);

const HOME_LABELS: [Own; 8] = [
    (HOME, "Home", None),
    (BILLS, "Home/Bills", None),
    (GARDEN, "Home/Garden", None),
    (TRAVEL, "Travel", None),
    (LISBON, "Travel/Lisbon", None),
    (RECEIPTS, "Receipts", None),
    (FOLLOW_UP, "Follow up", Some(("#fb4c2f", "#ffffff"))),
    (READ_LATER, "Read later", Some(("#16a766", "#ffffff"))),
];

const WORK_LABELS: [Own; 8] = [
    (CLIENTS, "Clients", None),
    (HARBOR, "Clients/Harbor Coffee", None),
    (FERNHILL, "Clients/Fernhill Bakery", None),
    (STUDIO, "Studio", None),
    (HIRING, "Studio/Hiring", None),
    (INVOICES, "Invoices", None),
    (WAITING, "Waiting", Some(("#ffad47", "#ffffff"))),
    (TO_REVIEW, "To review", Some(("#4a86e8", "#ffffff"))),
];

/// The two accounts, each with its labels.
pub fn accounts() -> [(Person, &'static [Own]); 2] {
    [(SAM_HOME, &HOME_LABELS), (SAM_WORK, &WORK_LABELS)]
}

/// Store both accounts and their labels. Returns how many accounts.
pub fn put(store: &Store) -> DemoResult<usize> {
    let accounts = accounts();
    for (person, own) in &accounts {
        let address = person.email();
        let mut account = Account::new(address.clone(), Provider::Google);
        account.display_name = Some(person.name.to_owned());
        store.put_account(&account)?;
        for (id, kind) in SYSTEM {
            store.put_label(&address, &Label::new(id, id, LabelKind::System(kind)))?;
        }
        for &(id, name, color) in own.iter() {
            let mut label = Label::new(id, name, LabelKind::User);
            label.color = color.map(|(background, text)| LabelColor {
                background: background.to_owned(),
                text: text.to_owned(),
            });
            store.put_label(&address, &label)?;
        }
    }
    Ok(accounts.len())
}
