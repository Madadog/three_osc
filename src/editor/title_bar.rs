//! The title bar: the plugin's name, the preset browser and the tuning window's button (see
//! [`super::tuning_window`]).

use std::path::PathBuf;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{self, Color32, PopupCloseBehavior, RichText, Stroke, Ui};

use super::{load_wavetable_file, Gui};
use crate::folders;
use crate::params::ThreeOscParams;
use crate::presets::{self, EnumIds, Preset, Targets, USER_CATEGORY};
use crate::ThreeOscPlugin;

const PRESET_COMBO_WIDTH: f32 = 190.0;
/// How long after loading a preset its parameters may still be on their way through the host,
/// in seconds, during which it isn't marked as changed.
const LOAD_SETTLE_SECONDS: f64 = 0.5;
const ERROR_COLOR: Color32 = Color32::from_rgb(255, 120, 100);

/// The presets the browser lists, and what it is doing.
pub struct PresetBrowser {
    /// The factory presets, then the user's, in the order the arrows step through them.
    presets: Vec<Preset>,
    /// Files in the presets folder that couldn't be read as presets.
    broken: Vec<String>,
    folder: PathBuf,
    scanned: bool,
    enum_ids: EnumIds,
    /// What the current preset sets the parameters to (and the preset's name), to tell when
    /// they have been changed.
    targets: Option<(String, Targets)>,
    /// When the last preset was loaded, in the GUI's time.
    loaded_at: f64,
    /// The name being typed in to save a preset as.
    save_name: String,
    /// What went wrong with the last load or save, if anything.
    message: Option<String>,
}

impl PresetBrowser {
    pub fn new() -> Self {
        Self {
            presets: Vec::new(),
            broken: Vec::new(),
            folder: folders::presets(),
            scanned: false,
            enum_ids: EnumIds::new(),
            targets: None,
            loaded_at: f64::NEG_INFINITY,
            save_name: String::new(),
            message: None,
        }
    }

    fn rescan(&mut self) {
        let (user, broken) = presets::user_presets(&self.folder);
        self.presets = presets::factory();
        self.presets.extend(user);
        self.broken = broken;
        self.scanned = true;
        self.targets = None;
    }

    fn is_user_preset(&self, name: &str) -> bool {
        self.presets.iter().any(|preset| preset.name == name && preset.category == USER_CATEGORY)
    }

    /// Sets the parameters to a preset's, through the host.
    fn load(&mut self, index: usize, params: &ThreeOscParams, setter: &ParamSetter, gui_time: f64, executor: &AsyncExecutor<ThreeOscPlugin>) {
        let preset = &self.presets[index];
        let targets = Targets::new(preset, params, &self.enum_ids);
        let context = setter.raw_context;
        for (ptr, target) in &targets.values {
            // Safety: the pointers point into `params`, which outlives the editor
            unsafe {
                if (ptr.unmodulated_normalized_value() - target).abs() > 1e-6 {
                    context.raw_begin_set_parameter(*ptr);
                    context.raw_set_parameter_normalized(*ptr, *target);
                    context.raw_end_set_parameter(*ptr);
                }
            }
        }
        for (osc, path) in targets.files.iter().enumerate() {
            let file = params.wavetable_file(osc);
            if file.path() != *path {
                load_wavetable_file(file, osc, path.clone(), executor);
            }
        }
        *params.preset_name.write().unwrap() = preset.name.clone();
        self.message = (!targets.problems.is_empty()).then(|| {
            format!("Parts of \"{}\" couldn't be loaded: {}.", preset.name, targets.problems.join(", "))
        });
        self.targets = Some((preset.name.clone(), targets));
        self.loaded_at = gui_time;
    }

    /// Whether the parameters have been changed since the current preset was loaded (as far as
    /// can be told: a preset that isn't listed any more can't be compared).
    fn changed(&mut self, params: &ThreeOscParams, name: &str, gui_time: f64) -> bool {
        if gui_time - self.loaded_at < LOAD_SETTLE_SECONDS {
            return false;
        }
        if self.targets.as_ref().map(|(for_name, _)| for_name.as_str()) != Some(name) {
            self.targets = self
                .presets
                .iter()
                .find(|preset| preset.name == name)
                .map(|preset| (name.to_string(), Targets::new(preset, params, &self.enum_ids)));
        }
        self.targets.as_ref().is_some_and(|(_, targets)| !targets.matches(params))
    }

    /// Saves the current settings as a user preset, and makes it the current one.
    fn save(&mut self, params: &ThreeOscParams) {
        let name = self.save_name.trim().to_string();
        let preset = presets::capture(params, &name);
        match presets::save(&self.folder, &preset) {
            Ok(_) => {
                self.rescan();
                *params.preset_name.write().unwrap() = name;
                self.message = None;
            }
            Err(error) => self.message = Some(error),
        }
    }

    /// Why the name being typed in can't be saved under, if it can't.
    fn save_problem(&self) -> Option<String> {
        let name = self.save_name.trim();
        if let Err(problem) = presets::check_name(name) {
            return Some(problem);
        }
        self.presets
            .iter()
            .any(|preset| preset.name == name && preset.category != USER_CATEGORY)
            .then(|| String::from("That's a factory preset's name."))
    }
}

impl Default for PresetBrowser {
    fn default() -> Self {
        Self::new()
    }
}

/// The plugin's name and version, the presets and the tuning, and some help at the right.
pub fn title_bar(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    ui.horizontal(|ui| {
        ui.heading("Three Osc");
        ui.label(RichText::new(format!("v{}", ThreeOscPlugin::VERSION)).weak());
        ui.add_space(14.0);
        presets(ui, params, setter, gui);
        ui.add_space(10.0);
        super::tuning_window::button(ui, params, &mut gui.state.tuning);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Double-click a slider to reset it, Shift+drag for fine control, click a \
                         value to type it in. Hover a label for help.",
                    )
                    .weak(),
                )
                .truncate(),
            );
        });
    });
}

/// The preset browser: arrows to step through the presets, a list of them, and saving.
fn presets(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let time = ui.input(|input| input.time);
    let browser = &mut gui.state.presets;
    if !browser.scanned {
        browser.rescan();
    }
    let current = params.preset_name.read().unwrap().clone();
    let index = browser.presets.iter().position(|preset| preset.name == current);
    let count = browser.presets.len();
    let mut picked = None;

    if arrow_button(ui, true).on_hover_text("The previous preset.").clicked() && count > 0 {
        picked = Some(index.map_or(count - 1, |index| (index + count - 1) % count));
    }
    if arrow_button(ui, false).on_hover_text("The next preset.").clicked() && count > 0 {
        picked = Some(index.map_or(0, |index| (index + 1) % count));
    }

    let changed = browser.changed(params, &current, time);
    let shown = if changed { format!("{current} *") } else { current.clone() };
    let mut rescan = false;
    egui::ComboBox::from_id_salt("preset")
        .selected_text(RichText::new(shown).strong())
        .width(PRESET_COMBO_WIDTH)
        .height(560.0)
        .truncate()
        .show_ui(ui, |ui| {
            let mut category = "";
            for (i, preset) in browser.presets.iter().enumerate() {
                if preset.category != category {
                    category = &preset.category;
                    ui.label(RichText::new(category).small().weak());
                }
                if ui.selectable_label(Some(i) == index, format!("  {}", preset.name)).clicked() {
                    picked = Some(i);
                }
            }
            if !browser.presets.iter().any(|preset| preset.category == USER_CATEGORY) {
                ui.label(RichText::new(USER_CATEGORY).small().weak());
                ui.label(RichText::new("  Save a preset to see it here").weak());
            }
            if !browser.broken.is_empty() {
                ui.label(
                    RichText::new(format!("Not presets: {}", browser.broken.join(", ")))
                        .small()
                        .color(ERROR_COLOR),
                );
            }
            ui.separator();
            if ui.button("Rescan the presets folder").clicked() {
                rescan = true;
            }
        })
        .response
        .on_hover_text(if changed {
            "The preset. The * means its settings have been changed since it was loaded."
        } else {
            "The preset. Pick one to load it; the arrows step through them."
        });
    if rescan {
        browser.rescan();
    }
    if let Some(index) = picked {
        browser.load(index, params, setter, time, gui.executor);
    }

    // Saving, in a popup under the button
    let save = ui.button("Save").on_hover_text("Saves the current settings as a preset of your own.");
    let popup = ui.make_persistent_id("save preset");
    if save.clicked() {
        browser.save_name = if browser.is_user_preset(&current) { current.clone() } else { String::new() };
        ui.memory_mut(|memory| memory.toggle_popup(popup));
    }
    egui::popup_below_widget(ui, popup, &save, PopupCloseBehavior::CloseOnClickOutside, |ui| {
        // (Popups lay out justified, which spreads wrapped text out)
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| save_popup(ui, params, browser, save.clicked()));
    });

    if let Some(message) = &browser.message {
        let clear = ui
            .add(egui::Label::new(RichText::new("⚠").color(ERROR_COLOR)).sense(egui::Sense::click()))
            .on_hover_text(format!("{message}\n(Click to dismiss.)"))
            .clicked();
        if clear {
            browser.message = None;
        }
    }
}

/// The save popup's contents: a name to save the preset as. `opened` is whether it was opened
/// this frame.
fn save_popup(ui: &mut Ui, params: &ThreeOscParams, browser: &mut PresetBrowser, opened: bool) {
    ui.set_width(250.0);
    ui.label("Save the current settings as:");
    let edit = ui.add(
        egui::TextEdit::singleline(&mut browser.save_name)
            .hint_text("Preset name")
            .desired_width(240.0),
    );
    if opened {
        edit.request_focus();
    }
    let entered = edit.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
    let problem = browser.save_problem();
    let exists = browser.is_user_preset(browser.save_name.trim());
    let label = if exists { "Replace" } else { "Save" };
    let clicked = ui.add_enabled(problem.is_none(), egui::Button::new(label)).clicked();
    if problem.is_none() && (clicked || entered) {
        browser.save(params);
        ui.memory_mut(|memory| memory.close_popup());
    }
    match &problem {
        Some(problem) if !browser.save_name.is_empty() => {
            ui.label(RichText::new(problem).small().color(ERROR_COLOR));
        }
        _ if exists => {
            ui.label(RichText::new("Replaces your preset of that name.").small().weak());
        }
        _ => {}
    }
    ui.add(
        egui::Label::new(
            RichText::new(format!("Saved in {}", browser.folder.display())).small().weak(),
        )
        .wrap(),
    );
}

/// A small button with an arrow pointing left or right, drawn rather than a glyph (which the
/// default fonts may lack).
fn arrow_button(ui: &mut Ui, left: bool) -> egui::Response {
    let size = egui::vec2(18.0, ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let visuals = ui.style().interact(&response);
    ui.painter().rect(rect, visuals.corner_radius, visuals.weak_bg_fill, visuals.bg_stroke, egui::StrokeKind::Inside);
    let centre = rect.center();
    let (x, y) = (3.5, 4.5);
    let points = if left {
        vec![centre + egui::vec2(-x, 0.0), centre + egui::vec2(x, -y), centre + egui::vec2(x, y)]
    } else {
        vec![centre + egui::vec2(x, 0.0), centre + egui::vec2(-x, -y), centre + egui::vec2(-x, y)]
    };
    ui.painter().add(egui::Shape::convex_polygon(points, visuals.fg_stroke.color, Stroke::NONE));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browser_lists_factory_then_user_presets() {
        let dir = std::env::temp_dir().join(format!("three_osc_browser_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut browser = PresetBrowser::new();
        browser.folder = dir.clone();
        browser.rescan();
        let factory = presets::factory().len();
        assert_eq!(browser.presets.len(), factory);

        let params = ThreeOscParams::default();
        browser.save_name = String::from("Init");
        assert_eq!(browser.save_problem().as_deref(), Some("That's a factory preset's name."));
        browser.save_name = String::from("bad/name");
        assert!(browser.save_problem().is_some());
        browser.save_name = String::from(" Mine ");
        assert_eq!(browser.save_problem(), None);
        browser.save(&params);
        assert_eq!(browser.message, None);
        assert_eq!(*params.preset_name.read().unwrap(), "Mine");
        assert_eq!(browser.presets.len(), factory + 1);
        assert!(browser.is_user_preset("Mine") && !browser.is_user_preset("Init"));
        // The saved preset is what the parameters are
        assert!(!browser.changed(&params, "Mine", 10.0));
        // ...and so is Init, the defaults; but not a preset that changes things
        assert!(!browser.changed(&params, "Init", 10.0));
        assert!(browser.changed(&params, "Reese", 10.0));
        // Not while a preset that was just loaded settles
        browser.loaded_at = 9.9;
        assert!(!browser.changed(&params, "Reese", 10.0));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
