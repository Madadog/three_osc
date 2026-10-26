//! The Focus theme: one part of the synth at a time, large and explained, on a light page.
//!
//! A sidebar lists every part of the synth, each with what it's set to, and a search for any
//! control by its name or what it does. The part picked fills the rest of the window: what it
//! does, its display, its choices and large knobs, and beside them the modulation routes into
//! it (and, for a modulation source, out of it). Under the knobs, the last control hovered says
//! what it does, rather than a tooltip. The modulation sources in the sidebar can be dragged
//! onto the knobs.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};
use nih_plug_egui::widgets::ParamSlider;

use super::catalog::{self, Colors, Control};
use super::knob::{self, Controls};
use super::widgets::{self, in_rect};
use super::{
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, theme_picker, title_bar,
    tuning_window, wavetable_file_combo, wavetable_file_status, Gui, Live, Matrix, WaveDisplay, MOD_COLOR,
};
use crate::params::{FilterModelParam, FilterTypeParam, OscillatorWave, PolyphonyParam, ThreeOscParams, Wave};
use crate::synth::modulation::{ModSource, ModTarget};
use crate::ThreeOscPlugin;

const PAPER: Color32 = Color32::from_rgb(245, 242, 235);
const SIDEBAR: Color32 = Color32::from_rgb(235, 231, 221);
const SHEET: Color32 = Color32::from_rgb(252, 251, 247);
const EDGE: Color32 = Color32::from_rgb(214, 208, 195);
const INK: Color32 = Color32::from_rgb(42, 41, 37);
const INK_WEAK: Color32 = Color32::from_rgb(120, 115, 105);
const WIDGET: Color32 = Color32::from_rgb(232, 227, 216);
const WIDGET_HOVERED: Color32 = Color32::from_rgb(222, 216, 203);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(210, 203, 188);
const TRACK: Color32 = Color32::from_rgb(222, 216, 204);
const ACCENT: Color32 = Color32::from_rgb(47, 111, 222);
const MODULATION: Color32 = Color32::from_rgb(218, 118, 28);
const GLOBAL: Color32 = Color32::from_rgb(120, 86, 200);

const MARGIN: f32 = 10.0;
const HEADER_HEIGHT: f32 = 36.0;
const SIDEBAR_WIDTH: f32 = 280.0;
const ENTRY_HEIGHT: f32 = 36.0;
const GAP: f32 = 12.0;
const MODULATION_WIDTH: f32 = 330.0;
const HELP_HEIGHT: f32 = 92.0;
const MIN_WIDTH: f32 = 1100.0;
const MIN_HEIGHT: f32 = 760.0;
/// The widest the knobs' cells get.
const MAX_CELL: f32 = 120.0;

const KNOBS: knob::Style = knob::Style {
    font_size: 13.0,
    tooltips: false,
    max_diameter: 62.0,
    ..knob::Style::STUDIO
};
const COLORS: Colors = Colors {
    sound: ACCENT,
    modulation: MODULATION,
    global: GLOBAL,
};

/// The displays' colours: light, like the page.
pub const PALETTE: displays::Palette = displays::Palette {
    screen: Color32::from_rgb(255, 254, 250),
    edge: EDGE,
    signal: ACCENT,
    live: Color32::from_rgb(214, 70, 40),
    guide: Color32::from_rgb(232, 226, 214),
    label: Color32::from_rgb(160, 154, 142),
};

/// The look of the Focus theme, for egui: dark ink on paper.
pub fn style() -> egui::Style {
    let mut style = egui::Style {
        visuals: egui::Visuals::light(),
        ..egui::Style::default()
    };
    let visuals = &mut style.visuals;
    visuals.panel_fill = PAPER;
    visuals.window_fill = SHEET;
    visuals.window_stroke = Stroke::new(1.0_f32, EDGE);
    visuals.extreme_bg_color = TRACK;
    visuals.faint_bg_color = SIDEBAR;
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(8);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = SHEET;
    widgets.noninteractive.weak_bg_fill = SHEET;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, EDGE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, INK);
    for (widget, fill) in [
        (&mut widgets.inactive, WIDGET),
        (&mut widgets.hovered, WIDGET_HOVERED),
        (&mut widgets.active, WIDGET_ACTIVE),
        (&mut widgets.open, WIDGET_HOVERED),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.corner_radius = CornerRadius::same(6);
    }
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, INK);
    widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::BLACK);
    widgets.active.fg_stroke = Stroke::new(2.0_f32, Color32::BLACK);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, Color32::BLACK);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, EDGE);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, ACCENT);
    visuals.selection.bg_fill = Color32::from_rgb(190, 212, 248);
    visuals.selection.stroke = Stroke::new(1.0_f32, INK);
    style.text_styles = [
        (egui::TextStyle::Small, FontId::proportional(11.5)),
        (egui::TextStyle::Body, FontId::proportional(14.0)),
        (egui::TextStyle::Button, FontId::proportional(14.0)),
        (egui::TextStyle::Heading, FontId::proportional(24.0)),
        (egui::TextStyle::Monospace, FontId::monospace(13.0)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(10.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.interact_size.y = 24.0;
    style
}

/// What the Focus layout shows.
pub struct FocusState {
    module: Module,
    search: String,
    /// The control last hovered, and what it does.
    help: Option<(String, &'static str)>,
}

impl FocusState {
    pub fn new() -> Self {
        Self {
            module: Module::Oscillator(0),
            search: String::new(),
            help: None,
        }
    }
}

/// A part of the synth, as the sidebar lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Module {
    Oscillator(usize),
    Filter,
    Voice,
    Output,
    VolumeEnvelope,
    FilterEnvelope,
    Envelope(usize),
    Lfo(usize),
    Midi,
    Routes,
    Distortion,
    Chorus,
    Delay,
    Reverb,
    Settings,
}

/// The sidebar's groups of modules.
const GROUPS: [(&str, &[Module]); 4] = [
    (
        "Sound",
        &[Module::Oscillator(0), Module::Oscillator(1), Module::Oscillator(2), Module::Filter, Module::Voice, Module::Output],
    ),
    (
        "Modulation",
        &[
            Module::VolumeEnvelope,
            Module::FilterEnvelope,
            Module::Envelope(0),
            Module::Envelope(1),
            Module::Lfo(0),
            Module::Lfo(1),
            Module::Lfo(2),
            Module::Midi,
            Module::Routes,
        ],
    ),
    ("Effects", &[Module::Distortion, Module::Chorus, Module::Delay, Module::Reverb]),
    ("", &[Module::Settings]),
];

impl Module {
    fn name(self) -> String {
        match self {
            Self::Oscillator(index) => format!("Osc {}", index + 1),
            Self::Filter => String::from("Filter"),
            Self::Voice => String::from("Voice"),
            Self::Output => String::from("Output"),
            Self::VolumeEnvelope => String::from("Volume Envelope"),
            Self::FilterEnvelope => String::from("Filter Envelope"),
            Self::Envelope(index) => format!("Envelope {}", index + 1),
            Self::Lfo(index) => format!("LFO {}", index + 1),
            Self::Midi => String::from("MIDI Sources"),
            Self::Routes => String::from("Modulation Routes"),
            Self::Distortion => String::from("Distortion"),
            Self::Chorus => String::from("Chorus"),
            Self::Delay => String::from("Delay"),
            Self::Reverb => String::from("Reverb"),
            Self::Settings => String::from("Settings"),
        }
    }

    /// The modulation source the module is, if it is one.
    fn source(self) -> Option<ModSource> {
        match self {
            Self::Oscillator(index) => Some(ModSource::osc(index)),
            Self::VolumeEnvelope => Some(ModSource::VolumeEnv),
            Self::FilterEnvelope => Some(ModSource::FilterEnv),
            Self::Envelope(index) => Some(ModSource::envelope(index)),
            Self::Lfo(index) => Some(ModSource::lfo(index)),
            _ => None,
        }
    }

    /// What the module does, for the top of its page.
    fn about(self) -> &'static str {
        match self {
            Self::Oscillator(0) => {
                "The first of the three oscillators, each playing a wavetable. Osc 2 can modulate \
                 its phase, frequency and amplitude, and it can be a modulation source itself."
            }
            Self::Oscillator(1) => {
                "The second oscillator. Osc 3 can modulate it, and it can modulate Osc 1, as well \
                 as being heard."
            }
            Self::Oscillator(_) => {
                "The third oscillator, which can modulate Osc 2 as well as being heard (or only \
                 modulate it, at a level of 0)."
            }
            Self::Filter => {
                "The filter the oscillators go through: its model and type, where it cuts off \
                 and how it resonates. Its envelope moves the cutoff for every note. Drag the \
                 display across for the cutoff and up and down for the resonance."
            }
            Self::Voice => "How notes are played: one at a time or many, gliding between them, and the pitch of the whole synth.",
            Self::Output => "What the synth plays, after the effects, and how loud and wide it is.",
            Self::VolumeEnvelope => {
                "Shapes each note's volume over time. Drag the points on its display for the \
                 times and the sustain level, and the small handles to bend the curves."
            }
            Self::FilterEnvelope => "Moves the filter's cutoff over each note, by its Amount. It's also a modulation source.",
            Self::Envelope(_) => "An envelope that's only a modulation source: drag it from the list onto any knob.",
            Self::Lfo(_) => {
                "A low frequency oscillator, a modulation source that swings back and forth. \
                 Each note has its own, started when the note starts."
            }
            Self::Midi => "The modulation sources that come from how notes are played, and a random value for each note.",
            Self::Routes => {
                "Every modulation route: a source, the control it moves, and how far. Routes are \
                 also added by dragging a source from the list onto a knob."
            }
            Self::Distortion => help::DISTORTION,
            Self::Chorus => help::CHORUS,
            Self::Delay => help::DELAY,
            Self::Reverb => help::REVERB,
            Self::Settings => "How the synth trades quality for CPU, the folder of wavetable files, and the theme.",
        }
    }

    /// The module's knobs.
    fn controls(self, params: &ThreeOscParams) -> Vec<Control<'_>> {
        let fx = &params.fx;
        match self {
            Self::Oscillator(index) => {
                let mut controls = catalog::oscillator(params.oscillator(index), index).to_vec();
                controls.extend(catalog::oscillator_modulation(params, index).into_iter().flatten());
                controls
            }
            Self::Filter => {
                let mut controls = catalog::filter(&params.filter).to_vec();
                controls.push(catalog::filter_envelope_amount(&params.filter_env));
                controls
            }
            Self::Voice => catalog::voice(&params.global).to_vec(),
            Self::Output => catalog::output(&params.global).to_vec(),
            Self::VolumeEnvelope => catalog::envelope(&params.vol_env).to_vec(),
            Self::FilterEnvelope => {
                let mut controls = catalog::envelope(&params.filter_env.envelope).to_vec();
                controls.push(catalog::filter_envelope_amount(&params.filter_env).labelled("Amount"));
                controls
            }
            Self::Envelope(index) => catalog::envelope(&params.mod_envs[index].envelope).to_vec(),
            Self::Lfo(index) => vec![catalog::lfo_rate(&params.lfos[index], index)],
            Self::Distortion => catalog::distortion(&fx.distortion).to_vec(),
            Self::Chorus => catalog::chorus(&fx.chorus).to_vec(),
            Self::Delay => catalog::delay(&fx.delay).to_vec(),
            Self::Reverb => catalog::reverb(&fx.reverb).to_vec(),
            Self::Midi | Self::Routes | Self::Settings => Vec::new(),
        }
    }

    /// What the module is set to, in a few words.
    fn summary(self, params: &ThreeOscParams, matrix: &Matrix) -> String {
        let value = |param: &FloatParam| param.normalized_value_to_string(param.modulated_normalized_value(), true);
        let envelope = |envelope: &crate::params::EnvelopeParams| {
            format!(
                "{} · {} · {} · {}",
                value(&envelope.attack),
                value(&envelope.decay),
                value(&envelope.sustain),
                value(&envelope.release)
            )
        };
        let effect = |enabled: &BoolParam, mix: &FloatParam| {
            if enabled.value() {
                format!("On · mix {}", value(mix))
            } else {
                String::from("Off")
            }
        };
        let fx = &params.fx;
        match self {
            Self::Oscillator(index) => {
                let osc = params.oscillator(index);
                let wave = OscillatorWave::variants()[osc.wave.value().to_index()];
                let voices = osc.voices.value();
                let unison = if voices > 1 { format!(" · {voices} voices") } else { String::new() };
                format!("{wave} · {}{unison}", value(&osc.amp))
            }
            Self::Filter => {
                let filter = &params.filter;
                let model = FilterModelParam::variants()[filter.model.value().to_index()];
                let kind = FilterTypeParam::variants()[filter.filter_type.value().to_index()];
                format!("{model} {kind} · {}", value(&filter.cutoff))
            }
            Self::Voice => PolyphonyParam::variants()[params.global.polyphony.value().to_index()].to_string(),
            Self::Output => value(&params.global.output_gain),
            Self::VolumeEnvelope => envelope(&params.vol_env),
            Self::FilterEnvelope => envelope(&params.filter_env.envelope),
            Self::Envelope(index) => envelope(&params.mod_envs[index].envelope),
            Self::Lfo(index) => {
                let lfo = &params.lfos[index];
                format!("{} · {}", Wave::variants()[lfo.wave.value().to_index()], value(&lfo.rate))
            }
            Self::Midi => String::from("Velocity, key, wheel, touch, random"),
            Self::Routes => {
                let used = matrix.slots.iter().filter(|slot| Matrix::is_used(slot)).count();
                format!("{used} in use")
            }
            Self::Distortion => effect(&fx.distortion.enabled, &fx.distortion.mix),
            Self::Chorus => effect(&fx.chorus.enabled, &fx.chorus.mix),
            Self::Delay => effect(&fx.delay.enabled, &fx.delay.mix),
            Self::Reverb => effect(&fx.reverb.enabled, &fx.reverb.mix),
            Self::Settings => String::from("Quality, wavetables, theme"),
        }
    }

    /// The colour of its dot in the sidebar.
    fn color(self) -> Color32 {
        match self {
            Self::Oscillator(_) | Self::Filter | Self::Distortion | Self::Chorus | Self::Delay | Self::Reverb => ACCENT,
            Self::Voice | Self::Output | Self::Settings => GLOBAL,
            _ => MODULATION,
        }
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
    let width = ui.available_width().max(MIN_WIDTH);
    let height = ui.available_height().max(MIN_HEIGHT);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let area = area.shrink(MARGIN);
    let header = Rect::from_min_size(area.min, egui::vec2(area.width(), HEADER_HEIGHT));
    let sidebar = Rect::from_min_max(Pos2::new(area.left(), header.bottom() + GAP), Pos2::new(area.left() + SIDEBAR_WIDTH, area.bottom()));
    let page = Rect::from_min_max(Pos2::new(sidebar.right() + GAP, sidebar.top()), area.max);

    in_rect(ui, "header", header, Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new("Three Osc").size(22.0).strong().color(INK));
        ui.label(RichText::new(format!("v{}", ThreeOscPlugin::VERSION)).small().color(INK_WEAK));
        ui.add_space(18.0);
        title_bar::presets(ui, params, setter, gui);
        ui.add_space(8.0);
        tuning_window::button(ui, params, &mut gui.state.tuning);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme_picker(ui, gui));
    });
    let painter = ui.painter();
    painter.rect(sidebar, CornerRadius::same(10), SIDEBAR, Stroke::new(1.0_f32, EDGE), StrokeKind::Inside);
    painter.rect(page, CornerRadius::same(10), SHEET, Stroke::new(1.0_f32, EDGE), StrokeKind::Inside);
    sidebar_list(ui, sidebar.shrink(10.0), params, &controls, gui);
    module_page(ui, page.shrink(18.0), params, &controls, gui);
}

/// The sidebar: a search box over the modules (or, while searching, the controls found).
fn sidebar_list(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    in_rect(ui, "sidebar", rect, Layout::top_down(Align::Min), |ui| {
        let search = &mut gui.state.focus.search;
        ui.add(
            egui::TextEdit::singleline(search)
                .hint_text("Search for a control…")
                .desired_width(rect.width()),
        )
        .on_hover_text("Finds a control by its name or by what it does.");
        ui.add_space(4.0);
        let query = search.trim().to_lowercase();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            if query.is_empty() {
                for (group, modules) in GROUPS {
                    if !group.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new(group.to_uppercase()).size(11.0).strong().color(INK_WEAK).extra_letter_spacing(1.0));
                    }
                    for module in modules {
                        entry(ui, *module, params, controls, gui);
                    }
                }
            } else {
                search_results(ui, &query, params, controls, gui);
            }
        });
    });
}

/// A module's entry in the sidebar: its name and what it's set to, picked by clicking it (and
/// dragged onto a knob, for a modulation source).
fn entry(ui: &mut Ui, module: Module, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let width = ui.available_width();
    let sense = if module.source().is_some() { Sense::click_and_drag() } else { Sense::click() };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ENTRY_HEIGHT), sense);
    let selected = gui.state.focus.module == module;
    let lit = module.source().is_some_and(|source| controls.highlight == Some(source));
    let painter = ui.painter();
    if selected || response.hovered() || lit {
        let fill = if selected { SHEET } else { WIDGET };
        painter.rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0_f32, if selected { EDGE } else { fill }), StrokeKind::Inside);
    }
    if selected {
        let bar = Rect::from_min_size(rect.min + egui::vec2(0.0, 6.0), egui::vec2(3.0, rect.height() - 12.0));
        painter.rect_filled(bar, CornerRadius::same(2), module.color());
    }
    // A thumbnail of the oscillators' and LFOs' waves, and a dot for the rest
    let thumbnail = Rect::from_min_size(rect.min + egui::vec2(10.0, 6.0), egui::vec2(44.0, rect.height() - 12.0));
    match module {
        Module::Oscillator(index) => {
            let wave = WaveDisplay::new(index, params.oscillator(index), params.wavetable_file(index), gui);
            in_rect(ui, ("thumbnail", index), thumbnail, Layout::top_down(Align::Min), |ui| wave.show(ui, thumbnail.height()));
        }
        Module::Lfo(index) => {
            let wave = params.lfos[index].wave.value().to_osc_wave();
            let phase = gui.live.monitor.lfo_phase(index);
            in_rect(ui, ("lfo thumbnail", index), thumbnail, Layout::top_down(Align::Min), |ui| {
                displays::lfo(ui, &wave, MOD_COLOR, phase, thumbnail.height())
            });
        }
        _ => {
            ui.painter().circle_filled(thumbnail.center(), 5.0, module.color());
        }
    }
    let text_left = thumbnail.right() + 10.0;
    let painter = ui.painter();
    painter.text(Pos2::new(text_left, rect.top() + 10.0), egui::Align2::LEFT_CENTER, module.name(), FontId::proportional(13.5), INK);
    let summary = module.summary(params, controls.matrix);
    let summary = painter.layout(summary, FontId::proportional(11.0), INK_WEAK, rect.right() - text_left - 8.0);
    painter.galley(Pos2::new(text_left, rect.top() + 18.0), summary, INK_WEAK);
    if response.clicked() {
        gui.state.focus.module = module;
    }
    // A modulation source's entry is its handle too
    if let Some(source) = module.source() {
        let name = module.name();
        widgets::source_handle(ui, response, source, controls.matrix, |painter, pointer| {
            let pill = Rect::from_center_size(pointer, egui::vec2(8.0 * name.len() as f32 + 24.0, 24.0));
            painter.rect(pill, CornerRadius::same(12), MOD_COLOR.gamma_multiply(0.85), Stroke::NONE, StrokeKind::Inside);
            painter.text(pill.center(), egui::Align2::CENTER_CENTER, &name, FontId::proportional(13.0), INK);
        });
    }
}

/// The controls (and modules) a search finds: by a control's label, its parameter's name or
/// what it does, and by a module's name.
fn search_results(ui: &mut Ui, query: &str, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let mut found = false;
    for (_, modules) in GROUPS {
        for module in modules {
            let name = module.name();
            if name.to_lowercase().contains(query) {
                entry(ui, *module, params, controls, gui);
                found = true;
            }
            for control in module.controls(params) {
                let matches = [control.label, control.name(), control.help]
                    .iter()
                    .any(|text| text.to_lowercase().contains(query));
                if matches {
                    found = true;
                    let label = RichText::new(format!("{}  ·  {name}", control.name())).color(INK);
                    let button = ui.add(egui::Button::new(label).frame(false)).on_hover_text(control.help);
                    if button.clicked() {
                        gui.state.focus.module = *module;
                        gui.state.focus.help = Some((control.name().to_string(), control.help));
                    }
                }
            }
        }
    }
    if !found {
        ui.label(RichText::new("Nothing found.").color(INK_WEAK));
    }
}

/// The page of the module picked: what it does, its display, its choices and knobs and the
/// help for the last control hovered, with its modulation routes beside them.
fn module_page(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let module = gui.state.focus.module;
    let knobs = module.controls(params);
    let wide = matches!(module, Module::Routes | Module::Settings | Module::Midi);
    let (main, side) = if wide || knobs.is_empty() {
        (rect, None)
    } else {
        let side = Rect::from_min_max(Pos2::new(rect.right() - MODULATION_WIDTH, rect.top()), rect.max);
        (Rect::from_min_max(rect.min, Pos2::new(side.left() - GAP * 2.0, rect.bottom())), Some(side))
    };

    // The title, with the module's handle when it's a modulation source, and what it does
    let top = in_rect(ui, "title", main, Layout::top_down(Align::Min), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(module.name()).size(26.0).strong().color(INK));
            if let Some(source) = module.source() {
                let chips = widgets::ChipStyle {
                    font: FontId::proportional(13.0),
                    height: 24.0,
                    ..widgets::ChipStyle::new(INK, SHEET)
                };
                widgets::chip(ui, source, "Drag to modulate", controls, &chips);
            }
        });
        ui.add(egui::Label::new(RichText::new(module.about()).size(14.0).color(INK_WEAK)).wrap());
        ui.min_rect().bottom()
    });
    let body = Rect::from_min_max(Pos2::new(main.left(), top + GAP), main.max);
    match module {
        Module::Routes => routes_page(ui, body, controls, gui),
        Module::Settings => {
            in_rect(ui, "settings", body, Layout::top_down(Align::Min), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(body.width().min(760.0));
                    ui.heading("Quality");
                    widgets::quality_settings(ui, &params.global, controls.setter, 140.0);
                    ui.separator();
                    widgets::wavetable_files(ui, params, controls.setter, gui);
                    ui.separator();
                    ui.heading("Theme");
                    widgets::theme_choices(ui, gui);
                });
            });
        }
        Module::Midi => midi_page(ui, body, controls),
        _ => controls_page(ui, body, module, &knobs, params, controls, gui),
    }
    if let Some(side) = side {
        modulation_side(ui, side, module, &knobs, controls);
    }
}

/// A module's display (if it has one), choices and knobs, with the help for the last control
/// hovered under them.
fn controls_page(
    ui: &mut Ui,
    rect: Rect,
    module: Module,
    knobs: &[Control],
    params: &ThreeOscParams,
    controls: &Controls,
    gui: &mut Gui,
) {
    let setter = controls.setter;
    let help_rect = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - HELP_HEIGHT), rect.max);
    let rows = knobs.len().div_ceil(5);
    let cell = (rect.width() / knobs.len().min(5) as f32).min(MAX_CELL);
    let knob_height = KNOBS.height(cell);
    let knobs_height = rows as f32 * (knob_height + 6.0);
    let knobs_top = help_rect.top() - GAP - knobs_height;

    // The choices, and the display in the room left above the knobs
    let content = Rect::from_min_max(rect.min, Pos2::new(rect.right(), knobs_top - GAP));
    let live = gui.live;
    let mut hovered_choice = None;
    in_rect(ui, "content", content, Layout::top_down(Align::Min), |ui| {
        let mut choice = |ui: &mut Ui, label: &str, add: &mut dyn FnMut(&mut Ui), help: &'static str| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).color(INK_WEAK));
                let response = ui.scope(|ui| add(ui)).response;
                if ui.rect_contains_pointer(response.rect) {
                    hovered_choice = Some((label.to_string(), help));
                }
            });
        };
        match module {
            Module::Oscillator(index) => {
                let osc = params.oscillator(index);
                choice(ui, "Wave", &mut |ui| combo_cell(ui, &osc.wave, index, 180.0, help::OSC_WAVE, setter), help::OSC_WAVE);
                let file = params.wavetable_file(index);
                if osc.wave.value() == OscillatorWave::File {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("File").color(INK_WEAK));
                        wavetable_file_combo(ui, index, file, gui, 260.0);
                        if let Some(status) = wavetable_file_status(file) {
                            ui.label(RichText::new(status).small().color(INK_WEAK));
                        }
                    });
                }
                let height = ui.available_height();
                WaveDisplay::new(index, osc, file, gui).show(ui, height.max(60.0));
            }
            Module::Filter => {
                let (filter, filter_env) = (&params.filter, &params.filter_env);
                ui.horizontal(|ui| {
                    choice(ui, "Model", &mut |ui| combo_cell(ui, &filter.model, 0, 140.0, help::FILTER_MODEL, setter), help::FILTER_MODEL);
                    choice(ui, "Type", &mut |ui| combo_cell(ui, &filter.filter_type, 0, 140.0, help::FILTER_TYPE, setter), help::FILTER_TYPE);
                });
                let oversampling = params.global.oversampling.value().to_oversampling();
                let (settings, playing) = filter_display_inputs(filter, filter_env, oversampling, live);
                let edit = displays::FilterEdit {
                    cutoff: &filter.cutoff,
                    resonance: &filter.resonance,
                    setter,
                };
                let height = ui.available_height().max(60.0);
                displays::filter(ui, settings, &mut gui.state.filter_response, playing, height, Some(edit));
            }
            Module::Voice => {
                let global = &params.global;
                choice(ui, "Polyphony", &mut |ui| combo_cell(ui, &global.polyphony, 0, 160.0, help::POLYPHONY, setter), help::POLYPHONY);
            }
            Module::Output => {
                let (width, height) = (ui.available_width(), ui.available_height().max(80.0));
                analyser::output(ui, &mut gui.state.analyser, &live.monitor.output, live.sample_rate, width, height);
            }
            Module::VolumeEnvelope | Module::FilterEnvelope | Module::Envelope(_) => {
                let envelope = match module {
                    Module::VolumeEnvelope => &params.vol_env,
                    Module::FilterEnvelope => &params.filter_env.envelope,
                    Module::Envelope(index) => &params.mod_envs[index].envelope,
                    _ => unreachable!(),
                };
                let height = ui.available_height().max(60.0);
                envelope_display(ui, envelope, setter, live, height);
            }
            Module::Lfo(index) => {
                let lfo = &params.lfos[index];
                choice(ui, "Wave", &mut |ui| combo_cell(ui, &lfo.wave, index, 160.0, help::LFO_WAVE, setter), help::LFO_WAVE);
                let wave = lfo.wave.value().to_osc_wave();
                let height = ui.available_height().max(60.0);
                displays::lfo(ui, &wave, MOD_COLOR, live.monitor.lfo_phase(index), height);
            }
            Module::Distortion | Module::Chorus | Module::Delay | Module::Reverb => {
                effect_content(ui, module, params, controls, live, &mut choice);
            }
            _ => {}
        }
    });
    if let Some(help) = hovered_choice {
        gui.state.focus.help = Some(help);
    }

    // The knobs, five to a row, centred
    for (row, knobs) in knobs.chunks(5).enumerate() {
        let width = knobs.len() as f32 * cell;
        let top = knobs_top + row as f32 * (knob_height + 6.0);
        let row_rect = Rect::from_min_size(Pos2::new(rect.center().x - width / 2.0, top), egui::vec2(width, knob_height));
        let hovered = in_rect(ui, ("knobs", row), row_rect, Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let mut hovered = None;
            for knob in knobs {
                let response = knob.show(ui, cell, controls);
                if response.hovered() || response.dragged() {
                    hovered = Some((knob.name().to_string(), knob.help));
                }
            }
            hovered
        });
        if hovered.is_some() {
            gui.state.focus.help = hovered;
        }
    }

    // The help for the last control hovered
    let painter = ui.painter();
    painter.rect(help_rect, CornerRadius::same(8), PAPER, Stroke::new(1.0_f32, EDGE), StrokeKind::Inside);
    in_rect(ui, "help", help_rect.shrink(12.0), Layout::top_down(Align::Min), |ui| match &gui.state.focus.help {
        Some((name, help)) => {
            ui.label(RichText::new(name).strong().color(INK));
            ui.add(egui::Label::new(RichText::new(*help).color(INK_WEAK)).wrap());
        }
        None => {
            ui.label(
                RichText::new(
                    "Hover a control to see what it does here. Drag a knob up or down to turn it \
                     (Shift for finer), double-click it to reset it, click its value to type one, \
                     and right-click it for its modulation.",
                )
                .color(INK_WEAK),
            );
        }
    });
}

/// Adds a labelled choice: its label, what adds the control, and what it does.
type Choice<'a> = dyn FnMut(&mut Ui, &str, &mut dyn FnMut(&mut Ui), &'static str) + 'a;

/// An effect's switches and display.
fn effect_content(
    ui: &mut Ui,
    module: Module,
    params: &ThreeOscParams,
    controls: &Controls,
    live: Live,
    choice: &mut Choice,
) {
    let fx = &params.fx;
    let setter = controls.setter;
    let settings = match live.modulation() {
        Some(modulation) => fx.settings().modulated(&modulation),
        None => fx.settings(),
    };
    let (enabled, about) = match module {
        Module::Distortion => (&fx.distortion.enabled, help::DISTORTION),
        Module::Chorus => (&fx.chorus.enabled, help::CHORUS),
        Module::Delay => (&fx.delay.enabled, help::DELAY),
        _ => (&fx.reverb.enabled, help::REVERB),
    };
    ui.horizontal(|ui| {
        choice(ui, "On", &mut |ui| drop(widgets::switch(ui, enabled, about, setter, ACCENT)), about);
        if module == Module::Delay {
            let ping_pong = &fx.delay.ping_pong;
            choice(
                ui,
                "Ping-Pong",
                &mut |ui| drop(widgets::switch(ui, ping_pong, help::DELAY_PING_PONG, setter, ACCENT)),
                help::DELAY_PING_PONG,
            );
        }
    });
    let height = ui.available_height().max(60.0);
    match module {
        Module::Distortion => {
            let s = settings.distortion;
            displays::distortion(ui, height, s.drive_db, s.mix, s.enabled);
        }
        Module::Chorus => displays::chorus(ui, height, settings.chorus.depth, settings.chorus.enabled),
        Module::Delay => {
            let s = settings.delay;
            displays::delay(ui, height, s.time, s.feedback, s.mix, s.ping_pong, s.enabled);
        }
        _ => {
            let s = settings.reverb;
            displays::reverb(ui, height, s.size, s.damping, s.enabled);
        }
    }
}

/// The MIDI sources, each with what it is, to drag onto knobs.
fn midi_page(ui: &mut Ui, rect: Rect, controls: &Controls) {
    in_rect(ui, "midi", rect, Layout::top_down(Align::Min), |ui| {
        let chips = widgets::ChipStyle {
            font: FontId::proportional(14.0),
            height: 28.0,
            ..widgets::ChipStyle::new(INK, SHEET)
        };
        egui::Grid::new("midi sources").num_columns(2).spacing([18.0, 14.0]).show(ui, |ui| {
            for source in ModSource::MIDI {
                widgets::chip(ui, source, ModSource::variants()[source.to_index()], controls, &chips);
                ui.add(egui::Label::new(RichText::new(source.help()).color(INK_WEAK)).wrap());
                ui.end_row();
            }
        });
    });
}

/// Every route, to edit, with a way to add one.
fn routes_page(ui: &mut Ui, rect: Rect, controls: &Controls, gui: &mut Gui) {
    let added = &mut gui.state.added_routes;
    in_rect(ui, "routes", rect, Layout::top_down(Align::Min), |ui| {
        widgets::add_route_button(ui, controls.matrix, added);
        ui.add_space(6.0);
        widgets::routes_list(ui, controls, added, [170.0, 220.0, 220.0]);
    });
}

/// The routes into a module's knobs, and (for a modulation source) out of it, each with its
/// amount and a way to remove it.
fn modulation_side(ui: &mut Ui, rect: Rect, module: Module, knobs: &[Control], controls: &Controls) {
    let painter = ui.painter();
    painter.line_segment([rect.left_top() - egui::vec2(GAP, 0.0), rect.left_bottom() - egui::vec2(GAP, 0.0)], Stroke::new(1.0_f32, EDGE));
    let matrix = controls.matrix;
    let setter = controls.setter;
    let label_of = |target: ModTarget| {
        knobs
            .iter()
            .find(|knob| knob.target == target)
            .map(|knob| knob.label.to_string())
            .unwrap_or_else(|| ModTarget::variants()[target.to_index()].to_string())
    };
    in_rect(ui, "modulation", rect, Layout::top_down(Align::Min), |ui| {
        ui.label(RichText::new("MODULATION").size(12.0).strong().color(INK_WEAK).extra_letter_spacing(1.0));
        let into: Vec<_> = matrix
            .slots
            .iter()
            .filter(|slot| Matrix::is_used(slot) && knobs.iter().any(|knob| knob.target == slot.target.value()))
            .collect();
        ui.label(RichText::new("Into its knobs").strong().color(INK));
        if into.is_empty() {
            ui.label(RichText::new("Nothing modulates it yet: drag a source from the list onto a knob.").small().color(INK_WEAK));
        }
        egui::Grid::new("into").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
            for slot in into {
                let source = ModSource::variants()[slot.source.value().to_index()];
                ui.label(RichText::new(format!("{source} › {}", label_of(slot.target.value()))).color(INK));
                ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(90.0));
                if ui.small_button("×").on_hover_text(help::CLEAR_ROUTE).clicked() {
                    Matrix::clear(setter, slot);
                }
                ui.end_row();
            }
        });
        if let Some(source) = module.source() {
            ui.add_space(12.0);
            ui.label(RichText::new("From it").strong().color(INK));
            let out: Vec<_> = matrix
                .slots
                .iter()
                .filter(|slot| Matrix::is_used(slot) && slot.source.value() == source)
                .collect();
            if out.is_empty() {
                ui.label(RichText::new("It doesn't modulate anything yet: drag its handle onto a knob.").small().color(INK_WEAK));
            }
            egui::Grid::new("out").num_columns(3).spacing([8.0, 6.0]).show(ui, |ui| {
                for slot in out {
                    ui.label(RichText::new(ModTarget::variants()[slot.target.value().to_index()]).color(INK));
                    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(90.0));
                    if ui.small_button("×").on_hover_text(help::CLEAR_ROUTE).clicked() {
                        Matrix::clear(setter, slot);
                    }
                    ui.end_row();
                }
            });
        }
    });
}
