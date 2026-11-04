//! A message being written: by the person, or proposed by an agent for the
//! person to send.

use serde::{Deserialize, Serialize};

use crate::{EmailAddress, Mailbox, ProviderMessageId, ProviderThreadId};

/// What a draft answers, so it threads where it belongs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answering {
    /// The provider's thread, for the provider to file the reply in.
    pub thread: Option<ProviderThreadId>,
    /// `In-Reply-To`: the answered message's `Message-ID`, without brackets.
    pub in_reply_to: Option<String>,
    /// `References`, oldest first, without brackets.
    pub references: Vec<String>,
}

/// An agent's part in a draft: it wrote it, the person sends it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    /// The agent's name, as Settings › Agents shows it.
    pub agent: String,
    /// Why it wrote it, in its own words.
    pub reason: Option<String>,
}

/// A message being written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    /// The account it goes from.
    pub account: EmailAddress,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub subject: String,
    /// Plain text, quoted history included.
    pub body: String,
    pub answering: Answering,
    /// The provider's draft this one replaces, when it was opened from one.
    pub replaces: Option<ProviderMessageId>,
    pub proposal: Option<Proposal>,
}

impl Draft {
    /// An empty message from `account`.
    pub fn new(account: EmailAddress) -> Self {
        Self {
            account,
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: String::new(),
            body: String::new(),
            answering: Answering::default(),
            replaces: None,
            proposal: None,
        }
    }

    /// Everyone it goes to.
    pub fn recipients(&self) -> impl Iterator<Item = &Mailbox> {
        self.to.iter().chain(&self.cc).chain(&self.bcc)
    }
}

/// Why an address field does not read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not an email address")]
pub struct NotAnAddress(pub String);

/// Read an address field as a person types it: `Ada <ada@example.org>,
/// bob@example.org; "Smith, C" <c@example.org>`. Commas and semicolons
/// separate addresses outside quotes and angle brackets.
pub fn parse_mailboxes(text: &str) -> Result<Vec<Mailbox>, NotAnAddress> {
    let mut out = Vec::new();
    let (mut part, mut quoted, mut angled) = (String::new(), false, false);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '<' if !quoted => angled = true,
            '>' if !quoted => angled = false,
            ',' | ';' if !quoted && !angled => {
                out.extend(one(&part)?);
                part.clear();
                continue;
            }
            _ => {}
        }
        part.push(c);
    }
    out.extend(one(&part)?);
    Ok(out)
}

/// One `Name <address>` or bare address; nothing for blank text.
fn one(text: &str) -> Result<Option<Mailbox>, NotAnAddress> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let bad = || NotAnAddress(text.to_owned());
    let (name, address) = match (text.rfind('<'), text.rfind('>')) {
        (Some(open), Some(close)) if open < close => {
            let name = text[..open].trim().trim_matches('"').trim();
            (Some(name.to_owned()), &text[open + 1..close])
        }
        _ => (None, text),
    };
    let address = EmailAddress::parse(address.trim()).map_err(|_| bad())?;
    if address.domain().is_empty()
        || !address.domain().contains('.') && address.domain() != "localhost"
    {
        return Err(bad());
    }
    Ok(Some(Mailbox::new(name, address)))
}

/// Mailboxes as an address field shows them.
pub fn format_mailboxes(mailboxes: &[Mailbox]) -> String {
    mailboxes
        .iter()
        .map(|m| match &m.name {
            Some(name) if name.contains([',', ';', '"']) => {
                format!("\"{}\" <{}>", name.replace('"', ""), m.address)
            }
            _ => m.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_people_type() {
        let list = parse_mailboxes(
            "Ada Lovelace <ada@example.org>, bob@example.org; \"Smith, C\" <c@example.org>,",
        )
        .unwrap();
        let shown: Vec<String> = list.iter().map(ToString::to_string).collect();
        assert_eq!(
            shown,
            [
                "Ada Lovelace <ada@example.org>",
                "bob@example.org",
                "Smith, C <c@example.org>"
            ]
        );
        assert_eq!(
            format_mailboxes(&list),
            "Ada Lovelace <ada@example.org>, bob@example.org, \"Smith, C\" <c@example.org>"
        );
        assert_eq!(parse_mailboxes("  ").unwrap(), []);
        assert!(parse_mailboxes("ada@example.org, nobody").is_err());
        assert!(parse_mailboxes("ada@nowhere").is_err());
    }
}
