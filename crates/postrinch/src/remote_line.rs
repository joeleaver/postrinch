//! The line under a search's heading about the servers' part
//! ([`crate::remote_search`]): who is being asked, what they found, and
//! why a server could not search.

use crate::remote_search::{Asked, RemoteSearch, RemoteStatus};

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
        RemoteStatus::Found { count, incomplete } => {
            let found = match count {
                0 => format!("{who} found nothing more in mail not downloaded yet"),
                n => format!("{who} found {n} more in mail not downloaded yet"),
            };
            match incomplete {
                Some(why) => format!("{found}; {why}."),
                None => format!("{found}."),
            }
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

#[cfg(test)]
mod tests {
    use super::*;

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
            incomplete: None,
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
            incomplete: Some("1 folder was not searched (Archive is locked)".into()),
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
}
