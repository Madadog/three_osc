//! The editor's themes, each a layout of the controls with its own look: Studio (see
//! [`super::studio`]) and Classic (see [`super::classic`]). Which one the editor shows is a
//! preference rather than part of a project, so it is kept in a settings file (see
//! [`folders::settings()`]) for every instance of the plugin, and read when the first editor
//! opens.

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use nih_plug_egui::egui;
use serde::{Deserialize, Serialize};

use crate::folders;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Studio,
    Classic,
}

impl Theme {
    pub const ALL: [Self; 2] = [Self::Studio, Self::Classic];

    pub fn name(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::Classic => "Classic",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Studio => {
                "Knobs grouped in cards, with the envelopes, LFOs and other modulation sources \
                 always in view below them, and pages for the effects, the modulation routes \
                 and the advanced settings. The layout stretches to fill the window."
            }
            Self::Classic => {
                "The original layout: every control on one page as a slider, with the \
                 modulation matrix always in view and tabs for the effects and the advanced \
                 settings."
            }
        }
    }

    fn from_u8(value: u8) -> Self {
        Self::ALL.get(value as usize).copied().unwrap_or_default()
    }
}

/// What the settings file holds.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Settings {
    theme: Theme,
}

/// The theme every editor shows, or [`UNREAD`] until the settings file has been read.
static THEME: AtomicU8 = AtomicU8::new(UNREAD);
const UNREAD: u8 = u8::MAX;

/// The theme the editors show: the one last picked, or Studio if none has been.
pub fn current() -> Theme {
    match THEME.load(Ordering::Relaxed) {
        UNREAD => {
            let theme = load(&folders::settings());
            THEME.store(theme as u8, Ordering::Relaxed);
            theme
        }
        value => Theme::from_u8(value),
    }
}

/// Switches every editor to `theme` and saves it for next time, or says why it couldn't be
/// saved (the switch happens either way).
pub fn set(theme: Theme) -> Result<(), String> {
    THEME.store(theme as u8, Ordering::Relaxed);
    save(&folders::settings(), theme)
}

/// The theme saved in a settings file, or the default if there's none (or it can't be read).
fn load(path: &Path) -> Theme {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Settings>(&text).ok())
        .map(|settings| settings.theme)
        .unwrap_or_default()
}

fn save(path: &Path, theme: Theme) -> Result<(), String> {
    let problem = |error: &dyn std::fmt::Display| format!("Couldn't save the theme in {}: {error}", path.display());
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|error| problem(&error))?;
    }
    let text = serde_json::to_string_pretty(&Settings { theme }).map_err(|error| problem(&error))?;
    std::fs::write(path, text).map_err(|error| problem(&error))
}

/// Gives the editor the theme's look.
pub fn apply(ctx: &egui::Context, theme: Theme) {
    ctx.set_style(match theme {
        Theme::Studio => super::studio::style(),
        Theme::Classic => egui::Style::default(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_is_saved_and_read_back() {
        let dir = std::env::temp_dir().join(format!("three_osc_theme_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("Settings.json");
        // Without a file, or with one that can't be read, it's the default
        assert_eq!(load(&path), Theme::Studio);
        save(&path, Theme::Classic).unwrap();
        assert_eq!(load(&path), Theme::Classic);
        save(&path, Theme::Studio).unwrap();
        assert_eq!(load(&path), Theme::Studio);
        std::fs::write(&path, "{\"theme\": \"Neon\"}").unwrap();
        assert_eq!(load(&path), Theme::Studio);
        // Settings it doesn't know about are fine
        std::fs::write(&path, "{\"theme\": \"Classic\", \"scale\": 1.5}").unwrap();
        assert_eq!(load(&path), Theme::Classic);
        std::fs::remove_dir_all(&dir).unwrap();

        for theme in Theme::ALL {
            assert_eq!(Theme::from_u8(theme as u8), theme);
        }
    }
}
