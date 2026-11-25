//! A folder as LIST shows it: its name on the server, its delimiter, and
//! its attributes, RFC 6154's special uses among them.

use async_imap::types::{Name, NameAttribute};

/// An RFC 6154 special use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialUse {
    All,
    Archive,
    Drafts,
    Flagged,
    Junk,
    Sent,
    Trash,
}

/// One folder from LIST.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedFolder {
    /// The full name as the server spells it (modified UTF-7, with the
    /// server's delimiter).
    pub name: String,
    /// The hierarchy delimiter; `None` for a flat namespace.
    pub delimiter: Option<char>,
    pub special_use: Option<SpecialUse>,
    /// Whether it can be selected: not `\Noselect` or `\NonExistent`.
    pub selectable: bool,
    /// Whether folders can be made under it: not `\Noinferiors`.
    pub may_have_children: bool,
}

impl ListedFolder {
    pub(crate) fn from_name(name: &Name) -> Self {
        let mut special_use = None;
        let mut selectable = true;
        let mut may_have_children = true;
        for attribute in name.attributes() {
            match attribute {
                NameAttribute::NoSelect => selectable = false,
                NameAttribute::NoInferiors => may_have_children = false,
                NameAttribute::All => special_use = Some(SpecialUse::All),
                NameAttribute::Archive => special_use = Some(SpecialUse::Archive),
                NameAttribute::Drafts => special_use = Some(SpecialUse::Drafts),
                NameAttribute::Flagged => special_use = Some(SpecialUse::Flagged),
                NameAttribute::Junk => special_use = Some(SpecialUse::Junk),
                NameAttribute::Sent => special_use = Some(SpecialUse::Sent),
                NameAttribute::Trash => special_use = Some(SpecialUse::Trash),
                // RFC 5258: a name that does not exist is not selectable
                // (and implies \Noselect, which some servers leave out).
                NameAttribute::Extension(other) if other.eq_ignore_ascii_case("\\NonExistent") => {
                    selectable = false;
                }
                _ => {}
            }
        }
        Self {
            name: name.name().to_owned(),
            delimiter: name.delimiter().and_then(|d| d.chars().next()),
            special_use,
            selectable,
            may_have_children,
        }
    }
}
