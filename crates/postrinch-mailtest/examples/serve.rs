//! The fake IMAP and SMTP servers, kept running for a test window to add
//! an account to by hand: `cargo run --release -p postrinch-mailtest
//! --example serve -- <dir>`. It writes both servers' certificates to
//! `<dir>/certs.pem` (start the window with `SSL_CERT_FILE` set to it, so
//! it trusts them) and prints the servers' ports, then serves until it is
//! killed. The one user is `ada@example.org`, password `correct horse`:
//! test values that open nothing real.
//!
//! Creating the file `<dir>/refuse` makes IMAP refuse the next sign-in, as
//! a server does when the password changed, so the window's "needs to
//! sign in" and Sign in again can be walked through; the file is removed
//! once seen.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use postrinch_mailtest::{
    ImapConfig, ImapFault, ImapServer, NewMessage, SmtpConfig, SmtpServer, SpecialUse,
    TestCertificate,
};

const USER: &str = "ada@example.org";
const PASSWORD: &str = "correct horse";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir: PathBuf = std::env::args_os()
        .nth(1)
        .ok_or("give the directory for certs.pem")?
        .into();
    let imap = ImapServer::start(ImapConfig::new().user(USER, PASSWORD)).await?;
    let smtp = SmtpServer::start(SmtpConfig::new().user(USER, PASSWORD)).await?;
    let user = imap.user(USER);
    user.create_special_folder("Sent", SpecialUse::Sent)?;
    user.create_special_folder("Drafts", SpecialUse::Drafts)?;
    user.create_special_folder("Trash", SpecialUse::Trash)?;
    user.create_folder("Copies")?;
    user.create_folder("Receipts")?;
    let hello = "From: Ann <ann@example.com>\r\nTo: ada@example.org\r\n\
                 Subject: Welcome to the fake server\r\n\
                 Date: Tue, 24 Nov 2026 09:00:00 +0000\r\n\
                 Message-ID: <welcome@example.com>\r\n\r\nNothing here is real.\r\n";
    user.append("INBOX", NewMessage::new(hello.as_bytes().to_vec()))?;

    std::fs::create_dir_all(&dir)?;
    let pem = pem(&[imap.certificate(), smtp.certificate()]);
    std::fs::write(dir.join("certs.pem"), pem)?;
    let port = |addr: Option<std::net::SocketAddr>| addr.map_or(0, |a| a.port());
    println!("user {USER}");
    println!("imap 127.0.0.1 {} TLS", port(imap.tls_addr()));
    println!("smtp 127.0.0.1 {} TLS", port(smtp.tls_addr()));
    println!("smtp 127.0.0.1 {} STARTTLS", port(smtp.starttls_addr()));
    println!("certs {}", dir.join("certs.pem").display());
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        refuse_when_asked(&imap, &dir.join("refuse"));
    }
}

/// When `trigger` exists: the next sign-in is refused, and `trigger`
/// removed. The server offers AUTH=PLAIN, so a password signs in by
/// AUTHENTICATE, not LOGIN.
fn refuse_when_asked(imap: &ImapServer, trigger: &Path) {
    if std::fs::remove_file(trigger).is_err() {
        return;
    }
    let refused = ImapFault::No("[AUTHENTICATIONFAILED] Invalid credentials".into());
    imap.fail_next("AUTHENTICATE", 1, refused);
    println!("the next sign-in is refused");
}

/// The certificates, PEM-encoded, one after the other.
fn pem(certificates: &[&TestCertificate]) -> String {
    let mut out = String::new();
    for certificate in certificates {
        let encoded = base64::engine::general_purpose::STANDARD.encode(certificate.der());
        out.push_str("-----BEGIN CERTIFICATE-----");
        for (i, c) in encoded.chars().enumerate() {
            if i % 64 == 0 {
                out.push('\n');
            }
            out.push(c);
        }
        out.push_str("\n-----END CERTIFICATE-----\n");
    }
    out
}
