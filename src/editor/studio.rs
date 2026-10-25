//! The Studio theme: the controls as knobs (see [`knob`]) grouped in cards, laid out to fill
//! the window.
//!
//! The header holds the presets, the tuning, the pages and the theme. Below it is the page: the
//! oscillators and the filter (Synth), or the effects, the modulation routes or the advanced
//! settings in their place. Underneath that, whatever the page, are the modulation sources: the
//! four envelopes across the window, then the three LFOs beside the output's spectrogram and
//! meters, and along the bottom the sources that come from MIDI. Every source has a handle, an
//! orange chip with dots, that can be dragged onto a knob to route the source there; the knob
//! then shows the route, and right-clicking it changes it. Hovering a handle lights up the
//! knobs it modulates.

use std::hash::Hash;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, Align, Color32, CornerRadius, FontId, Galley, Layout, Pos2, Rect, Response, RichText, Sense, Stroke,
    StrokeKind, Ui, UiBuilder,
};
use nih_plug_egui::widgets::ParamSlider;
use std::sync::Arc;

use super::knob::{self, Controls, Knob};
use super::theme::{self, Theme};
use super::{
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, load_wavetable_file, set_param,
    switch_theme, theme_picker, title_bar, tuning_window, wavetable_file_combo, wavetable_file_status,
    wavetable_folder_rows, Gui, Live, Matrix, WaveDisplay, MOD_COLOR,
};
use crate::params::{EnvelopeParams, FxParams, GlobalParams, LfoParams, OscillatorWave, ThreeOscParams};
use crate::synth::modulation::{ModSource, ModTarget, N_SLOTS};
use crate::ThreeOscPlugin;

const BACKGROUND: Color32 = Color32::from_rgb(17, 19, 24);
const CARD: Color32 = Color32::from_rgb(27, 30, 37);
const CARD_STROKE: Color32 = Color32::from_rgb(42, 46, 56);
/// The background of the displays and the text fields.
const DISPLAY: Color32 = Color32::from_rgb(12, 14, 18);
const WIDGET: Color32 = Color32::from_rgb(42, 46, 57);
const WIDGET_HOVERED: Color32 = Color32::from_rgb(55, 60, 73);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(66, 72, 88);
const TEXT: Color32 = Color32::from_rgb(208, 213, 222);
const TEXT_STRONG: Color32 = Color32::from_rgb(244, 246, 250);
const TEXT_WEAK: Color32 = Color32::from_rgb(126, 134, 148);
/// The colour of the sound's path, the oscillators, the filter and the effects, as their
/// displays draw it. (The modulation sources are in the modulation colour.)
const SOUND_COLOR: Color32 = Color32::from_rgb(90, 180, 255);
/// The colour of the settings for the whole synth.
const GLOBAL_COLOR: Color32 = Color32::from_rgb(175, 160, 255);

/// Space between the cards, around the window's edge (besides the window's own margin), and
/// inside the cards.
const GAP: f32 = 8.0;
const MARGIN: f32 = 6.0;
const PADDING: f32 = 10.0;
const HEADER_HEIGHT: f32 = 34.0;
const FOOTER_HEIGHT: f32 = 26.0;
/// A card's title row, and the space between it and what's below, and between a display and
/// the knobs under it.
const TITLE_HEIGHT: f32 = 22.0;
const INNER_GAP: f32 = 6.0;
/// The space between rows of knobs.
const ROW_GAP: f32 = 2.0;
/// The shortest the displays get, when the window is short.
const DISPLAY_MIN: f32 = 46.0;
/// The room an oscillator's wavetable file takes, when it plays one.
const FILE_ROWS: f32 = 42.0;
/// The shortest the output's spectrogram and meters get.
const OUTPUT_MIN: f32 = 92.0;
/// The narrowest the layout gets: a narrower window scrolls.
const MIN_WIDTH: f32 = 1180.0;
const COMBO_WIDTH: f32 = 112.0;
/// How the height left over once every row has what it needs is shared out: to the page, the
/// envelopes and the LFOs.
const SPARE_SHARES: [f32; 3] = [0.35, 0.45, 0.2];

/// The look of the Studio theme, for egui.
pub fn style() -> egui::Style {
    let mut style = egui::Style::default();
    let visuals = &mut style.visuals;
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = CARD;
    visuals.window_stroke = Stroke::new(1.0_f32, CARD_STROKE);
    visuals.extreme_bg_color = DISPLAY;
    visuals.faint_bg_color = Color32::from_rgb(32, 35, 43);
    visuals.window_corner_radius = CornerRadius::same(8);
    visuals.menu_corner_radius = CornerRadius::same(8);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = CARD;
    widgets.noninteractive.weak_bg_fill = CARD;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, CARD_STROKE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    for (widget, fill) in [
        (&mut widgets.inactive, WIDGET),
        (&mut widgets.hovered, WIDGET_HOVERED),
        (&mut widgets.active, WIDGET_ACTIVE),
        (&mut widgets.open, WIDGET_HOVERED),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.corner_radius = CornerRadius::same(5);
    }
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT_STRONG);
    widgets.active.fg_stroke = Stroke::new(2.0_f32, TEXT_STRONG);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(92, 100, 118));
    widgets.active.bg_stroke = Stroke::new(1.0_f32, SOUND_COLOR);
    visuals.selection.bg_fill = Color32::from_rgb(36, 84, 132);
    visuals.selection.stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    style.text_styles = [
        (egui::TextStyle::Small, FontId::proportional(10.5)),
        (egui::TextStyle::Body, FontId::proportional(13.0)),
        (egui::TextStyle::Button, FontId::proportional(13.0)),
        (egui::TextStyle::Heading, FontId::proportional(18.0)),
        (egui::TextStyle::Monospace, FontId::monospace(12.5)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 5.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style.spacing.interact_size.y = 22.0;
    style.spacing.menu_margin = egui::Margin::same(8);
    style
}

/// What the Studio layout shows.
pub struct StudioState {
    page: Page,
    /// The modulation matrix's slots the Matrix page shows although they don't route anything
    /// (yet), because they were added there.
    added_routes: [bool; N_SLOTS],
}

impl StudioState {
    pub fn new() -> Self {
        Self {
            page: Page::Synth,
            added_routes: [false; N_SLOTS],
        }
    }
}

/// What the top of the window shows. The modulation sources stay in view below it whatever it
/// is, to be dragged onto the knobs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Synth,
    Effects,
    Matrix,
    Advanced,
}

/// The id of the modulation source whose handle is hovered, kept from one frame to the next for
/// the knobs it modulates to light up.
fn highlight_id() -> egui::Id {
    egui::Id::new("studio highlighted source")
}

pub fn draw(ui: &mut Ui, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let matrix = Matrix {
        slots: &params.mod_slots,
        monitor: gui.live.monitor,
    };
    let highlight = ui
        .ctx()
        .data_mut(|data| data.remove_temp::<Option<ModSource>>(highlight_id()))
        .flatten();
    let controls = Controls { setter, matrix: &matrix, highlight };

    let width = ui.available_width().max(MIN_WIDTH + 2.0 * MARGIN);
    let height = ui.available_height().max(Areas::needed_height(width - 2.0 * MARGIN) + 2.0 * MARGIN);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    let areas = Areas::new(area.shrink(MARGIN));

    header(ui, areas.header, params, &controls, gui);
    match gui.state.studio.page {
        Page::Synth => synth_page(ui, &areas, params, &controls, gui),
        Page::Effects => effects_page(ui, areas.page, &params.fx, &controls, gui.live),
        Page::Matrix => matrix_page(ui, areas.page, &controls, gui),
        Page::Advanced => advanced_page(ui, areas.page, params, setter, gui),
    }
    envelopes(ui, areas.envelopes, params, &controls, gui.live);
    lfos(ui, areas.lfos, params, &controls, gui);
    footer(ui, areas.footer, &controls);
}

/// Where everything goes in a window of a given size.
struct Areas {
    header: Rect,
    page: Rect,
    /// The width of the filter's column, right of the oscillators.
    filter_column: f32,
    envelopes: Rect,
    lfos: Rect,
    footer: Rect,
}

impl Areas {
    fn filter_column(width: f32) -> f32 {
        (width * 0.29).clamp(360.0, 470.0)
    }
    fn oscillator_width(width: f32) -> f32 {
        (width - Self::filter_column(width) - 3.0 * GAP) / 3.0
    }
    fn envelope_width(width: f32) -> f32 {
        (width - 3.0 * GAP) / 4.0
    }
    fn output_width(width: f32) -> f32 {
        (width * 0.3).clamp(340.0, 520.0)
    }
    fn lfo_width(width: f32) -> f32 {
        (width - Self::output_width(width) - 3.0 * GAP) / 3.0
    }

    /// The heights the page, the envelopes and the LFOs need, in a window `width` wide.
    fn needs(width: f32) -> [f32; 3] {
        let osc_cell = cell_width(Self::oscillator_width(width), OSC_CELLS);
        let page = card_height(DISPLAY_MIN + FILE_ROWS + INNER_GAP + 3.0 * knob::height(osc_cell) + 2.0 * ROW_GAP);
        let envelope_cell = cell_width(Self::envelope_width(width), ENVELOPE_CELLS);
        let envelopes = card_height(DISPLAY_MIN + INNER_GAP + knob::height(envelope_cell));
        let lfos = card_height(knob::height(LFO_CELL).max(OUTPUT_MIN));
        [page, envelopes, lfos]
    }

    fn needed_height(width: f32) -> f32 {
        HEADER_HEIGHT + FOOTER_HEIGHT + 4.0 * GAP + Self::needs(width).iter().sum::<f32>()
    }

    fn new(area: Rect) -> Self {
        let width = area.width();
        let needs = Self::needs(width);
        let spare = (area.height() - Self::needed_height(width)).max(0.0);
        let [page, envelopes, lfos] = std::array::from_fn(|row| needs[row] + spare * SPARE_SHARES[row]);
        let mut top = area.top();
        let mut next = |height: f32| {
            let rect = Rect::from_min_size(Pos2::new(area.left(), top), egui::vec2(width, height));
            top += height + GAP;
            rect
        };
        Self {
            header: next(HEADER_HEIGHT),
            page: next(page),
            filter_column: Self::filter_column(width),
            envelopes: next(envelopes),
            lfos: next(lfos),
            footer: next(FOOTER_HEIGHT),
        }
    }
}

/// How many knobs wide an oscillator's rows are, and the envelopes'.
const OSC_CELLS: usize = 5;
const ENVELOPE_CELLS: usize = 5;
/// The width of an LFO's rate knob, and the output's knobs.
const LFO_CELL: f32 = 72.0;

/// The width of each of `cells` knobs across a card `width` wide.
fn cell_width(width: f32, cells: usize) -> f32 {
    (width - 2.0 * PADDING) / cells as f32
}

/// The height of a card whose contents are `body` tall.
fn card_height(body: f32) -> f32 {
    2.0 * PADDING + TITLE_HEIGHT + INNER_GAP + body
}

/// Runs `add` in a child of `ui` that fills `rect`, laid out by `layout`, with the ids of the
/// widgets in it scoped by `id_salt`.
fn in_rect<R>(ui: &mut Ui, id_salt: impl Hash, rect: Rect, layout: Layout, add: impl FnOnce(&mut Ui) -> R) -> R {
    let mut child = ui.new_child(UiBuilder::new().id_salt(id_salt).max_rect(rect).layout(layout));
    add(&mut child)
}

/// A card's title row and the area under it for its contents.
struct Card {
    title: Rect,
    body: Rect,
}

/// Draws a card in `rect`, with a short stripe of `color` on its top edge.
fn card(ui: &Ui, rect: Rect, color: Color32) -> Card {
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(8), CARD, Stroke::new(1.0_f32, CARD_STROKE), StrokeKind::Inside);
    let stripe = Rect::from_min_size(rect.min + egui::vec2(PADDING, 0.0), egui::vec2(26.0, 2.0));
    painter.rect_filled(stripe, CornerRadius::same(1), color);
    let inner = rect.shrink(PADDING);
    let title = Rect::from_min_size(inner.min, egui::vec2(inner.width(), TITLE_HEIGHT));
    let body = Rect::from_min_max(Pos2::new(inner.left(), title.bottom() + INNER_GAP), inner.max);
    Card { title, body }
}

/// A card's title, in small capitals.
fn title_text(ui: &mut Ui, title: &str) -> Response {
    ui.label(RichText::new(title.to_uppercase()).size(12.0).strong().extra_letter_spacing(0.8).color(TEXT_STRONG))
}

/// Fills a card's title row: `left` from the left, then `right` from the right.
fn title_row(ui: &mut Ui, id_salt: impl Hash, rect: Rect, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    in_rect(ui, id_salt, rect, Layout::left_to_right(Align::Center), |ui| {
        left(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
}

/// Lays out a row of knobs, each `cell` wide, from the left of `rect`.
fn knob_row(ui: &mut Ui, id_salt: impl Hash, rect: Rect, add: impl FnOnce(&mut Ui)) {
    in_rect(ui, id_salt, rect, Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        add(ui);
    });
}

/// The rectangle of a row of knobs `top` down a card's body.
fn row_rect(body: Rect, top: f32, height: f32) -> Rect {
    Rect::from_min_size(Pos2::new(body.left(), top), egui::vec2(body.width(), height))
}

/// A modulation source's handle: `label` on an orange chip with dots, and the number of routes
/// it drives. Dragged onto a knob it routes the source there, and while it's hovered the knobs
/// it modulates light up.
fn source_chip(ui: &mut Ui, source: ModSource, label: &str, controls: &Controls) -> Response {
    let routes = controls.matrix.routes_from(source);
    let painter = ui.painter();
    let text = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.0), TEXT_STRONG);
    let badge = (routes > 0).then(|| painter.layout_no_wrap(routes.to_string(), FontId::proportional(10.0), BACKGROUND));
    let badge_width = badge.as_ref().map_or(0.0, |badge| badge.size().x.max(8.0) + 10.0);
    let size = egui::vec2(GRIP_WIDTH + text.size().x + badge_width + 14.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if response.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), source);
    }
    let lit = response.hovered() || response.dragged() || controls.highlight == Some(source);
    paint_chip(ui.painter(), rect, &text, badge.as_ref(), lit);
    if response.dragged() {
        // A copy of it follows the pointer
        if let Some(pointer) = ui.ctx().pointer_interact_pos() {
            let layer = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, response.id));
            paint_chip(&layer, Rect::from_center_size(pointer, size), &text, badge.as_ref(), true);
        }
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        response
    } else {
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            ui.ctx().data_mut(|data| data.insert_temp(highlight_id(), Some(source)));
        }
        let routes = match routes {
            0 => String::from("It doesn't modulate anything yet."),
            1 => String::from("It modulates 1 knob, lit up while this is hovered."),
            routes => format!("It modulates {routes} knobs, lit up while this is hovered."),
        };
        response.on_hover_text(format!("{}\nDrag this onto a knob to modulate it. {routes}", source.help()))
    }
}

/// The width of a chip's dots.
const GRIP_WIDTH: f32 = 10.0;

fn paint_chip(painter: &egui::Painter, rect: Rect, text: &Arc<Galley>, badge: Option<&Arc<Galley>>, lit: bool) {
    let fill = MOD_COLOR.gamma_multiply(if lit { 0.36 } else { 0.18 });
    painter.rect(rect, CornerRadius::same(10), fill, Stroke::new(1.0_f32, MOD_COLOR), StrokeKind::Inside);
    // Two columns of three dots, to grab
    for column in 0..2 {
        for row in 0..3 {
            let dot = Pos2::new(rect.left() + 8.0 + 4.0 * column as f32, rect.center().y + 4.0 * (row as f32 - 1.0));
            painter.circle_filled(dot, 1.1, MOD_COLOR);
        }
    }
    let text_pos = Pos2::new(rect.left() + 6.0 + GRIP_WIDTH, rect.center().y - text.size().y / 2.0);
    painter.galley(text_pos, text.clone(), TEXT_STRONG);
    if let Some(badge) = badge {
        let width = badge.size().x.max(8.0) + 4.0;
        let badge_rect = Rect::from_center_size(
            Pos2::new(rect.right() - 6.0 - width / 2.0, rect.center().y),
            egui::vec2(width, 13.0),
        );
        painter.rect_filled(badge_rect, CornerRadius::same(7), MOD_COLOR);
        painter.galley(badge_rect.center() - badge.size() / 2.0, badge.clone(), BACKGROUND);
    }
}

/// A switch for a parameter that is either on or off.
fn switch(ui: &mut Ui, param: &BoolParam, help: &str, setter: &ParamSetter) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 16.0), Sense::click());
    if response.clicked() {
        set_param(setter, param, !param.value());
    }
    let on = ui.ctx().animate_bool(response.id, param.value());
    let painter = ui.painter();
    let track = if param.value() { SOUND_COLOR.gamma_multiply(0.75) } else { WIDGET };
    painter.rect(rect, CornerRadius::same(8), track, Stroke::new(1.0_f32, CARD_STROKE), StrokeKind::Inside);
    let x = egui::lerp((rect.left() + 8.0)..=(rect.right() - 8.0), on);
    let knob = if response.hovered() { TEXT_STRONG } else { TEXT };
    painter.circle_filled(Pos2::new(x, rect.center().y), 6.0, knob);
    response.on_hover_text(help)
}

/// The header: the plugin's name, the presets, the tuning, the pages and the theme.
fn header(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    in_rect(ui, "header", rect, Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new("THREE OSC").size(17.0).strong().extra_letter_spacing(1.5).color(TEXT_STRONG));
        ui.label(RichText::new(format!("v{}", ThreeOscPlugin::VERSION)).small().color(TEXT_WEAK));
        ui.add_space(16.0);
        title_bar::presets(ui, params, controls.setter, gui);
        ui.add_space(8.0);
        tuning_window::button(ui, params, &mut gui.state.tuning);
        ui.add_space(24.0);
        let page = &mut gui.state.studio.page;
        let effects = params.fx.enabled_count();
        let routes = controls.matrix.slots.iter().filter(|slot| Matrix::is_used(slot)).count();
        page_tab(ui, page, Page::Synth, "SYNTH", None, "The oscillators and the filter.");
        page_tab(
            ui,
            page,
            Page::Effects,
            "EFFECTS",
            (effects > 0).then(|| format!("{effects} on")),
            "The effects after the voices: distortion, chorus, delay and reverb, in that order. \
             They take the oscillators' place, so the modulation sources stay in view to be \
             dragged onto them.",
        );
        page_tab(
            ui,
            page,
            Page::Matrix,
            "MATRIX",
            (routes > 0).then(|| routes.to_string()),
            "Every modulation route, to change, add and remove.",
        );
        page_tab(ui, page, Page::Advanced, "ADVANCED", None, help::ADVANCED);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme_picker(ui, gui));
    });
}

/// A button for one of the pages, lit while it's shown, with what it holds (`badge`) after its
/// name.
fn page_tab(ui: &mut Ui, current: &mut Page, page: Page, label: &str, badge: Option<String>, help: &str) {
    let painter = ui.painter();
    let text = painter.layout_no_wrap(label.to_owned(), FontId::proportional(12.5), TEXT_STRONG);
    let badge = badge.map(|badge| painter.layout_no_wrap(badge, FontId::proportional(10.5), MOD_COLOR));
    let width = text.size().x + badge.as_ref().map_or(0.0, |badge| badge.size().x + 6.0) + 24.0;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 24.0), Sense::click());
    if response.clicked() {
        *current = page;
    }
    let selected = *current == page;
    let (fill, stroke) = if selected {
        (SOUND_COLOR.gamma_multiply(0.22), Stroke::new(1.0_f32, SOUND_COLOR))
    } else if response.hovered() {
        (WIDGET_HOVERED, Stroke::new(1.0_f32, CARD_STROKE))
    } else {
        (WIDGET, Stroke::new(1.0_f32, CARD_STROKE))
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(6), fill, stroke, StrokeKind::Inside);
    let left = rect.left() + 12.0;
    painter.galley(Pos2::new(left, rect.center().y - text.size().y / 2.0), text.clone(), TEXT_STRONG);
    if let Some(badge) = badge {
        let at = Pos2::new(left + text.size().x + 6.0, rect.center().y - badge.size().y / 2.0);
        painter.galley(at, badge, MOD_COLOR);
    }
    response.on_hover_text(help);
}

/// The oscillators, with the filter and the voice settings to their right.
fn synth_page(ui: &mut Ui, areas: &Areas, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let page = areas.page;
    let osc_width = (page.width() - areas.filter_column - 3.0 * GAP) / 3.0;
    for index in 0..3 {
        let left = page.left() + index as f32 * (osc_width + GAP);
        let rect = Rect::from_min_size(Pos2::new(left, page.top()), egui::vec2(osc_width, page.height()));
        oscillator(ui, rect, index, params, controls, gui);
    }
    let column = Rect::from_min_max(Pos2::new(page.right() - areas.filter_column, page.top()), page.max);
    let voice_height = card_height(knob::height(cell_width(column.width(), VOICE_CELLS)));
    let filter_rect = Rect::from_min_max(column.min, Pos2::new(column.right(), column.bottom() - voice_height - GAP));
    let voice_rect = Rect::from_min_max(Pos2::new(column.left(), column.bottom() - voice_height), column.max);
    filter(ui, filter_rect, params, controls, gui);
    voice(ui, voice_rect, &params.global, controls);
}

fn oscillator(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let osc = params.oscillator(index);
    let file = params.wavetable_file(index);
    let setter = controls.setter;
    let card = card(ui, rect, SOUND_COLOR);
    let name = format!("Osc {}", index + 1);
    title_row(
        ui,
        ("osc title", index),
        card.title,
        |ui| {
            source_chip(ui, ModSource::osc(index), &name, controls);
        },
        |ui| combo_cell(ui, &osc.wave, index, COMBO_WIDTH, help::OSC_WAVE, setter),
    );

    let body = card.body;
    let cell = cell_width(rect.width(), OSC_CELLS);
    let knob_height = knob::height(cell);
    let rows_top = body.bottom() - 3.0 * knob_height - 2.0 * ROW_GAP;
    let playing_file = osc.wave.value() == OscillatorWave::File;
    let files_height = if playing_file { FILE_ROWS } else { 0.0 };
    let display_height = (rows_top - INNER_GAP - files_height - body.top()).max(DISPLAY_MIN);
    let wave = WaveDisplay::new(index, osc, file, gui);
    in_rect(ui, ("osc display", index), body, Layout::top_down(Align::Min), |ui| {
        wave.show(ui, display_height);
        if playing_file {
            ui.horizontal(|ui| {
                ui.label(RichText::new("File").color(TEXT_WEAK)).on_hover_text(help::WAVETABLE_FILE);
                let width = ui.available_width() - 4.0;
                wavetable_file_combo(ui, index, file, gui, width);
            });
            if let Some(status) = wavetable_file_status(file) {
                ui.add(egui::Label::new(RichText::new(status).small().color(TEXT_WEAK)).truncate());
            }
        }
    });

    let row = |row: usize| row_rect(body, rows_top + row as f32 * (knob_height + ROW_GAP), knob_height);
    knob_row(ui, ("osc row 1", index), row(0), |ui| {
        Knob::new(&osc.amp, "Level", help::OSC_AMP, SOUND_COLOR)
            .modulated(ModTarget::osc_amp(index))
            .show(ui, cell, controls);
        Knob::new(&osc.pan, "Pan", help::OSC_PAN, SOUND_COLOR)
            .modulated(ModTarget::osc_pan(index))
            .show(ui, cell, controls);
        Knob::new(&osc.octave, "Octave", help::OSC_OCTAVE, SOUND_COLOR)
            .modulated(ModTarget::osc_octave(index))
            .show(ui, cell, controls);
        Knob::new(&osc.semitone, "Semi", help::OSC_SEMITONE, SOUND_COLOR)
            .modulated(ModTarget::osc_pitch(index))
            .show(ui, cell, controls);
        Knob::new(&osc.multiplier, "Mult", help::OSC_MULTIPLIER, SOUND_COLOR)
            .modulated(ModTarget::osc_multiplier(index))
            .show(ui, cell, controls);
    });
    knob_row(ui, ("osc row 2", index), row(1), |ui| {
        Knob::new(&osc.position, "Position", help::OSC_POSITION, SOUND_COLOR)
            .modulated(ModTarget::osc_position(index))
            .show(ui, cell, controls);
        Knob::new(&osc.phase, "Phase", help::OSC_PHASE, SOUND_COLOR).show(ui, cell, controls);
        Knob::new(&osc.phase_rand, "Rand", help::OSC_PHASE_RAND, SOUND_COLOR).show(ui, cell, controls);
        Knob::new(&osc.voices, "Unison", help::OSC_VOICES, SOUND_COLOR).show(ui, cell, controls);
        Knob::new(&osc.super_detune, "Detune", help::OSC_DETUNE, SOUND_COLOR)
            .modulated(ModTarget::osc_detune(index))
            .show(ui, cell, controls);
    });
    let modulation = [Some(&params.osc1_mod), Some(&params.osc2_mod), None][index];
    let third = row(2);
    match modulation {
        Some(modulation) => {
            let caption = Rect::from_min_size(third.min, egui::vec2(2.0 * cell, third.height()));
            ui.painter().text(
                caption.center(),
                egui::Align2::CENTER_CENTER,
                format!("FROM OSC {}  ›", index + 2),
                FontId::proportional(11.0),
                TEXT_WEAK,
            );
            let knobs = Rect::from_min_max(Pos2::new(caption.right(), third.top()), third.max);
            let target = |target: Option<ModTarget>| target.unwrap_or(ModTarget::None);
            knob_row(ui, ("osc row 3", index), knobs, |ui| {
                Knob::new(&modulation.pm, "PM", help::OSC_PM, SOUND_COLOR)
                    .modulated(target(ModTarget::osc_pm(index)))
                    .show(ui, cell, controls);
                Knob::new(&modulation.fm, "FM", help::OSC_FM, SOUND_COLOR)
                    .modulated(target(ModTarget::osc_fm(index)))
                    .show(ui, cell, controls);
                Knob::new(&modulation.am, "AM", help::OSC_AM, SOUND_COLOR)
                    .modulated(target(ModTarget::osc_am(index)))
                    .show(ui, cell, controls);
            });
        }
        None => {
            ui.painter().text(
                third.center(),
                egui::Align2::CENTER_CENTER,
                "Osc 3 modulates Osc 2, with Osc 2's PM, FM and AM.",
                FontId::proportional(11.0),
                TEXT_WEAK,
            );
        }
    }
}

/// How many knobs wide the filter's and the voice settings' rows are.
const FILTER_CELLS: usize = 5;
const VOICE_CELLS: usize = 5;

fn filter(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let (filter, filter_env) = (&params.filter, &params.filter_env);
    let setter = controls.setter;
    let card = card(ui, rect, SOUND_COLOR);
    title_row(
        ui,
        "filter title",
        card.title,
        |ui| {
            title_text(ui, "Filter");
        },
        |ui| {
            combo_cell(ui, &filter.filter_type, 0, COMBO_WIDTH - 16.0, help::FILTER_TYPE, setter);
            combo_cell(ui, &filter.model, 0, COMBO_WIDTH - 16.0, help::FILTER_MODEL, setter);
        },
    );
    let body = card.body;
    let cell = cell_width(rect.width(), FILTER_CELLS);
    let knob_height = knob::height(cell);
    let display_height = (body.height() - knob_height - INNER_GAP).max(DISPLAY_MIN);
    let oversampling = params.global.oversampling.value().to_oversampling();
    let (settings, live) = filter_display_inputs(filter, filter_env, oversampling, gui.live);
    let response = &mut gui.state.filter_response;
    in_rect(ui, "filter display", body, Layout::top_down(Align::Min), |ui| {
        let edit = displays::FilterEdit {
            cutoff: &filter.cutoff,
            resonance: &filter.resonance,
            setter,
        };
        displays::filter(ui, settings, response, live, display_height, Some(edit));
    });
    knob_row(ui, "filter knobs", row_rect(body, body.bottom() - knob_height, knob_height), |ui| {
        Knob::new(&filter.cutoff, "Cutoff", help::FILTER_CUTOFF, SOUND_COLOR)
            .modulated(ModTarget::Cutoff)
            .show(ui, cell, controls);
        Knob::new(&filter.resonance, "Resonance", help::FILTER_RESONANCE, SOUND_COLOR)
            .modulated(ModTarget::Resonance)
            .show(ui, cell, controls);
        Knob::new(&filter.drive, "Drive", help::FILTER_DRIVE, SOUND_COLOR)
            .modulated(ModTarget::Drive)
            .show(ui, cell, controls);
        Knob::new(&filter.keytrack, "Keytrack", help::FILTER_KEYTRACK, SOUND_COLOR).show(ui, cell, controls);
        Knob::new(&filter_env.amount, "Env Amt", help::FILTER_ENV_AMOUNT, MOD_COLOR).show(ui, cell, controls);
    });
}

/// The settings for how notes are played: the polyphony, gliding, the pitch wheel and the
/// pitch of the whole synth.
fn voice(ui: &mut Ui, rect: Rect, global: &GlobalParams, controls: &Controls) {
    let card = card(ui, rect, GLOBAL_COLOR);
    title_row(
        ui,
        "voice title",
        card.title,
        |ui| {
            title_text(ui, "Voice");
        },
        |ui| combo_cell(ui, &global.polyphony, 0, COMBO_WIDTH, help::POLYPHONY, controls.setter),
    );
    let cell = cell_width(rect.width(), VOICE_CELLS);
    knob_row(ui, "voice knobs", card.body, |ui| {
        Knob::new(&global.portamento_rate, "Glide", help::PORTAMENTO, GLOBAL_COLOR).show(ui, cell, controls);
        Knob::new(&global.pitch_offset, "Slide", help::PITCH_SLIDE, GLOBAL_COLOR).show(ui, cell, controls);
        Knob::new(&global.bend_range, "Bend", help::BEND_RANGE, GLOBAL_COLOR).show(ui, cell, controls);
        Knob::new(&global.octave_detune, "Drift", help::OCTAVE_DRIFT, GLOBAL_COLOR).show(ui, cell, controls);
        Knob::new(&global.global_pitch, "Pitch", help::GLOBAL_PITCH, GLOBAL_COLOR)
            .modulated(ModTarget::Pitch)
            .show(ui, cell, controls);
    });
}

/// The four envelopes, side by side.
fn envelopes(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, live: Live) {
    let width = (rect.width() - 3.0 * GAP) / 4.0;
    let envelopes = [
        (ModSource::VolumeEnv, "Volume Env.", &params.vol_env, "Shapes each note's volume."),
        (ModSource::FilterEnv, "Filter Env.", &params.filter_env.envelope, "Moves the filter's cutoff."),
        (ModSource::Env1, "Env 1", &params.mod_envs[0].envelope, "Free to modulate anything."),
        (ModSource::Env2, "Env 2", &params.mod_envs[1].envelope, "Free to modulate anything."),
    ];
    for (index, (source, name, envelope, about)) in envelopes.into_iter().enumerate() {
        let left = rect.left() + index as f32 * (width + GAP);
        let card_rect = Rect::from_min_size(Pos2::new(left, rect.top()), egui::vec2(width, rect.height()));
        let amount = (source == ModSource::FilterEnv).then_some(&params.filter_env.amount);
        envelope_card(ui, card_rect, source, name, about, envelope, amount, controls, live);
    }
}

#[allow(clippy::too_many_arguments)]
fn envelope_card(
    ui: &mut Ui,
    rect: Rect,
    source: ModSource,
    name: &str,
    about: &str,
    envelope: &EnvelopeParams,
    amount: Option<&FloatParam>,
    controls: &Controls,
    live: Live,
) {
    let card = card(ui, rect, MOD_COLOR);
    title_row(
        ui,
        (name, "title"),
        card.title,
        |ui| {
            source_chip(ui, source, name, controls);
        },
        |ui| {
            ui.label(RichText::new(about).small().color(TEXT_WEAK));
        },
    );
    let body = card.body;
    let cell = cell_width(rect.width(), ENVELOPE_CELLS);
    let knob_height = knob::height(cell);
    let display_height = (body.height() - knob_height - INNER_GAP).max(DISPLAY_MIN);
    in_rect(ui, (name, "display"), body, Layout::top_down(Align::Min), |ui| {
        envelope_display(ui, envelope, controls.setter, live, display_height);
    });
    // Centred, whether there are four knobs or five
    let knobs = if amount.is_some() { 5.0 } else { 4.0 };
    let row = row_rect(body, body.bottom() - knob_height, knob_height);
    let row = Rect::from_center_size(row.center(), egui::vec2(knobs * cell, row.height()));
    knob_row(ui, (name, "knobs"), row, |ui| {
        Knob::new(&envelope.attack, "Attack", help::ENV_ATTACK, MOD_COLOR).show(ui, cell, controls);
        Knob::new(&envelope.decay, "Decay", help::ENV_DECAY, MOD_COLOR).show(ui, cell, controls);
        Knob::new(&envelope.sustain, "Sustain", help::ENV_SUSTAIN, MOD_COLOR).show(ui, cell, controls);
        Knob::new(&envelope.release, "Release", help::ENV_RELEASE, MOD_COLOR).show(ui, cell, controls);
        if let Some(amount) = amount {
            Knob::new(amount, "Amount", help::FILTER_ENV_AMOUNT, MOD_COLOR).show(ui, cell, controls);
        }
    });
}

/// The three LFOs, and the output beside them.
fn lfos(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let output_width = Areas::output_width(rect.width());
    let width = Areas::lfo_width(rect.width());
    for (index, lfo) in params.lfos.iter().enumerate() {
        let left = rect.left() + index as f32 * (width + GAP);
        let card_rect = Rect::from_min_size(Pos2::new(left, rect.top()), egui::vec2(width, rect.height()));
        lfo_card(ui, card_rect, index, lfo, controls, gui.live);
    }
    let output = Rect::from_min_max(Pos2::new(rect.right() - output_width, rect.top()), rect.max);
    output_card(ui, output, &params.global, controls, gui);
}

fn lfo_card(ui: &mut Ui, rect: Rect, index: usize, lfo: &LfoParams, controls: &Controls, live: Live) {
    let card = card(ui, rect, MOD_COLOR);
    title_row(
        ui,
        ("lfo title", index),
        card.title,
        |ui| {
            source_chip(ui, ModSource::lfo(index), &format!("LFO {}", index + 1), controls);
        },
        |ui| combo_cell(ui, &lfo.wave, index, COMBO_WIDTH, help::LFO_WAVE, controls.setter),
    );
    let body = card.body;
    let knob_rect = Rect::from_min_size(
        Pos2::new(body.left(), body.center().y - knob::height(LFO_CELL) / 2.0),
        egui::vec2(LFO_CELL, knob::height(LFO_CELL)),
    );
    knob_row(ui, ("lfo knob", index), knob_rect, |ui| {
        Knob::new(&lfo.rate, "Rate", help::LFO_RATE, MOD_COLOR)
            .modulated(ModTarget::lfo_rate(index))
            .show(ui, LFO_CELL, controls);
    });
    let display = Rect::from_min_max(Pos2::new(knob_rect.right() + INNER_GAP, body.top()), body.max);
    let wave = lfo.wave.value().to_osc_wave();
    let phase = live.monitor.lfo_phase(index);
    in_rect(ui, ("lfo display", index), display, Layout::top_down(Align::Min), |ui| {
        displays::lfo(ui, &wave, MOD_COLOR, phase, display.height());
    });
}

/// What the synth plays, after the effects, as a spectrogram and meters, with the output's
/// level and stereo width.
fn output_card(ui: &mut Ui, rect: Rect, global: &GlobalParams, controls: &Controls, gui: &mut Gui) {
    let card = card(ui, rect, GLOBAL_COLOR);
    title_row(
        ui,
        "output title",
        card.title,
        |ui| {
            title_text(ui, "Output").on_hover_text(analyser::HELP);
        },
        |_| {},
    );
    let body = card.body;
    let knobs = Rect::from_min_max(
        Pos2::new(body.right() - 2.0 * LFO_CELL, body.center().y - knob::height(LFO_CELL) / 2.0),
        Pos2::new(body.right(), body.center().y + knob::height(LFO_CELL) / 2.0),
    );
    knob_row(ui, "output knobs", knobs, |ui| {
        Knob::new(&global.output_gain, "Gain", help::OUTPUT_GAIN, GLOBAL_COLOR)
            .modulated(ModTarget::Volume)
            .show(ui, LFO_CELL, controls);
        Knob::new(&global.stereo_width, "Width", help::STEREO_WIDTH, GLOBAL_COLOR).show(ui, LFO_CELL, controls);
    });
    let analyser_rect = Rect::from_min_max(body.min, Pos2::new(knobs.left() - INNER_GAP, body.bottom()));
    let live = gui.live;
    in_rect(ui, "output display", analyser_rect, Layout::top_down(Align::Min), |ui| {
        analyser::output(
            ui,
            &mut gui.state.analyser,
            &live.monitor.output,
            live.sample_rate,
            analyser_rect.width(),
            analyser_rect.height(),
        );
    });
}

/// The bottom row: the sources from MIDI, and how the knobs work.
fn footer(ui: &mut Ui, rect: Rect, controls: &Controls) {
    in_rect(ui, "footer", rect, Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new("MORE SOURCES").size(11.0).strong().extra_letter_spacing(0.8).color(TEXT_WEAK));
        for source in ModSource::MIDI {
            source_chip(ui, source, ModSource::variants()[source.to_index()], controls);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    RichText::new(
                        "Knobs: drag up or down, Shift for finer · double-click resets · click the \
                         value to type one · right-click for modulation",
                    )
                    .size(11.0)
                    .color(TEXT_WEAK),
                )
                .truncate(),
            );
        });
    });
}

/// The effects in the order they process the sound.
fn effects_page(ui: &mut Ui, rect: Rect, fx: &FxParams, controls: &Controls, live: Live) {
    // The displays show the settings as the newest note's modulation moves them
    let settings = match live.modulation() {
        Some(modulation) => fx.settings().modulated(&modulation),
        None => fx.settings(),
    };
    let width = (rect.width() - 3.0 * GAP) / 4.0;
    let card_at = |index: usize| {
        let left = rect.left() + index as f32 * (width + GAP);
        Rect::from_min_size(Pos2::new(left, rect.top()), egui::vec2(width, rect.height()))
    };
    // Arrows between them, for the order
    for index in 1..4 {
        let x = card_at(index).left() - GAP / 2.0;
        ui.painter().text(
            Pos2::new(x, rect.center().y),
            egui::Align2::CENTER_CENTER,
            "›",
            FontId::proportional(16.0),
            TEXT_WEAK,
        );
    }
    let setter = controls.setter;
    let (distortion, chorus, delay, reverb) = (&fx.distortion, &fx.chorus, &fx.delay, &fx.reverb);
    let distortion_settings = settings.distortion;
    effect_card(
        ui,
        card_at(0),
        Effect { name: "Distortion", enabled: &distortion.enabled, help: help::DISTORTION, setter },
        |ui, height| {
            let settings = distortion_settings;
            displays::distortion(ui, height, settings.drive_db, settings.mix, settings.enabled);
        },
        |ui, cell| {
            Knob::new(&distortion.drive, "Drive", help::DISTORTION_DRIVE, SOUND_COLOR)
                .modulated(ModTarget::DistortionDrive)
                .show(ui, cell, controls);
            Knob::new(&distortion.mix, "Mix", help::DISTORTION_MIX, SOUND_COLOR)
                .modulated(ModTarget::DistortionMix)
                .show(ui, cell, controls);
        },
        |_| {},
    );
    let chorus_settings = settings.chorus;
    effect_card(
        ui,
        card_at(1),
        Effect { name: "Chorus", enabled: &chorus.enabled, help: help::CHORUS, setter },
        |ui, height| displays::chorus(ui, height, chorus_settings.depth, chorus_settings.enabled),
        |ui, cell| {
            Knob::new(&chorus.rate, "Rate", help::CHORUS_RATE, SOUND_COLOR)
                .modulated(ModTarget::ChorusRate)
                .show(ui, cell, controls);
            Knob::new(&chorus.depth, "Depth", help::CHORUS_DEPTH, SOUND_COLOR)
                .modulated(ModTarget::ChorusDepth)
                .show(ui, cell, controls);
            Knob::new(&chorus.mix, "Mix", help::CHORUS_MIX, SOUND_COLOR)
                .modulated(ModTarget::ChorusMix)
                .show(ui, cell, controls);
        },
        |_| {},
    );
    let delay_settings = settings.delay;
    effect_card(
        ui,
        card_at(2),
        Effect { name: "Delay", enabled: &delay.enabled, help: help::DELAY, setter },
        |ui, height| {
            let settings = delay_settings;
            displays::delay(ui, height, settings.time, settings.feedback, settings.mix, settings.ping_pong, settings.enabled);
        },
        |ui, cell| {
            Knob::new(&delay.time, "Time", help::DELAY_TIME, SOUND_COLOR)
                .modulated(ModTarget::DelayTime)
                .show(ui, cell, controls);
            Knob::new(&delay.feedback, "Feedback", help::DELAY_FEEDBACK, SOUND_COLOR)
                .modulated(ModTarget::DelayFeedback)
                .show(ui, cell, controls);
            Knob::new(&delay.mix, "Mix", help::DELAY_MIX, SOUND_COLOR)
                .modulated(ModTarget::DelayMix)
                .show(ui, cell, controls);
        },
        |ui| {
            ui.label(RichText::new("Ping-Pong").color(TEXT_WEAK)).on_hover_text(help::DELAY_PING_PONG);
            switch(ui, &delay.ping_pong, help::DELAY_PING_PONG, setter);
        },
    );
    let reverb_settings = settings.reverb;
    effect_card(
        ui,
        card_at(3),
        Effect { name: "Reverb", enabled: &reverb.enabled, help: help::REVERB, setter },
        |ui, height| displays::reverb(ui, height, reverb_settings.size, reverb_settings.damping, reverb_settings.enabled),
        |ui, cell| {
            Knob::new(&reverb.size, "Size", help::REVERB_SIZE, SOUND_COLOR)
                .modulated(ModTarget::ReverbSize)
                .show(ui, cell, controls);
            Knob::new(&reverb.damping, "Damping", help::REVERB_DAMPING, SOUND_COLOR)
                .modulated(ModTarget::ReverbDamping)
                .show(ui, cell, controls);
            Knob::new(&reverb.mix, "Mix", help::REVERB_MIX, SOUND_COLOR)
                .modulated(ModTarget::ReverbMix)
                .show(ui, cell, controls);
        },
        |_| {},
    );
}

/// What an effect's card needs besides its display and knobs.
struct Effect<'a> {
    name: &'a str,
    enabled: &'a BoolParam,
    help: &'a str,
    setter: &'a ParamSetter<'a>,
}

/// How many knobs wide the effects' rows are.
const EFFECT_CELLS: usize = 4;

/// An effect's card: its name and its switch (and `extra`, at the right of the title), its
/// display, and its knobs, dimmed while it's off.
fn effect_card(
    ui: &mut Ui,
    rect: Rect,
    effect: Effect,
    display: impl FnOnce(&mut Ui, f32),
    knobs: impl FnOnce(&mut Ui, f32),
    extra: impl FnOnce(&mut Ui),
) {
    let Effect { name, enabled, help, setter } = effect;
    let card = card(ui, rect, SOUND_COLOR);
    title_row(
        ui,
        (name, "title"),
        card.title,
        |ui| {
            switch(ui, enabled, help, setter);
            title_text(ui, name).on_hover_text(help);
        },
        extra,
    );
    let body = card.body;
    let cell = cell_width(rect.width(), EFFECT_CELLS);
    let knob_height = knob::height(cell);
    let display_height = (body.height() - knob_height - INNER_GAP).max(DISPLAY_MIN);
    in_rect(ui, (name, "display"), body, Layout::top_down(Align::Min), |ui| display(ui, display_height));
    let row = row_rect(body, body.bottom() - knob_height, knob_height);
    knob_row(ui, (name, "knobs"), row, |ui| {
        if !enabled.value() {
            ui.multiply_opacity(0.55);
        }
        knobs(ui, cell);
    });
}

/// The modulation routes, with how modulation works beside them.
fn matrix_page(ui: &mut Ui, rect: Rect, controls: &Controls, gui: &mut Gui) {
    let about_width = (rect.width() * 0.3).clamp(300.0, 440.0);
    let routes = Rect::from_min_max(rect.min, Pos2::new(rect.right() - about_width - GAP, rect.bottom()));
    let about = Rect::from_min_max(Pos2::new(routes.right() + GAP, rect.top()), rect.max);
    routes_card(ui, routes, controls, gui);

    let card = card(ui, about, MOD_COLOR);
    title_row(
        ui,
        "about title",
        card.title,
        |ui| {
            title_text(ui, "How modulation works");
        },
        |_| {},
    );
    in_rect(ui, "about", card.body, Layout::top_down(Align::Min), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for paragraph in [
                "A route moves its target by its source times its amount. At 100% a source's \
                 full swing moves a knob across its whole range, the pitch targets by an octave, \
                 and the cutoff by 10 octaves. A target's routes add up.",
                "To add a route, drag a source's handle (an orange chip: the oscillators', the \
                 envelopes', the LFOs' and those along the bottom) onto a knob. Its knob then \
                 shows an orange arc for how far its routes can move it, and a light dot for \
                 where the newest note's modulation has it.",
                "Right-click a knob to change its routes' amounts, remove them or add another. \
                 Hovering a source's handle lights up the knobs it modulates.",
                "The LFOs, Key Track, Random and the oscillators swing both ways, from -1 to 1; \
                 the other sources go from 0 to 1.",
            ] {
                ui.label(RichText::new(paragraph).color(TEXT));
                ui.add_space(4.0);
            }
        });
    });
}

fn routes_card(ui: &mut Ui, rect: Rect, controls: &Controls, gui: &mut Gui) {
    let matrix = controls.matrix;
    let setter = controls.setter;
    let added = &mut gui.state.studio.added_routes;
    // A slot added here stops being kept once it's used, so that removing it hides it
    for (index, slot) in matrix.slots.iter().enumerate() {
        if Matrix::is_used(slot) {
            added[index] = false;
        }
    }
    let used = matrix.slots.iter().filter(|slot| Matrix::is_used(slot)).count();
    let card = card(ui, rect, MOD_COLOR);
    title_row(
        ui,
        "routes title",
        card.title,
        |ui| {
            title_text(ui, "Modulation routes");
            ui.label(RichText::new(format!("{used} of {N_SLOTS} in use")).small().color(TEXT_WEAK));
        },
        |ui| {
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
        },
    );
    in_rect(ui, "routes", card.body, Layout::top_down(Align::Min), |ui| {
        let shown: Vec<usize> = (0..N_SLOTS)
            .filter(|index| Matrix::is_used(&matrix.slots[*index]) || added[*index])
            .collect();
        if shown.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new(
                        "Nothing is modulated yet. Drag a source's handle (an orange chip) onto a \
                         knob, or add a route here.",
                    )
                    .color(TEXT_WEAK),
                );
            });
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("routes").num_columns(5).spacing([10.0, 6.0]).show(ui, |ui| {
                for heading in ["Source", "", "Target", "Amount", ""] {
                    ui.label(RichText::new(heading).small().color(TEXT_WEAK));
                }
                ui.end_row();
                for index in shown {
                    let slot = &matrix.slots[index];
                    combo_cell(ui, &slot.source, index, 140.0, slot.source.value().help(), setter);
                    ui.label(RichText::new("›").color(TEXT_WEAK));
                    combo_cell(ui, &slot.target, index, 190.0, slot.target.value().help(), setter);
                    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(180.0))
                        .on_hover_text(help::MOD_AMOUNT);
                    if ui.button("Remove").on_hover_text(help::CLEAR_ROUTE).clicked() {
                        Matrix::clear(setter, slot);
                        added[index] = false;
                    }
                    ui.end_row();
                }
            });
        });
    });
}

/// The settings that trade quality against CPU, the wavetable files, and the theme.
fn advanced_page(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, setter: &ParamSetter, gui: &mut Gui) {
    let widths = [0.38, 0.37, 0.25].map(|share| (rect.width() - 2.0 * GAP) * share);
    let mut left = rect.left();
    let mut next = |width: f32| {
        let card = Rect::from_min_size(Pos2::new(left, rect.top()), egui::vec2(width, rect.height()));
        left += width + GAP;
        card
    };
    let (quality, files, theme) = (next(widths[0]), next(widths[1]), next(widths[2]));

    let card_rect = quality;
    let quality = card(ui, card_rect, GLOBAL_COLOR);
    title_row(
        ui,
        "quality title",
        quality.title,
        |ui| {
            title_text(ui, "Quality");
        },
        |_| {},
    );
    let global = &params.global;
    in_rect(ui, "quality", quality.body, Layout::top_down(Align::Min), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            setting(ui, "Oversampling", help::OVERSAMPLING, |ui| {
                combo_cell(ui, &global.oversampling, 0, COMBO_WIDTH, help::OVERSAMPLING, setter)
            });
            setting(ui, "Interpolation", help::INTERPOLATION, |ui| {
                combo_cell(ui, &global.interpolation, 0, COMBO_WIDTH, help::INTERPOLATION, setter)
            });
            setting(ui, "Modulation Rate", help::MODULATION_RATE, |ui| {
                combo_cell(ui, &global.modulation_rate, 0, COMBO_WIDTH, help::MODULATION_RATE, setter)
            });
        });
    });

    let card_rect = files;
    let files = card(ui, card_rect, SOUND_COLOR);
    title_row(
        ui,
        "files title",
        files.title,
        |ui| {
            title_text(ui, "Wavetable files").on_hover_text(help::WAVETABLE_FOLDER);
        },
        |ui| {
            if ui.button("Rescan").on_hover_text("Lists the folder's files again.").clicked() {
                gui.state.rescan();
            }
        },
    );
    in_rect(ui, "files", files.body, Layout::top_down(Align::Min), |ui| {
        wavetable_folder_rows(ui, params, setter, gui, (files.body.width() - 200.0).max(120.0));
        ui.add_space(4.0);
        ui.label(RichText::new("In the folder:").small().color(TEXT_WEAK));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if gui.state.files.is_empty() {
                ui.label(RichText::new("No .wav files yet.").color(TEXT_WEAK));
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
                    ui.add(egui::Label::new(RichText::new(&name).color(TEXT)).truncate());
                });
            }
        });
    });

    let card_rect = theme;
    let theme_card = card(ui, card_rect, GLOBAL_COLOR);
    title_row(
        ui,
        "theme title",
        theme_card.title,
        |ui| {
            title_text(ui, "Theme");
        },
        |_| {},
    );
    in_rect(ui, "theme", theme_card.body, Layout::top_down(Align::Min), |ui| {
        let current = theme::current();
        for option in Theme::ALL {
            if ui.radio(current == option, RichText::new(option.name()).strong()).clicked() && current != option {
                switch_theme(ui, gui, option);
            }
            ui.label(RichText::new(option.description()).small().color(TEXT_WEAK));
            ui.add_space(6.0);
        }
        ui.label(
            RichText::new("The theme is kept for every project, in Settings.json in Documents/Three Osc.")
                .small()
                .color(TEXT_WEAK),
        );
        if let Some(problem) = &gui.state.theme_problem {
            ui.label(RichText::new(problem).small().color(title_bar::ERROR_COLOR));
        }
    });
}

/// A setting on the advanced page: its name and control, with what it does written out under
/// them.
fn setting(ui: &mut Ui, name: &str, help: &str, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).strong().color(TEXT_STRONG));
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
    ui.label(RichText::new(help).small().color(TEXT_WEAK));
    ui.add_space(8.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rows_fill_the_window_without_overlapping() {
        let default = egui::vec2(crate::editor::DEFAULT_WIDTH as f32, crate::editor::DEFAULT_HEIGHT as f32);
        for size in [default, egui::vec2(MIN_WIDTH, 700.0), egui::vec2(1920.0, 1200.0), egui::vec2(2560.0, 1440.0)] {
            // As in the window, at the size the layout needs at least
            let width = size.x.max(MIN_WIDTH);
            let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, size.y.max(Areas::needed_height(width))));
            let areas = Areas::new(area);
            let rows = [areas.header, areas.page, areas.envelopes, areas.lfos, areas.footer];
            for pair in rows.windows(2) {
                assert!((pair[1].top() - pair[0].bottom() - GAP).abs() < 1e-3, "{size:?}: {pair:?}");
            }
            assert!((areas.footer.bottom() - area.bottom()).abs() < 1e-3, "{size:?}");
            for (row, need) in [areas.page, areas.envelopes, areas.lfos].iter().zip(Areas::needs(width)) {
                assert!(row.height() >= need - 1e-3, "{size:?}");
            }
            assert!(areas.filter_column < width / 2.0);
        }
        // The default window holds the whole layout without scrolling
        let inside = default - egui::vec2(16.0, 16.0) - egui::vec2(2.0 * MARGIN, 2.0 * MARGIN);
        assert!(inside.x >= MIN_WIDTH && inside.y >= Areas::needed_height(inside.x));
    }
}
