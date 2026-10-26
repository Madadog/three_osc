//! The Vintage theme: the synth as the front panel of an old polysynth, with faders.
//!
//! Three tiers of sections run across a dark panel between wooden end cheeks: the master section
//! and the oscillators; the filter, its envelope, the amp's envelope, the effects, the voices and
//! the MIDI sources; then the envelopes, the LFOs and a scope. Each section is printed under a
//! coloured strip with its name. Every control is a fader, which shows its value in place of its name while it's hovered
//! or moved, and the choices are lit buttons, or a display stepped through with arrows. The
//! faders line up across each tier, and they get longer, and the panel wider, to fill a larger
//! window. The scope shows the section last pressed on (whose strip's light is lit), and each
//! modulation source has a handle to drag onto the faders.

use std::f32::consts::TAU;
use std::hash::Hash;

use nih_plug::prelude::*;
use nih_plug_egui::egui::text::LayoutJob;
use nih_plug_egui::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Painter, PopupCloseBehavior, Pos2, Rect, Response,
    RichText, Sense, Shape, Stroke, StrokeKind, TextFormat, Ui,
};

use super::catalog::{self, Colors, Control};
use super::knob::{self, Controls};
use super::widgets::{self, in_rect, ChipStyle, Display};
use super::{
    displays, help, set_param, theme_picker, title_bar, tuning_window, wavetable_file_combo, wavetable_file_status,
    Gui, Matrix, MOD_COLOR,
};
use crate::params::{OscillatorWave, ThreeOscParams, Wave};
use crate::synth::modulation::ModSource;

const BACKDROP: Color32 = Color32::from_rgb(17, 16, 15);
const PANEL: Color32 = Color32::from_rgb(40, 40, 43);
const NAMEPLATE_FILL: Color32 = Color32::from_rgb(25, 25, 27);
/// The panel's print, and the print on its light strips.
const SILK: Color32 = Color32::from_rgb(234, 230, 220);
const SILK_WEAK: Color32 = Color32::from_rgb(150, 146, 138);
const INK: Color32 = Color32::from_rgb(28, 26, 24);
const LINE: Color32 = Color32::from_rgb(112, 110, 106);
/// The rubber of the buttons and the faders' caps.
const WIDGET: Color32 = Color32::from_rgb(70, 70, 75);
const WIDGET_HOVERED: Color32 = Color32::from_rgb(88, 88, 94);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(108, 108, 115);
const SLOT: Color32 = Color32::from_rgb(12, 12, 13);
const WOOD: Color32 = Color32::from_rgb(104, 60, 32);
const WOOD_DARK: Color32 = Color32::from_rgb(66, 36, 17);
const WOOD_LIGHT: Color32 = Color32::from_rgb(142, 88, 50);
const LED_ON: Color32 = Color32::from_rgb(255, 64, 40);
const LED_OFF: Color32 = Color32::from_rgb(78, 24, 19);
const LED_SCREEN: Color32 = Color32::from_rgb(16, 6, 5);
const AMBER: Color32 = Color32::from_rgb(255, 178, 64);
const SOUND_COLOR: Color32 = Color32::from_rgb(240, 232, 214);
const GLOBAL_COLOR: Color32 = Color32::from_rgb(150, 192, 232);

/// The sections' strips, and what's printed on them.
const CREAM: (Color32, Color32) = (Color32::from_rgb(216, 208, 188), INK);
const RED: (Color32, Color32) = (Color32::from_rgb(190, 58, 44), SILK);
const YELLOW: (Color32, Color32) = (Color32::from_rgb(226, 176, 50), INK);
const BLUE: (Color32, Color32) = (Color32::from_rgb(62, 116, 186), SILK);
const GREEN: (Color32, Color32) = (Color32::from_rgb(62, 142, 96), SILK);
const GREY: (Color32, Color32) = (Color32::from_rgb(152, 152, 150), INK);
const BLACK: (Color32, Color32) = (Color32::from_rgb(14, 13, 12), AMBER);

const CHEEK: f32 = 26.0;
const MARGIN: f32 = 12.0;
const GAP: f32 = 8.0;
const PADDING: f32 = 8.0;
const STRIP: f32 = 20.0;
const NAMEPLATE: f32 = 52.0;
/// The height of a row of switches over a section's faders, and the space under it.
const SWITCHES: f32 = 26.0;
const ROW_GAP: f32 = 4.0;
/// The width of a fader's column, and how far its cap travels, at the least and the most.
const CELL_MIN: f32 = 28.0;
const CELL_MAX: f32 = 40.0;
const TRAVEL_MIN: f32 = 70.0;
const TRAVEL_MAX: f32 = 150.0;
/// The width of an LFO's column of wave buttons beside its fader, and the height of each.
const WAVES_WIDTH: f32 = 46.0;
const WAVE_ROW: f32 = 18.0;
/// The least room for the scope and for the MIDI sources' handles, which get what their tiers
/// have left.
const SCOPE_MIN: f32 = 200.0;
const MIDI_MIN: f32 = 90.0;
/// The widths of the oscillators' wave displays, the filter model's and the file combo boxes.
const WAVE_SELECTOR_WIDTH: f32 = 120.0;
const MODEL_SELECTOR_WIDTH: f32 = 100.0;
const FILE_WIDTH: f32 = 80.0;
const SELECTOR_HEIGHT: f32 = 20.0;

const COLORS: Colors = Colors {
    sound: SOUND_COLOR,
    modulation: MOD_COLOR,
    global: GLOBAL_COLOR,
};

/// The scope's colours: amber on black.
pub const PALETTE: displays::Palette = displays::Palette {
    screen: Color32::from_rgb(14, 11, 6),
    edge: Color32::from_rgb(70, 58, 38),
    signal: AMBER,
    live: Color32::from_rgb(255, 242, 200),
    guide: Color32::from_rgb(44, 34, 18),
    label: Color32::from_rgb(140, 112, 64),
};

/// The look of the Vintage theme, for egui: grey rubber buttons on a dark panel.
pub fn style() -> egui::Style {
    let mut style = egui::Style::default();
    let visuals = &mut style.visuals;
    visuals.panel_fill = BACKDROP;
    visuals.window_fill = Color32::from_rgb(34, 34, 37);
    visuals.window_stroke = Stroke::new(1.0_f32, LINE);
    visuals.extreme_bg_color = SLOT;
    visuals.faint_bg_color = Color32::from_rgb(46, 46, 50);
    visuals.window_corner_radius = CornerRadius::same(3);
    visuals.menu_corner_radius = CornerRadius::same(3);
    visuals.hyperlink_color = AMBER;
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    // (What faint print fades towards, light enough for the faders' names to stay readable)
    widgets.noninteractive.weak_bg_fill = Color32::from_rgb(120, 118, 112);
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, SILK);
    for (widget, fill) in [
        (&mut widgets.inactive, WIDGET),
        (&mut widgets.hovered, WIDGET_HOVERED),
        (&mut widgets.active, WIDGET_ACTIVE),
        (&mut widgets.open, WIDGET_HOVERED),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(150));
        widget.fg_stroke = Stroke::new(1.0_f32, SILK);
        widget.corner_radius = CornerRadius::same(2);
    }
    widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
    widgets.active.fg_stroke = Stroke::new(1.5_f32, Color32::WHITE);
    visuals.selection.bg_fill = Color32::from_rgb(140, 44, 30);
    visuals.selection.stroke = Stroke::new(1.0_f32, SILK);
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style
}

/// The faders, with caps that travel `travel` pixels.
fn faders(travel: f32) -> knob::Style {
    knob::Style {
        shape: knob::Shape::Fader(travel),
        font_size: 9.5,
        value_below: false,
        ..knob::Style::STUDIO
    }
}

fn chips() -> ChipStyle {
    ChipStyle {
        font: FontId::proportional(10.0),
        height: 16.0,
        ..ChipStyle::new(SILK, PANEL)
    }
}

/// What the Vintage layout's scope shows.
pub struct VintageState {
    shown: Part,
}

impl VintageState {
    pub fn new() -> Self {
        Self { shown: Part::Master }
    }
}

/// A section of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Master,
    Oscillator(usize),
    Filter,
    FilterEnvelope,
    Amp,
    Voice,
    Scope,
    Envelope(usize),
    Lfo(usize),
    Midi,
    Distortion,
    Chorus,
    Delay,
    Reverb,
}

/// The tiers of sections, top to bottom, each with how many rows of switches its sections have
/// over their faders.
const TIERS: [(&[Part], usize); 3] = [
    (&[Part::Master, Part::Oscillator(0), Part::Oscillator(1), Part::Oscillator(2)], 1),
    (
        &[
            Part::Filter,
            Part::FilterEnvelope,
            Part::Amp,
            Part::Distortion,
            Part::Chorus,
            Part::Delay,
            Part::Reverb,
            Part::Voice,
            Part::Midi,
        ],
        2,
    ),
    (&[Part::Envelope(0), Part::Envelope(1), Part::Lfo(0), Part::Lfo(1), Part::Lfo(2), Part::Scope], 1),
];

impl Part {
    fn name(self) -> String {
        match self {
            Self::Master => String::from("MASTER"),
            Self::Oscillator(index) => format!("OSCILLATOR {}", index + 1),
            Self::Filter => String::from("FILTER"),
            Self::FilterEnvelope => String::from("FILTER ENV"),
            Self::Amp => String::from("AMP ENV"),
            Self::Voice => String::from("VOICE"),
            Self::Scope => String::from("SCOPE"),
            Self::Envelope(index) => format!("ENVELOPE {}", index + 1),
            Self::Lfo(index) => format!("LFO {}", index + 1),
            Self::Midi => String::from("MIDI"),
            Self::Distortion => String::from("DIST"),
            Self::Chorus => String::from("CHORUS"),
            Self::Delay => String::from("DELAY"),
            Self::Reverb => String::from("REVERB"),
        }
    }

    /// The colour of the section's strip, and of the print on it.
    fn strip(self) -> (Color32, Color32) {
        match self {
            Self::Master | Self::Voice => GREY,
            Self::Oscillator(_) => CREAM,
            Self::Filter => RED,
            Self::FilterEnvelope | Self::Amp | Self::Envelope(_) => YELLOW,
            Self::Lfo(_) | Self::Midi => BLUE,
            Self::Distortion | Self::Chorus | Self::Delay | Self::Reverb => GREEN,
            Self::Scope => BLACK,
        }
    }

    /// The modulation source the section is, if it is one.
    fn source(self) -> Option<ModSource> {
        match self {
            Self::Oscillator(index) => Some(ModSource::osc(index)),
            Self::FilterEnvelope => Some(ModSource::FilterEnv),
            Self::Amp => Some(ModSource::VolumeEnv),
            Self::Envelope(index) => Some(ModSource::envelope(index)),
            Self::Lfo(index) => Some(ModSource::lfo(index)),
            _ => None,
        }
    }

    /// What the scope shows for the section, if it has anything to.
    fn display(self) -> Option<Display> {
        match self {
            Self::Master => Some(Display::Output),
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
            Self::Voice | Self::Scope | Self::Midi => None,
        }
    }

    /// The section's faders, with short names.
    fn knobs(self, params: &ThreeOscParams) -> Vec<Control<'_>> {
        let fx = &params.fx;
        let knobs = match self {
            Self::Master => catalog::output(&params.global).to_vec(),
            Self::Oscillator(index) => {
                // Its level, pitch and unison, then its wave's shape and the next oscillator's
                // modulation of it
                let osc = catalog::oscillator(params.oscillator(index), index);
                let mut knobs = [0, 1, 2, 3, 4, 8, 9, 5, 6, 7].map(|knob| osc[knob]).to_vec();
                knobs.extend(catalog::oscillator_modulation(params, index).into_iter().flatten());
                knobs
            }
            Self::Filter => {
                let mut knobs = catalog::filter(&params.filter).to_vec();
                knobs.push(catalog::filter_envelope_amount(&params.filter_env));
                knobs
            }
            Self::FilterEnvelope => {
                let mut knobs = catalog::envelope(&params.filter_env.envelope).to_vec();
                knobs.push(catalog::filter_envelope_amount(&params.filter_env).labelled("AMT"));
                knobs
            }
            Self::Amp => catalog::envelope(&params.vol_env).to_vec(),
            Self::Voice => catalog::voice(&params.global).to_vec(),
            Self::Envelope(index) => catalog::envelope(&params.mod_envs[index].envelope).to_vec(),
            Self::Lfo(index) => vec![catalog::lfo_rate(&params.lfos[index], index)],
            Self::Distortion => catalog::distortion(&fx.distortion).to_vec(),
            Self::Chorus => catalog::chorus(&fx.chorus).to_vec(),
            Self::Delay => catalog::delay(&fx.delay).to_vec(),
            Self::Reverb => catalog::reverb(&fx.reverb).to_vec(),
            Self::Scope | Self::Midi => Vec::new(),
        };
        knobs.into_iter().map(Control::shortened).collect()
    }

    /// The width of the section's faders and what's beside them, with faders `cell` apart, or
    /// `None` for the scope and the MIDI sources, which take what their tiers have left.
    fn width(self, params: &ThreeOscParams, cell: f32) -> Option<f32> {
        match self {
            Self::Scope | Self::Midi => None,
            Self::Lfo(_) => Some(cell + WAVES_WIDTH),
            _ => Some(self.knobs(params).len() as f32 * cell),
        }
    }

    /// The least width of a section that takes what its tier has left.
    fn min_width(self) -> f32 {
        match self {
            Self::Scope => SCOPE_MIN,
            Self::Midi => MIDI_MIN,
            _ => 0.0,
        }
    }
}

/// Where everything goes in a window.
struct Plan {
    /// How far apart the faders are, and how far their caps travel.
    cell: f32,
    travel: f32,
    cheeks: [Rect; 2],
    /// The panel between the cheeks, and the maker's name plate across its top.
    panel: Rect,
    nameplate: Rect,
    parts: Vec<(Part, Rect)>,
    /// What the panel and the window around it take, for the window to scroll when the panel
    /// is larger than it.
    extent: Rect,
}

impl Plan {
    fn new(window: Rect, params: &ThreeOscParams) -> Self {
        // The top tier, which has nothing to stretch, is as wide as the window lets it be
        let (top, _) = TIERS[0];
        let spacing = |tier: &[Part]| tier.len() as f32 * 2.0 * PADDING + (tier.len() - 1) as f32 * GAP;
        let faders: usize = top.iter().map(|part| part.knobs(params).len()).sum();
        let around = 2.0 * (CHEEK + MARGIN) + spacing(top);
        let cell = ((window.width() - around) / faders as f32).clamp(CELL_MIN, CELL_MAX);
        let inner_width = spacing(top) + faders as f32 * cell;

        // The faders as long as the window's height lets them be
        let tier_height = |rows: usize, travel: f32| {
            STRIP + 2.0 * PADDING + rows as f32 * (SWITCHES + ROW_GAP) + faders_height(travel, cell)
        };
        let height = |travel: f32| {
            2.0 * MARGIN + NAMEPLATE + TIERS.iter().map(|&(_, rows)| GAP + tier_height(rows, travel)).sum::<f32>()
        };
        let travel = (TRAVEL_MIN + (window.height() - height(TRAVEL_MIN)) / 3.0).clamp(TRAVEL_MIN, TRAVEL_MAX);

        // In the middle of the window, or at its top left when the window is smaller
        let size = egui::vec2(inner_width + 2.0 * (CHEEK + MARGIN), height(travel));
        let outer = Rect::from_min_size(window.min + ((window.size() - size) / 2.0).max(egui::Vec2::ZERO), size);
        let cheeks = [
            Rect::from_min_size(outer.min, egui::vec2(CHEEK, size.y)),
            Rect::from_min_size(Pos2::new(outer.right() - CHEEK, outer.top()), egui::vec2(CHEEK, size.y)),
        ];
        let panel = Rect::from_x_y_ranges(cheeks[0].right()..=cheeks[1].left(), outer.y_range());
        let inner = panel.shrink(MARGIN);
        let nameplate = Rect::from_min_size(inner.min, egui::vec2(inner.width(), NAMEPLATE));
        let mut parts = Vec::new();
        let mut y = nameplate.bottom();
        for (tier, rows) in TIERS {
            y += GAP;
            let height = tier_height(rows, travel);
            let fixed = spacing(tier) + tier.iter().filter_map(|part| part.width(params, cell)).sum::<f32>();
            let rest = inner.width() - fixed;
            let mut x = inner.left();
            for &part in tier {
                let width = part.width(params, cell).unwrap_or(rest.max(part.min_width())) + 2.0 * PADDING;
                parts.push((part, Rect::from_min_size(Pos2::new(x, y), egui::vec2(width, height))));
                x += width + GAP;
            }
            y += height;
        }
        Self {
            cell,
            travel,
            cheeks,
            panel,
            nameplate,
            parts,
            extent: Rect::from_min_max(window.min, outer.max.max(window.max)),
        }
    }
}

/// The height of a row of faders `cell` apart whose caps travel `travel` pixels, with their
/// names.
fn faders_height(travel: f32, cell: f32) -> f32 {
    faders(travel).height(cell)
}

pub fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let matrix = Matrix {
        slots: &params.mod_slots,
        monitor: gui.live.monitor,
    };
    let plan = Plan::new(ui.available_rect_before_wrap(), params);
    let controls = Controls {
        setter,
        matrix: &matrix,
        highlight: widgets::take_highlight(ui.ctx()),
        style: faders(plan.travel),
        colors: COLORS,
        positions: None,
    };
    ui.allocate_rect(plan.extent, Sense::hover());
    for (cheek, left) in plan.cheeks.into_iter().zip([true, false]) {
        paint_cheek(ui.painter(), cheek, left);
    }
    let painter = ui.painter();
    painter.rect_filled(plan.panel, CornerRadius::ZERO, PANEL);
    let edge = |y: f32, color: Color32| {
        painter.line_segment([Pos2::new(plan.panel.left(), y), Pos2::new(plan.panel.right(), y)], Stroke::new(1.0_f32, color));
    };
    edge(plan.panel.top() + 0.5, Color32::from_white_alpha(30));
    edge(plan.panel.bottom() - 0.5, Color32::from_black_alpha(140));
    nameplate(ui, plan.nameplate, params, &controls, gui);
    for &(part, rect) in &plan.parts {
        section(ui, part, rect, plan.cell, params, &controls, gui);
    }
    widgets::routes_window(ui.ctx(), &controls, gui);
}

/// A wooden end cheek, with its grain, light along its outer edge and a shadow along the panel.
fn paint_cheek(painter: &Painter, rect: Rect, left: bool) {
    let rounding = if left {
        CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }
    } else {
        CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }
    };
    painter.rect_filled(rect, rounding, WOOD);
    let lines = 7;
    for line in 0..lines {
        let x = rect.left() + (line as f32 + 0.5) * rect.width() / lines as f32;
        let phase = line as f32 * 2.1 + if left { 0.0 } else { 5.0 };
        let points = (0..=80)
            .map(|step| {
                let t = step as f32 / 80.0;
                let waver = 1.4 * (t * 7.0 + phase).sin() + 0.7 * (t * 19.0 + 1.3 * phase).sin();
                Pos2::new(x + waver, rect.top() + 3.0 + t * (rect.height() - 6.0))
            })
            .collect();
        let color = if line % 2 == 0 { WOOD_DARK } else { WOOD_LIGHT };
        painter.add(Shape::line(points, Stroke::new(1.0_f32, color.gamma_multiply(0.7))));
    }
    let (outer, inner) = if left {
        (rect.left() + 1.0, rect.right() - 1.0)
    } else {
        (rect.right() - 1.0, rect.left() + 1.0)
    };
    let light = Stroke::new(1.0_f32, Color32::from_white_alpha(45));
    painter.line_segment([Pos2::new(outer, rect.top() + 6.0), Pos2::new(outer, rect.bottom() - 6.0)], light);
    let shadow = Stroke::new(2.0_f32, Color32::from_black_alpha(120));
    painter.line_segment([Pos2::new(inner, rect.top()), Pos2::new(inner, rect.bottom())], shadow);
}

/// The maker's name plate, with the presets, the tuning, the routes and settings windows, and
/// the theme.
fn nameplate(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(3), NAMEPLATE_FILL, Stroke::new(1.0_f32, Color32::from_black_alpha(160)), StrokeKind::Inside);
    let logo = TextFormat {
        font_id: FontId::proportional(26.0),
        color: SILK,
        italics: true,
        extra_letter_spacing: 0.5,
        ..Default::default()
    };
    let logo = painter.layout_job(LayoutJob::single_section(String::from("three_osc"), logo));
    let logo_rect = Align2::LEFT_TOP.anchor_size(rect.min + egui::vec2(14.0, 4.0), logo.size());
    painter.galley(logo_rect.min, logo, SILK);
    let line = Rect::from_min_size(Pos2::new(rect.left() + 15.0, logo_rect.bottom() - 2.0), egui::vec2(240.0, 12.0));
    silk(painter, line, Align2::LEFT_CENTER, "POLYPHONIC WAVETABLE SYNTHESIZER", 8.5, SILK_WEAK, 1.6);
    // Three stripes after the name, in the strips' colours
    for (index, color) in [RED.0, YELLOW.0, CREAM.0].into_iter().enumerate() {
        let x = logo_rect.right() + 10.0 + index as f32 * 9.0;
        let stripe = Rect::from_min_size(Pos2::new(x, logo_rect.top() + 9.0), egui::vec2(5.0, 15.0));
        painter.rect_filled(stripe, CornerRadius::same(1), color);
    }

    let area = Rect::from_min_max(Pos2::new(rect.left() + 290.0, rect.top()), rect.max - egui::vec2(12.0, 0.0));
    in_rect(ui, "nameplate", area, Layout::left_to_right(Align::Center), |ui| {
        title_bar::presets(ui, params, controls.setter, gui);
        ui.add_space(8.0);
        tuning_window::button(ui, params, &mut gui.state.tuning);
        widgets::routes_button(ui, controls.matrix, gui);
        widgets::settings_button(ui, gui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme_picker(ui, gui));
    });
}

/// A section: its box and strip, its switches, and its faders (or, for the scope and the MIDI
/// sources, what it has instead). Pressing anywhere on a section that has a display shows it on
/// the scope.
fn section(ui: &mut Ui, part: Part, rect: Rect, cell: f32, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let shown = &mut gui.state.vintage.shown;
    if part.display().is_some() && ui.rect_contains_pointer(rect) && ui.input(|input| input.pointer.primary_pressed()) {
        *shown = part;
    }
    let lit = part.display().map(|_| *shown == part);
    let caption = (part == Part::Scope).then(|| shown.name());
    let body = frame(ui, rect, part, lit, caption);
    let row = |index: usize| {
        let top = body.top() + index as f32 * (SWITCHES + ROW_GAP);
        Rect::from_min_size(Pos2::new(body.left(), top), egui::vec2(body.width(), SWITCHES))
    };
    let setter = controls.setter;
    let fx = &params.fx;
    match part {
        Part::Scope => {
            let shown = gui.state.vintage.shown;
            in_rect(ui, "scope", body, Layout::top_down(Align::Min), |ui| {
                if let Some(display) = shown.display() {
                    display.show(ui, body.height(), params, setter, gui);
                }
            });
            return;
        }
        Part::Midi => {
            in_rect(ui, "midi", body, Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for source in ModSource::MIDI {
                    let name = ModSource::variants()[source.to_index()].to_uppercase();
                    widgets::chip(ui, source, &name, controls, &chips());
                }
            });
            return;
        }
        Part::Oscillator(index) => oscillator_switches(ui, row(0), index, params, controls, gui),
        Part::Filter => {
            let filter = &params.filter;
            let glyphs = [Glyph::Lowpass, Glyph::Bandpass, Glyph::Highpass];
            let glyphs = glyphs.map(Label::Glyph);
            choice_buttons(ui, row(0), &filter.filter_type, &glyphs, help::FILTER_TYPE, setter);
            in_rect(ui, "filter model", row(1), Layout::left_to_right(Align::Center), |ui| {
                selector(ui, &filter.model, "filter model", MODEL_SELECTOR_WIDTH, help::FILTER_MODEL, setter);
            });
        }
        Part::Voice => {
            let labels = ["POLY", "MONO", "LEGATO"].map(Label::Text);
            choice_buttons(ui, row(0), &params.global.polyphony, &labels, help::POLYPHONY, setter);
        }
        Part::Distortion => switches(ui, row(0), &[("ON", &fx.distortion.enabled, help::DISTORTION)], setter),
        Part::Chorus => switches(ui, row(0), &[("ON", &fx.chorus.enabled, help::CHORUS)], setter),
        Part::Delay => {
            let toggles = [("ON", &fx.delay.enabled, help::DELAY), ("PING PONG", &fx.delay.ping_pong, help::DELAY_PING_PONG)];
            switches(ui, row(0), &toggles, setter);
        }
        Part::Reverb => switches(ui, row(0), &[("ON", &fx.reverb.enabled, help::REVERB)], setter),
        _ => {}
    }
    // The other modulation sources' handles, where the oscillators have their waves
    if let (Some(source), false) = (part.source(), matches!(part, Part::Oscillator(_))) {
        in_rect(ui, (part.name(), "handle"), row(0), Layout::left_to_right(Align::Center), |ui| {
            widgets::chip(ui, source, "MOD", controls, &chips());
        });
    }

    // The faders along the bottom, and an LFO's wave buttons beside its one
    let height = controls.style.height(cell);
    let faders = Rect::from_min_max(Pos2::new(body.left(), body.bottom() - height), body.max);
    let knobs = part.knobs(params);
    in_rect(ui, (part.name(), "faders"), faders, Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        catalog::show_all(ui, &knobs, cell, controls);
    });
    if let Part::Lfo(index) = part {
        let column = Rect::from_min_max(Pos2::new(faders.left() + cell + 6.0, faders.top()), faders.max);
        lfo_waves(ui, column, index, params, setter);
    }
}

/// Prints a section's box, and its strip with its name, the light that's lit while the scope
/// shows it (if it has a display) and a `caption` on the right. Returns the room under the strip.
fn frame(ui: &Ui, rect: Rect, part: Part, lit: Option<bool>, caption: Option<String>) -> Rect {
    let painter = ui.painter();
    painter.rect_stroke(rect, CornerRadius::same(3), Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
    let strip = Rect::from_min_size(rect.min, egui::vec2(rect.width(), STRIP)).shrink(1.0);
    let (fill, print) = part.strip();
    painter.rect_filled(strip, CornerRadius { nw: 2, ne: 2, sw: 0, se: 0 }, fill);
    let mut text = strip.shrink2(egui::vec2(6.0, 0.0));
    if let Some(lit) = lit {
        led(painter, Pos2::new(strip.right() - 9.0, strip.center().y), lit);
        text.max.x -= 14.0;
    }
    silk(painter, text, Align2::LEFT_CENTER, &part.name(), 10.5, print, 1.0);
    if let Some(caption) = caption {
        silk(painter, text, Align2::RIGHT_CENTER, &caption, 10.0, print, 0.8);
        let hint = ui.interact(strip, ui.id().with("scope strip"), Sense::hover());
        hint.on_hover_text("Shows the section last pressed on, whose light is lit.");
    }
    Rect::from_min_max(
        Pos2::new(rect.left() + PADDING, strip.bottom() + PADDING),
        rect.max - egui::vec2(PADDING, PADDING),
    )
}

/// An oscillator's wave, its wavetable file if it plays one, and its handle.
fn oscillator_switches(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let osc = params.oscillator(index);
    in_rect(ui, ("oscillator switches", index), rect, Layout::left_to_right(Align::Center), |ui| {
        selector(ui, &osc.wave, ("wave", index), WAVE_SELECTOR_WIDTH, help::OSC_WAVE, controls.setter);
        if osc.wave.value() == OscillatorWave::File {
            let file = params.wavetable_file(index);
            // (A long file name is cut short to fit)
            ui.allocate_ui(egui::vec2(FILE_WIDTH, SELECTOR_HEIGHT), |ui| {
                wavetable_file_combo(ui, index, file, gui, FILE_WIDTH);
            });
            if let Some(status) = wavetable_file_status(file) {
                ui.label(RichText::new("?").color(SILK_WEAK)).on_hover_text(status);
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            widgets::chip(ui, ModSource::osc(index), "MOD", controls, &chips());
        });
    });
}

/// Prints `text` in spaced letters, aligned in `rect`, smaller if it's too long for it.
fn silk(painter: &Painter, rect: Rect, align: Align2, text: &str, size: f32, color: Color32, spacing: f32) {
    let layout = |size: f32, spacing: f32| {
        let format = TextFormat {
            font_id: FontId::proportional(size),
            color,
            extra_letter_spacing: spacing,
            ..Default::default()
        };
        painter.layout_job(LayoutJob::single_section(text.to_owned(), format))
    };
    let mut galley = layout(size, spacing);
    if galley.size().x > rect.width() {
        let scale = (rect.width() / galley.size().x).max(0.6);
        galley = layout(size * scale, spacing * scale);
    }
    painter.galley(align.align_size_within_rect(galley.size(), rect).min, galley, color);
}

/// A rubber button's body, lighter while it's hovered or pressed.
fn rubber(painter: &Painter, rect: Rect, response: &Response) {
    let fill = if response.is_pointer_button_down_on() {
        WIDGET_ACTIVE
    } else if response.hovered() {
        WIDGET_HOVERED
    } else {
        WIDGET
    };
    painter.rect(rect, CornerRadius::same(2), fill, Stroke::new(1.0_f32, Color32::from_black_alpha(170)), StrokeKind::Inside);
    let top = rect.top() + 1.5;
    let highlight = Stroke::new(1.0_f32, Color32::from_white_alpha(30));
    painter.line_segment([Pos2::new(rect.left() + 2.0, top), Pos2::new(rect.right() - 2.0, top)], highlight);
}

/// A red light, glowing while it's lit.
fn led(painter: &Painter, center: Pos2, lit: bool) {
    if lit {
        painter.circle_filled(center, 5.5, LED_ON.gamma_multiply(0.25));
    }
    let color = if lit { LED_ON } else { LED_OFF };
    painter.circle(center, 2.7, color, Stroke::new(1.0_f32, Color32::from_black_alpha(130)));
}

/// What's printed over a lit button.
#[derive(Debug, Clone, Copy)]
enum Label {
    Text(&'static str),
    Glyph(Glyph),
}

/// A lit button filling `rect`: its label over a rubber button with a light in it.
fn led_button(ui: &mut Ui, rect: Rect, id_salt: impl Hash, label: Label, lit: bool) -> Response {
    let response = ui.interact(rect, ui.id().with(id_salt), Sense::click());
    let painter = ui.painter();
    let label_rect = Rect::from_min_size(rect.min, egui::vec2(rect.width(), 10.0));
    match label {
        Label::Text(text) => silk(painter, label_rect, Align2::CENTER_CENTER, text, 8.5, SILK, 0.6),
        Label::Glyph(glyph) => {
            glyph.paint(painter, Rect::from_center_size(label_rect.center(), egui::vec2(16.0, 9.0)), SILK);
        }
    }
    let width = (rect.width() - 8.0).clamp(14.0, 30.0);
    let button = Rect::from_center_size(Pos2::new(rect.center().x, rect.bottom() - 7.0), egui::vec2(width, 12.0));
    rubber(painter, button, &response);
    led(painter, button.center(), lit);
    response
}

/// Lit buttons side by side across `rect`, one for each of a parameter's choices with its
/// label, the one chosen lit.
fn choice_buttons<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    rect: Rect,
    param: &EnumParam<T>,
    labels: &[Label],
    help: &str,
    setter: &ParamSetter,
) {
    let names = T::variants();
    let width = rect.width() / names.len() as f32;
    let current = param.value().to_index();
    for (index, (name, &label)) in names.iter().zip(labels).enumerate() {
        let cell = Rect::from_min_size(rect.min + egui::vec2(index as f32 * width, 0.0), egui::vec2(width, rect.height()));
        let response = led_button(ui, cell, (param.name(), index), label, index == current);
        let response = response.on_hover_text(format!("{name}\n{help}"));
        if response.clicked() && index != current {
            set_param(setter, param, T::from_index(index));
        }
    }
}

/// Lit buttons from the left of `rect` that switch parameters on and off: each with its label,
/// its parameter and what it does.
fn switches(ui: &mut Ui, rect: Rect, toggles: &[(&'static str, &BoolParam, &str)], setter: &ParamSetter) {
    let width = (rect.width() / toggles.len() as f32).min(64.0);
    for (index, &(label, param, about)) in toggles.iter().enumerate() {
        let cell = Rect::from_min_size(rect.min + egui::vec2(index as f32 * width, 0.0), egui::vec2(width, rect.height()));
        let response = led_button(ui, cell, param.name(), Label::Text(label), param.value());
        if response.on_hover_text(about).clicked() {
            set_param(setter, param, !param.value());
        }
    }
}

/// A display of a parameter's choice in lit letters, with arrows either side to step through
/// the choices (round from the last to the first) and a list of them all on a click.
fn selector<T: Enum + PartialEq + Copy + 'static>(
    ui: &mut Ui,
    param: &EnumParam<T>,
    id_salt: impl Hash,
    width: f32,
    help: &str,
    setter: &ParamSetter,
) {
    let names = T::variants();
    let current = param.value().to_index();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, SELECTOR_HEIGHT), Sense::hover());
    let id = ui.id().with(id_salt);
    let arrow = 16.0;
    let mut picked = None;
    for (step, glyph, x) in [(names.len() - 1, "◀", rect.left()), (1, "▶", rect.right() - arrow)] {
        let button = Rect::from_min_size(Pos2::new(x, rect.top()), egui::vec2(arrow, rect.height()));
        let response = ui.interact(button, id.with(glyph), Sense::click());
        rubber(ui.painter(), button, &response);
        ui.painter().text(button.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(8.0), SILK);
        if response.on_hover_text(help).clicked() {
            picked = Some((current + step) % names.len());
        }
    }
    let window = Rect::from_x_y_ranges((rect.left() + arrow + 2.0)..=(rect.right() - arrow - 2.0), rect.y_range());
    let response = ui.interact(window, id.with("window"), Sense::click());
    let painter = ui.painter();
    painter.rect(window, CornerRadius::same(2), LED_SCREEN, Stroke::new(1.0_f32, Color32::from_black_alpha(200)), StrokeKind::Inside);
    let text = names[current].to_uppercase();
    let font = FontId::monospace(11.0);
    let galley = painter.layout_no_wrap(text.clone(), font.clone(), LED_ON);
    let galley = if galley.size().x > window.width() - 6.0 {
        let size = font.size * (window.width() - 6.0) / galley.size().x;
        painter.layout_no_wrap(text, FontId::monospace(size), LED_ON)
    } else {
        galley
    };
    painter.galley(window.center() - galley.size() / 2.0, galley, LED_ON);
    let response = response.on_hover_text(format!("{help}\n(Click for the list of them all.)"));
    let list = id.with("list");
    if response.clicked() {
        ui.memory_mut(|memory| memory.toggle_popup(list));
    }
    egui::popup_below_widget(ui, list, &response, PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(window.width());
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for (index, name) in names.iter().enumerate() {
                if ui.selectable_label(index == current, *name).clicked() {
                    picked = Some(index);
                }
            }
        });
    });
    if let Some(index) = picked.filter(|&index| index != current) {
        set_param(setter, param, T::from_index(index));
    }
}

/// An LFO's waves, as a column of lit buttons with their shapes beside them.
fn lfo_waves(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, setter: &ParamSetter) {
    let param = &params.lfos[index].wave;
    let current = param.value().to_index();
    let top = rect.center().y - 2.5 * WAVE_ROW;
    for (wave, glyph) in Glyph::LFO_WAVES.into_iter().enumerate() {
        let row = Rect::from_min_size(Pos2::new(rect.left(), top + wave as f32 * WAVE_ROW), egui::vec2(rect.width(), WAVE_ROW));
        let response = ui.interact(row, ui.id().with(("lfo wave", index, wave)), Sense::click());
        let painter = ui.painter();
        glyph.paint(painter, Rect::from_min_size(Pos2::new(row.left(), row.center().y - 4.5), egui::vec2(16.0, 9.0)), SILK);
        let button = Rect::from_center_size(Pos2::new(row.right() - 12.0, row.center().y), egui::vec2(24.0, 12.0));
        rubber(painter, button, &response);
        led(painter, button.center(), wave == current);
        let response = response.on_hover_text(format!("{}\n{}", Wave::variants()[wave], help::LFO_WAVE));
        if response.clicked() && wave != current {
            set_param(setter, param, Wave::from_index(wave));
        }
    }
}

/// A little picture printed over a button: a wave's shape, or a filter's response.
#[derive(Debug, Clone, Copy)]
enum Glyph {
    Sine,
    Triangle,
    Saw,
    Exponential,
    Square,
    Lowpass,
    Bandpass,
    Highpass,
}

impl Glyph {
    /// The LFOs' waves, in the order of their parameter's choices.
    const LFO_WAVES: [Self; 5] = [Self::Sine, Self::Triangle, Self::Saw, Self::Exponential, Self::Square];

    fn paint(self, painter: &Painter, rect: Rect, color: Color32) {
        let at = |x: f32, y: f32| Pos2::new(rect.left() + x * rect.width(), rect.bottom() - y * rect.height());
        let curve = |shape: &dyn Fn(f32) -> f32| {
            (0..=24)
                .map(|step| {
                    let x = step as f32 / 24.0;
                    at(x, shape(x))
                })
                .collect::<Vec<_>>()
        };
        let points = match self {
            Self::Sine => curve(&|x| 0.5 + 0.5 * (x * TAU).sin()),
            Self::Triangle => vec![at(0.0, 0.5), at(0.25, 1.0), at(0.75, 0.0), at(1.0, 0.5)],
            Self::Saw => vec![at(0.0, 0.0), at(1.0, 1.0), at(1.0, 0.0)],
            Self::Exponential => curve(&|x| (x * 3.0).exp_m1() / 3.0_f32.exp_m1()),
            Self::Square => vec![at(0.0, 1.0), at(0.5, 1.0), at(0.5, 0.0), at(1.0, 0.0)],
            Self::Lowpass => curve(&|x| if x < 0.5 { 0.8 } else { 0.8 * (1.0 - ((x - 0.5) / 0.5).powf(1.5)) }),
            Self::Highpass => curve(&|x| if x > 0.5 { 0.8 } else { 0.8 * (1.0 - ((0.5 - x) / 0.5).powf(1.5)) }),
            Self::Bandpass => curve(&|x| 0.9 * (-(x - 0.5).powi(2) * 20.0).exp()),
        };
        painter.add(Shape::line(points, Stroke::new(1.2_f32, color)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_fits_and_nothing_overlaps() {
        let params = ThreeOscParams::default();
        for size in [egui::vec2(700.0, 500.0), egui::vec2(1384.0, 984.0), egui::vec2(2600.0, 1600.0)] {
            let window = Rect::from_min_size(Pos2::new(10.0, 20.0), size);
            let plan = Plan::new(window, &params);
            let inner = plan.panel.shrink(MARGIN);
            assert!(plan.extent.contains_rect(plan.panel), "{size:?}");
            assert!(inner.contains_rect(plan.nameplate), "{size:?}");
            for (index, &(part, rect)) in plan.parts.iter().enumerate() {
                assert!(inner.expand(0.01).contains_rect(rect), "{part:?} at {size:?}");
                assert!(!rect.intersects(plan.nameplate), "{part:?} at {size:?}");
                assert!(rect.width() >= part.min_width() + 2.0 * PADDING, "{part:?} at {size:?}");
                for &(other, other_rect) in &plan.parts[index + 1..] {
                    assert!(!rect.shrink(0.5).intersects(other_rect), "{part:?} and {other:?} at {size:?}");
                }
            }
            // The LFOs' wave buttons fit beside their faders
            assert!(faders_height(plan.travel, plan.cell) >= 5.0 * WAVE_ROW, "{size:?}");
        }
        // A window the size the editor opens at fits the whole panel
        let window = Rect::from_min_size(Pos2::ZERO, egui::vec2(1384.0, 984.0));
        let plan = Plan::new(window, &params);
        assert!(window.expand(0.01).contains_rect(plan.panel.expand2(egui::vec2(CHEEK, 0.0))), "{:?}", plan.panel);
    }
}
