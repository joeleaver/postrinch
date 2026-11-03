//! Light or dark: the person's choice, kept in Postrinch's settings file,
//! and the system's preference when they leave it to the system.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use rinch::prelude::*;
use serde::{Deserialize, Serialize};

/// How often the system's preference is checked again, so the window
/// follows it when the person changes it.
const RECHECK: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Appearance {
    /// Follow the system; dark when the system does not say.
    #[default]
    System,
    Light,
    Dark,
}

/// Postrinch's own settings, the ones that belong to this computer rather
/// than to an account.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub appearance: Appearance,
    /// The panes' widths as the person left them, in px.
    #[serde(default)]
    pub sidebar_width: Option<f32>,
    #[serde(default)]
    pub list_width: Option<f32>,
    #[serde(default)]
    pub mark_read: crate::reading::MarkRead,
    #[serde(default)]
    pub after_move: crate::reading::AfterMove,
}

impl Settings {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("postrinch").join("settings.json"))
    }

    /// The saved settings; the defaults when there are none or they do not
    /// read.
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("{} does not read, using defaults: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("this system has no configuration directory")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Whether the system prefers dark. Dark when it does not say, or cannot
/// be asked.
pub fn system_prefers_dark() -> bool {
    !matches!(dark_light::detect(), Ok(dark_light::Mode::Light))
}

/// Keep `system_dark` following the system's preference, from a thread of
/// its own.
pub fn watch_system(system_dark: Signal<bool>) {
    let spawned = thread::Builder::new()
        .name("appearance".into())
        .spawn(move || {
            let mut last = system_prefers_dark();
            loop {
                thread::sleep(RECHECK);
                let now = system_prefers_dark();
                if now != last {
                    last = now;
                    system_dark.send(now);
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("the window will not follow the system's light or dark: {e}");
    }
}

/// Whether the window is dark, given the choice and the system.
pub fn is_dark(appearance: Appearance, system_dark: bool) -> bool {
    match appearance {
        Appearance::System => system_dark,
        Appearance::Light => false,
        Appearance::Dark => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_wins_over_the_system() {
        assert!(is_dark(Appearance::System, true));
        assert!(!is_dark(Appearance::System, false));
        assert!(!is_dark(Appearance::Light, true));
        assert!(is_dark(Appearance::Dark, false));
    }

    #[test]
    fn settings_read_with_defaults_for_what_is_missing() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.appearance, Appearance::System);
        let s: Settings = serde_json::from_str(r#"{"appearance":"Light"}"#).unwrap();
        assert_eq!(s.appearance, Appearance::Light);
    }
}
