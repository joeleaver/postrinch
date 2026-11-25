//! The line under a search's heading about the servers' part
//! ([`crate::remote_search`]): who is being asked, what they found, and
//! why a server could not search.

use crate::remote_search::{Asked, Note, RemoteSearch, RemoteStatus, Trouble};

impl Asked {
    /// Who is asked, at the start of a sentence.
    fn name(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail",
            Self::Server => "The server",
            Self::Servers => "The servers",
        }
    }

    /// The one of them something happened to, at the start of a sentence.
    fn one(self) -> &'static str {
        match self {
            Self::Servers => "A server",
            asked => asked.name(),
        }
    }

    /// Who is asked, within a sentence.
    fn named(self) -> &'static str {
        match self {
            Self::Gmail => "Gmail",
            Self::Server => "the server",
            Self::Servers => "the servers",
        }
    }
}

/// The line under a search's heading about the servers' part, if it has
/// one.
pub fn remote_line(remote: Option<&RemoteSearch>) -> String {
    let Some(remote) = remote else {
        return String::new();
    };
    let messages = |n: usize| {
        if n == 1 {
            "1 message".to_owned()
        } else {
            format!("{} messages", crate::sync::thousands(n))
        }
    };
    let (who, whom) = (remote.asked.name(), remote.asked.named());
    let here_only = |n: usize| {
        format!(
            "mail not downloaded yet ({}) is searched by subject and sender only.",
            messages(n)
        )
    };
    match &remote.status {
        RemoteStatus::Asking { waiting } => format!(
            "Asking {whom} too: {} here are not downloaded yet\u{2026}",
            messages(*waiting)
        ),
        RemoteStatus::Found { count, notes } => {
            let found = match count {
                0 => format!("{who} found nothing more in mail not downloaded yet"),
                n => format!("{who} found {n} more in mail not downloaded yet"),
            };
            let several = remote.asked == Asked::Servers;
            let notes: Vec<String> = notes.iter().map(|n| note(n, several)).collect();
            if notes.is_empty() {
                format!("{found}.")
            } else {
                format!("{found}; {}.", notes.join("; "))
            }
        }
        RemoteStatus::Unanswered { waiting, notes } => {
            let notes: Vec<String> = notes.iter().map(|n| note(n, true)).collect();
            format!(
                "No server could search, so {} ({}).",
                here_only(*waiting).trim_end_matches('.'),
                notes.join("; ")
            )
        }
        RemoteStatus::Offline { waiting } => format!("Offline: {}", here_only(*waiting)),
        RemoteStatus::Busy { waiting } => {
            format!("{} is busy, so {}", remote.asked.one(), here_only(*waiting))
        }
        RemoteStatus::Refused(why) => format!(
            "{} could not search ({why}), so these are the results from mail here.",
            remote.asked.one()
        ),
        RemoteStatus::Failed(why) => format!("{who} could not be asked: {why}"),
    }
}

/// What one account's server left out, or why it did not search, within
/// a sentence; named by its account when several are asked.
fn note(note: &Note, several: bool) -> String {
    let account = &note.account;
    match (&note.what, several) {
        (Trouble::Incomplete(why), false) => why.clone(),
        (Trouble::Incomplete(why), true) => format!("for {account}, {why}"),
        (Trouble::Offline, _) => format!("{account}'s server could not be reached"),
        (Trouble::Busy, _) => format!("{account}'s server is busy"),
        (Trouble::Refused(why), _) => format!("{account}'s server could not search ({why})"),
        (Trouble::Failed(why), _) => format!("{account} could not be asked ({why})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(account: &str, what: Trouble) -> Note {
        Note {
            account: postrinch_model::EmailAddress::parse(account).unwrap(),
            what,
        }
    }

    fn line(asked: Asked, status: RemoteStatus) -> String {
        remote_line(Some(&RemoteSearch {
            query: "rota".into(),
            asked,
            status,
            rows: Vec::new(),
        }))
    }

    #[test]
    fn the_line_names_who_is_asked() {
        let asking = RemoteStatus::Asking { waiting: 1200 };
        assert_eq!(
            line(Asked::Gmail, asking.clone()),
            "Asking Gmail too: 1,200 messages here are not downloaded yet\u{2026}"
        );
        assert_eq!(
            line(Asked::Server, asking.clone()),
            "Asking the server too: 1,200 messages here are not downloaded yet\u{2026}"
        );
        assert!(line(Asked::Servers, asking).starts_with("Asking the servers too"));
        let found = |count| RemoteStatus::Found {
            count,
            notes: Vec::new(),
        };
        assert_eq!(
            line(Asked::Gmail, found(0)),
            "Gmail found nothing more in mail not downloaded yet."
        );
        assert_eq!(
            line(Asked::Server, found(3)),
            "The server found 3 more in mail not downloaded yet."
        );
        assert_eq!(remote_line(None), "");
    }

    #[test]
    fn the_line_says_what_was_not_searched_and_why() {
        let partly = RemoteStatus::Found {
            count: 1,
            notes: vec![note(
                "ada@example.org",
                Trouble::Incomplete("1 folder was not searched (Archive is locked)".into()),
            )],
        };
        assert_eq!(
            line(Asked::Server, partly),
            "The server found 1 more in mail not downloaded yet; \
             1 folder was not searched (Archive is locked)."
        );
        assert_eq!(
            line(Asked::Server, RemoteStatus::Busy { waiting: 1 }),
            "The server is busy, so mail not downloaded yet (1 message) is \
             searched by subject and sender only."
        );
        assert!(
            line(Asked::Servers, RemoteStatus::Busy { waiting: 2 }).starts_with("A server is busy")
        );
        let refused =
            RemoteStatus::Refused("it cannot search for text that is not plain ASCII".into());
        assert_eq!(
            line(Asked::Server, refused.clone()),
            "The server could not search (it cannot search for text that is not \
             plain ASCII), so these are the results from mail here."
        );
        assert!(line(Asked::Servers, refused).starts_with("A server could not search"));
        assert_eq!(
            line(Asked::Gmail, RemoteStatus::Offline { waiting: 2 }),
            "Offline: mail not downloaded yet (2 messages) is searched by subject \
             and sender only."
        );
    }

    #[test]
    fn each_accounts_trouble_stays_on_the_line() {
        // One account found mail; the other's server was busy, and a third
        // searched all but one folder.
        let found = RemoteStatus::Found {
            count: 2,
            notes: vec![
                note("ada@example.org", Trouble::Busy),
                note(
                    "bob@example.org",
                    Trouble::Incomplete("1 folder was not searched (Archive is locked)".into()),
                ),
                note("cy@example.org", Trouble::Refused("no ASCII".into())),
            ],
        };
        assert_eq!(
            line(Asked::Servers, found),
            "The servers found 2 more in mail not downloaded yet; \
             ada@example.org's server is busy; for bob@example.org, 1 folder was \
             not searched (Archive is locked); cy@example.org's server could not \
             search (no ASCII)."
        );
        let none = RemoteStatus::Unanswered {
            waiting: 3,
            notes: vec![
                note("ada@example.org", Trouble::Busy),
                note("bob@example.org", Trouble::Offline),
            ],
        };
        assert_eq!(
            line(Asked::Servers, none),
            "No server could search, so mail not downloaded yet (3 messages) is \
             searched by subject and sender only (ada@example.org's server is busy; \
             bob@example.org's server could not be reached)."
        );
    }
}
