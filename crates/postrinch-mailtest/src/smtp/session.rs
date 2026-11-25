//! One SMTP connection (RFC 5321 submission, with RFC 4954 AUTH and RFC
//! 3207 STARTTLS).

use std::io;
use std::sync::Arc;

use super::data::{self, Data};
use super::{ReceivedMessage, SmtpFault, SmtpShared, SmtpStage};
use crate::conn::Conn;
use crate::sasl::{self, Claim, Mechanism};

/// The envelope of the message being submitted.
#[derive(Default)]
struct Transaction {
    from: Option<String>,
    to: Vec<String>,
}

struct Session {
    shared: Arc<SmtpShared>,
    conn: Conn,
    greeted: bool,
    /// Greeted with a refusal other than 421: only QUIT is served.
    refusing: bool,
    user: Option<String>,
    transaction: Transaction,
}

/// What to do after a command.
enum Next {
    Continue,
    Close,
}

/// Serves one connection until the client quits or goes away.
pub(super) async fn run(shared: Arc<SmtpShared>, conn: Conn) {
    let mut session = Session {
        shared,
        conn,
        greeted: false,
        refusing: false,
        user: None,
        transaction: Transaction::default(),
    };
    // An I/O error ends the session as a closed connection would.
    let _ = session.serve().await;
    session.conn.close().await;
}

/// What follows a programmed reply: RFC 5321 3.8, a 421 closes the
/// connection; any other code leaves it open.
fn after_reply(code: u16) -> Next {
    if code == 421 {
        Next::Close
    } else {
        Next::Continue
    }
}

/// The address inside `FROM:<a@b> SIZE=10` or `TO:<a@b>`, and the rest.
fn path<'a>(args: &'a str, keyword: &str) -> Option<(String, &'a str)> {
    let head = args.get(..keyword.len())?;
    let rest = &args[keyword.len()..];
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('<')?;
    let end = rest.find('>')?;
    Some((rest[..end].to_string(), rest[end + 1..].trim()))
}

impl Session {
    /// Whether this port offers STARTTLS: the plain-text port does unless
    /// the test said otherwise.
    fn offers_starttls(&self) -> bool {
        !self.conn.is_tls() && self.shared.lock().config.offer_starttls
    }

    async fn reply(&mut self, code: u16, text: &str) -> io::Result<()> {
        self.conn
            .write_all(format!("{code} {text}\r\n").as_bytes())
            .await
    }

    async fn serve(&mut self) -> io::Result<()> {
        let fault = self.shared.lock().take_fault(SmtpStage::Greeting);
        match fault {
            None => self.reply(220, "localhost ESMTP fake ready").await?,
            Some(SmtpFault::Disconnect | SmtpFault::StoreThenDisconnect) => return Ok(()),
            Some(SmtpFault::Reply { code, text }) => {
                self.reply(code, &text).await?;
                if let Next::Close = after_reply(code) {
                    return Ok(());
                }
                self.refusing = true;
            }
        }
        while let Some(line) = self.conn.read_line().await? {
            let line = String::from_utf8_lossy(&line).into_owned();
            let (verb, args) = match line.split_once(' ') {
                Some((v, a)) => (v.to_ascii_uppercase(), a.trim().to_string()),
                None => (line.trim().to_ascii_uppercase(), String::new()),
            };
            let logged = if verb == "AUTH" {
                format!("AUTH {}", args.split(' ').next().unwrap_or_default())
            } else {
                line.clone()
            };
            self.shared.lock().log.push(logged);
            if let Next::Close = self.command(&verb, &args).await? {
                return Ok(());
            }
        }
        Ok(())
    }

    async fn command(&mut self, verb: &str, args: &str) -> io::Result<Next> {
        if self.refusing && verb != "QUIT" {
            // RFC 5321 3.1: after a 554 greeting, only QUIT is served.
            self.reply(503, "5.5.1 No SMTP service here").await?;
            return Ok(Next::Continue);
        }
        match verb {
            "EHLO" => {
                if let Some(next) = self.fault(SmtpStage::Ehlo).await? {
                    return Ok(next);
                }
                self.ehlo().await?
            }
            "HELO" => {
                self.greeted = true;
                self.transaction = Transaction::default();
                self.reply(250, "localhost").await?;
            }
            "STARTTLS" if self.conn.is_tls() => self.reply(503, "5.5.1 Already encrypted").await?,
            "STARTTLS" if !self.offers_starttls() => {
                self.reply(502, "5.5.1 Command not implemented").await?
            }
            "STARTTLS" => {
                if let Some(next) = self.fault(SmtpStage::StartTls).await? {
                    // Still in plain text, as before the command.
                    return Ok(next);
                }
                self.reply(220, "2.0.0 Ready to start TLS").await?;
                let acceptor = self.shared.acceptor.clone();
                self.conn.start_tls(&acceptor).await?;
                // RFC 3207 4.2: forget everything from before TLS.
                self.greeted = false;
                self.user = None;
                self.transaction = Transaction::default();
            }
            "AUTH" => return self.auth(args).await,
            "MAIL" => return self.mail(args).await,
            "RCPT" => return self.rcpt(args).await,
            "DATA" => return self.data().await,
            "RSET" => {
                self.transaction = Transaction::default();
                self.reply(250, "2.0.0 OK").await?;
            }
            "NOOP" => self.reply(250, "2.0.0 OK").await?,
            "VRFY" => self.reply(252, "2.5.0 Cannot verify").await?,
            "QUIT" => {
                self.reply(221, "2.0.0 Bye").await?;
                return Ok(Next::Close);
            }
            _ => self.reply(500, "5.5.2 Unknown command").await?,
        }
        Ok(Next::Continue)
    }

    async fn ehlo(&mut self) -> io::Result<()> {
        self.greeted = true;
        self.transaction = Transaction::default();
        let max = self.shared.lock().config.max_size;
        let mut lines = vec![
            "localhost".to_string(),
            "PIPELINING".to_string(),
            "8BITMIME".to_string(),
            format!("SIZE {max}"),
        ];
        if self.offers_starttls() {
            lines.push("STARTTLS".to_string());
        } else {
            let mechanisms = self.shared.lock().config.mechanisms.clone();
            let mut auth = vec!["AUTH"];
            for mechanism in mechanisms {
                auth.push(mechanism.name());
                if mechanism == Mechanism::Plain {
                    auth.push("LOGIN");
                }
            }
            if auth.len() > 1 {
                lines.push(auth.join(" "));
            }
        }
        let mut out = String::new();
        for (i, line) in lines.iter().enumerate() {
            let sep = if i + 1 == lines.len() { ' ' } else { '-' };
            out.push_str(&format!("250{sep}{line}\r\n"));
        }
        self.conn.write_all(out.as_bytes()).await
    }

    /// Applies a programmed failure, if any, for a stage other than DATA.
    /// `Some` when the command must stop here.
    async fn fault(&mut self, stage: SmtpStage) -> io::Result<Option<Next>> {
        let fault = self.shared.lock().take_fault(stage);
        match fault {
            None => Ok(None),
            Some(SmtpFault::Disconnect | SmtpFault::StoreThenDisconnect) => Ok(Some(Next::Close)),
            Some(SmtpFault::Reply { code, text }) => {
                self.reply(code, &text).await?;
                Ok(Some(after_reply(code)))
            }
        }
    }

    async fn read_sasl(&mut self, challenge: &str) -> io::Result<Option<Vec<u8>>> {
        self.reply(334, challenge).await?;
        let line =
            self.conn.read_line().await?.ok_or_else(|| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "client left mid-AUTH")
            })?;
        if line == b"*" {
            return Ok(None);
        }
        Ok(Some(line))
    }

    async fn auth(&mut self, args: &str) -> io::Result<Next> {
        if !self.greeted {
            self.reply(503, "5.5.1 EHLO first").await?;
            return Ok(Next::Continue);
        }
        if self.offers_starttls() {
            self.reply(530, "5.7.0 Must issue a STARTTLS command first")
                .await?;
            return Ok(Next::Continue);
        }
        if self.user.is_some() {
            self.reply(503, "5.5.1 Already authenticated").await?;
            return Ok(Next::Continue);
        }
        if let Some(next) = self.fault(SmtpStage::Auth).await? {
            return Ok(next);
        }
        let (mechanism, initial) = match args.split_once(' ') {
            Some((m, i)) => (m.to_ascii_uppercase(), Some(i.trim().as_bytes().to_vec())),
            None => (args.to_ascii_uppercase(), None),
        };
        let offered = self.shared.lock().config.mechanisms.clone();
        let sasl_mechanism = Mechanism::named(&mechanism).filter(|m| offered.contains(m));
        let claim = match (sasl_mechanism, mechanism.as_str()) {
            (Some(sasl_mechanism), _) => {
                let response = match initial {
                    Some(r) => Some(r),
                    None => self.read_sasl("").await?,
                };
                let Some(response) = response else {
                    self.reply(501, "5.0.0 Authentication cancelled").await?;
                    return Ok(Next::Continue);
                };
                let message = sasl::decode(&response).unwrap_or_default();
                sasl_mechanism.parse(&message)
            }
            (None, "LOGIN") if offered.contains(&Mechanism::Plain) => {
                self.login_exchange(initial).await?
            }
            _ => {
                self.reply(504, "5.5.4 Unrecognized authentication type")
                    .await?;
                return Ok(Next::Continue);
            }
        };
        let accepted = {
            let state = self.shared.lock();
            match &claim {
                Some(Claim::Password { user, password }) => {
                    state.password_ok(user, password).then(|| user.clone())
                }
                Some(Claim::Bearer { user, token }) => {
                    state.token_ok(user, token).then(|| user.clone())
                }
                None => None,
            }
        };
        match accepted {
            Some(user) => {
                self.user = Some(user);
                self.reply(235, "2.7.0 Authentication successful").await?;
            }
            None => {
                if let Some(failure) = sasl_mechanism.and_then(Mechanism::failure_challenge) {
                    // As Google does for XOAUTH2, and RFC 7628 3.2.2 for
                    // OAUTHBEARER: an error challenge, the client's answer,
                    // then 535.
                    let challenge = sasl::encode(failure);
                    self.read_sasl(&challenge).await?;
                    if self.shared.lock().config.challenges_again {
                        while self.read_sasl(&challenge).await?.is_some() {}
                        self.reply(501, "5.0.0 Authentication cancelled").await?;
                        return Ok(Next::Continue);
                    }
                }
                self.reply(535, "5.7.8 Authentication credentials invalid")
                    .await?;
            }
        }
        Ok(Next::Continue)
    }

    /// AUTH LOGIN: the user name and password, each asked for in turn.
    async fn login_exchange(&mut self, initial: Option<Vec<u8>>) -> io::Result<Option<Claim>> {
        let user = match initial {
            Some(u) => Some(u),
            // "Username:" and "Password:", base64.
            None => self.read_sasl("VXNlcm5hbWU6").await?,
        };
        let Some(user) = user else {
            return Ok(None);
        };
        let Some(password) = self.read_sasl("UGFzc3dvcmQ6").await? else {
            return Ok(None);
        };
        let decode = |b: &[u8]| sasl::decode(b).and_then(|d| String::from_utf8(d).ok());
        Ok(match (decode(&user), decode(&password)) {
            (Some(user), Some(password)) => Some(Claim::Password { user, password }),
            _ => None,
        })
    }

    async fn mail(&mut self, args: &str) -> io::Result<Next> {
        if self.user.is_none() {
            self.reply(530, "5.7.0 Authentication required").await?;
            return Ok(Next::Continue);
        }
        if self.transaction.from.is_some() {
            self.reply(503, "5.5.1 Nested MAIL command").await?;
            return Ok(Next::Continue);
        }
        if let Some(next) = self.fault(SmtpStage::Mail).await? {
            return Ok(next);
        }
        let Some((from, params)) = path(args, "FROM:") else {
            self.reply(501, "5.5.4 Syntax: MAIL FROM:<address>").await?;
            return Ok(Next::Continue);
        };
        let max = self.shared.lock().config.max_size;
        let declared = params
            .split_whitespace()
            .find_map(|p| p.strip_prefix("SIZE=").or_else(|| p.strip_prefix("size=")))
            .and_then(|s| s.parse::<usize>().ok());
        if declared.is_some_and(|size| size > max) {
            self.reply(552, "5.3.4 Message too big").await?;
            return Ok(Next::Continue);
        }
        self.transaction.from = Some(from);
        self.reply(250, "2.1.0 OK").await?;
        Ok(Next::Continue)
    }

    async fn rcpt(&mut self, args: &str) -> io::Result<Next> {
        if self.transaction.from.is_none() {
            self.reply(503, "5.5.1 MAIL first").await?;
            return Ok(Next::Continue);
        }
        if let Some(next) = self.fault(SmtpStage::Rcpt).await? {
            return Ok(next);
        }
        let Some((to, _)) = path(args, "TO:") else {
            self.reply(501, "5.5.4 Syntax: RCPT TO:<address>").await?;
            return Ok(Next::Continue);
        };
        self.transaction.to.push(to);
        self.reply(250, "2.1.5 OK").await?;
        Ok(Next::Continue)
    }

    async fn data(&mut self) -> io::Result<Next> {
        if self.transaction.to.is_empty() {
            self.reply(503, "5.5.1 RCPT first").await?;
            return Ok(Next::Continue);
        }
        self.reply(354, "End data with <CR><LF>.<CR><LF>").await?;
        let max = self.shared.lock().config.max_size;
        let read = data::read(&mut self.conn, max).await?;
        let transaction = std::mem::take(&mut self.transaction);
        let data = match read {
            Data::Message(data) => data,
            Data::Refused(code, text) => {
                self.reply(code, text).await?;
                return Ok(Next::Continue);
            }
            Data::RefusedAndClosed(code, text) => {
                self.reply(code, text).await?;
                return Ok(Next::Close);
            }
            Data::Eof => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "client left mid-DATA",
                ));
            }
        };
        let message = ReceivedMessage {
            auth_user: self.user.clone(),
            mail_from: transaction.from.unwrap_or_default(),
            rcpt_to: transaction.to,
            data,
        };
        let fault = self.shared.lock().take_fault(SmtpStage::Data);
        match fault {
            Some(SmtpFault::Disconnect) => return Ok(Next::Close),
            Some(SmtpFault::Reply { code, text }) => {
                self.reply(code, &text).await?;
                return Ok(after_reply(code));
            }
            Some(SmtpFault::StoreThenDisconnect) => {
                self.shared.lock().messages.push(message);
                return Ok(Next::Close);
            }
            None => {}
        }
        self.shared.lock().messages.push(message);
        self.reply(250, "2.0.0 OK queued").await?;
        Ok(Next::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        assert_eq!(
            path("FROM:<a@b.example> SIZE=10", "FROM:"),
            Some(("a@b.example".to_string(), "SIZE=10"))
        );
        assert_eq!(path("to: <x@y>", "TO:"), Some(("x@y".to_string(), "")));
        assert_eq!(path("FROM:<>", "FROM:"), Some((String::new(), "")));
        assert_eq!(path("FROM:a@b", "FROM:"), None);
        assert_eq!(path("TO", "TO:"), None);
    }
}
