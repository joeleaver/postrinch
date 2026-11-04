//! A draft as an RFC 5322 message, ready for a provider.

use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use postrinch_model::{Draft, Mailbox};

fn address(mailbox: &Mailbox) -> Address<'static> {
    match &mailbox.name {
        Some(name) => Address::new_address(Some(name.clone()), mailbox.address.to_string()),
        None => Address::new_address(None::<String>, mailbox.address.to_string()),
    }
}

fn list(mailboxes: &[Mailbox]) -> Address<'static> {
    Address::new_list(mailboxes.iter().map(address).collect())
}

/// The message a draft makes, from `from_name` at its account, with
/// `message_id` (without brackets) as its `Message-ID`.
pub fn build_message(draft: &Draft, from_name: Option<&str>, message_id: &str) -> Vec<u8> {
    let from = Mailbox::new(from_name.map(str::to_owned), draft.account.clone());
    let mut message = MessageBuilder::new()
        .from(address(&from))
        .subject(draft.subject.clone())
        .message_id(message_id.to_owned())
        .text_body(draft.body.replace("\r\n", "\n"));
    if !draft.to.is_empty() {
        message = message.to(list(&draft.to));
    }
    if !draft.cc.is_empty() {
        message = message.cc(list(&draft.cc));
    }
    // Gmail delivers to Bcc and takes the header out of what others get.
    if !draft.bcc.is_empty() {
        message = message.bcc(list(&draft.bcc));
    }
    if let Some(reply_to) = &draft.answering.in_reply_to {
        message = message.in_reply_to(reply_to.clone());
    }
    if !draft.answering.references.is_empty() {
        message = message.references(draft.answering.references.clone());
    }
    message
        .write_to_vec()
        .expect("writing to memory cannot fail")
}

#[cfg(test)]
mod tests {
    use postrinch_model::{EmailAddress, ProviderThreadId};

    use super::*;

    fn mailbox(name: Option<&str>, address: &str) -> Mailbox {
        Mailbox::new(
            name.map(str::to_owned),
            EmailAddress::parse(address).unwrap(),
        )
    }

    #[test]
    fn a_reply_reads_back_as_written() {
        let mut draft = Draft::new(EmailAddress::parse("ada@example.org").unwrap());
        draft.to = vec![mailbox(Some("Michael Faraday"), "michael@example.org")];
        draft.cc = vec![mailbox(Some("Somerville, Mary"), "mary@example.org")];
        draft.bcc = vec![mailbox(None, "charles@example.org")];
        draft.subject = "Re: Thursday — the rota".into();
        draft.body = "Thanks, that settles it.\n\n> I can take Thursday.".into();
        draft.answering.in_reply_to = Some("m1@example.org".into());
        draft.answering.references = vec!["m0@example.org".into(), "m1@example.org".into()];
        draft.answering.thread = Some(ProviderThreadId("t1".into()));

        let raw = build_message(&draft, Some("Ada Lovelace"), "out1@example.org");
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.subject(), Some("Re: Thursday — the rota"));
        assert_eq!(parsed.message_id(), Some("out1@example.org"));
        assert_eq!(
            parsed.in_reply_to().and_then(|r| r.first()),
            Some("m1@example.org")
        );
        let from = parsed.from().unwrap().first().unwrap();
        assert_eq!(
            (from.name(), from.address()),
            (Some("Ada Lovelace"), Some("ada@example.org"))
        );
        let cc = parsed.cc().unwrap().first().unwrap();
        assert_eq!(cc.name(), Some("Somerville, Mary"));
        assert_eq!(
            parsed.bcc().unwrap().first().unwrap().address(),
            Some("charles@example.org")
        );
        let body = parsed.body_text(0).unwrap().replace("\r\n", "\n");
        assert_eq!(body.trim_end(), draft.body);
    }
}
