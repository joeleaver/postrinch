//! Mozilla's autoconfig format (`clientConfig` version 1.1), as the domain's
//! own autoconfig files and Thunderbird's ISPDB serve it.
//!
//! Only the first `emailProvider` is read, and in it the IMAP incoming
//! servers, the SMTP outgoing servers, the display name, and the
//! `documentation` and `enable` pages. POP3 servers are ignored, and so are
//! plain-text servers (`socketType` plain, or none given, which means
//! plain).

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, escape};
use thiserror::Error;

use crate::address::Address;
use crate::types::{Candidate, Security, Username};

#[cfg(test)]
mod tests;

/// An autoconfig file that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum XmlError {
    /// The XML is not well formed.
    #[error("the autoconfig XML is malformed: {0}")]
    Malformed(String),
    /// The document is XML but not a `clientConfig`.
    #[error("the document is not an autoconfig clientConfig")]
    NotClientConfig,
}

impl From<quick_xml::Error> for XmlError {
    fn from(e: quick_xml::Error) -> Self {
        XmlError::Malformed(e.to_string())
    }
}

impl From<quick_xml::encoding::EncodingError> for XmlError {
    fn from(e: quick_xml::encoding::EncodingError) -> Self {
        XmlError::Malformed(e.to_string())
    }
}

/// What an autoconfig file says about the provider, for one address.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ProviderConfig {
    /// `displayName`, else `displayShortName`.
    pub provider_name: Option<String>,
    /// IMAP servers that are not plain text, TLS first.
    pub imap: Vec<Candidate>,
    /// SMTP servers that are not plain text, TLS first.
    pub smtp: Vec<Candidate>,
    /// `<documentation url>`.
    pub documentation_url: Option<String>,
    /// `<enable visiturl>`.
    pub enable_url: Option<String>,
    /// Every IMAP server kept signs in with OAuth2 only.
    pub oauth2_only: bool,
}

/// A server as the file gives it, before placeholders are filled in.
#[derive(Debug, Default)]
struct RawServer {
    protocol: String,
    hostname: String,
    port: String,
    socket_type: String,
    username: Option<String>,
    auth: Vec<String>,
}

/// The parts of the first `emailProvider`, as text.
#[derive(Debug, Default)]
struct RawProvider {
    display_name: Option<String>,
    display_short_name: Option<String>,
    incoming: Vec<RawServer>,
    outgoing: Vec<RawServer>,
    documentation_url: Option<String>,
    enable_url: Option<String>,
}

/// Parses an autoconfig file for `address`.
pub(crate) fn parse(xml: &str, address: &Address) -> Result<ProviderConfig, XmlError> {
    let raw = read_provider(xml)?;
    let imap_raw: Vec<&RawServer> = raw
        .incoming
        .iter()
        .filter(|s| s.protocol.eq_ignore_ascii_case("imap"))
        .filter(|s| security(&s.socket_type).is_some())
        .collect();
    let oauth2_only = !imap_raw.is_empty()
        && imap_raw
            .iter()
            .all(|s| !s.auth.is_empty() && s.auth.iter().all(|a| a.eq_ignore_ascii_case("OAuth2")));
    let imap = candidates(imap_raw.into_iter(), address);
    let smtp = candidates(
        raw.outgoing
            .iter()
            .filter(|s| s.protocol.eq_ignore_ascii_case("smtp")),
        address,
    );
    Ok(ProviderConfig {
        provider_name: raw.display_name.or(raw.display_short_name),
        imap,
        smtp,
        documentation_url: raw.documentation_url,
        enable_url: raw.enable_url,
        oauth2_only,
    })
}

/// Turns raw servers into candidates: plain text and unusable ones dropped,
/// duplicates dropped, TLS before STARTTLS.
fn candidates<'a>(
    servers: impl Iterator<Item = &'a RawServer>,
    address: &Address,
) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for server in servers {
        let Some(candidate) = candidate(server, address) else {
            continue;
        };
        let duplicate = out.iter().any(|c| {
            c.host == candidate.host && c.port == candidate.port && c.security == candidate.security
        });
        if !duplicate {
            out.push(candidate);
        }
    }
    out.sort_by_key(|c| c.security == Security::StartTls);
    out
}

fn candidate(server: &RawServer, address: &Address) -> Option<Candidate> {
    let security = security(&server.socket_type)?;
    let host = fill(&server.hostname, address).to_ascii_lowercase();
    let host = host.strip_suffix('.').unwrap_or(&host).to_owned();
    if host.is_empty() || !matches!(url::Host::parse(&host), Ok(url::Host::Domain(_))) {
        return None;
    }
    let port: u16 = server.port.parse().ok().filter(|p| *p != 0)?;
    let username = match server.username.as_deref() {
        None | Some("%EMAILADDRESS%") => Username::Address,
        Some("%EMAILLOCALPART%") => Username::LocalPart,
        Some(name) => Username::Literal(fill(name, address)),
    };
    Some(Candidate {
        host,
        port,
        security,
        username,
    })
}

/// The security a `socketType` asks for; `None` for plain text or anything
/// unknown.
fn security(socket_type: &str) -> Option<Security> {
    if socket_type.eq_ignore_ascii_case("SSL") || socket_type.eq_ignore_ascii_case("TLS") {
        Some(Security::Tls)
    } else if socket_type.eq_ignore_ascii_case("STARTTLS") {
        Some(Security::StartTls)
    } else {
        None
    }
}

/// Fills in the address placeholders.
fn fill(text: &str, address: &Address) -> String {
    text.replace("%EMAILADDRESS%", &address.full)
        .replace("%EMAILLOCALPART%", &address.local)
        .replace("%EMAILDOMAIN%", &address.domain)
}

fn is_https(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|u| u.scheme() == "https")
}

/// Walks the document and collects the first `emailProvider`.
fn read_provider(xml: &str) -> Result<RawProvider, XmlError> {
    let mut reader = Reader::from_str(xml);
    let mut provider = RawProvider::default();
    // Element names from the root down to the current one.
    let mut path: Vec<String> = Vec::new();
    let mut text = String::new();
    let mut providers_seen = 0usize;
    let mut saw_root = false;
    loop {
        match reader.read_event()? {
            Event::Start(e) => {
                let name = local_name(&e)?;
                if path.is_empty() {
                    if name != "clientConfig" {
                        return Err(XmlError::NotClientConfig);
                    }
                    saw_root = true;
                }
                path.push(name);
                if path.len() == 2 && path[1] == "emailProvider" {
                    providers_seen += 1;
                }
                if providers_seen == 1 {
                    open(&mut provider, &path, &e)?;
                }
                text.clear();
            }
            Event::Empty(e) => {
                let name = local_name(&e)?;
                if path.is_empty() {
                    if name != "clientConfig" {
                        return Err(XmlError::NotClientConfig);
                    }
                    saw_root = true;
                    continue;
                }
                path.push(name);
                if providers_seen == 1 {
                    open(&mut provider, &path, &e)?;
                }
                path.pop();
            }
            Event::Text(e) => text.push_str(&e.xml10_content()?),
            Event::CData(e) => text.push_str(&e.decode()?),
            Event::GeneralRef(e) => {
                if let Some(c) = e.resolve_char_ref()? {
                    text.push(c);
                } else if let Some(s) = escape::resolve_predefined_entity(&e.decode()?) {
                    text.push_str(s);
                }
            }
            Event::End(_) => {
                if providers_seen == 1 {
                    close(&mut provider, &path, text.trim());
                }
                text.clear();
                path.pop();
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::DocType(_) | Event::Comment(_) => {}
        }
    }
    if !saw_root {
        return Err(XmlError::NotClientConfig);
    }
    Ok(provider)
}

fn local_name(e: &BytesStart<'_>) -> Result<String, XmlError> {
    Ok(std::str::from_utf8(e.local_name().as_ref())
        .map_err(|e| XmlError::Malformed(e.to_string()))?
        .to_owned())
}

fn attribute(e: &BytesStart<'_>, name: &str) -> Result<Option<String>, XmlError> {
    match e.try_get_attribute(name).map_err(quick_xml::Error::from)? {
        Some(attr) => Ok(Some(
            attr.normalized_value(XmlVersion::Implicit1_0)?
                .trim()
                .to_owned(),
        )),
        None => Ok(None),
    }
}

/// An element opened inside the first provider: servers start, and the
/// pages are read from their attributes.
fn open(provider: &mut RawProvider, path: &[String], e: &BytesStart<'_>) -> Result<(), XmlError> {
    if path.len() != 3 {
        return Ok(());
    }
    match path[2].as_str() {
        "incomingServer" | "outgoingServer" => {
            let server = RawServer {
                protocol: attribute(e, "type")?.unwrap_or_default(),
                ..RawServer::default()
            };
            if path[2] == "incomingServer" {
                provider.incoming.push(server);
            } else {
                provider.outgoing.push(server);
            }
        }
        // The first HTTPS page of each kind; plain-HTTP links are not
        // offered.
        "documentation" | "instruction" if provider.documentation_url.is_none() => {
            provider.documentation_url = attribute(e, "url")?.filter(|u| is_https(u));
        }
        "enable" if provider.enable_url.is_none() => {
            provider.enable_url = attribute(e, "visiturl")?.filter(|u| is_https(u));
        }
        _ => {}
    }
    Ok(())
}

/// An element closed inside the first provider, with its text.
fn close(provider: &mut RawProvider, path: &[String], text: &str) {
    let value = || (!text.is_empty()).then(|| text.to_owned());
    match path.len() {
        3 => match path[2].as_str() {
            "displayName" if provider.display_name.is_none() => provider.display_name = value(),
            "displayShortName" if provider.display_short_name.is_none() => {
                provider.display_short_name = value();
            }
            _ => {}
        },
        4 => {
            let server = match path[2].as_str() {
                "incomingServer" => provider.incoming.last_mut(),
                "outgoingServer" => provider.outgoing.last_mut(),
                _ => None,
            };
            let Some(server) = server else { return };
            match path[3].as_str() {
                "hostname" => server.hostname = text.to_owned(),
                "port" => server.port = text.to_owned(),
                "socketType" => server.socket_type = text.to_owned(),
                "username" => server.username = value(),
                "authentication" => server.auth.extend(value()),
                _ => {}
            }
        }
        _ => {}
    }
}
