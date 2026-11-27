//! Loading Social mode, on the loader's thread (loader.rs): the views, each
//! one's unread count and the groups its places are in, the open view's
//! places, and, while the view panel is open, every group and place to
//! choose from.

use std::collections::HashMap;

use postrinch_commands::{CommandError, Commands};

use crate::social_shown::marking_groups;
use crate::social_ui::Choices;
use crate::state::State;

/// What Social wants loaded.
#[derive(Debug, Clone)]
pub struct Ask {
    /// The open view's id.
    pub open: String,
    /// Whether the panel is open, and wants what it offers.
    pub panel: bool,
}

/// Load what `ask` wants into the window's signals. What loads is shown
/// even where some of it fails: a view whose places or count cannot be
/// read shows without them, and the first failure is said.
pub fn answer(commands: &Commands, state: State, ask: Ask) {
    let mut trouble: Option<String> = None;
    let mut note = |e: CommandError| {
        trouble.get_or_insert_with(|| e.to_string());
    };
    let views = match commands.social_views() {
        Ok(views) => views,
        Err(e) => {
            state.error.send(Some(e.to_string()));
            return;
        }
    };
    let mut tallies = HashMap::new();
    let mut groups = HashMap::new();
    let mut open_places = Vec::new();
    for entry in &views {
        let id = entry.view.id.clone();
        match commands.social_view_tally(&id) {
            Ok(tally) => {
                tallies.insert(id.clone(), tally);
            }
            Err(e) => note(e),
        }
        let places = match commands.social_view_places(&id) {
            Ok(places) => places,
            Err(e) => {
                note(e);
                continue;
            }
        };
        groups.insert(id.clone(), marking_groups(entry, &places));
        if id == ask.open {
            open_places = places;
        }
    }
    // The open view's places are sent first: the header waits on them.
    state.social.places.send((ask.open, open_places));
    state.social.views.send(views);
    state.social.tallies.send(tallies);
    state.social.groups.send(groups);
    if ask.panel {
        match commands.social_groups().and_then(|groups| {
            Ok(Choices {
                groups,
                places: commands.social_places()?,
            })
        }) {
            Ok(choices) => state.social.choices.send(choices),
            Err(e) => note(e),
        }
    }
    if let Some(trouble) = trouble {
        state.error.send(Some(trouble));
    }
}
