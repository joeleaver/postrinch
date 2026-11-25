//! A GreenMail server for one test: its standalone jar run by `java` on
//! two loopback ports (IMAPS and SMTPS), with a certificate made for the
//! run by `keytool` and signed by a CA made with it, which the test
//! trusts. The process is killed when the server is dropped.
//!
//! Where the jar is comes from `POSTRINCH_GREENMAIL_JAR`, which
//! `tools/greenmail-smoke.sh` sets after downloading and checking it.
//! `java` and `keytool` are `$JAVA_HOME/bin`'s when it is set, else the
//! `PATH`'s.

use std::fs::File;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rustls::pki_types::CertificateDer;

/// The environment variable naming GreenMail's standalone jar.
pub const JAR_VAR: &str = "POSTRINCH_GREENMAIL_JAR";

/// How long GreenMail may take to listen (it takes about a second).
const STARTUP: Duration = Duration::from_secs(60);

/// The keystores' password: they live in the test's own temporary
/// directory and hold a key made for this run alone.
const STORE_PASSWORD: &str = "greenmail-smoke";

/// A user GreenMail knows, signing in with the whole address.
pub struct User {
    pub address: &'static str,
    pub password: &'static str,
}

/// A running GreenMail.
pub struct GreenMail {
    process: Child,
    pub imaps: SocketAddr,
    pub smtps: SocketAddr,
    /// The CA that signed the servers' certificate.
    pub ca: CertificateDer<'static>,
    log: PathBuf,
}

impl GreenMail {
    /// Starts GreenMail with these users, in `dir`, and waits until both
    /// servers listen. Panics, saying what to do, when the jar is not
    /// named or the server does not start.
    pub fn start(dir: &Path, users: &[User]) -> Self {
        let jar = std::env::var_os(JAR_VAR)
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                panic!(
                    "{JAR_VAR} is not set: run tools/greenmail-smoke.sh, which downloads \
                 GreenMail's standalone jar, checks it and runs this test"
                )
            });
        assert!(
            jar.is_file(),
            "{JAR_VAR} names {jar:?}, which is not a file"
        );
        let (keystore, ca) = make_certificates(dir);
        let held = free_ports(2);
        let addr = |i: usize| held[i].local_addr().expect("a bound port");
        let (imaps, smtps) = (addr(0), addr(1));
        let users: Vec<String> = users.iter().map(user_spec).collect();
        let log = dir.join("greenmail.log");
        let out = File::create(&log).expect("GreenMail's log file");
        let err = out.try_clone().expect("GreenMail's log file");
        // Let the ports go only now, for GreenMail to bind.
        drop(held);
        let process = Command::new(java_tool("java"))
            .arg("-Dgreenmail.hostname=127.0.0.1")
            .arg(format!("-Dgreenmail.imaps.port={}", imaps.port()))
            .arg(format!("-Dgreenmail.smtps.port={}", smtps.port()))
            .arg(format!("-Dgreenmail.users={}", users.join(",")))
            .arg("-Dgreenmail.users.login=email")
            .arg(format!(
                "-Dgreenmail.tls.keystore.file={}",
                keystore.display()
            ))
            .arg(format!(
                "-Dgreenmail.tls.keystore.password={STORE_PASSWORD}"
            ))
            .arg("-jar")
            .arg(&jar)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
            .unwrap_or_else(|e| panic!("java could not be started ({e}): is a JDK installed?"));
        let mut server = Self {
            process,
            imaps,
            smtps,
            ca,
            log,
        };
        server.wait_until_listening();
        server
    }

    /// Waits until both ports take a connection, or panics with the log
    /// when GreenMail ends or does not listen in time.
    fn wait_until_listening(&mut self) {
        let started = Instant::now();
        loop {
            if let Some(status) = self.process.try_wait().expect("GreenMail's status") {
                panic!(
                    "GreenMail ended ({status}) before listening:\n{}",
                    self.log_text()
                );
            }
            let listening = [self.imaps, self.smtps]
                .iter()
                .all(|addr| TcpStream::connect_timeout(addr, Duration::from_millis(200)).is_ok());
            if listening {
                return;
            }
            if started.elapsed() > STARTUP {
                panic!(
                    "GreenMail did not listen within {STARTUP:?}:\n{}",
                    self.log_text()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// What GreenMail has written, for a failure's message.
    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_else(|e| format!("(no log: {e})"))
    }
}

impl Drop for GreenMail {
    fn drop(&mut self) {
        // Only this child, by its own handle.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// GreenMail's form for a user: `local:password@domain`.
fn user_spec(user: &User) -> String {
    let (local, domain) = user.address.split_once('@').expect("an address");
    format!("{local}:{}@{domain}", user.password)
}

/// `count` distinct loopback ports nothing listens on now. Every listener
/// is held until all are bound, so no two ports are the same; they are
/// closed when the caller drops them, just before GreenMail binds the
/// ports. Another process could take one in between (the kernel picks
/// ephemeral ports at random from a wide range, so that is unlikely); then
/// GreenMail fails to bind and the test fails with its log, never passes.
fn free_ports(count: usize) -> Vec<TcpListener> {
    (0..count)
        .map(|_| TcpListener::bind("127.0.0.1:0").expect("a free loopback port"))
        .collect()
}

/// `java` or `keytool`: from `$JAVA_HOME/bin` when it is set.
fn java_tool(name: &str) -> PathBuf {
    match std::env::var_os("JAVA_HOME") {
        Some(home) => Path::new(&home).join("bin").join(name),
        None => PathBuf::from(name),
    }
}

/// Makes a CA and a certificate for 127.0.0.1 it signs, in `dir`: the
/// server's keystore (its key, the certificate and the CA) and the CA's
/// certificate, which the test trusts.
fn make_certificates(dir: &Path) -> (PathBuf, CertificateDer<'static>) {
    let path = |name: &str| dir.join(name).display().to_string();
    let (ca_store, keystore) = (path("ca.p12"), path("greenmail.p12"));
    let (ca_der, request, signed) = (path("ca.der"), path("server.csr"), path("server.pem"));
    let key = |alias: &str, name: &str| {
        let dname = format!("CN={name}");
        let pair = [
            "-genkeypair",
            "-alias",
            alias,
            "-keyalg",
            "EC",
            "-groupname",
        ];
        let mut args: Vec<String> = pair.map(String::from).to_vec();
        args.extend(["secp256r1", "-dname", &dname, "-validity", "2"].map(String::from));
        args
    };

    let mut ca = key("ca", "Postrinch GreenMail test CA");
    ca.extend(["-ext", "bc:c"].map(String::from));
    keytool(&ca_store, &ca);
    keytool(&keystore, &key("greenmail", "127.0.0.1"));
    keytool(
        &keystore,
        &strings(&["-certreq", "-alias", "greenmail", "-file", &request]),
    );
    let sign = [
        "-gencert",
        "-alias",
        "ca",
        "-infile",
        &request,
        "-outfile",
        &signed,
        "-rfc",
        "-ext",
        "san=ip:127.0.0.1,dns:localhost",
        "-ext",
        "eku=serverAuth",
        "-validity",
        "2",
    ];
    keytool(&ca_store, &strings(&sign));
    keytool(
        &ca_store,
        &strings(&["-exportcert", "-alias", "ca", "-file", &ca_der]),
    );
    let import = |alias: &str, file: &str| {
        strings(&["-importcert", "-noprompt", "-alias", alias, "-file", file])
    };
    keytool(&keystore, &import("ca", &ca_der));
    keytool(&keystore, &import("greenmail", &signed));

    let ca = std::fs::read(&ca_der).expect("the CA's certificate");
    (PathBuf::from(keystore), CertificateDer::from(ca))
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|a| a.to_string()).collect()
}

/// Runs `keytool` on the PKCS#12 keystore `store` with these arguments,
/// or panics with what it said.
fn keytool(store: &str, args: &[String]) {
    let output = Command::new(java_tool("keytool"))
        .args(args)
        .args([
            "-keystore",
            store,
            "-storetype",
            "PKCS12",
            "-storepass",
            STORE_PASSWORD,
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap_or_else(|e| panic!("keytool could not be started ({e}): is a JDK installed?"));
    assert!(
        output.status.success(),
        "keytool {} failed: {}{}",
        args.first().map(String::as_str).unwrap_or(""),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
