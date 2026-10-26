//! Pieces the layouts share: the handles the modulation sources are dragged from, switches,
//! tabs, the rows of the modulation routes, the settings that aren't part of a sound (with a
//! window to hold them, for the layouts without a page of their own for them), and the parts'
//! displays for the layouts with one display that shows any of them.

use std::hash::Hash;
use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, Color32, CornerRadius, FontId, Galley, Layout, Painter, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder,
};
use nih_plug_egui::widgets::ParamSlider;

use super::knob::Controls;
use super::theme::{self, Theme};
use super::{
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, load_wavetable_file, set_param,
    switch_theme, title_bar, wavetable_folder_rows, Gui, Live, Matrix, WaveDisplay, MOD_COLOR,
};
use crate::params::{GlobalParams, ModSlotParams, OscillatorWave, ThreeOscParams};
use crate::synth::modulation::{ModSource, N_SLOTS};

/// Runs `add` in a child of `ui` that fills `rect`, laid out by `layout`, with the ids of the
/// widgets in it scoped by `id_salt`.
pub fn in_rect<R>(ui: &mut Ui, id_salt: impl Hash, rect: Rect, layout: Layout, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child = ui.new_child(UiBuilder::new().id_salt(id_salt).max_rect(rect).layout(layout));
    add(&mut child)
}

/// The id of the modulation source whose handle is hovered, kept from one frame to the next for
/// the knobs it modulates to light up.
fn highlight_id() -> egui::Id {
    egui::Id::new("highlighted source")
}

/// The modulation source whose handle was hovered in the last frame, whose knobs light up in this
/// one.
pub fn take_highlight(ctx: &egui::Context) -> Option<ModSource> {
    ctx.data_mut(|data| data.remove_temp::<Option<ModSource>>(highlight_id())).flatten()
}

/// Makes `response` a modulation source's handle: dragging it onto a knob routes the source
/// there. While it's dragged, `follow` draws what follows the pointer (on top of everything,
/// given where the pointer is); while it's hovered, the knobs the source modulates light up, and
/// it says what the source is.
pub fn source_handle(
    ui: &Ui,
    response: Response,
    source: ModSource,
    matrix: &Matrix,
    follow: impl FnOnce(&Painter, Pos2),
) -> Response {
    if response.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), source);
    }
    if response.dragged() {
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            follow(&ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, response.id)), pointer);
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        return response;
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        ui.ctx().data_mut(|data| data.insert_temp(highlight_id(), Some(source)));
    }
    let routes = match matrix.routes_from(source) {
        0 => String::from("It doesn't modulate anything yet."),
        1 => String::from("It modulates 1 knob, lit up while this is hovered."),
        routes => format!("It modulates {routes} knobs, lit up while this is hovered."),
    };
    response.on_hover_text(format!("{}\nDrag this onto a knob to modulate it. {routes}", source.help()))
}

/// How a layout's source chips look.
#[derive(Debug, Clone)]
pub struct ChipStyle {
    pub color: Color32,
    pub text: Color32,
    /// The colour of the number on the badge, on the chip's colour.
    pub badge_text: Color32,
    pub font: FontId,
    pub height: f32,
}

impl ChipStyle {
    /// An orange chip with `text` on it.
    pub fn new(text: Color32, badge_text: Color32) -> Self {
        Self {
            color: MOD_COLOR,
            text,
            badge_text,
            font: FontId::proportional(12.0),
            height: 20.0,
        }
    }
}

/// The width of a chip's dots.
const GRIP_WIDTH: f32 = 10.0;

/// A modulation source's handle (see [`source_handle()`]) as a chip: `label` with dots to grab,
/// and a badge with the number of routes the source drives.
pub fn chip(ui: &mut Ui, source: ModSource, label: &str, controls: &Controls, style: &ChipStyle) -> Response {
    let routes = controls.matrix.routes_from(source);
    let painter = ui.painter();
    let text = painter.layout_no_wrap(label.to_owned(), style.font.clone(), style.text);
    let badge_font = FontId::new(style.font.size - 2.0, style.font.family.clone());
    let badge = (routes > 0).then(|| painter.layout_no_wrap(routes.to_string(), badge_font, style.badge_text));
    let badge_width = badge.as_ref().map_or(0.0, |badge| badge.size().x.max(8.0) + 10.0);
    let size = egui::vec2(GRIP_WIDTH + text.size().x + badge_width + 14.0, style.height);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let lit = response.hovered() || response.dragged() || controls.highlight == Some(source);
    paint_chip(ui.painter(), rect, &text, badge.as_ref(), style, lit);
    source_handle(ui, response, source, controls.matrix, |painter, pointer| {
        paint_chip(painter, Rect::from_center_size(pointer, size), &text, badge.as_ref(), style, true);
    })
}

fn paint_chip(painter: &Painter, rect: Rect, text: &Arc<Galley>, badge: Option<&Arc<Galley>>, style: &ChipStyle, lit: bool) {
    let color = style.color;
    let fill = color.gamma_multiply(if lit { 0.36 } else { 0.18 });
    let rounding = CornerRadius::same((rect.height() / 2.0) as u8);
    painter.rect(rect, rounding, fill, Stroke::new(1.0_f32, color), StrokeKind::Inside);
    // Two columns of three dots, to grab
    for column in 0..2 {
        for row in 0..3 {
            let dot = Pos2::new(rect.left() + 8.0 + 4.0 * column as f32, rect.center().y + 4.0 * (row as f32 - 1.0));
            painter.circle_filled(dot, 1.1, color);
        }
    }
    let text_pos = Pos2::new(rect.left() + 6.0 + GRIP_WIDTH, rect.center().y - text.size().y / 2.0);
    painter.galley(text_pos, text.clone(), style.text);
    if let Some(badge) = badge {
        let width = badge.size().x.max(8.0) + 4.0;
        let badge_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 6.0 - width / 2.0, rect.center().y),
            egui::vec2(width, 13.0),
        );
        painter.rect_filled(badge_rect, CornerRadius::same(7), color);
        painter.galley(badge_rect.center() - badge.size() / 2.0, badge.clone(), style.badge_text);
    }
}

/// A switch for a parameter that is either on or off, `on` coloured while it's on.
pub fn switch(ui: &mut Ui, param: &BoolParam, help: &str, setter: &ParamSetter, on: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 16.0), Sense::click());
    if response.clicked() {
        set_param(setter, param, !param.value());
    }
    let position = ui.ctx().animate_bool(response.id, param.value());
    let visuals = ui.visuals();
    let track = if param.value() { on.gamma_multiply(0.75) } else { visuals.widgets.inactive.bg_fill };
    let edge = Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color);
    ui.painter().rect(rect, CornerRadius::same(8), track, edge, StrokeKind::Inside);
    let x = egui::lerp((rect.left() + 8.0)..=(rect.right() - 8.0), position);
    let knob = if response.hovered() {
        visuals.widgets.hovered.fg_stroke.color
    } else {
        visuals.widgets.inactive.fg_stroke.color
    };
    ui.painter().circle_filled(Pos2::new(x, rect.center().y), 6.0, knob);
    response.on_hover_text(help)
}

/// A tab's button: `label`, with what it holds (`badge`, in the modulation colour) after it,
/// lit in `accent` while it's `selected`.
pub fn tab(ui: &mut Ui, label: &str, badge: Option<String>, selected: bool, accent: Color32, font: FontId) -> Response {
    let visuals = ui.visuals();
    let text_color = visuals.strong_text_color();
    let painter = ui.painter();
    let text = painter.layout_no_wrap(label.to_owned(), font.clone(), text_color);
    let badge_font = FontId::new(font.size - 2.0, font.family.clone());
    let badge = badge.map(|badge| painter.layout_no_wrap(badge, badge_font, MOD_COLOR));
    let width = text.size().x + badge.as_ref().map_or(0.0, |badge| badge.size().x + 6.0) + 24.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ui.spacing().interact_size.y + 2.0), Sense::click());
    let visuals = ui.visuals();
    let edge = visuals.widgets.noninteractive.bg_stroke.color;
    let (fill, stroke) = if selected {
        (accent.gamma_multiply(0.22), Stroke::new(1.0_f32, accent))
    } else if response.hovered() {
        (visuals.widgets.hovered.bg_fill, Stroke::new(1.0_f32, edge))
    } else {
        (visuals.widgets.inactive.bg_fill, Stroke::new(1.0_f32, edge))
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    let left = rect.left() + 12.0;
    painter.galley(Pos2::new(left, rect.center().y - text.size().y / 2.0), text.clone(), text_color);
    if let Some(badge) = badge {
        let at = Pos2::new(left + text.size().x + 6.0, rect.center().y - badge.size().y / 2.0);
        painter.galley(at, badge, MOD_COLOR);
    }
    response
}

/// A row of a grid of modulation routes: a slot's source and target, its amount (in a slider
/// `slider_width` wide), and a button to remove it. Returns whether it was removed.
pub fn route_row(ui: &mut Ui, index: usize, slot: &ModSlotParams, setter: &ParamSetter, widths: [f32; 3]) -> bool {
    let [source_width, target_width, slider_width] = widths;
    combo_cell(ui, &slot.source, index, source_width, slot.source.value().help(), setter);
    ui.label(RichText::new("›").weak());
    combo_cell(ui, &slot.target, index, target_width, slot.target.value().help(), setter);
    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(slider_width))
        .on_hover_text(help::MOD_AMOUNT);
    let removed = ui.button("Remove").on_hover_text(help::CLEAR_ROUTE).clicked();
    if removed {
        Matrix::clear(setter, slot);
    }
    ui.end_row();
    removed
}

/// A button that adds a row to the routes editor (see [`routes_list()`]), while a slot is free.
pub fn add_route_button(ui: &mut Ui, matrix: &Matrix, added: &mut [bool; N_SLOTS]) {
    let free = matrix
        .slots
        .iter()
        .enumerate()
        .position(|(index, slot)| !Matrix::is_used(slot) && !added[index]);
    let button = ui
        .add_enabled(free.is_some(), egui::Button::new("Add a route"))
        .on_hover_text("Adds a row to pick a source and a target in.")
        .on_disabled_hover_text("All of the routes are in use.");
    if let (true, Some(free)) = (button.clicked(), free) {
        added[free] = true;
    }
}

/// The modulation routes in use, and the slots `added` (with [`add_route_button()`]) that don't
/// route anything yet, as rows to edit (see [`route_row()`]), in a scrolling grid. A slot added
/// stops being kept once it's used, so that removing it hides it.
pub fn routes_list(ui: &mut Ui, controls: &Controls, added: &mut [bool; N_SLOTS], widths: [f32; 3]) {
    let matrix = controls.matrix;
    for (index, slot) in matrix.slots.iter().enumerate() {
        if Matrix::is_used(slot) {
            added[index] = false;
        }
    }
    let shown: Vec<usize> = (0..N_SLOTS)
        .filter(|index| Matrix::is_used(&matrix.slots[*index]) || added[*index])
        .collect();
    if shown.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new(
                    "Nothing is modulated yet. Drag a source's handle onto a knob, or add a route \
                     here.",
                )
                .weak(),
            );
        });
        return;
    }
    egui::ScrollArea::vertical().id_salt("routes").auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("routes").num_columns(5).spacing([10.0, 6.0]).show(ui, |ui| {
            for heading in ["Source", "", "Target", "Amount", ""] {
                ui.label(RichText::new(heading).small().weak());
            }
            ui.end_row();
            for index in shown {
                if route_row(ui, index, &matrix.slots[index], controls.setter, widths) {
                    added[index] = false;
                }
            }
        });
    });
}

/// Where a window `width` wide first opens: in the middle, below the title bar, so that the
/// buttons that open and close it stay in reach.
fn below_the_header(ctx: &egui::Context, width: f32) -> Pos2 {
    Pos2::new(ctx.screen_rect().center().x - width / 2.0, 70.0)
}

/// A button that opens the routes window, with how many routes are in use.
pub fn routes_button(ui: &mut Ui, matrix: &Matrix, gui: &mut Gui) {
    let used = matrix.slots.iter().filter(|slot| Matrix::is_used(slot)).count();
    let label = if used > 0 { format!("Routes ({used})") } else { String::from("Routes") };
    let button = ui.button(label).on_hover_text("Every modulation route, to change, add and remove.");
    if button.clicked() {
        gui.state.routes_open = !gui.state.routes_open;
    }
}

/// The modulation routes in a window, while it's open, for the layouts without a page of their
/// own for them.
pub fn routes_window(ctx: &egui::Context, controls: &Controls, gui: &mut Gui) {
    let mut open = gui.state.routes_open;
    let added = &mut gui.state.added_routes;
    egui::Window::new("Modulation Routes")
        .open(&mut open)
        .collapsible(false)
        .default_size(egui::vec2(720.0, 360.0))
        .default_pos(below_the_header(ctx, 720.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(
                        "At 100% a source's full swing moves a control across its whole range, the \
                         pitch targets by an octave and the cutoff by 10 octaves.",
                    )
                    .small()
                    .weak(),
                );
            });
            add_route_button(ui, controls.matrix, added);
            ui.separator();
            routes_list(ui, controls, added, [140.0, 190.0, 180.0]);
        });
    gui.state.routes_open &= open;
}

/// A setting with its name and control, and what it does written out under them.
fn setting(ui: &mut Ui, name: &str, help: &str, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).strong());
        ui.with_layout(Layout::right_to_left(egui::Align::Center), control);
    });
    ui.label(RichText::new(help).small().weak());
    ui.add_space(8.0);
}

/// The settings that trade quality against CPU, each explained.
pub fn quality_settings(ui: &mut Ui, global: &GlobalParams, setter: &ParamSetter, combo_width: f32) {
    setting(ui, "Oversampling", help::OVERSAMPLING, |ui| {
        combo_cell(ui, &global.oversampling, 0, combo_width, help::OVERSAMPLING, setter)
    });
    setting(ui, "Interpolation", help::INTERPOLATION, |ui| {
        combo_cell(ui, &global.interpolation, 0, combo_width, help::INTERPOLATION, setter)
    });
    setting(ui, "Modulation Rate", help::MODULATION_RATE, |ui| {
        combo_cell(ui, &global.modulation_rate, 0, combo_width, help::MODULATION_RATE, setter)
    });
}

/// The wavetable folder (see [`wavetable_folder_rows()`]) and the files in it, with buttons to
/// play each on an oscillator.
pub fn wavetable_files(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Wavetable Files").strong()).on_hover_text(help::WAVETABLE_FOLDER);
        if ui.small_button("Rescan").on_hover_text("Lists the folder's files again.").clicked() {
            gui.state.rescan();
        }
    });
    let path_width = (ui.available_width() - 200.0).max(120.0);
    wavetable_folder_rows(ui, params, setter, gui, path_width);
    ui.add_space(4.0);
    ui.label(RichText::new("In the folder:").small().weak());
    egui::ScrollArea::vertical().id_salt("wavetable files").auto_shrink([false, true]).max_height(220.0).show(ui, |ui| {
        if gui.state.files.is_empty() {
            ui.label(RichText::new("No .wav files yet.").weak());
        }
        let names = gui.state.files.clone();
        for name in names {
            ui.horizontal(|ui| {
                for osc in 0..3 {
                    let button = ui.small_button(format!("Osc {}", osc + 1));
                    if button.on_hover_text(format!("Plays it on Osc {}.", osc + 1)).clicked() {
                        load_wavetable_file(params.wavetable_file(osc), osc, name.clone(), gui.executor);
                        set_param(setter, &params.oscillator(osc).wave, OscillatorWave::File);
                    }
                }
                ui.add(egui::Label::new(&name).truncate());
            });
        }
    });
}

/// Every theme, to pick one, with what each is like.
pub fn theme_choices(ui: &mut Ui, gui: &mut Gui) {
    let current = theme::current();
    for option in Theme::ALL {
        if ui.radio(current == option, RichText::new(option.name()).strong()).clicked() && current != option {
            switch_theme(ui, gui, option);
        }
        ui.label(RichText::new(option.description()).small().weak());
        ui.add_space(6.0);
    }
    ui.label(RichText::new("The theme is kept for every project, in Settings.json in Documents/Three Osc.").small().weak());
    if let Some(problem) = &gui.state.theme_problem {
        ui.label(RichText::new(problem).small().color(title_bar::ERROR_COLOR));
    }
}

/// A button that opens the settings window.
pub fn settings_button(ui: &mut Ui, gui: &mut Gui) {
    let button = ui
        .button("Settings")
        .on_hover_text("Quality against CPU, the wavetable folder and the theme.");
    if button.clicked() {
        gui.state.settings_open = !gui.state.settings_open;
    }
}

/// The settings window, while it's open: the quality settings, the wavetable folder and the
/// theme.
pub fn settings_window(ctx: &egui::Context, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let mut open = gui.state.settings_open;
    egui::Window::new("Settings")
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_width(460.0)
        .default_pos(below_the_header(ctx, 460.0))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().max_height(620.0).show(ui, |ui| {
                ui.set_width(440.0);
                ui.heading("Quality");
                quality_settings(ui, &params.global, setter, 120.0);
                ui.separator();
                wavetable_files(ui, params, setter, gui);
                ui.separator();
                ui.heading("Theme");
                theme_choices(ui, gui);
            });
        });
    gui.state.settings_open &= open;
}

/// What a layout's one display for every part of the synth can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Oscillator(usize),
    Filter,
    FilterEnvelope,
    Amp,
    Envelope(usize),
    Lfo(usize),
    Distortion,
    Chorus,
    Delay,
    Reverb,
    Output,
}

impl Display {
    /// Shows it, `height` tall: an oscillator's wave, an envelope, the filter's response, an
    /// LFO's wave, an effect, or the output's spectrogram and meters.
    pub fn show(self, ui: &mut Ui, height: f32, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
        let live = gui.live;
        match self {
            Self::Oscillator(index) => {
                WaveDisplay::new(index, params.oscillator(index), params.wavetable_file(index), gui).show(ui, height);
            }
            Self::Filter => {
                let (filter, filter_env) = (&params.filter, &params.filter_env);
                let oversampling = params.global.oversampling.value().to_oversampling();
                let (settings, playing) = filter_display_inputs(filter, filter_env, oversampling, live);
                let edit = displays::FilterEdit {
                    cutoff: &filter.cutoff,
                    resonance: &filter.resonance,
                    setter,
                };
                displays::filter(ui, settings, &mut gui.state.filter_response, playing, height, Some(edit));
            }
            Self::FilterEnvelope => envelope_display(ui, &params.filter_env.envelope, setter, live, height),
            Self::Amp => envelope_display(ui, &params.vol_env, setter, live, height),
            Self::Envelope(index) => envelope_display(ui, &params.mod_envs[index].envelope, setter, live, height),
            Self::Lfo(index) => {
                let wave = params.lfos[index].wave.value().to_osc_wave();
                displays::lfo(ui, &wave, MOD_COLOR, live.monitor.lfo_phase(index), height);
            }
            Self::Distortion | Self::Chorus | Self::Delay | Self::Reverb => self.effect(ui, params, live, height),
            Self::Output => {
                let width = ui.available_width();
                analyser::output(ui, &mut gui.state.analyser, &live.monitor.output, live.sample_rate, width, height);
            }
        }
    }

    /// An effect's display, as the newest note's modulation has the effect set.
    fn effect(self, ui: &mut Ui, params: &ThreeOscParams, live: Live, height: f32) {
        let fx = &params.fx;
        let settings = match live.modulation() {
            Some(modulation) => fx.settings().modulated(&modulation),
            None => fx.settings(),
        };
        let (d, c, e, r) = (settings.distortion, settings.chorus, settings.delay, settings.reverb);
        match self {
            Self::Distortion => displays::distortion(ui, height, d.drive_db, d.mix, d.enabled),
            Self::Chorus => displays::chorus(ui, height, c.depth, c.enabled),
            Self::Delay => displays::delay(ui, height, e.time, e.feedback, e.mix, e.ping_pong, e.enabled),
            Self::Reverb => displays::reverb(ui, height, r.size, r.damping, r.enabled),
            _ => {}
        }
    }
}
