//! Senders or recipients a view looks for (Has: any of them) or leaves out
//! (Not), chosen as labels are (chooser.rs): the typed text itself (a
//! domain, say), or anyone mail was from or to who matches it, found on
//! the loader's quick thread. They write `from:`, `-from:` and
//! `{from:a from:b}` into the view's search (`to:` likewise).

use postrinch_commands::{Filters, LabelPick};
use rinch::prelude::*;

use crate::state::State;
use crate::ui::chooser::{Offer, Wiring, chooser};
use crate::ui::view_panel_mail::{change, filters};

/// Which people a picker chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    From,
    To,
}

impl Side {
    fn name(self) -> &'static str {
        match self {
            Side::From => "From",
            Side::To => "To",
        }
    }

    /// The filter's lists for this side: any of, and none of.
    fn lists(self, f: &mut Filters) -> (&mut Vec<String>, &mut Vec<String>) {
        match self {
            Side::From => (&mut f.from, &mut f.not_from),
            Side::To => (&mut f.to, &mut f.not_to),
        }
    }
}

#[component]
pub fn people_picker(state: State, side: Side) -> NodeHandle {
    let wiring = Wiring {
        chosen: move || {
            let mut f = filters(state);
            let (has, not) = side.lists(&mut f);
            let mut all: Vec<(String, LabelPick)> =
                has.drain(..).map(|p| (p, LabelPick::Has)).collect();
            all.extend(not.drain(..).map(|p| (p, LabelPick::Not)));
            all
        },
        offered: move |typed: &str| {
            let mut offers = vec![Offer {
                value: typed.to_owned(),
                shown: format!("Anyone matching \u{201c}{typed}\u{201d}"),
                faint: String::new(),
                dot: None,
            }];
            state.people.with(|(part, people)| {
                if part.trim() == typed {
                    offers.extend(people.iter().map(|p| Offer {
                        value: p.address.clone(),
                        shown: match &p.name {
                            Some(name) => format!("{name} \u{2039}{}\u{203a}", p.address),
                            None => p.address.clone(),
                        },
                        faint: String::new(),
                        dot: None,
                    }));
                }
            });
            offers
        },
        asked: move |who: &str| {
            let mut f = filters(state);
            let (has, not) = side.lists(&mut f);
            if has.iter().any(|p| p.eq_ignore_ascii_case(who)) {
                Some(LabelPick::Has)
            } else if not.iter().any(|p| p.eq_ignore_ascii_case(who)) {
                Some(LabelPick::Not)
            } else {
                None
            }
        },
        set: move |who: String, pick: Option<LabelPick>| {
            change(state, move |f| {
                let (has, not) = side.lists(f);
                has.retain(|p| !p.eq_ignore_ascii_case(&who));
                not.retain(|p| !p.eq_ignore_ascii_case(&who));
                match pick {
                    Some(LabelPick::Has) => has.push(who),
                    Some(LabelPick::Not) => not.push(who),
                    None => {}
                }
            })
        },
        finding: move |typed: String| state.people_find.set(typed),
    };
    chooser(
        __scope,
        side.name(),
        "Add a name, address or domain",
        wiring,
        None,
    )
}
