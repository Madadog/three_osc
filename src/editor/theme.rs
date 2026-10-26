//! The editor's themes, each a layout of the controls with its own look: Studio (see
//! [`super::studio`]), Classic (see [`super::classic`]), Flow (see [`super::flow`]), Rack (see
//! [`super::rack`]), Focus (see [`super::focus`]), Compact (see [`super::compact`]) and Vintage
//! (see [`super::vintage`]). Which one the editor shows is a preference rather than part of a
//! project, so it is kept in a settings file (see [`folders::settings()`]) for every instance of
//! the plugin, and read when the first editor opens.

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};

use nih_plug_egui::egui;
use serde::{Deserialize, Serialize};

use super::displays::Palette;
use crate::folders;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Theme {
    #[default]
    Studio,
    Classic,
    Flow,
    Rack,
    Focus,
    Compact,
    Vintage,
}

impl Theme {
    pub const ALL: [Self; 7] = [
        Self::Studio,
        Self::Classic,
        Self::Flow,
        Self::Rack,
        Self::Focus,
        Self::Compact,
        Self::Vintage,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Studio => "Studio",
            Self::Classic => "Classic",
            Self::Flow => "Flow",
            Self::Rack => "Rack",
            Self::Focus => "Focus",
            Self::Compact => "Compact",
            Self::Vintage => "Vintage",
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
            Self::Flow => {
                "The controls in the order the sound flows through them, from left to right, \
                 drawn as a blueprint: the oscillators (with each one's modulation of the next \
                 on the arrow between them), the mixer, the filter, the amp, the effects and \
                 the output, with the other modulation sources in a lane underneath."
            }
            Self::Rack => {
                "Every part of the synth as a module in a rack, like modular hardware: each \
                 modulation source has an output jack to drag onto a knob, and every route is \
                 drawn as a patch cable from its jack to its knob. The knobs show their values \
                 while they're hovered."
            }
            Self::Focus => {
                "One part of the synth at a time, large and explained, on a light page: a \
                 sidebar lists every part with what it's set to, and searches for any control, \
                 and the part picked shows what it does, its display, large knobs, its \
                 modulation routes, and what the control last hovered does."
            }
            Self::Compact => {
                "Every control at once as a small knob that shows its value while it's \
                 hovered, so the whole synth fits a small window, with one large display for \
                 the part last touched. Green on black, like a terminal."
            }
            Self::Vintage => {
                "The front panel of an old polysynth, between wooden end cheeks: every control \
                 is a fader, the choices are lit buttons or displays stepped through with \
                 arrows, and a scope shows the section last pressed on. The faders get longer, \
                 and the panel wider, to fill the window."
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
    let (style, palette) = match theme {
        Theme::Studio => (super::studio::style(), super::studio::PALETTE),
        Theme::Classic => (egui::Style::default(), Palette::DARK),
        Theme::Flow => (super::flow::style(), super::flow::PALETTE),
        Theme::Rack => (super::rack::style(), super::rack::PALETTE),
        Theme::Focus => (super::focus::style(), super::focus::PALETTE),
        Theme::Compact => (super::compact::style(), super::compact::PALETTE),
        Theme::Vintage => (super::vintage::style(), super::vintage::PALETTE),
    };
    ctx.set_style(style);
    super::displays::set_palette(ctx, palette);
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
