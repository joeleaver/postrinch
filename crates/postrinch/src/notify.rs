//! Desktop notices (DESIGN.md "Notifications"): new mail in the views the
//! person chose, reminders before events, and agents' proposals and
//! refused mail waiting. Every half minute, and whenever sync changes the
//! store, the loader looks for mail that arrived since the window opened
//! and reminders that fell due; each is told once. A click on a notice
//! brings the window forward and opens what it is about.

use std::cell::RefCell;
use std::collections::HashSet;

use jiff::tz::TimeZone;
use postrinch_commands::{Arrived, Due, Watched};
use postrinch_model::{Answer, EventWhen, Timestamp};
use rinch::notification::Notification;
use rinch::prelude::*;
use rinch::{Effect, untracked};

use crate::loader::Request;
use crate::notices::{NoticePrefs, Several, Shows};
use crate::state::{State, View};

/// How often the loader is asked, for reminders between syncs.
const EVERY: std::time::Duration = std::time::Duration::from_secs(30);
/// How far back a reminder still shows, after the window was asleep.
const LATE_MS: i64 = 15 * 60_000;
/// The most mail told at once before it becomes one notice.
const MOST: usize = 3;

/// What the loader found to tell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Found {
    pub arrived: Vec<Arrived>,
    pub due: Vec<Due>,
}

/// The notices' signals. `Copy`, like the window's state.
#[derive(Clone, Copy)]
pub struct Notices {
    pub prefs: Signal<NoticePrefs>,
    pub found: Signal<Found>,
    tick: Signal<u64>,
    /// When the window opened: nothing before it is told.
    opened: Signal<i64>,
}

thread_local! {
    /// What was told already: messages, and reminders as event and minute.
    static TOLD: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

fn told(key: String) -> bool {
    TOLD.with(|t| !t.borrow_mut().insert(key))
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

impl Notices {
    pub fn new(prefs: NoticePrefs) -> Self {
        Self {
            prefs: Signal::new(prefs),
            found: Signal::new(Found::default()),
            tick: Signal::new(0),
            opened: Signal::new(now_ms()),
        }
    }
}

impl State {
    /// Ask the loader for what to tell, every half minute and after sync.
    pub fn watch_notices(self, ask: impl Fn(Request) + 'static) {
        let tick = self.notices.tick;
        let _ = std::thread::Builder::new()
            .name("notices".into())
            .spawn(move || {
                loop {
                    std::thread::sleep(EVERY);
                    tick.update_send(|n| *n += 1);
                }
            });
        Effect::new(move || {
            self.notices.tick.get();
            self.revision.get();
            let prefs = self.notices.prefs.get();
            let views = untracked(|| self.views.get());
            let calendars = untracked(|| self.calendar.calendars.get());
            let opened = untracked(|| self.notices.opened.get());
            let mut watched: Vec<Watched> = if prefs.mail {
                views
                    .iter()
                    .filter(|v| prefs.view_on(&v.view.key))
                    .map(|v| Watched {
                        name: v.view.name.clone(),
                        query: v.view.query.clone(),
                        accounts: v.view.accounts.clone(),
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if prefs.invitations {
                // Invitations wherever they land in the inbox.
                watched.push(Watched {
                    name: INVITATIONS.into(),
                    query: "in:inbox".into(),
                    accounts: Vec::new(),
                });
            }
            let now = now_ms();
            let reminders = prefs.reminders.then(|| {
                let after = opened.max(now - LATE_MS);
                (
                    Timestamp::from_millis(after),
                    Timestamp::from_millis(now),
                    prefs.rules(&calendars),
                )
            });
            ask(Request::Notices {
                watched,
                since: Timestamp::from_millis(opened),
                known: prefs.known_only,
                reminders,
            });
        });
        Effect::new(move || {
            let found = self.notices.found.get();
            untracked(|| self.tell(found));
        });
        self.watch_waiting();
    }

    fn tell(self, found: Found) {
        let prefs = self.notices.prefs.get();
        let tz = TimeZone::system();
        let now = jiff::Zoned::now().with_time_zone(tz.clone());
        let weekday = now.weekday().to_monday_zero_offset() as usize;
        let quiet = prefs.quiet_at(weekday, now.time());
        let mail_quiet = quiet || (prefs.not_in_front && window_focused());
        tracing::debug!(
            "notices: {} new messages, {} reminders; quiet {quiet}, in front {}",
            found.arrived.len(),
            found.due.len(),
            window_focused()
        );
        let fresh: Vec<Arrived> = found
            .arrived
            .into_iter()
            .filter(|a| a.view != INVITATIONS || a.invitation)
            .filter(|a| !told(format!("m|{}|{}", a.account.key(), a.message.0)))
            .collect();
        if !mail_quiet && !fresh.is_empty() {
            self.tell_mail(&prefs, fresh);
        }
        for due in found.due {
            let key = format!(
                "r|{}|{}|{}",
                due.happening.event.id,
                due.minutes,
                due.at.as_millis()
            );
            if told(key) || (quiet && !prefs.quiet_reminders) {
                continue;
            }
            self.tell_reminder(due, &tz);
        }
    }

    fn tell_mail(self, prefs: &NoticePrefs, mail: Vec<Arrived>) {
        let together = match prefs.several {
            Several::EachUpToThree => mail.len() > MOST,
            Several::OneForAll => mail.len() > 1,
        };
        if together {
            let mut views: Vec<&str> = mail
                .iter()
                .map(|a| {
                    if a.view == INVITATIONS {
                        "Inbox"
                    } else {
                        a.view.as_str()
                    }
                })
                .collect();
            views.dedup();
            let title = format!("{} new messages in {}", mail.len(), views.join(", "));
            let mut who: Vec<&str> = mail.iter().map(|a| a.from.as_str()).collect();
            who.dedup();
            let body = match prefs.shows {
                Shows::Nothing => String::new(),
                _ => who.join(", "),
            };
            let state = self;
            show(Notification::new(title).body(body).on_click(move || {
                show_current_window();
                state.view.set(View::Mail);
            }));
            return;
        }
        for a in mail {
            let what = if a.invitation { "Invitation" } else { "Mail" };
            let (title, body) = match prefs.shows {
                Shows::Everything => (
                    with_kind(a.invitation, &a.from),
                    format!("{}\n{}", a.subject, a.snippet),
                ),
                Shows::WhoAndSubject => (with_kind(a.invitation, &a.from), a.subject.clone()),
                Shows::Nothing => (
                    format!("New {} in {}", what.to_lowercase(), view_name(&a)),
                    String::new(),
                ),
            };
            let (state, row) = (self, (a.account.clone(), a.thread.clone(), None));
            show(Notification::new(title).body(body).on_click(move || {
                show_current_window();
                state.view.set(View::Mail);
                state.open_row(row.clone());
            }));
        }
    }

    fn tell_reminder(self, due: Due, tz: &TimeZone) {
        let h = due.happening;
        let e = &h.event;
        let title = e.summary.clone().unwrap_or_else(|| "(no title)".into());
        let starts = match e.when {
            EventWhen::Times { start, .. } => start.as_millis(),
            EventWhen::Days { .. } => crate::calendar_layout::span_day(&e.when, tz)
                .map_or(0, |d| crate::calendar_ui::midnight(d).as_millis()),
        };
        let left = (starts - now_ms()) / 60_000;
        let when = match e.when {
            EventWhen::Days { .. } => "Today".to_owned(),
            EventWhen::Times { .. } if left <= 0 => {
                format!("Now, {}", crate::dates::event_hours(&e.when, tz))
            }
            EventWhen::Times { .. } if left < 60 => format!(
                "In {left} minute{}, {}",
                if left == 1 { "" } else { "s" },
                crate::dates::event_hours(&e.when, tz)
            ),
            EventWhen::Times { .. } => crate::dates::event(&e.when, tz),
        };
        let mut body = when;
        if let Some(place) = &e.location {
            body.push_str(&format!("\n{place}"));
        }
        if e.guest && e.answer.is_none() {
            body.push_str("\nNot answered yet");
        } else if e.answer == Some(Answer::Maybe) {
            body.push_str("\nYou said maybe");
        }
        let (state, happening) = (self, h.clone());
        show(Notification::new(title).body(body).on_click(move || {
            show_current_window();
            state.view.set(View::Calendar);
            if let Some(day) =
                crate::calendar_layout::span_day(&happening.event.when, &TimeZone::system())
            {
                state.calendar_go(day, None);
            }
            state.calendar.pending.set(Some((
                happening.account.clone(),
                happening.event.id.clone(),
            )));
        }));
    }

    /// Send a notice now, to see how they look.
    pub fn test_notice(self) {
        show(
            Notification::new("Postrinch")
                .body("This is how a notice looks. A click on one opens what it is about."),
        );
    }
}

impl State {
    /// Tell the person when something waits for them: an agent's proposal
    /// (a message, a rule, an answer, a calendar change), or mail Gmail
    /// refused. What waits when the window opens is not told again.
    fn watch_waiting(self) {
        let seeded = std::cell::Cell::new(false);
        Effect::new(move || {
            let mut now: Vec<Waiting> = Vec::new();
            for q in self.outgoing.get() {
                let subject = if q.draft.subject.trim().is_empty() {
                    "(no subject)".to_owned()
                } else {
                    q.draft.subject.clone()
                };
                let open = q.clone();
                if q.refused {
                    now.push((
                        format!("refused|{}", q.id),
                        format!("Not sent: {subject}"),
                        Box::new(move || self.open_proposal(&open)),
                    ));
                } else if q.kind == postrinch_store::OutgoingKind::Proposed {
                    let agent = q
                        .draft
                        .proposal
                        .as_ref()
                        .map_or("An agent", |p| p.agent.as_str())
                        .to_owned();
                    now.push((
                        format!("mail|{}", q.id),
                        format!("{agent} wrote a message for you: {subject}"),
                        Box::new(move || self.open_proposal(&open)),
                    ));
                }
            }
            for p in self.rules.proposals.get() {
                let title = format!(
                    "{} proposed a rule: {}",
                    p.proposed.agent, p.proposed.rule.name
                );
                let open = p.clone();
                now.push((
                    format!("rule|{}|{}", p.account.key(), p.proposed.id),
                    title,
                    Box::new(move || {
                        self.view.set(View::Settings);
                        self.settings_section
                            .set(crate::state::SettingsSection::Rules);
                        self.rules.open_proposal(&open);
                    }),
                ));
            }
            for (account, p) in self.invitations.proposals.get() {
                let title = format!(
                    "{} suggests an answer: {}",
                    p.agent,
                    p.summary.clone().unwrap_or_default()
                );
                now.push((
                    format!("answer|{}|{}", account.key(), p.message.0),
                    title,
                    Box::new(move || self.open_answer(&account, &p)),
                ));
            }
            for (account, p) in self.calendar.proposals.get() {
                let title = format!(
                    "{} proposes a calendar change: {}",
                    p.agent,
                    p.change.summary().unwrap_or("(no title)")
                );
                now.push((
                    format!("event|{}|{}", account.key(), p.id),
                    title,
                    Box::new(move || self.open_event_proposal(&account, &p)),
                ));
            }
            let first = !seeded.replace(true);
            let prefs = untracked(|| self.notices.prefs.get());
            for (key, title, open) in now {
                if told(format!("w|{key}")) || first {
                    continue;
                }
                let on = if key.starts_with("refused") {
                    prefs.refused
                } else {
                    prefs.agents
                };
                if on {
                    show(Notification::new(title).on_click(move || {
                        show_current_window();
                        open();
                    }));
                }
            }
        });
    }
}

/// Something waiting for the person: its key, its notice's title, and
/// what a click opens.
type Waiting = (String, String, Box<dyn Fn()>);

/// The view name new mail is told under: invitations come from the inbox.
const INVITATIONS: &str = "\u{1f}invitations";

fn view_name(a: &Arrived) -> &str {
    if a.view == INVITATIONS {
        "Inbox"
    } else {
        &a.view
    }
}

fn with_kind(invitation: bool, from: &str) -> String {
    if invitation {
        format!("Invitation from {from}")
    } else {
        from.to_owned()
    }
}

/// Show a notice; trouble is the log's (a desktop without a notice
/// daemon, say).
pub(crate) fn show(n: Notification) {
    if let Err(e) = n.show() {
        tracing::warn!("a notice could not be shown: {e}");
    }
}
