//! The Compact theme: every control at once, small, so that the whole synth fits a small
//! window, green on black like a terminal.
//!
//! The knobs have short names, which they show their values in place of while they're hovered
//! or turned. The sections flow across the window, wrapping onto more rows when it's narrow,
//! with their choices beside their titles. One display under them, as large as the window
//! allows, shows the section last pressed on (or the one picked from its tabs), so that the
//! sections needn't have displays of their own. A modulation source's section has a handle in
//! its title to drag onto the knobs.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Layout, RichText, Sense, Stroke, StrokeKind, Ui,
};

use super::catalog::{self, Colors, Control};
use super::knob::{self, Controls};
use super::widgets::{self, ChipStyle, Display};
use super::{
    combo_cell, displays, help, set_param, short_combo_cell, theme_picker, title_bar, tuning_window,
    wavetable_file_combo, wavetable_file_status, Gui, Matrix, MOD_COLOR,
};
use crate::params::{LfoParams, LfoTriggerParam, OscillatorWave, ThreeOscParams};
use crate::synth::modulation::ModSource;

const BACKGROUND: Color32 = Color32::from_rgb(5, 8, 6);
const PANEL: Color32 = Color32::from_rgb(10, 17, 12);
const EDGE: Color32 = Color32::from_rgb(30, 54, 38);
const TEXT: Color32 = Color32::from_rgb(178, 240, 192);
const TEXT_STRONG: Color32 = Color32::from_rgb(228, 255, 233);
const TEXT_WEAK: Color32 = Color32::from_rgb(82, 128, 94);
const WIDGET: Color32 = Color32::from_rgb(14, 26, 18);
const WIDGET_HOVERED: Color32 = Color32::from_rgb(22, 42, 29);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(30, 58, 40);
const SCREEN: Color32 = Color32::from_rgb(2, 5, 3);
const SOUND_COLOR: Color32 = Color32::from_rgb(60, 255, 130);
const GLOBAL_COLOR: Color32 = Color32::from_rgb(80, 220, 255);

const MARGIN: f32 = 6.0;
const GAP: f32 = 6.0;
const PADDING: f32 = 6.0;
const HEADER_HEIGHT: f32 = 26.0;
/// The height of a section's title, which has the section's choices beside it.
const TITLE_HEIGHT: f32 = 22.0;
/// The space between a section's rows.
const ROW_GAP: f32 = 2.0;
/// The width of each knob.
const CELL: f32 = 44.0;
const CHIP_HEIGHT: f32 = 16.0;
const TABS_HEIGHT: f32 = 22.0;
const DISPLAY_MIN: f32 = 150.0;
/// The width of an LFO's section, which has its sync and trigger beside its knobs, and of the
/// MIDI sources' one, which has two rows of chips.
const LFO_WIDTH: f32 = 184.0;
const MIDI_WIDTH: f32 = 300.0;
/// The widths of the combo boxes, which fit their longest choices.
const OSC_WAVE_WIDTH: f32 = 110.0;
const FILE_WIDTH: f32 = 100.0;
const FILTER_MODEL_WIDTH: f32 = 76.0;
const FILTER_TYPE_WIDTH: f32 = 84.0;
const POLYPHONY_WIDTH: f32 = 98.0;
const LFO_WAVE_WIDTH: f32 = 104.0;
/// The width of an LFO's sync and trigger buttons.
const TOGGLE_WIDTH: f32 = 46.0;

const KNOBS: knob::Style = knob::Style {
    font_size: 10.0,
    monospace: true,
    value_below: false,
    max_diameter: 26.0,
    ..knob::Style::STUDIO
};
const COLORS: Colors = Colors {
    sound: SOUND_COLOR,
    modulation: MOD_COLOR,
    global: GLOBAL_COLOR,
};

/// The displays' colours: phosphor on black.
pub const PALETTE: displays::Palette = displays::Palette {
    screen: SCREEN,
    edge: EDGE,
    signal: SOUND_COLOR,
    live: Color32::from_rgb(255, 242, 176),
    guide: Color32::from_rgb(16, 38, 24),
    label: Color32::from_rgb(63, 107, 76),
};

/// The look of the Compact theme, for egui: monospaced green on black.
pub fn style() -> egui::Style {
    let mut style = egui::Style::default();
    let visuals = &mut style.visuals;
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = PANEL;
    visuals.window_stroke = Stroke::new(1.0_f32, EDGE);
    visuals.extreme_bg_color = SCREEN;
    visuals.faint_bg_color = WIDGET;
    visuals.window_corner_radius = CornerRadius::same(2);
    visuals.menu_corner_radius = CornerRadius::same(2);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    widgets.noninteractive.weak_bg_fill = PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    for (widget, fill) in [
        (&mut widgets.inactive, WIDGET),
        (&mut widgets.hovered, WIDGET_HOVERED),
        (&mut widgets.active, WIDGET_ACTIVE),
        (&mut widgets.open, WIDGET_HOVERED),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, EDGE);
        widget.corner_radius = CornerRadius::same(2);
    }
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT_STRONG);
    widgets.active.fg_stroke = Stroke::new(2.0_f32, TEXT_STRONG);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, SOUND_COLOR.gamma_multiply(0.6));
    visuals.selection.bg_fill = Color32::from_rgb(24, 80, 44);
    visuals.selection.stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    let mono = |size: f32| FontId::new(size, FontFamily::Monospace);
    style.text_styles = [
        (egui::TextStyle::Small, mono(10.0)),
        (egui::TextStyle::Body, mono(11.0)),
        (egui::TextStyle::Button, mono(11.0)),
        (egui::TextStyle::Heading, mono(14.0)),
        (egui::TextStyle::Monospace, mono(11.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 2.0);
    style.spacing.interact_size.y = 20.0;
    style
}

fn chips() -> ChipStyle {
    ChipStyle {
        font: FontId::monospace(11.0),
        height: CHIP_HEIGHT,
        ..ChipStyle::new(TEXT_STRONG, BACKGROUND)
    }
}

/// What the Compact layout's display shows.
pub struct CompactState {
    shown: Section,
}

impl CompactState {
    pub fn new() -> Self {
        Self { shown: Section::Oscillator(0) }
    }
}

/// A section of controls, and what the display shows for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Oscillator(usize),
    Filter,
    FilterEnvelope,
    Amp,
    Voice,
    Envelope(usize),
    Lfo(usize),
    Midi,
    Distortion,
    Chorus,
    Delay,
    Reverb,
    Output,
}

impl Section {
    /// The sections, in the order they flow across the window: the sound, then its modulation,
    /// then the effects and the output.
    const ALL: [Self; 18] = [
        Self::Oscillator(0),
        Self::Oscillator(1),
        Self::Oscillator(2),
        Self::Filter,
        Self::FilterEnvelope,
        Self::Amp,
        Self::Voice,
        Self::Envelope(0),
        Self::Envelope(1),
        Self::Lfo(0),
        Self::Lfo(1),
        Self::Lfo(2),
        Self::Midi,
        Self::Distortion,
        Self::Chorus,
        Self::Delay,
        Self::Reverb,
        Self::Output,
    ];

    fn name(self) -> String {
        match self {
            Self::Oscillator(index) => format!("OSC {}", index + 1),
            Self::Filter => String::from("FILTER"),
            Self::FilterEnvelope => String::from("FILTER ENV"),
            Self::Amp => String::from("AMP ENV"),
            Self::Voice => String::from("VOICE"),
            Self::Envelope(index) => format!("ENV {}", index + 1),
            Self::Lfo(index) => format!("LFO {}", index + 1),
            Self::Midi => String::from("MIDI"),
            Self::Distortion => String::from("DIST"),
            Self::Chorus => String::from("CHORUS"),
            Self::Delay => String::from("DELAY"),
            Self::Reverb => String::from("REVERB"),
            Self::Output => String::from("OUTPUT"),
        }
    }

    /// The modulation source the section is, if it is one.
    fn source(self) -> Option<ModSource> {
        match self {
            Self::Oscillator(index) => Some(ModSource::osc(index)),
            Self::Amp => Some(ModSource::VolumeEnv),
            Self::FilterEnvelope => Some(ModSource::FilterEnv),
            Self::Envelope(index) => Some(ModSource::envelope(index)),
            Self::Lfo(index) => Some(ModSource::lfo(index)),
            _ => None,
        }
    }

    /// What the display shows for the section, if it has anything to.
    fn display(self) -> Option<Display> {
        match self {
            Self::Oscillator(index) => Some(Display::Oscillator(index)),
            Self::Filter => Some(Display::Filter),
            Self::FilterEnvelope => Some(Display::FilterEnvelope),
            Self::Amp => Some(Display::Amp),
            Self::Envelope(index) => Some(Display::Envelope(index)),
            Self::Lfo(index) => Some(Display::Lfo(index)),
            Self::Distortion => Some(Display::Distortion),
            Self::Chorus => Some(Display::Chorus),
            Self::Delay => Some(Display::Delay),
            Self::Reverb => Some(Display::Reverb),
            Self::Output => Some(Display::Output),
            Self::Voice | Self::Midi => None,
        }
    }

    /// The section's knobs, with short names, and how many columns they're laid out in.
    fn knobs(self, params: &ThreeOscParams) -> (Vec<Control<'_>>, usize) {
        let fx = &params.fx;
        let (knobs, columns) = match self {
            Self::Oscillator(index) => {
                // Its level, pitch and unison, then its wave's shape and the next oscillator's
                // modulation of it
                let osc = catalog::oscillator(params.oscillator(index), index);
                let mut knobs = [0, 1, 2, 3, 4, 8, 9, 5, 6, 7].map(|knob| osc[knob]).to_vec();
                knobs.extend(catalog::oscillator_modulation(params, index).into_iter().flatten());
                (knobs, 7)
            }
            Self::Filter => {
                let mut knobs = catalog::filter(&params.filter).to_vec();
                knobs.push(catalog::filter_envelope_amount(&params.filter_env));
                (knobs, 5)
            }
            Self::FilterEnvelope => {
                let mut knobs = catalog::envelope(&params.filter_env.envelope).to_vec();
                knobs.push(catalog::filter_envelope_amount(&params.filter_env).labelled("AMT"));
                (knobs, 5)
            }
            Self::Amp => (catalog::envelope(&params.vol_env).to_vec(), 4),
            Self::Voice => (catalog::voice(&params.global).to_vec(), 5),
            Self::Envelope(index) => (catalog::envelope(&params.mod_envs[index].envelope).to_vec(), 4),
            Self::Lfo(index) => (catalog::lfo(&params.lfos[index], index).to_vec(), 3),
            Self::Midi => (Vec::new(), 1),
            Self::Distortion => (catalog::distortion(&fx.distortion).to_vec(), 2),
            Self::Chorus => (catalog::chorus(&fx.chorus).to_vec(), 3),
            Self::Delay => (catalog::delay(&fx.delay).to_vec(), 3),
            Self::Reverb => (catalog::reverb(&fx.reverb).to_vec(), 3),
            Self::Output => (catalog::output(&params.global).to_vec(), 2),
        };
        let knobs = knobs.into_iter().map(Control::shortened).collect();
        (knobs, columns)
    }

    /// The section's size, with its padding.
    fn size(self, params: &ThreeOscParams) -> egui::Vec2 {
        let (knobs, columns) = self.knobs(params);
        let rows = knobs.len().div_ceil(columns);
        let width = match self {
            Self::Lfo(_) => LFO_WIDTH,
            Self::Midi => MIDI_WIDTH,
            _ => columns as f32 * CELL,
        };
        let body = match self {
            Self::Midi => 2.0 * CHIP_HEIGHT + ROW_GAP,
            _ => rows as f32 * KNOBS.height(CELL) + rows.saturating_sub(1) as f32 * ROW_GAP,
        };
        egui::vec2(width, TITLE_HEIGHT + ROW_GAP + body) + egui::Vec2::splat(2.0 * PADDING)
    }
}

pub fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let matrix = Matrix {
        slots: &params.mod_slots,
        monitor: gui.live.monitor,
    };
    let controls = Controls {
        setter,
        matrix: &matrix,
        highlight: widgets::take_highlight(ui.ctx()),
        style: KNOBS,
        colors: COLORS,
        positions: None,
    };
    ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
    ui.add_space(MARGIN);
    ui.horizontal(|ui| {
        ui.add_space(MARGIN);
        ui.set_height(HEADER_HEIGHT);
        ui.label(RichText::new("three_osc").strong().color(SOUND_COLOR));
        ui.label(RichText::new(">").color(TEXT_WEAK));
        title_bar::presets(ui, params, setter, gui);
        tuning_window::button(ui, params, &mut gui.state.tuning);
        widgets::routes_button(ui, &matrix, gui);
        widgets::settings_button(ui, gui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(MARGIN);
            theme_picker(ui, gui);
        });
    });

    // The sections, flowing across the window, then the display in the rest of it
    let area = ui.available_rect_before_wrap().shrink2(egui::vec2(MARGIN, 0.0));
    let used = widgets::in_rect(ui, "body", area, Layout::top_down(Align::Min), |ui| {
        ui.with_layout(Layout::left_to_right(Align::Min).with_main_wrap(true), |ui| {
            for section in Section::ALL {
                section_box(ui, section, params, &controls, gui);
            }
        });
        tabs(ui, gui);
        let height = (ui.available_height() - MARGIN).max(DISPLAY_MIN);
        if let Some(display) = gui.state.compact.shown.display() {
            display.show(ui, height, params, setter, gui);
        }
        ui.min_rect()
    });
    // (For the window to scroll if it's too small for all that)
    ui.allocate_rect(used, Sense::hover());
    ui.add_space(MARGIN);
    widgets::routes_window(ui.ctx(), &controls, gui);
}

/// A section: its title (a handle, for a modulation source) with its choices beside it, and its
/// knobs. Pressing anywhere on it shows it on the display.
fn section_box(ui: &mut Ui, section: Section, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let (rect, _) = ui.allocate_exact_size(section.size(params), Sense::hover());
    let pointed = section.display().is_some() && ui.rect_contains_pointer(rect);
    if pointed && ui.input(|input| input.pointer.primary_pressed()) {
        gui.state.compact.shown = section;
    }
    let edge = if gui.state.compact.shown == section {
        SOUND_COLOR.gamma_multiply(0.7)
    } else if pointed {
        TEXT_WEAK
    } else {
        EDGE
    };
    ui.painter().rect(rect, CornerRadius::same(2), PANEL, Stroke::new(1.0_f32, edge), StrokeKind::Inside);
    let layout = Layout::top_down(Align::Min);
    widgets::in_rect(ui, ("section", section.name()), rect.shrink(PADDING), layout, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(GAP, ROW_GAP);
        let title_size = egui::vec2(ui.available_width(), TITLE_HEIGHT);
        ui.allocate_ui_with_layout(title_size, Layout::left_to_right(Align::Center), |ui| {
            title(ui, section, params, controls, gui);
        });
        let (knobs, columns) = section.knobs(params);
        match section {
            Section::Lfo(index) => {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    catalog::show_all(ui, &knobs, CELL, controls);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.add_space(4.0);
                        lfo_toggles(ui, &params.lfos[index], controls.setter);
                    });
                });
            }
            Section::Midi => {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(GAP, ROW_GAP);
                    for source in ModSource::MIDI {
                        widgets::chip(ui, source, ModSource::variants()[source.to_index()], controls, &chips());
                    }
                });
            }
            _ => {
                for knobs in knobs.chunks(columns) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        catalog::show_all(ui, knobs, CELL, controls);
                    });
                }
            }
        }
    });
}

/// A section's title, and its choices beside it: the oscillators' waves, the filter's model and
/// type, the polyphony, and the effects' switches.
fn title(ui: &mut Ui, section: Section, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    match section.source() {
        Some(source) => {
            widgets::chip(ui, source, &section.name(), controls, &chips());
        }
        None => {
            ui.label(RichText::new(section.name()).color(TEXT_STRONG));
        }
    }
    let setter = controls.setter;
    let fx = &params.fx;
    let switch = |ui: &mut Ui, param, about| {
        widgets::switch(ui, param, about, setter, SOUND_COLOR);
    };
    match section {
        Section::Oscillator(index) => {
            let osc = params.oscillator(index);
            combo_cell(ui, &osc.wave, index, OSC_WAVE_WIDTH, help::OSC_WAVE, setter);
            if osc.wave.value() == OscillatorWave::File {
                let file = params.wavetable_file(index);
                // (A long file name is cut short to fit)
                ui.allocate_ui(egui::vec2(FILE_WIDTH, TITLE_HEIGHT), |ui| {
                    wavetable_file_combo(ui, index, file, gui, FILE_WIDTH);
                });
                if let Some(status) = wavetable_file_status(file) {
                    ui.label(RichText::new("?").color(TEXT_WEAK)).on_hover_text(status);
                }
            }
        }
        Section::Filter => {
            combo_cell(ui, &params.filter.model, 0, FILTER_MODEL_WIDTH, help::FILTER_MODEL, setter);
            combo_cell(ui, &params.filter.filter_type, 0, FILTER_TYPE_WIDTH, help::FILTER_TYPE, setter);
        }
        Section::Voice => combo_cell(ui, &params.global.polyphony, 0, POLYPHONY_WIDTH, help::POLYPHONY, setter),
        Section::Lfo(index) => {
            // (Beside a chip that grows with its count of routes)
            let width = ui.available_width().min(LFO_WAVE_WIDTH);
            short_combo_cell(ui, &params.lfos[index].wave, index, width, help::LFO_WAVE, setter);
        }
        Section::Distortion => switch(ui, &fx.distortion.enabled, help::DISTORTION),
        Section::Chorus => switch(ui, &fx.chorus.enabled, help::CHORUS),
        Section::Delay => {
            switch(ui, &fx.delay.enabled, help::DELAY);
            ui.label(RichText::new("PP").color(if fx.delay.ping_pong.value() { TEXT_STRONG } else { TEXT_WEAK }))
                .on_hover_text(help::DELAY_PING_PONG);
            switch(ui, &fx.delay.ping_pong, help::DELAY_PING_PONG);
        }
        Section::Reverb => switch(ui, &fx.reverb.enabled, help::REVERB),
        _ => {}
    }
}

/// An LFO's sync, and its trigger (which a click moves on to the next), as small buttons.
fn lfo_toggles(ui: &mut Ui, lfo: &LfoParams, setter: &ParamSetter) {
    ui.spacing_mut().item_spacing.y = 4.0;
    // (With less padding than other buttons, so that six letters fit on one line)
    ui.spacing_mut().button_padding.x = 2.0;
    let toggle = |ui: &mut Ui, text: &str, lit: bool, about: &str| {
        let text = RichText::new(text).size(10.0).color(if lit { BACKGROUND } else { TEXT });
        let button = egui::Button::new(text)
            .wrap_mode(egui::TextWrapMode::Extend)
            .fill(if lit { MOD_COLOR } else { WIDGET });
        ui.add_sized(egui::vec2(TOGGLE_WIDTH, 16.0), button).on_hover_text(about)
    };
    let synced = lfo.sync.value();
    if toggle(ui, "SYNC", synced, help::LFO_SYNC).clicked() {
        set_param(setter, &lfo.sync, !synced);
    }
    let trigger = lfo.trigger.value();
    let name = ["RETRIG", "FREE", "1-SHOT"][trigger.to_index()];
    if toggle(ui, name, trigger != LfoTriggerParam::Retrigger, help::LFO_TRIGGER).clicked() {
        let next = LfoTriggerParam::from_index((trigger.to_index() + 1) % LfoTriggerParam::variants().len());
        set_param(setter, &lfo.trigger, next);
    }
}

/// The tabs that pick what the display shows.
fn tabs(ui: &mut Ui, gui: &mut Gui) {
    ui.horizontal_wrapped(|ui| {
        ui.set_min_height(TABS_HEIGHT);
        ui.label(RichText::new("DISPLAY").color(TEXT_WEAK))
            .on_hover_text("Shows the section last pressed on, or the one picked here.");
        for section in Section::ALL.into_iter().filter(|section| section.display().is_some()) {
            let shown = gui.state.compact.shown == section;
            let text = RichText::new(section.name()).color(if shown { BACKGROUND } else { TEXT });
            let button = egui::Button::new(text).fill(if shown { SOUND_COLOR } else { WIDGET });
            if ui.add(button).clicked() {
                gui.state.compact.shown = section;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sections_fit_a_small_window() {
        let params = ThreeOscParams::default();
        // Flowed across a window 1000 pixels wide, as the layout does it, the sections, the
        // tabs and the smallest display fit in 600 pixels
        let width = 1000.0 - 2.0 * MARGIN;
        let (mut x, mut row_height, mut height) = (0.0_f32, 0.0_f32, 0.0_f32);
        for section in Section::ALL {
            let size = section.size(&params);
            assert!(size.x <= width, "{section:?}");
            if x > 0.0 && x + size.x > width {
                height += row_height + GAP;
                (x, row_height) = (0.0, 0.0);
            }
            x += size.x + GAP;
            row_height = row_height.max(size.y);
        }
        height += row_height;
        let total = MARGIN + HEADER_HEIGHT + GAP + height + GAP + TABS_HEIGHT + GAP + DISPLAY_MIN + MARGIN;
        assert!(total <= 600.0, "{total}");
    }

    #[test]
    fn the_knobs_names_fit_their_cells() {
        let params = ThreeOscParams::default();
        for section in Section::ALL {
            for knob in section.knobs(&params).0 {
                // (In a monospaced font, about 6 pixels a letter)
                assert!(knob.label.len() <= 5, "{} in {section:?}", knob.label);
            }
        }
    }
}
