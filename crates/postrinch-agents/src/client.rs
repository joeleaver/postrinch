//! The bridge's side: connect to the running app as an agent and make calls.
//!
//! Blocking; one call at a time. A connection that drops is made again on
//! the next call, so the app can be closed and opened while an agent's
//! session goes on. Every failure is one sentence an agent can pass on.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;

use crate::wire::{Call, Greeting, Hello, MAX_LINE, Outcome, Request, Response};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
/// How long one call may take: a search of a large mailbox takes seconds.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

const NOT_RUNNING: &str =
    "Postrinch isn't running on this computer. Ask the person to open it, then try again.";
const NO_KEY: &str = "Postrinch needs this app set up as an agent: the person runs \
     `postrinch-cli agent add <name>` and puts the key it prints in this app's \
     POSTRINCH_AGENT_KEY setting.";

struct Open {
    reader: BufReader<std::io::Take<TcpStream>>,
    writer: TcpStream,
}

pub struct Client {
    addr: String,
    key: Option<String>,
    open: Mutex<Option<Open>>,
    next_id: Mutex<u64>,
}

impl Client {
    pub fn new(addr: String, key: Option<String>) -> Self {
        Self {
            addr,
            key: key.filter(|k| !k.trim().is_empty()),
            open: Mutex::new(None),
            next_id: Mutex::new(0),
        }
    }

    /// Make a call. A connection lost since the last call is made again
    /// once before giving up.
    pub fn call(&self, call: &Call) -> Result<Value, String> {
        let id = {
            let mut next = self.next_id.lock().unwrap_or_else(|e| e.into_inner());
            *next += 1;
            *next
        };
        let mut open = self.open.lock().unwrap_or_else(|e| e.into_inner());
        for attempt in 0..2 {
            if open.is_none() {
                *open = Some(self.connect()?);
            }
            let link = open.as_mut().expect("just opened");
            match exchange(link, id, call) {
                Ok(Outcome::Ok(value)) => return Ok(value),
                Ok(Outcome::Error(sentence)) => return Err(sentence),
                Err(e) => {
                    *open = None;
                    // A call that changes mail is not sent twice: it may
                    // have been made before the connection dropped.
                    if attempt == 1 || call.changes() {
                        tracing::warn!("the connection to Postrinch failed: {e}");
                        return Err("The connection to Postrinch was lost. Try again.".into());
                    }
                }
            }
        }
        unreachable!("the loop returns")
    }

    fn connect(&self) -> Result<Open, String> {
        let Some(key) = &self.key else {
            return Err(NO_KEY.into());
        };
        let addrs: Vec<_> = self
            .addr
            .to_socket_addrs()
            .map_err(|_| format!("{:?} is not an address Postrinch can listen at.", self.addr))?
            .collect();
        let stream = addrs
            .iter()
            .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).ok())
            .ok_or_else(|| NOT_RUNNING.to_owned())?;
        let lost = |_| NOT_RUNNING.to_owned();
        stream.set_read_timeout(Some(CALL_TIMEOUT)).map_err(lost)?;
        let writer = stream.try_clone().map_err(lost)?;
        let mut link = Open {
            reader: BufReader::new(stream.take(MAX_LINE)),
            writer,
        };
        write_line(&mut link.writer, &Hello::new(key.clone())).map_err(lost)?;
        let line = read_line(&mut link.reader).map_err(lost)?;
        match serde_json::from_str::<Greeting>(&line) {
            Ok(Greeting::Welcome { agent }) => {
                tracing::info!("connected to Postrinch as {agent:?}");
                Ok(link)
            }
            Ok(Greeting::Refused { reason }) => Err(reason),
            Err(_) => Err(
                "Postrinch answered in a way this bridge does not understand: \
                 update Postrinch and postrinch-mcp together."
                    .into(),
            ),
        }
    }
}

fn exchange(link: &mut Open, id: u64, call: &Call) -> std::io::Result<Outcome> {
    write_line(
        &mut link.writer,
        &Request {
            id,
            call: call.clone(),
        },
    )?;
    loop {
        let line = read_line(&mut link.reader)?;
        let response: Response = serde_json::from_str(&line).map_err(std::io::Error::other)?;
        // An answer to an earlier call that gave up waiting is passed over.
        if response.id == id {
            return Ok(response.outcome);
        }
    }
}

fn write_line(out: &mut TcpStream, value: &impl serde::Serialize) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    line.push(b'\n');
    out.write_all(&line)?;
    out.flush()
}

fn read_line(reader: &mut BufReader<std::io::Take<TcpStream>>) -> std::io::Result<String> {
    reader.get_mut().set_limit(MAX_LINE);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    Ok(line)
}
