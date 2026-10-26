//! The Rack theme: every part of the synth as a module in a rack, like modular hardware.
//!
//! Two rows of aluminium panels sit on rails, the sound's modules (the oscillators, the filter,
//! the amp and its envelope) above the modulation sources and the effects. Every modulation
//! source has an output jack: dragging from it onto a knob routes the source there, and every
//! route is drawn as a patch cable from its source's jack to its knob (hovering a jack picks its
//! cables out). The knobs show their values while they are hovered, as the panels have no room
//! to print them.

use std::cell::RefCell;
use std::hash::Hash;

use nih_plug::prelude::*;
use nih_plug_egui::egui::epaint::CubicBezierShape;
use nih_plug_egui::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui,
};

use super::catalog::{self, Colors, Control};
use super::knob::{self, Controls};
use super::widgets::{self, in_rect};
use super::{
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, theme_picker, title_bar,
    tuning_window, wavetable_file_combo, wavetable_file_status, Gui, Live, Matrix, WaveDisplay, MOD_COLOR,
};
use crate::params::{EnvelopeParams, OscillatorWave, ThreeOscParams};
use crate::synth::modulation::{ModSource, ModTarget, N_SOURCES};
use crate::ThreeOscPlugin;

const CASE: Color32 = Color32::from_rgb(22, 23, 25);
const RAIL: Color32 = Color32::from_rgb(120, 125, 131);
const RAIL_EDGE: Color32 = Color32::from_rgb(160, 165, 170);
const PANEL: Color32 = Color32::from_rgb(216, 219, 222);
const PANEL_EDGE: Color32 = Color32::from_rgb(150, 154, 159);
const SCREW: Color32 = Color32::from_rgb(170, 174, 179);
const INK: Color32 = Color32::from_rgb(28, 30, 33);
const INK_WEAK: Color32 = Color32::from_rgb(96, 100, 106);
const TRACK: Color32 = Color32::from_rgb(178, 182, 187);
const WIDGET: Color32 = Color32::from_rgb(232, 234, 236);
const WIDGET_HOVERED: Color32 = Color32::from_rgb(244, 245, 246);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(200, 204, 208);
const SCREEN: Color32 = Color32::from_rgb(12, 16, 13);
/// The colours of the modules' stripes, and the knobs on them.
const SOUND_COLOR: Color32 = Color32::from_rgb(30, 120, 210);
const GLOBAL_COLOR: Color32 = Color32::from_rgb(120, 90, 200);
const EFFECT_COLOR: Color32 = Color32::from_rgb(180, 60, 140);

const MARGIN: f32 = 6.0;
const HEADER_HEIGHT: f32 = 36.0;
const RAIL_HEIGHT: f32 = 11.0;
const GAP: f32 = 6.0;
const PADDING: f32 = 8.0;
const TITLE_HEIGHT: f32 = 30.0;
/// The room a jack takes, with its label.
const JACK_HEIGHT: f32 = 46.0;
const DISPLAY_MIN: f32 = 36.0;
const MIN_WIDTH: f32 = 1180.0;
const MIN_HEIGHT: f32 = 840.0;

const KNOBS: knob::Style = knob::Style {
    font_size: 10.5,
    value_below: false,
    max_diameter: 34.0,
    ..knob::Style::STUDIO
};
const COLORS: Colors = Colors {
    sound: SOUND_COLOR,
    modulation: Color32::from_rgb(220, 120, 20),
    global: GLOBAL_COLOR,
};

/// The displays' colours: oscilloscope screens.
pub const PALETTE: displays::Palette = displays::Palette {
    screen: SCREEN,
    edge: Color32::from_rgb(70, 74, 79),
    signal: Color32::from_rgb(110, 255, 150),
    live: Color32::from_rgb(255, 242, 176),
    guide: Color32::from_rgb(30, 46, 34),
    label: Color32::from_rgb(80, 110, 88),
};

/// The colours of the patch cables, by source.
const CABLES: [Color32; N_SOURCES] = [
    Color32::GRAY,
    Color32::from_rgb(230, 60, 60),
    Color32::from_rgb(240, 200, 40),
    Color32::from_rgb(70, 200, 90),
    Color32::from_rgb(50, 190, 230),
    Color32::from_rgb(220, 90, 220),
    Color32::from_rgb(255, 140, 40),
    Color32::from_rgb(240, 240, 240),
    Color32::from_rgb(70, 110, 240),
    Color32::from_rgb(40, 200, 170),
    Color32::from_rgb(255, 110, 160),
    Color32::from_rgb(170, 230, 60),
    Color32::from_rgb(160, 100, 240),
    Color32::from_rgb(150, 110, 70),
    Color32::from_rgb(110, 140, 160),
    Color32::from_rgb(190, 160, 120),
];

/// The look of the Rack theme, for egui: dark text on light panels.
pub fn style() -> egui::Style {
    let mut style = egui::Style {
        visuals: egui::Visuals::light(),
        ..egui::Style::default()
    };
    let visuals = &mut style.visuals;
    visuals.panel_fill = CASE;
    visuals.window_fill = PANEL;
    visuals.window_stroke = Stroke::new(1.0_f32, PANEL_EDGE);
    visuals.extreme_bg_color = TRACK;
    visuals.faint_bg_color = WIDGET;
    visuals.override_text_color = None;
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = PANEL;
    widgets.noninteractive.weak_bg_fill = PANEL;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, PANEL_EDGE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, INK);
    for (widget, fill) in [
        (&mut widgets.inactive, WIDGET),
        (&mut widgets.hovered, WIDGET_HOVERED),
        (&mut widgets.active, WIDGET_ACTIVE),
        (&mut widgets.open, WIDGET_HOVERED),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, PANEL_EDGE);
        widget.corner_radius = CornerRadius::same(3);
    }
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, INK);
    widgets.hovered.fg_stroke = Stroke::new(1.5_f32, Color32::BLACK);
    widgets.active.fg_stroke = Stroke::new(2.0_f32, Color32::BLACK);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, Color32::BLACK);
    visuals.selection.bg_fill = Color32::from_rgb(150, 190, 235);
    visuals.selection.stroke = Stroke::new(1.0_f32, INK);
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(6.0, 2.0);
    style.spacing.interact_size.y = 20.0;
    style
}

/// What the Rack layout shows.
pub struct RackState {
    /// Whether the patch cables are drawn.
    cables: bool,
}

impl RackState {
    pub fn new() -> Self {
        Self { cables: true }
    }
}

/// Where each jack is, noted while the modules are drawn, for the cables to start from.
type Jacks = RefCell<Vec<(ModSource, Pos2)>>;

pub fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let matrix = Matrix {
        slots: &params.mod_slots,
        monitor: gui.live.monitor,
    };
    let positions = RefCell::new(Vec::new());
    let controls = Controls {
        setter,
        matrix: &matrix,
        highlight: widgets::take_highlight(ui.ctx()),
        style: KNOBS,
        colors: COLORS,
        positions: Some(&positions),
    };
    let jacks: Jacks = RefCell::new(Vec::new());
    let width = ui.available_width().max(MIN_WIDTH + 2.0 * MARGIN);
    let height = ui.available_height().max(MIN_HEIGHT + 2.0 * MARGIN);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let area = area.shrink(MARGIN);

    let header = Rect::from_min_size(area.min, egui::vec2(area.width(), HEADER_HEIGHT));
    header_strip(ui, header, params, &controls, gui);
    let rows = rows(area, header);
    let hp = hp_width(area.width());
    for (row, modules) in rows.iter().zip([ROW_SOUND, ROW_MODULATION]) {
        rails(ui.painter(), *row);
        let mut left = row.left();
        for (module, width) in modules {
            let rect = Rect::from_x_y_ranges(left..=left + width * hp, row.y_range());
            draw_module(ui, *module, rect, params, &controls, &jacks, gui);
            left = rect.right();
        }
        // Blank panels fill the rest of the row
        if row.right() - left > 1.0 {
            panel(ui.painter(), Rect::from_x_y_ranges(left..=row.right(), row.y_range()), None);
        }
    }
    if gui.state.rack.cables {
        let floor = area.bottom().min(ui.clip_rect().bottom());
        cables(ui.painter(), floor, &matrix, &positions.borrow(), &jacks.borrow(), controls.highlight);
    }
    widgets::routes_window(ui.ctx(), &controls, gui);
}

/// The modules, in their rows, with their widths in HP (as racks measure them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Module {
    Oscillator(usize),
    Filter,
    Amp,
    FilterEnvelope,
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

const ROW_SOUND: &[(Module, f32)] = &[
    (Module::Oscillator(0), 14.0),
    (Module::Oscillator(1), 14.0),
    (Module::Oscillator(2), 14.0),
    (Module::Filter, 16.0),
    (Module::Amp, 10.0),
    (Module::FilterEnvelope, 10.0),
    (Module::Voice, 8.0),
];
const ROW_MODULATION: &[(Module, f32)] = &[
    (Module::Envelope(0), 10.0),
    (Module::Envelope(1), 10.0),
    (Module::Lfo(0), 6.0),
    (Module::Lfo(1), 6.0),
    (Module::Lfo(2), 6.0),
    (Module::Midi, 6.0),
    (Module::Distortion, 6.0),
    (Module::Chorus, 6.0),
    (Module::Delay, 7.0),
    (Module::Reverb, 6.0),
    (Module::Output, 12.0),
];

/// The width of an HP in a rack `width` wide: the longer row fills it.
fn hp_width(width: f32) -> f32 {
    let total = |row: &[(Module, f32)]| row.iter().map(|(_, width)| width).sum::<f32>();
    width / total(ROW_SOUND).max(total(ROW_MODULATION))
}

/// The two rows of modules, between their rails.
fn rows(area: Rect, header: Rect) -> [Rect; 2] {
    let top = header.bottom() + GAP;
    let height = (area.bottom() - top - GAP) / 2.0 - 2.0 * RAIL_HEIGHT;
    [0, 1].map(|row| {
        let row_top = top + RAIL_HEIGHT + row as f32 * (height + 2.0 * RAIL_HEIGHT + GAP);
        Rect::from_x_y_ranges(area.x_range(), row_top..=row_top + height)
    })
}

/// The rails above and below a row of modules, with their slots.
fn rails(painter: &Painter, row: Rect) {
    for top in [row.top() - RAIL_HEIGHT, row.bottom()] {
        let rail = Rect::from_x_y_ranges(row.x_range(), top..=top + RAIL_HEIGHT);
        painter.rect_filled(rail, CornerRadius::ZERO, RAIL);
        painter.line_segment([rail.left_top(), rail.right_top()], Stroke::new(1.0_f32, RAIL_EDGE));
        let mut x = rail.left() + 10.0;
        while x < rail.right() - 10.0 {
            let slot = Rect::from_center_size(Pos2::new(x, rail.center().y), egui::vec2(9.0, 3.0));
            painter.rect_filled(slot, CornerRadius::same(1), Color32::from_gray(60));
            x += 30.0;
        }
    }
}

/// Draws a module's panel, with its screws and its name over a stripe of `accent`. Returns the
/// area under its name.
fn panel(painter: &Painter, rect: Rect, title: Option<(&str, Color32)>) -> Rect {
    let rect = rect.shrink2(egui::vec2(0.5, 0.0));
    painter.rect(rect, CornerRadius::same(2), PANEL, Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
    // A highlight along the top edge, as on brushed metal
    painter.line_segment(
        [rect.left_top() + egui::vec2(1.0, 1.0), rect.right_top() + egui::vec2(-1.0, 1.0)],
        Stroke::new(1.0_f32, Color32::from_gray(240)),
    );
    for corner in [rect.left_top() + egui::vec2(7.0, 7.0), rect.right_bottom() - egui::vec2(7.0, 7.0)] {
        painter.circle(corner, 3.5, SCREW, Stroke::new(1.0_f32, PANEL_EDGE));
        painter.line_segment([corner - egui::vec2(2.0, 0.0), corner + egui::vec2(2.0, 0.0)], Stroke::new(1.0_f32, INK_WEAK));
    }
    let Some((title, accent)) = title else {
        return rect;
    };
    painter.text(
        Pos2::new(rect.center().x, rect.top() + 14.0),
        egui::Align2::CENTER_CENTER,
        title.to_uppercase(),
        FontId::proportional(12.0),
        INK,
    );
    let stripe = Rect::from_center_size(Pos2::new(rect.center().x, rect.top() + 25.0), egui::vec2(rect.width() - 24.0, 2.0));
    painter.rect_filled(stripe, CornerRadius::same(1), accent);
    Rect::from_min_max(
        Pos2::new(rect.left() + PADDING, rect.top() + TITLE_HEIGHT),
        Pos2::new(rect.right() - PADDING, rect.bottom() - PADDING - 4.0),
    )
}

/// The header across the top of the case: the plugin's name, the presets, the tuning, the routes
/// and settings windows, the cables, and the theme.
fn header_strip(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    // A blank panel the width of the rack, as the maker's name plate
    panel(ui.painter(), rect, None);
    in_rect(ui, "header", rect.shrink2(egui::vec2(18.0, 0.0)), Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new("THREE OSC").size(17.0).strong().color(INK).extra_letter_spacing(2.0));
        ui.label(RichText::new(format!("v{}", ThreeOscPlugin::VERSION)).small().color(INK_WEAK));
        ui.add_space(16.0);
        title_bar::presets(ui, params, controls.setter, gui);
        ui.add_space(8.0);
        tuning_window::button(ui, params, &mut gui.state.tuning);
        ui.add_space(8.0);
        widgets::routes_button(ui, controls.matrix, gui);
        widgets::settings_button(ui, gui);
        ui.add_space(8.0);
        let cables = &mut gui.state.rack.cables;
        ui.checkbox(cables, "Cables")
            .on_hover_text("Shows every modulation route as a patch cable from its source's jack to its knob.");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme_picker(ui, gui));
    });
}

/// Lays `knobs` out in rows of `columns` from the top of `rect`, returning how tall they are.
fn knob_grid(ui: &mut Ui, id_salt: impl Hash, rect: Rect, knobs: &[Control], columns: usize, controls: &Controls) -> f32 {
    let cell = rect.width() / columns as f32;
    let height = KNOBS.height(cell);
    for (row, knobs) in knobs.chunks(columns).enumerate() {
        let top = rect.top() + row as f32 * (height + 4.0);
        // A row short of knobs is centred
        let width = knobs.len() as f32 * cell;
        let row_rect = Rect::from_min_size(Pos2::new(rect.center().x - width / 2.0, top), egui::vec2(width, height));
        in_rect(ui, (&id_salt, row), row_rect, Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            catalog::show_all(ui, knobs, cell, controls);
        });
    }
    knobs.len().div_ceil(columns) as f32 * (height + 4.0)
}

/// The height of `count` knobs in rows of `columns` across `width`.
fn grid_height(width: f32, count: usize, columns: usize) -> f32 {
    count.div_ceil(columns) as f32 * (KNOBS.height(width / columns as f32) + 4.0)
}

/// A source's output jack, centred at the bottom of `body`, labelled `label`: dragged from onto a
/// knob it routes the source there, drawing a cable to the pointer while it's dragged.
fn jack(ui: &mut Ui, body: Rect, label: &str, source: ModSource, controls: &Controls, jacks: &Jacks) {
    let center = Pos2::new(body.center().x, body.bottom() - 12.0);
    jack_at(ui, center, label, source, controls, jacks);
}

fn jack_at(ui: &mut Ui, center: Pos2, label: &str, source: ModSource, controls: &Controls, jacks: &Jacks) {
    let rect = Rect::from_center_size(center - egui::vec2(0.0, 8.0), egui::vec2(44.0, 40.0));
    let response = ui.interact(rect, ui.id().with(("jack", source.to_index())), Sense::click_and_drag());
    let lit = response.hovered() || response.dragged() || controls.highlight == Some(source);
    let painter = ui.painter();
    painter.text(center - egui::vec2(0.0, 19.0), egui::Align2::CENTER_CENTER, label, FontId::proportional(9.5), INK);
    // A hex nut round a socket
    let nut: Vec<Pos2> = (0..6)
        .map(|corner| center + 10.0 * egui::Vec2::angled(std::f32::consts::FRAC_PI_3 * corner as f32))
        .collect();
    painter.add(egui::Shape::convex_polygon(nut, if lit { MOD_COLOR } else { SCREW }, Stroke::new(1.0_f32, PANEL_EDGE)));
    painter.circle_filled(center, 5.5, Color32::from_gray(20));
    painter.circle_filled(center, 2.5, Color32::from_gray(70));
    jacks.borrow_mut().push((source, center));
    let color = CABLES[source.to_index()];
    let floor = ui.clip_rect().bottom();
    widgets::source_handle(ui, response, source, controls.matrix, |painter, pointer| {
        cable(painter, center, pointer, floor, color, 1.0);
    });
}

/// A patch cable from `from` to `to`, sagging under its weight (but not below `floor`),
/// `strength` of the way opaque.
fn cable(painter: &Painter, from: Pos2, to: Pos2, floor: f32, color: Color32, strength: f32) {
    // A cubic's control points pull it down about three quarters of the way to them
    let room = (floor - from.y.max(to.y)).max(0.0) / 0.75;
    let sag = (30.0 + 0.25 * from.distance(to)).min(room);
    let points = [from, from + egui::vec2(0.0, sag), to + egui::vec2(0.0, sag), to];
    let shadow = Color32::from_black_alpha((140.0 * strength) as u8);
    painter.add(CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, Stroke::new(6.0_f32, shadow)));
    let color = color.gamma_multiply(0.9 * strength);
    painter.add(CubicBezierShape::from_points_stroke(points, false, Color32::TRANSPARENT, Stroke::new(3.5_f32, color)));
    for end in [from, to] {
        painter.circle(end, 4.0, color, Stroke::new(1.0_f32, shadow));
    }
}

/// Every route as a cable from its source's jack to its knob. While a jack is hovered, its
/// cables stand out from the others.
fn cables(
    painter: &Painter,
    floor: f32,
    matrix: &Matrix,
    knobs: &[(ModTarget, Pos2)],
    jacks: &[(ModSource, Pos2)],
    highlight: Option<ModSource>,
) {
    for slot in matrix.slots.iter().filter(|slot| Matrix::is_used(slot)) {
        let (source, target) = (slot.source.value(), slot.target.value());
        let from = jacks.iter().find(|(jack, _)| *jack == source).map(|(_, at)| *at);
        let to = knobs.iter().find(|(knob, _)| *knob == target).map(|(_, at)| *at);
        if let (Some(from), Some(to)) = (from, to) {
            let strength = match highlight {
                Some(lit) if lit != source => 0.3,
                _ => 1.0,
            };
            // Plugged in just above the knob, so that it stays in view
            cable(painter, from, to - egui::vec2(0.0, 14.0), floor, CABLES[source.to_index()], strength);
        }
    }
}

fn draw_module(ui: &mut Ui, module: Module, rect: Rect, params: &ThreeOscParams, controls: &Controls, jacks: &Jacks, gui: &mut Gui) {
    match module {
        Module::Oscillator(index) => oscillator(ui, rect, index, params, controls, jacks, gui),
        Module::Filter => filter(ui, rect, params, controls, gui),
        Module::Amp => {
            let source = (ModSource::VolumeEnv, "OUT");
            envelope_module(ui, rect, "Amp", source, &params.vol_env, None, controls, jacks, gui.live);
        }
        Module::FilterEnvelope => {
            let source = (ModSource::FilterEnv, "OUT");
            let amount = catalog::filter_envelope_amount(&params.filter_env).labelled("Amount");
            envelope_module(ui, rect, "Filter Env", source, &params.filter_env.envelope, Some(amount), controls, jacks, gui.live);
        }
        Module::Voice => voice(ui, rect, params, controls),
        Module::Envelope(index) => {
            let source = (ModSource::envelope(index), "OUT");
            let title = ["Env 1", "Env 2"][index];
            envelope_module(ui, rect, title, source, &params.mod_envs[index].envelope, None, controls, jacks, gui.live);
        }
        Module::Lfo(index) => lfo(ui, rect, index, params, controls, jacks, gui.live),
        Module::Midi => midi(ui, rect, controls, jacks),
        Module::Distortion | Module::Chorus | Module::Delay | Module::Reverb => effect(ui, rect, module, params, controls, gui.live),
        Module::Output => output(ui, rect, params, controls, gui),
    }
}

#[allow(clippy::too_many_arguments)]
fn oscillator(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, controls: &Controls, jacks: &Jacks, gui: &mut Gui) {
    let osc = params.oscillator(index);
    let file = params.wavetable_file(index);
    let body = panel(ui.painter(), rect, Some((&format!("Osc {}", index + 1), SOUND_COLOR)));
    let mut knobs = catalog::oscillator(osc, index).to_vec();
    knobs.extend(catalog::oscillator_modulation(params, index).into_iter().flatten());
    let grid = grid_height(body.width(), knobs.len(), 4);
    let playing_file = osc.wave.value() == OscillatorWave::File;
    let files = if playing_file { 42.0 } else { 0.0 };
    let display_height = (body.height() - 26.0 - files - grid - JACK_HEIGHT - 8.0).max(DISPLAY_MIN);
    let wave = WaveDisplay::new(index, osc, file, gui);
    let top = in_rect(ui, ("osc", index), body, Layout::top_down(Align::Min), |ui| {
        combo_cell(ui, &osc.wave, index, body.width() - 12.0, help::OSC_WAVE, controls.setter);
        wave.show(ui, display_height);
        if playing_file {
            wavetable_file_combo(ui, index, file, gui, body.width() - 12.0);
            if let Some(status) = wavetable_file_status(file) {
                ui.add(egui::Label::new(RichText::new(status).small().color(INK_WEAK)).truncate());
            }
        }
        ui.min_rect().bottom()
    });
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), top + 6.0), body.max);
    knob_grid(ui, ("osc knobs", index), grid_rect, &knobs, 4, controls);
    jack(ui, body, "MOD OUT", ModSource::osc(index), controls, jacks);
}

fn filter(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let (filter, filter_env) = (&params.filter, &params.filter_env);
    let setter = controls.setter;
    let body = panel(ui.painter(), rect, Some(("Filter", SOUND_COLOR)));
    let knobs = catalog::filter(filter);
    let grid = grid_height(body.width(), knobs.len(), 2);
    let display_height = (body.height() - 2.0 * 26.0 - grid - 8.0).max(DISPLAY_MIN);
    let oversampling = params.global.oversampling.value().to_oversampling();
    let (settings, live) = filter_display_inputs(filter, filter_env, oversampling, gui.live);
    let response = &mut gui.state.filter_response;
    let top = in_rect(ui, "filter", body, Layout::top_down(Align::Min), |ui| {
        combo_cell(ui, &filter.model, 0, body.width() - 12.0, help::FILTER_MODEL, setter);
        combo_cell(ui, &filter.filter_type, 0, body.width() - 12.0, help::FILTER_TYPE, setter);
        let edit = displays::FilterEdit {
            cutoff: &filter.cutoff,
            resonance: &filter.resonance,
            setter,
        };
        displays::filter(ui, settings, response, live, display_height, Some(edit));
        ui.min_rect().bottom()
    });
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), top + 6.0), body.max);
    knob_grid(ui, "filter knobs", grid_rect, &knobs, 2, controls);
}

/// An envelope's module: its display, its knobs (and `amount`, for the filter's), and its jack.
#[allow(clippy::too_many_arguments)]
fn envelope_module(
    ui: &mut Ui,
    rect: Rect,
    title: &str,
    (source, jack_label): (ModSource, &str),
    envelope: &EnvelopeParams,
    amount: Option<Control>,
    controls: &Controls,
    jacks: &Jacks,
    live: Live,
) {
    let body = panel(ui.painter(), rect, Some((title, COLORS.modulation)));
    let mut knobs = catalog::envelope(envelope).to_vec();
    knobs.extend(amount);
    let columns = if knobs.len() > 4 { 3 } else { 2 };
    let grid = grid_height(body.width(), knobs.len(), columns);
    let display_height = (body.height() - grid - JACK_HEIGHT - 8.0).max(DISPLAY_MIN);
    in_rect(ui, (title, "display"), body, Layout::top_down(Align::Min), |ui| {
        envelope_display(ui, envelope, controls.setter, live, display_height);
    });
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), body.top() + display_height + 8.0), body.max);
    knob_grid(ui, (title, "knobs"), grid_rect, &knobs, columns, controls);
    jack(ui, body, jack_label, source, controls, jacks);
}

fn voice(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls) {
    let global = &params.global;
    let body = panel(ui.painter(), rect, Some(("Voice", GLOBAL_COLOR)));
    in_rect(ui, "voice", body, Layout::top_down(Align::Min), |ui| {
        combo_cell(ui, &global.polyphony, 0, body.width() - 12.0, help::POLYPHONY, controls.setter);
    });
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), body.top() + 34.0), body.max);
    knob_grid(ui, "voice knobs", grid_rect, &catalog::voice(global), 2, controls);
}

fn lfo(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, controls: &Controls, jacks: &Jacks, live: Live) {
    let lfo = &params.lfos[index];
    let body = panel(ui.painter(), rect, Some((&format!("LFO {}", index + 1), COLORS.modulation)));
    let rate = [catalog::lfo_rate(lfo, index)];
    let grid = grid_height(body.width(), 1, 1);
    let display_height = (body.height() - 26.0 - grid - JACK_HEIGHT - 8.0).max(DISPLAY_MIN);
    let wave = lfo.wave.value().to_osc_wave();
    let phase = live.monitor.lfo_phase(index);
    let top = in_rect(ui, ("lfo", index), body, Layout::top_down(Align::Min), |ui| {
        combo_cell(ui, &lfo.wave, index, body.width() - 12.0, help::LFO_WAVE, controls.setter);
        displays::lfo(ui, &wave, MOD_COLOR, phase, display_height);
        ui.min_rect().bottom()
    });
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), top + 6.0), body.max);
    knob_grid(ui, ("lfo knob", index), grid_rect, &rate, 1, controls);
    jack(ui, body, "OUT", ModSource::lfo(index), controls, jacks);
}

/// The sources that come from MIDI (and the random one), a jack each.
fn midi(ui: &mut Ui, rect: Rect, controls: &Controls, jacks: &Jacks) {
    let body = panel(ui.painter(), rect, Some(("MIDI", COLORS.modulation)));
    let labels = ["VELOCITY", "KEY", "WHEEL", "TOUCH", "RANDOM"];
    let step = (body.height() / labels.len() as f32).min(64.0);
    for (index, (source, label)) in ModSource::MIDI.into_iter().zip(labels).enumerate() {
        let center = Pos2::new(body.center().x, body.top() + (index as f32 + 0.5) * step + 10.0);
        jack_at(ui, center, label, source, controls, jacks);
    }
}

/// An effect's module: its switch, a display while there's room for it, and its knobs down
/// it.
fn effect(ui: &mut Ui, rect: Rect, module: Module, params: &ThreeOscParams, controls: &Controls, live: Live) {
    let fx = &params.fx;
    let setter = controls.setter;
    // The displays show the settings as the newest note's modulation moves them
    let settings = match live.modulation() {
        Some(modulation) => fx.settings().modulated(&modulation),
        None => fx.settings(),
    };
    let (title, enabled, about, knobs) = match module {
        Module::Distortion => ("Dist", &fx.distortion.enabled, help::DISTORTION, catalog::distortion(&fx.distortion).to_vec()),
        Module::Chorus => ("Chorus", &fx.chorus.enabled, help::CHORUS, catalog::chorus(&fx.chorus).to_vec()),
        Module::Delay => ("Delay", &fx.delay.enabled, help::DELAY, catalog::delay(&fx.delay).to_vec()),
        _ => ("Reverb", &fx.reverb.enabled, help::REVERB, catalog::reverb(&fx.reverb).to_vec()),
    };
    let body = panel(ui.painter(), rect, Some((title, EFFECT_COLOR)));
    let switches = in_rect(ui, (title, "switches"), body, Layout::top_down(Align::Center), |ui| {
        widgets::switch(ui, enabled, about, setter, EFFECT_COLOR);
        if module == Module::Delay {
            ui.label(RichText::new("PING-PONG").size(9.5).color(INK)).on_hover_text(help::DELAY_PING_PONG);
            widgets::switch(ui, &fx.delay.ping_pong, help::DELAY_PING_PONG, setter, EFFECT_COLOR);
        }
        ui.min_rect().bottom()
    });
    let grid = grid_height(body.width(), knobs.len(), 1);
    let display_height = body.bottom() - switches - grid - 14.0;
    let mut top = switches + 6.0;
    if display_height >= 30.0 {
        let display = Rect::from_min_size(Pos2::new(body.left(), top), egui::vec2(body.width(), display_height));
        in_rect(ui, (title, "display"), display, Layout::top_down(Align::Min), |ui| match module {
            Module::Distortion => {
                let s = settings.distortion;
                displays::distortion(ui, display_height, s.drive_db, s.mix, s.enabled);
            }
            Module::Chorus => displays::chorus(ui, display_height, settings.chorus.depth, settings.chorus.enabled),
            Module::Delay => {
                let s = settings.delay;
                displays::delay(ui, display_height, s.time, s.feedback, s.mix, s.ping_pong, s.enabled);
            }
            _ => {
                let s = settings.reverb;
                displays::reverb(ui, display_height, s.size, s.damping, s.enabled);
            }
        });
        top += display_height + 8.0;
    }
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), top), body.max);
    in_rect(ui, (title, "knobs"), grid_rect, Layout::top_down(Align::Min), |ui| {
        if !enabled.value() {
            ui.multiply_opacity(0.55);
        }
        knob_grid(ui, (title, "grid"), grid_rect, &knobs, 1, controls);
    });
}

/// What the synth plays: a spectrogram and meters, over the output's level and width.
fn output(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let body = panel(ui.painter(), rect, Some(("Output", GLOBAL_COLOR)));
    let knobs = catalog::output(&params.global);
    let grid = grid_height(body.width(), knobs.len(), 2);
    let analyser_height = body.height() - grid - 8.0;
    let live = gui.live;
    in_rect(ui, "output display", body, Layout::top_down(Align::Min), |ui| {
        analyser::output(ui, &mut gui.state.analyser, &live.monitor.output, live.sample_rate, body.width(), analyser_height);
    });
    let grid_rect = Rect::from_min_max(Pos2::new(body.left(), body.top() + analyser_height + 8.0), body.max);
    knob_grid(ui, "output knobs", grid_rect, &knobs, 2, controls);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_modules_fit_their_rows() {
        let default = egui::vec2(crate::editor::DEFAULT_WIDTH as f32, crate::editor::DEFAULT_HEIGHT as f32);
        for size in [default - egui::vec2(16.0, 16.0), egui::vec2(MIN_WIDTH, MIN_HEIGHT), egui::vec2(1920.0, 1200.0)] {
            let area = Rect::from_min_size(Pos2::ZERO, size);
            let header = Rect::from_min_size(area.min, egui::vec2(area.width(), HEADER_HEIGHT));
            let rows = rows(area, header);
            assert!(rows[0].top() >= header.bottom() + RAIL_HEIGHT);
            assert!(rows[1].top() >= rows[0].bottom() + 2.0 * RAIL_HEIGHT);
            assert!(rows[1].bottom() + RAIL_HEIGHT <= area.bottom() + 0.5);
            let hp = hp_width(area.width());
            for row in [ROW_SOUND, ROW_MODULATION] {
                assert!(row.iter().map(|(_, width)| width * hp).sum::<f32>() <= area.width() + 0.5);
            }
            // Every module that takes knobs has room for them, and its jack, in its height
            let height = rows[0].height() - TITLE_HEIGHT - PADDING - 4.0;
            let osc_width = 14.0 * hp - 2.0 * PADDING;
            assert!(26.0 + DISPLAY_MIN + grid_height(osc_width, 13, 4) + JACK_HEIGHT + 8.0 <= height, "{size:?}");
            let filter_width = 16.0 * hp - 2.0 * PADDING;
            assert!(2.0 * 26.0 + DISPLAY_MIN + grid_height(filter_width, 4, 2) + 8.0 <= height, "{size:?}");
        }
    }
}
