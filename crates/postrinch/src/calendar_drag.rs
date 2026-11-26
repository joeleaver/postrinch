//! Dragging events in the calendar's day and week: an event moves by
//! quarter hours, and across days in the week; pressed at its foot, it
//! stretches or shrinks instead. While dragged it shows where it will be.
//! Let go, an event with nobody else to tell is saved at once; one with
//! guests opens in the editor at its new time, for the person to save and
//! tell them, so a drag never sends anyone mail by itself.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use jiff::tz::TimeZone;
use postrinch_commands::Happening;
use postrinch_model::{EventDraft, EventWhen, Timestamp};
use rinch::prelude::*;

use crate::actions::sync;
use crate::calendar_edit::Form;
use crate::calendar_layout::HOUR;
use crate::calendar_ui::CalSpan;
use crate::state::State;

/// The step a drag moves by, in minutes.
const STEP: i64 = 15;
/// How near an event's foot a press stretches it, in px.
const FOOT: f32 = 8.0;
/// The movement under which a press is a click, in px.
const STILL: f32 = 4.0;

/// How the event being dragged has moved.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Moving {
    /// Minutes later (or earlier): its start, or for a stretch its end.
    pub minutes: i64,
    /// Days later in the week.
    pub days: i64,
    /// Stretching its end rather than moving it.
    pub stretch: bool,
    /// How far across it is drawn, in px.
    pub across: f32,
}

thread_local! {
    /// The hours grid, to measure its columns when a drag starts.
    static GRID: RefCell<Option<NodeHandle>> = const { RefCell::new(None) };
    /// A drag just ended: the click its release makes opens nothing.
    static DRAGGED: Cell<bool> = const { Cell::new(false) };
}

pub fn remember_grid(grid: &NodeHandle) {
    GRID.with(|g| *g.borrow_mut() = Some(grid.clone()));
}

/// Whether the click being handled ends a drag (and so should do nothing).
pub fn took_click() -> bool {
    DRAGGED.with(|d| d.replace(false))
}

/// Where an event is once moved: its start and end later by `minutes` (or
/// only its end, stretched, never shorter than a step) and by `days`, kept
/// at the same wall time in `tz`.
pub fn moved(when: EventWhen, m: Moving, tz: &TimeZone) -> EventWhen {
    let EventWhen::Times { start, end } = when else {
        return when;
    };
    let shift = |t: Timestamp, minutes: i64| -> Timestamp {
        let at = jiff::Timestamp::from_millisecond(t.as_millis())
            .map(|t| t.to_zoned(tz.clone()))
            .and_then(|z| z.checked_add(jiff::Span::new().days(m.days)))
            .map(|z| z.timestamp().as_millisecond() + minutes * 60_000);
        Timestamp::from_millis(at.unwrap_or(t.as_millis()))
    };
    if m.stretch {
        let least = start.as_millis() + STEP * 60_000;
        let end = (end.as_millis() + m.minutes * 60_000).max(least);
        EventWhen::Times {
            start,
            end: Timestamp::from_millis(end),
        }
    } else {
        EventWhen::Times {
            start: shift(start, m.minutes),
            end: shift(end, m.minutes),
        }
    }
}

/// Whether the person may move the event: a timed one, in a calendar the
/// account may change, that the account organizes.
pub fn may_move(state: State, h: &Happening) -> bool {
    let writable = state.calendar.calendars.with(|all| {
        all.iter()
            .any(|(a, c)| *a == h.account && c.id == h.event.calendar && c.writable)
    });
    let organizes = h.event.organizer.as_ref().is_none_or(|o| {
        o.address.key() == h.account.key() || o.address.to_string() == h.event.calendar
    });
    writable && organizes && matches!(h.event.when, EventWhen::Times { .. })
}

fn snap(px: f32) -> i64 {
    let minutes = f64::from(px) / HOUR * 60.0;
    (minutes / STEP as f64).round() as i64 * STEP
}

impl State {
    /// Start dragging `h`, pressed somewhere on `block`.
    pub fn drag_event(self, h: Happening, block: Rc<RefCell<Option<NodeHandle>>>) {
        // A new press: an earlier drag's release made no click to take.
        DRAGGED.with(|d| d.set(false));
        if !may_move(self, &h) {
            return;
        }
        let days_shown = match self.calendar.span.get() {
            CalSpan::Week => 7.0,
            _ => 1.0,
        };
        let column = GRID.with(|g| {
            g.borrow()
                .as_ref()
                .and_then(NodeHandle::bounding_rect)
                .map(|r| (r.width - 56.0) / days_shown)
        });
        let stretch: Rc<Cell<Option<bool>>> = Rc::default();
        let dragging = self.calendar.dragging;
        let key = (h.account.clone(), h.event.id.clone());
        let mark = key.clone();
        Drag::absolute()
            .on_move(move |x, y| {
                let Some(start) = Drag::start_context() else {
                    return;
                };
                let foot = match stretch.get() {
                    Some(foot) => foot,
                    None => {
                        let bounds = block.borrow().as_ref().and_then(NodeHandle::bounding_rect);
                        let foot = bounds.is_some_and(|b| start.mouse_y > b.y + b.height - FOOT);
                        stretch.set(Some(foot));
                        foot
                    }
                };
                let (dx, dy) = (x - start.mouse_x, y - start.mouse_y);
                if dx.abs() < STILL && dy.abs() < STILL {
                    return;
                }
                let days = match (foot, column) {
                    (false, Some(w)) if w > 0.0 && days_shown > 1.0 => (dx / w).round() as i64,
                    _ => 0,
                };
                let across = column.map_or(0.0, |w| days as f32 * w);
                let moving = Moving {
                    minutes: snap(dy),
                    days,
                    stretch: foot,
                    across,
                };
                dragging.set(Some((mark.0.clone(), mark.1.clone(), moving)));
            })
            .on_end(move |_, _| {
                let done = dragging
                    .get()
                    .filter(|(a, id, _)| (a, id) == (&key.0, &key.1));
                dragging.set(None);
                if let Some((_, _, m)) = done {
                    DRAGGED.with(|d| d.set(true));
                    if m.minutes != 0 || m.days != 0 {
                        self.drop_event(h.clone(), m);
                    }
                }
            })
            .start();
    }

    /// Put a dragged event where it was let go.
    fn drop_event(self, mut h: Happening, m: Moving) {
        let tz = TimeZone::system();
        let when = moved(h.event.when, m, &tz);
        let guests = h
            .event
            .attendees
            .iter()
            .any(|a| a.mailbox.address.key() != h.account.key());
        h.event.when = when;
        if guests {
            // Their word goes only by the person's Save.
            self.calendar.open.set(None);
            self.calendar.editing.set(Some(Form::of(&h, &tz)));
            return;
        }
        let Some(handle) = sync() else {
            self.error.set(Some(crate::invitations::OFF.into()));
            return;
        };
        // Shown where it was put at once.
        let (account, id) = (h.account.clone(), h.event.id.clone());
        self.calendar.happenings.update(|all| {
            for x in all
                .iter_mut()
                .filter(|x| x.account == account && x.event.id == id)
            {
                x.event.when = when;
            }
        });
        let mut draft = EventDraft::of(&h.event, &h.account);
        draft.time_zone = tz.iana_name().map(str::to_owned);
        let (error, revision) = (self.error, self.revision);
        std::thread::spawn(move || {
            let done = handle.on_calendar(&account, async |sync| {
                sync.change_event(&account, &id, &draft).await
            });
            if let Err(e) = done {
                error.send(Some(crate::invitations::say(&e)));
            }
            revision.update_send(|n| *n += 1);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> Timestamp {
        Timestamp::from_millis(s.parse::<jiff::Timestamp>().unwrap().as_millisecond())
    }

    fn times(s: &str, e: &str) -> EventWhen {
        EventWhen::Times {
            start: at(s),
            end: at(e),
        }
    }

    #[test]
    fn moves_by_quarters_and_days_and_stretches() {
        let tz = TimeZone::UTC;
        let when = times("2026-11-24T10:00:00Z", "2026-11-24T11:00:00Z");
        let m = |minutes, days, stretch| Moving {
            minutes,
            days,
            stretch,
            across: 0.0,
        };
        assert_eq!(
            moved(when, m(30, 1, false), &tz),
            times("2026-11-25T10:30:00Z", "2026-11-25T11:30:00Z")
        );
        assert_eq!(
            moved(when, m(-45, 0, true), &tz),
            times("2026-11-24T10:00:00Z", "2026-11-24T10:15:00Z")
        );
        // Never shorter than a step.
        assert_eq!(
            moved(when, m(-120, 0, true), &tz),
            times("2026-11-24T10:00:00Z", "2026-11-24T10:15:00Z")
        );
        assert_eq!(snap(HOUR as f32 / 2.0), 30);
        assert_eq!(snap(5.0), 0);
        assert_eq!(snap(-HOUR as f32 * 0.3), -15);
    }

    #[test]
    fn days_keep_the_wall_time_across_summer_time() {
        // New York leaves summer time on 1 Nov 2026: 09:00 stays 09:00.
        let tz = TimeZone::get("America/New_York").unwrap();
        let when = times("2026-10-31T13:00:00Z", "2026-10-31T14:00:00Z");
        let m = Moving {
            minutes: 0,
            days: 2,
            stretch: false,
            across: 0.0,
        };
        assert_eq!(
            moved(when, m, &tz),
            times("2026-11-02T14:00:00Z", "2026-11-02T15:00:00Z")
        );
    }
}
