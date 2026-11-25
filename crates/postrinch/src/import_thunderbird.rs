//! Importing accounts from Thunderbird, apart from the window (docs/imap.md,
//! "Adding an account", after design/mockups/add-imap-account.html): the
//! list of what was found, by profile, with what each account would come
//! in as; the form each IMAP account fills in, with Thunderbird's servers
//! and Postrinch's own sign-in; and the import's run through the picked
//! accounts. Passwords stay in Thunderbird: only its settings are read
//! (`postrinch-thunderbird`), and each account is signed in to as if added
//! by hand. The window's part is in `add_account_import` and
//! `ui::add_account_import`.

use postrinch_discover::{HostedBy, known_host};
use postrinch_model::{EmailAddress, Security};
use postrinch_sync::imap::OAuthProvider;
use postrinch_thunderbird as tb;

use crate::add_account::{AddForm, Found, Kind, ServerForm, Way};

/// Where the servers came from, as the found box says it.
pub const FROM_THUNDERBIRD: &str = "From Thunderbird's settings";

/// How a found account comes into Postrinch.
#[derive(Clone, PartialEq, Eq)]
pub enum Route {
    /// Over IMAP, with this form: Thunderbird's servers, and the
    /// password or the browser sign-in Postrinch has for the provider.
    Imap(Box<AddForm>),
    /// As a Google account, with Google's own sign-in (mail, labels and
    /// calendars), as a Gmail address is better added.
    Google(EmailAddress),
}

/// One account found, as the list shows it.
#[derive(Clone, PartialEq, Eq)]
pub struct Row {
    /// Its place in the list, which the picks name.
    pub id: usize,
    /// The address, or the account's name when it has none.
    pub who: String,
    /// The person's name on it, if any.
    pub name: String,
    /// The incoming server, and its kind when it is not IMAP.
    pub server: String,
    /// How it would sign in, or why it cannot be added.
    pub how: String,
    /// None when it cannot be added.
    pub route: Option<Route>,
}

/// One profile's accounts.
#[derive(Clone, PartialEq, Eq)]
pub struct Group {
    /// `Profile “default-release”`, with the installation when there are
    /// profiles of more than one.
    pub label: String,
    /// What could not be read of its settings.
    pub problem: Option<String>,
    pub rows: Vec<Row>,
}

/// Everything found in Thunderbird.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Listing {
    pub groups: Vec<Group>,
    /// Thunderbird's profile lists that could not be read, with why.
    pub trouble: Vec<String>,
}

impl Listing {
    /// The rows of every profile, in order.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.groups.iter().flat_map(|g| g.rows.iter())
    }

    pub fn row(&self, id: usize) -> Option<&Row> {
        self.rows().find(|r| r.id == id)
    }

    /// How many accounts could be added.
    pub fn addable(&self) -> usize {
        self.rows().filter(|r| r.route.is_some()).count()
    }
}

/// The list of what `found` holds. `here` are the addresses Postrinch has
/// already; `oauth_ready` says which browser sign-ins this copy has set
/// up. None when Thunderbird has no profiles here.
pub fn listing(
    found: &tb::Found,
    here: &[EmailAddress],
    oauth_ready: impl Fn(OAuthProvider) -> bool,
) -> Option<Listing> {
    if found.profiles.is_empty() && found.unreadable.is_empty() {
        return None;
    }
    let mixed = found
        .profiles
        .iter()
        .any(|p| p.install != found.profiles[0].install);
    let mut listed: Vec<EmailAddress> = Vec::new();
    let mut id = 0;
    let mut groups = Vec::new();
    for profile in &found.profiles {
        let mut rows = Vec::new();
        for account in &profile.accounts {
            rows.push(row(id, account, here, &mut listed, &oauth_ready));
            id += 1;
        }
        let problem = profile.problem.as_ref().map(problem_text);
        if rows.is_empty() && problem.is_none() {
            continue;
        }
        groups.push(Group {
            label: profile_label(profile, mixed),
            problem,
            rows,
        });
    }
    let trouble = found
        .unreadable
        .iter()
        .map(|(path, why)| format!("{} could not be read: {why}.", path.display()))
        .collect();
    Some(Listing { groups, trouble })
}

fn profile_label(profile: &tb::Profile, mixed: bool) -> String {
    let install = match profile.install {
        tb::Install::Native => "",
        tb::Install::Snap => " (Snap)",
        tb::Install::Flatpak => " (Flatpak)",
    };
    if mixed {
        format!("Profile “{}”{install}", profile.name)
    } else {
        format!("Profile “{}”", profile.name)
    }
}

fn problem_text(problem: &tb::Problem) -> String {
    match problem {
        tb::Problem::Unreadable(why) => {
            format!("This profile's settings could not be read: {why}.")
        }
        tb::Problem::Malformed { prefs, first_line } => format!(
            "{prefs} of this profile's settings could not be read (the first on line {first_line} of prefs.js), so they are left out."
        ),
    }
}

/// Why a kind of account cannot come in, or none for IMAP.
fn unsupported(kind: &tb::AccountKind) -> Option<(String, &'static str)> {
    let (what, tag) = match kind {
        tb::AccountKind::Imap => return None,
        tb::AccountKind::Pop => ("POP accounts aren't supported".to_owned(), "POP"),
        tb::AccountKind::News => ("Newsgroups aren't supported".to_owned(), "News"),
        tb::AccountKind::Feeds => ("Feeds aren't supported".to_owned(), "Feeds"),
        tb::AccountKind::LocalFolders => ("Local Folders aren't supported".to_owned(), ""),
        tb::AccountKind::Other(kind) => (format!("{kind} accounts aren't supported"), ""),
    };
    Some((what, tag))
}

fn row(
    id: usize,
    account: &tb::Account,
    here: &[EmailAddress],
    listed: &mut Vec<EmailAddress>,
    oauth_ready: &impl Fn(OAuthProvider) -> bool,
) -> Row {
    let identity = account.identity.clone().unwrap_or_default();
    let host = account.incoming.as_ref().map(|s| s.host.clone());
    let who = identity
        .address
        .clone()
        .or_else(|| account.label.clone())
        .unwrap_or_else(|| account.key.clone());
    let mut row = Row {
        id,
        who,
        name: identity.name.clone().unwrap_or_default(),
        server: host.clone().unwrap_or_default(),
        how: String::new(),
        route: None,
    };
    if let Some((why, tag)) = unsupported(&account.kind) {
        if !tag.is_empty() && !row.server.is_empty() {
            row.server = format!("{} · {tag}", row.server);
        }
        if matches!(
            account.kind,
            tb::AccountKind::Feeds | tb::AccountKind::LocalFolders
        ) {
            // Their "host" is a name of Thunderbird's, not a server.
            row.server.clear();
        }
        row.how = why;
        return row;
    }
    let (Some(incoming), Some(host)) = (&account.incoming, host) else {
        row.how = "Its server settings are missing".to_owned();
        return row;
    };
    let Some(address) = identity
        .address
        .as_deref()
        .and_then(|a| EmailAddress::parse(a).ok())
    else {
        row.how = "It has no email address".to_owned();
        return row;
    };
    if here.contains(&address) {
        row.how = "Already in Postrinch".to_owned();
        return row;
    }
    if listed.contains(&address) {
        row.how = "Listed above".to_owned();
        return row;
    }
    listed.push(address.clone());
    let known = known_host(&host);
    match known.hosted_by {
        Some(HostedBy::Google) => {
            row.how = "Sign in with Google".to_owned();
            row.route = Some(Route::Google(address));
        }
        Some(HostedBy::Microsoft) if !oauth_ready(OAuthProvider::Microsoft) => {
            row.how = "Microsoft sign-in isn't set up in this copy".to_owned();
        }
        _ => {
            let form = form_for(account, incoming, &address, known, oauth_ready);
            row.how = match form.way {
                Way::Browser(_) => "Sign in with Microsoft".to_owned(),
                Way::Password if form.found.app_password_url.is_some() => "App password".to_owned(),
                Way::Password => "Password".to_owned(),
            };
            row.route = Some(Route::Imap(Box::new(form)));
        }
    }
    row
}

/// The add-account form for an IMAP account found in Thunderbird.
fn form_for(
    account: &tb::Account,
    incoming: &tb::Server,
    address: &EmailAddress,
    known: postrinch_discover::KnownHost,
    oauth_ready: &impl Fn(OAuthProvider) -> bool,
) -> AddForm {
    let microsoft = known.hosted_by == Some(HostedBy::Microsoft);
    let oauth = microsoft
        .then_some(OAuthProvider::Microsoft)
        .filter(|p| oauth_ready(*p));
    let mut form = AddForm::new(Kind::Thunderbird);
    form.address = address.to_string();
    form.name = account
        .identity
        .as_ref()
        .and_then(|i| i.name.clone())
        .unwrap_or_default();
    form.imap = server_form(incoming, address);
    if let Some(outgoing) = &account.outgoing {
        form.smtp = server_form(outgoing, address);
    }
    form.found = Found {
        provider: known.provider_name.map(str::to_owned),
        source: Some(FROM_THUNDERBIRD),
        app_password_url: known.app_password_url.map(str::to_owned),
        oauth,
        // Microsoft has turned passwords off for mail apps.
        takes_passwords: !microsoft,
        google: false,
        // The person's own Thunderbird vouches for its servers.
        confirm_servers: false,
    };
    form.way = match oauth {
        Some(provider) => Way::Browser(provider),
        None => Way::Password,
    };
    form
}

/// A server of Thunderbird's as the form holds it. Postrinch never signs
/// in over a connection without encryption, so a plain one is tried with
/// STARTTLS on the same port (the check says so if the server offers
/// none). No user name is the address.
fn server_form(server: &tb::Server, address: &EmailAddress) -> ServerForm {
    ServerForm {
        host: server.host.clone(),
        port: server.port.to_string(),
        security: match server.security {
            tb::Security::Tls => Security::Tls,
            tb::Security::StartTls | tb::Security::Plain => Security::StartTls,
        },
        username: server
            .username
            .clone()
            .unwrap_or_else(|| address.to_string()),
    }
}

/// How one picked account's turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Added,
    /// Handed to Google's sign-in in the browser; a notice says how it went.
    ToGoogle,
    Skipped,
    /// The import was stopped before its turn.
    NotAdded,
}

impl Ended {
    pub fn text(self) -> &'static str {
        match self {
            Ended::Added => "added, syncing",
            Ended::ToGoogle => "signing in with Google in your browser",
            Ended::Skipped => "skipped",
            Ended::NotAdded => "not added",
        }
    }
}

/// An import: what was found, what is picked and, once started, each
/// picked account in turn.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Import {
    pub listing: Listing,
    /// The rows ticked, by id.
    pub picked: Vec<usize>,
    /// The rows being imported, in order, once started.
    pub queue: Vec<usize>,
    /// How each of the queue's accounts ended so far, with its address.
    pub ended: Vec<(String, Ended)>,
}

impl Import {
    /// Every account that can be added is ticked.
    pub fn new(listing: Listing) -> Self {
        let picked = listing
            .rows()
            .filter(|r| r.route.is_some())
            .map(|r| r.id)
            .collect();
        Self {
            listing,
            picked,
            ..Self::default()
        }
    }

    /// Ticks or clears a row that can be added.
    pub fn toggle(&mut self, id: usize) {
        if self.listing.row(id).is_none_or(|r| r.route.is_none()) {
            return;
        }
        if let Some(at) = self.picked.iter().position(|p| *p == id) {
            self.picked.remove(at);
        } else {
            self.picked.push(id);
        }
    }

    /// Starts on the ticked rows, in the list's order; false when none is.
    pub fn start(&mut self) -> bool {
        self.queue = self
            .listing
            .rows()
            .filter(|r| self.picked.contains(&r.id))
            .map(|r| r.id)
            .collect();
        self.ended.clear();
        !self.queue.is_empty()
    }

    /// The account whose turn it is.
    pub fn current(&self) -> Option<&Row> {
        let id = self.queue.get(self.ended.len())?;
        self.listing.row(*id)
    }

    /// "2 of 3".
    pub fn position(&self) -> String {
        format!("{} of {}", self.ended.len() + 1, self.queue.len())
    }

    /// The current account's turn ended so.
    pub fn end_current(&mut self, ended: Ended) {
        if let Some(row) = self.current() {
            let who = row.who.clone();
            self.ended.push((who, ended));
        }
    }

    /// Stop importing: the accounts still to come are not added.
    pub fn stop(&mut self) {
        while self.current().is_some() {
            self.end_current(Ended::NotAdded);
        }
    }

    pub fn stopped(&self) -> bool {
        self.ended.iter().any(|(_, e)| *e == Ended::NotAdded)
    }

    pub fn added(&self) -> usize {
        self.ended
            .iter()
            .filter(|(_, e)| *e == Ended::Added)
            .count()
    }
}

#[cfg(test)]
#[path = "import_thunderbird_tests.rs"]
mod tests;
