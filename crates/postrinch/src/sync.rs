//! Running sync in the background and reporting it to the window. Sync
//! itself starts in `sync_start`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use postrinch_model::EmailAddress;
use postrinch_sync::{Phase, Progress, SyncEvent};
use rinch::prelude::*;
use tokio::sync::Notify;

pub use crate::sync_start::{SyncHandle, start};

/// One account's sync, as the status line shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountSync {
    pub account: EmailAddress,
    /// What it is doing, or how the last pass ended.
    pub line: String,
    /// `done` of `total`, while downloading.
    pub progress: Option<(usize, usize)>,
    pub how: SyncHow,
}

/// What the status line says of an account that has to sign in again.
pub const NEEDS_SIGN_IN: &str = "Needs to sign in";

/// Where an account's sync stands, for the status line's summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncHow {
    /// Checking, fetching or downloading.
    Working,
    /// Its last pass ended well.
    Idle,
    /// It cannot sync until the person signs it in again.
    NeedsSignIn,
    /// Anything else that went wrong; it is tried again on its own.
    Trouble,
}

impl SyncHow {
    pub fn trouble(self) -> bool {
        matches!(self, SyncHow::NeedsSignIn | SyncHow::Trouble)
    }
}

/// The last of the accounts' passes to end, numbered so that the same
/// account's next pass is news too. What a pass keeps of an account
/// beyond its mail (an IMAP account's folders) is read again from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassEnd {
    pub account: EmailAddress,
    pub number: u64,
}

/// Notes in `ended` that a pass of `account` ended, from any thread.
pub fn pass_ended(ended: Signal<Option<PassEnd>>, account: EmailAddress) {
    ended.update_send(move |last| {
        let number = last.as_ref().map_or(1, |l| l.number + 1);
        *last = Some(PassEnd { account, number });
    });
}

/// Turn a sync event into the status line's word on the account, from the
/// sync thread. `ended` is told of each pass that ended: once its sync
/// part is done ([`SyncEvent::Synced`], before the hours a first
/// download can take), or when it stopped short.
pub(crate) fn report(
    event: SyncEvent,
    status: Signal<Vec<AccountSync>>,
    revision: Signal<u64>,
    ended: Signal<Option<PassEnd>>,
    wake: &Notify,
) {
    if let SyncEvent::Synced { account, .. }
    | SyncEvent::Failed { account, .. }
    | SyncEvent::NeedsSignIn { account }
    | SyncEvent::Busy { account, .. } = &event
    {
        pass_ended(ended, account.clone());
    }
    let (account, line, progress, how, changed) = match event {
        SyncEvent::Started { account } => (
            account,
            "Checking for new mail".to_owned(),
            None,
            SyncHow::Working,
            false,
        ),
        SyncEvent::Progress { account, progress } => {
            // Mail arriving shows as it arrives, not when the pass ends,
            // which for a new account's first sync is hours away.
            let changed = adds_mail(progress) && refresh_due();
            let (line, progress) = describe(progress);
            (account, line, progress, SyncHow::Working, changed)
        }
        // A pass that found nothing new changes nothing the window shows.
        SyncEvent::Synced { account, report } => {
            crate::rules::due_now(account.clone());
            let changed = report.headers + report.fetched + report.relabelled + report.removed > 0;
            (
                account,
                "New mail up to date".to_owned(),
                None,
                SyncHow::Idle,
                changed,
            )
        }
        SyncEvent::Done {
            account,
            downloaded,
        } => (
            account,
            "Up to date".to_owned(),
            None,
            SyncHow::Idle,
            downloaded > 0,
        ),
        SyncEvent::Failed {
            account,
            error,
            retry_in,
        } => {
            tracing::warn!("{account}: {error}");
            let line = format!(
                "Could not sync; trying again in {} s",
                retry_in.as_secs().max(1)
            );
            (account, line, None, SyncHow::Trouble, false)
        }
        SyncEvent::NeedsSignIn { account } => (
            account,
            NEEDS_SIGN_IN.to_owned(),
            None,
            SyncHow::NeedsSignIn,
            true,
        ),
        // The server's limit, not the account's sign-in: say so in its
        // words, and when it is tried again; the account is left signed in.
        SyncEvent::Busy {
            account,
            message,
            retry_in,
        } => {
            tracing::info!("{account}: server busy: {message}");
            (
                account,
                busy_line(&message, retry_in),
                None,
                SyncHow::Trouble,
                false,
            )
        }
        // Mail that went: show what is left to send, and fetch the server's
        // copy into Sent now rather than at the next check.
        SyncEvent::Sent { report, mail, .. } if report.refused.is_empty() && mail > 0 => {
            revision.update_send(|n| *n += 1);
            wake.notify_one();
            return;
        }
        // Changes are shown as made; sending them is news only when it
        // goes wrong.
        SyncEvent::Sent { report, .. } if report.refused.is_empty() => return,
        SyncEvent::Sent {
            account, report, ..
        } => {
            tracing::warn!(
                "{account}: the server refused changes: {:?}",
                report.refused
            );
            let line = match report.refused.len() {
                1 => "The server refused a change; showing its mail".to_owned(),
                n => format!("The server refused {n} changes; showing its mail"),
            };
            (account, line, None, SyncHow::Trouble, true)
        }
        SyncEvent::SendFailed {
            account,
            error,
            retry_in,
        } => {
            tracing::warn!("{account}: sending changes: {error}");
            let line = format!(
                "Could not send changes; trying again in {} s",
                retry_in.as_secs().max(1)
            );
            (account, line, None, SyncHow::Trouble, false)
        }
    };
    status.update_send(move |all| {
        let entry = AccountSync {
            account: account.clone(),
            line,
            progress,
            how,
        };
        match all.iter_mut().find(|s| s.account == account) {
            Some(existing) => *existing = entry,
            None => all.push(entry),
        }
    });
    if changed {
        revision.update_send(|n| *n += 1);
    }
}

/// Whether sync is storing mail the lists should show.
fn adds_mail(progress: Progress) -> bool {
    matches!(
        progress,
        Progress::Working {
            phase: Phase::Headers | Phase::Changes,
            ..
        }
    )
}

/// How often the window reloads while sync adds mail. A reload of a big
/// folder takes seconds, so more often keeps the loader busy for nothing.
const REFRESH_EVERY: Duration = Duration::from_secs(15);

/// Whether it has been [`REFRESH_EVERY`] since the last reload this asked
/// for; if so, it counts as asked for now.
fn refresh_due() -> bool {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let due = last.is_none_or(|t| t.elapsed() >= REFRESH_EVERY);
    if due {
        *last = Some(Instant::now());
    }
    due
}

fn describe(progress: Progress) -> (String, Option<(usize, usize)>) {
    match progress {
        Progress::Listing { found } => (format!("Listing mail: {}", thousands(found)), None),
        Progress::Working { phase, done, total } => {
            let what = match phase {
                Phase::Headers => "Fetching new mail",
                Phase::Changes => "Applying changes",
                Phase::Bodies => "Downloading older mail",
            };
            (
                format!("{what}: {} of {}", thousands(done), thousands(total)),
                Some((done, total)),
            )
        }
    }
}

/// An account's line for a busy server: when it is tried again, then
/// the server's own sentence.
fn busy_line(message: &str, retry_in: Duration) -> String {
    let message = message.trim();
    let when = wait_in_words(retry_in);
    if message.is_empty() {
        format!("Server busy; trying again in {when}")
    } else {
        format!("Server busy; trying again in {when}: {message}")
    }
}

/// A wait in words: seconds under two minutes, else whole
/// minutes, rounded up so it never promises sooner than it is.
fn wait_in_words(wait: Duration) -> String {
    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    if secs < 120 {
        format!("{} s", secs.max(1))
    } else {
        format!("{} min", secs.div_ceil(60))
    }
}

/// 3718 as "3,718".
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{busy_line, thousands, wait_in_words};

    #[test]
    fn busy_says_when_and_the_servers_words() {
        assert_eq!(
            busy_line(
                "Too many simultaneous connections. (Failure)",
                Duration::from_secs(25)
            ),
            "Server busy; trying again in 25 s: Too many simultaneous connections. (Failure)"
        );
        assert_eq!(
            busy_line("  ", Duration::from_secs(600)),
            "Server busy; trying again in 10 min"
        );
    }

    #[test]
    fn waits_in_words() {
        assert_eq!(wait_in_words(Duration::ZERO), "1 s");
        assert_eq!(wait_in_words(Duration::from_millis(14_200)), "15 s");
        assert_eq!(wait_in_words(Duration::from_secs(119)), "119 s");
        assert_eq!(wait_in_words(Duration::from_secs(120)), "2 min");
        assert_eq!(wait_in_words(Duration::from_secs(121)), "3 min");
        assert_eq!(wait_in_words(Duration::from_secs(900)), "15 min");
    }

    /// Where each event leaves the account, as the status line sums it up.
    #[test]
    fn each_event_says_where_the_account_stands() {
        use postrinch_model::EmailAddress;
        use postrinch_sync::{Phase, Progress, SyncEvent};
        use rinch::prelude::Signal;

        use super::{NEEDS_SIGN_IN, SyncHow, report};

        let status = Signal::new(Vec::new());
        let (revision, ended) = (Signal::new(0), Signal::new(None));
        let wake = tokio::sync::Notify::new();
        let account = EmailAddress::parse("ada@example.org").unwrap();
        let after = |event: SyncEvent| {
            report(event, status, revision, ended, &wake);
            let all = status.get();
            assert_eq!(all.len(), 1, "one entry per account");
            (all[0].how, all[0].line.clone(), all[0].progress)
        };
        let a = || account.clone();
        assert_eq!(
            after(SyncEvent::Started { account: a() }).0,
            SyncHow::Working
        );
        let working = after(SyncEvent::Progress {
            account: a(),
            progress: Progress::Working {
                phase: Phase::Bodies,
                done: 3,
                total: 10,
            },
        });
        assert_eq!((working.0, working.2), (SyncHow::Working, Some((3, 10))));
        assert_eq!(
            after(SyncEvent::Done {
                account: a(),
                downloaded: 0
            })
            .0,
            SyncHow::Idle
        );
        let failed = SyncEvent::Failed {
            account: a(),
            error: "no route".into(),
            retry_in: Duration::from_secs(30),
        };
        assert_eq!(after(failed).0, SyncHow::Trouble);
        let busy = SyncEvent::Busy {
            account: a(),
            message: String::new(),
            retry_in: Duration::from_secs(30),
        };
        assert_eq!(after(busy).0, SyncHow::Trouble);
        let signing = after(SyncEvent::NeedsSignIn { account: a() });
        assert_eq!(
            (signing.0, signing.1.as_str()),
            (SyncHow::NeedsSignIn, NEEDS_SIGN_IN)
        );
    }

    #[test]
    fn groups_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(3718), "3,718");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }
}
