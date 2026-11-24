//! CAPABILITY, LOGIN, AUTHENTICATE (PLAIN, XOAUTH2) and ENABLE.
//!
//! On the plain-text STARTTLS port the server advertises LOGINDISABLED
//! and refuses to sign anyone in until the connection is encrypted.

use std::io;

use super::config::Extension;
use super::response::Args;
use super::session::{Reply, Session};
use crate::sasl::{self, Claim};

const PRIVACY: &str = "[PRIVACYREQUIRED] Use STARTTLS first";
const FAILED: &str = "[AUTHENTICATIONFAILED] Invalid credentials";

impl Session {
    /// The capability list for the connection's current state.
    pub(crate) fn capabilities(&self) -> String {
        let state = self.shared.lock();
        let mut caps = vec!["IMAP4rev1", "LITERAL+", "SASL-IR", "UNSELECT"];
        if self.conn.is_tls() {
            caps.extend(["AUTH=PLAIN", "AUTH=XOAUTH2"]);
        } else {
            caps.extend(["STARTTLS", "LOGINDISABLED"]);
        }
        if state.has(Extension::Condstore) || state.has(Extension::Qresync) {
            caps.push("ENABLE");
        }
        caps.extend(
            Extension::ALL
                .into_iter()
                .filter(|e| state.has(*e))
                .map(Extension::capability),
        );
        caps.join(" ")
    }

    pub(crate) fn capability(&mut self, args: Args) -> Reply {
        if let Err(e) = args.end() {
            return Reply::Bad(e);
        }
        let line = format!("* CAPABILITY {}\r\n", self.capabilities());
        self.out.extend_from_slice(line.as_bytes());
        Reply::Ok("CAPABILITY completed".into())
    }

    /// Checks a claim against the configured users; signs in on success.
    fn sign_in(&mut self, claim: &Claim) -> bool {
        let state = self.shared.lock();
        let (user, ok) = match claim {
            Claim::Password { user, password } => (
                user,
                state
                    .accounts
                    .get(user)
                    .is_some_and(|a| a.password.as_deref() == Some(password.as_str())),
            ),
            Claim::Bearer { user, token } => (
                user,
                state
                    .accounts
                    .get(user)
                    .is_some_and(|a| a.oauth_token.as_deref() == Some(token.as_str())),
            ),
        };
        drop(state);
        if ok {
            self.user = Some(user.clone());
        }
        ok
    }

    fn signed_in(&self) -> Reply {
        Reply::Ok(format!("[CAPABILITY {}] Logged in", self.capabilities()))
    }

    pub(crate) fn login(&mut self, mut args: Args) -> Reply {
        let parsed = (|| -> Result<_, String> {
            let user = args.text()?;
            let password = args.text()?;
            args.end()?;
            Ok(Claim::Password { user, password })
        })();
        let claim = match parsed {
            Ok(c) => c,
            Err(e) => return Reply::Bad(e),
        };
        if !self.conn.is_tls() {
            return Reply::No(PRIVACY.into());
        }
        if self.sign_in(&claim) {
            self.signed_in()
        } else {
            Reply::No(FAILED.into())
        }
    }

    /// AUTHENTICATE PLAIN / XOAUTH2, with or without an initial response
    /// (SASL-IR).
    pub(crate) async fn authenticate(&mut self, mut args: Args) -> io::Result<Reply> {
        if self.user.is_some() {
            return Ok(Reply::Bad("Already logged in".into()));
        }
        let parsed = (|| -> Result<_, String> {
            let mechanism = args.atom()?.to_ascii_uppercase();
            let initial = if args.is_empty() {
                None
            } else {
                Some(args.text()?)
            };
            args.end()?;
            Ok((mechanism, initial))
        })();
        let (mechanism, initial) = match parsed {
            Ok(p) => p,
            Err(e) => return Ok(Reply::Bad(e)),
        };
        if !self.conn.is_tls() {
            return Ok(Reply::No(PRIVACY.into()));
        }
        if mechanism != "PLAIN" && mechanism != "XOAUTH2" {
            return Ok(Reply::No(format!("Unsupported mechanism {mechanism}")));
        }
        let response = match initial {
            Some(text) => text.into_bytes(),
            None => {
                self.conn.write_all(b"+ \r\n").await?;
                self.read_sasl_line().await?
            }
        };
        if response == b"*" {
            return Ok(Reply::Bad("Authentication cancelled".into()));
        }
        let Some(message) = sasl::decode(&response) else {
            return Ok(Reply::Bad("Response is not base64".into()));
        };
        let claim = if mechanism == "PLAIN" {
            sasl::parse_plain(&message)
        } else {
            sasl::parse_xoauth2(&message)
        };
        if claim.as_ref().is_some_and(|c| self.sign_in(c)) {
            return Ok(self.signed_in());
        }
        if mechanism == "XOAUTH2" {
            // As Google does: an error challenge, an empty answer, then NO.
            let challenge = format!("+ {}\r\n", sasl::encode(sasl::XOAUTH2_FAILURE));
            self.conn.write_all(challenge.as_bytes()).await?;
            self.read_sasl_line().await?;
        }
        Ok(Reply::No(FAILED.into()))
    }

    async fn read_sasl_line(&mut self) -> io::Result<Vec<u8>> {
        self.conn
            .read_line()
            .await?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "client left mid-SASL"))
    }

    /// ENABLE CONDSTORE / QRESYNC (RFC 5161). QRESYNC implies CONDSTORE.
    pub(crate) fn enable(&mut self, mut args: Args) -> Reply {
        let (condstore, qresync) = {
            let state = self.shared.lock();
            (
                state.has(Extension::Condstore),
                state.has(Extension::Qresync),
            )
        };
        if !condstore && !qresync {
            return Reply::Bad("Unknown command ENABLE".into());
        }
        let mut enabled = Vec::new();
        while !args.is_empty() {
            let name = match args.atom() {
                Ok(n) => n.to_ascii_uppercase(),
                Err(e) => return Reply::Bad(e),
            };
            match name.as_str() {
                "CONDSTORE" if condstore => {
                    self.condstore = true;
                    enabled.push(name);
                }
                "QRESYNC" if qresync => {
                    self.qresync = true;
                    self.condstore = true;
                    enabled.push(name);
                }
                _ => {}
            }
        }
        let mut line = String::from("* ENABLED");
        for name in &enabled {
            line.push(' ');
            line.push_str(name);
        }
        line.push_str("\r\n");
        self.out.extend_from_slice(line.as_bytes());
        Reply::Ok("ENABLE completed".into())
    }
}
