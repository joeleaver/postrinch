//! Loading Social mode, on the loader's thread (loader.rs): the views, each
//! one's unread count and the groups its places are in, the open view's
//! places, and, while the view panel is open, every group and place to
//! choose from.

use std::collections::HashMap;

use postrinch_commands::{CommandResult, Commands};

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

/// Load what `ask` wants into the window's signals.
pub fn answer(commands: &Commands, state: State, ask: Ask) {
    if let Err(e) = load(commands, state, ask) {
        state.error.send(Some(e.to_string()));
    }
}

fn load(commands: &Commands, state: State, ask: Ask) -> CommandResult<()> {
    let views = commands.social_views()?;
    let mut tallies = HashMap::new();
    let mut groups = HashMap::new();
    let mut open_places = Vec::new();
    for entry in &views {
        let id = entry.view.id.clone();
        let places = commands.social_view_places(&id)?;
        tallies.insert(id.clone(), commands.social_view_tally(&id)?);
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
        state.social.choices.send(Choices {
            groups: commands.social_groups()?,
            places: commands.social_places()?,
        });
    }
    Ok(())
}
