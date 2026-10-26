//! The Flow theme: the controls in the order the sound flows through them, from left to right,
//! as a block diagram drawn like a blueprint.
//!
//! The oscillators are stacked, each one's modulation of the next on the arrow between them
//! (Osc 3 into Osc 2, Osc 2 into Osc 1), and go into the mixer with their levels and pans, then
//! the filter (with its envelope's amount on the arrow up into it), the amp (the volume
//! envelope, with the voice settings under it) and the effects in the order they come, down
//! into the output. The other modulation sources are in a lane along the bottom, and the routes
//! and the settings are in windows of their own.

use std::hash::Hash;

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    self, Align, Color32, CornerRadius, FontId, Layout, Painter, Pos2, Rect, Response, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2,
};

use super::catalog::{self, Colors, Control};
use super::knob::{self, Controls};
use super::widgets::{self, in_rect, ChipStyle};
use super::{
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, lfo_display, set_param, theme_picker,
    title_bar, tuning_window, wavetable_file_combo, wavetable_file_status, Gui, Live, Matrix, WaveDisplay, MOD_COLOR,
};
use crate::params::{EnvelopeParams, OscillatorWave, ThreeOscParams};
use crate::synth::modulation::ModSource;
use crate::ThreeOscPlugin;

const BACKGROUND: Color32 = Color32::from_rgb(10, 23, 39);
const GRID: Color32 = Color32::from_rgb(15, 31, 51);
const GRID_MAJOR: Color32 = Color32::from_rgb(20, 41, 66);
const NODE: Color32 = Color32::from_rgb(13, 30, 50);
/// The nodes' outlines, and the wires between them.
const LINE: Color32 = Color32::from_rgb(50, 110, 160);
const WIRE: Color32 = Color32::from_rgb(90, 195, 255);
const SCREEN: Color32 = Color32::from_rgb(6, 15, 27);
const WIDGET: Color32 = Color32::from_rgb(20, 43, 68);
const WIDGET_HOVERED: Color32 = Color32::from_rgb(28, 58, 90);
const WIDGET_ACTIVE: Color32 = Color32::from_rgb(37, 74, 112);
const TEXT: Color32 = Color32::from_rgb(196, 223, 248);
const TEXT_STRONG: Color32 = Color32::from_rgb(236, 247, 255);
const TEXT_WEAK: Color32 = Color32::from_rgb(103, 139, 174);
const SOUND_COLOR: Color32 = Color32::from_rgb(85, 214, 255);
const GLOBAL_COLOR: Color32 = Color32::from_rgb(186, 170, 255);

const MARGIN: f32 = 8.0;
/// Space between nodes, between the columns (for the wires), and inside the nodes.
const GAP: f32 = 8.0;
const WIRE_GAP: f32 = 28.0;
const PADDING: f32 = 8.0;
const HEADER_HEIGHT: f32 = 34.0;
/// The captions over the columns.
const CAPTION_HEIGHT: f32 = 18.0;
const TITLE_HEIGHT: f32 = 22.0;
const INNER_GAP: f32 = 6.0;
const ROW_GAP: f32 = 2.0;
const DISPLAY_MIN: f32 = 40.0;
/// The room an oscillator's wavetable file takes under its display, when it plays one.
const FILE_ROWS: f32 = 40.0;
/// The width of the knobs on the arrows between nodes, and in the mixer.
const LINK_CELL: f32 = 50.0;
/// The lane of modulation sources gets at least this much height, and this much of what's
/// left over once the diagram has what it needs.
const LANE_MIN: f32 = 200.0;
const LANE_SHARE: f32 = 0.3;
/// The space between the effects, for the arrows down from one to the next.
const CHAIN_GAP: f32 = 18.0;
const MIN_WIDTH: f32 = 1180.0;
const COMBO_WIDTH: f32 = 104.0;

const KNOBS: knob::Style = knob::Style {
    font_size: 10.5,
    max_diameter: 38.0,
    ..knob::Style::STUDIO
};
const COLORS: Colors = Colors {
    sound: SOUND_COLOR,
    modulation: MOD_COLOR,
    global: GLOBAL_COLOR,
};

/// The displays' colours.
pub const PALETTE: displays::Palette = displays::Palette {
    screen: SCREEN,
    edge: LINE,
    signal: SOUND_COLOR,
    live: Color32::from_rgb(255, 238, 205),
    guide: Color32::from_rgb(22, 48, 78),
    label: Color32::from_rgb(80, 120, 160),
};

/// The look of the Flow theme, for egui.
pub fn style() -> egui::Style {
    let mut style = egui::Style::default();
    let visuals = &mut style.visuals;
    visuals.panel_fill = BACKGROUND;
    visuals.window_fill = NODE;
    visuals.window_stroke = Stroke::new(1.0_f32, LINE);
    visuals.extreme_bg_color = SCREEN;
    visuals.faint_bg_color = WIDGET;
    visuals.window_corner_radius = CornerRadius::same(4);
    visuals.menu_corner_radius = CornerRadius::same(4);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = NODE;
    widgets.noninteractive.weak_bg_fill = NODE;
    widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    for (widget, fill) in [
        (&mut widgets.inactive, WIDGET),
        (&mut widgets.hovered, WIDGET_HOVERED),
        (&mut widgets.active, WIDGET_ACTIVE),
        (&mut widgets.open, WIDGET_HOVERED),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.corner_radius = CornerRadius::same(3);
    }
    widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    widgets.hovered.fg_stroke = Stroke::new(1.5_f32, TEXT_STRONG);
    widgets.active.fg_stroke = Stroke::new(2.0_f32, TEXT_STRONG);
    widgets.open.fg_stroke = Stroke::new(1.0_f32, TEXT_STRONG);
    widgets.hovered.bg_stroke = Stroke::new(1.0_f32, WIRE);
    widgets.active.bg_stroke = Stroke::new(1.0_f32, WIRE);
    visuals.selection.bg_fill = Color32::from_rgb(30, 92, 140);
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
    style
}

fn chips() -> ChipStyle {
    ChipStyle::new(TEXT_STRONG, BACKGROUND)
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
    let width = ui.available_width().max(MIN_WIDTH + 2.0 * MARGIN);
    let height = ui.available_height().max(Plan::needed_height(width - 2.0 * MARGIN) + 2.0 * MARGIN);
    let (area, _) = ui.allocate_exact_size(egui::vec2(width, height), Sense::hover());
    paint_grid(ui.painter(), area);
    let plan = Plan::new(area.shrink(MARGIN));

    header(ui, plan.header, params, &controls, gui);
    wires(ui.painter(), &plan);
    for (rect, caption) in plan.captions() {
        ui.painter().text(rect.left_center(), egui::Align2::LEFT_CENTER, caption, caption_font(), TEXT_WEAK);
    }
    for row in 0..3 {
        oscillator(ui, plan.oscillators[row], Plan::oscillator_index(row), params, &controls, gui);
        mixer(ui, plan.mixers[row], Plan::oscillator_index(row), params, &controls);
    }
    for (link, index) in plan.links.into_iter().zip([1, 0]) {
        let knobs = catalog::oscillator_modulation(params, index).expect("Osc 1 and 2 are modulated");
        let caption = format!("Osc {} into Osc {}", index + 2, index + 1);
        modulation_link(ui, link, &caption, &knobs, &controls);
    }
    filter(ui, plan.filter, params, &controls, gui);
    modulation_link(
        ui,
        plan.envelope_link,
        "Filter Env. into the cutoff",
        &[catalog::filter_envelope_amount(&params.filter_env)],
        &controls,
    );
    envelope_node(
        ui,
        plan.filter_env,
        None,
        (ModSource::FilterEnv, "Filter Env."),
        &params.filter_env.envelope,
        &controls,
        gui.live,
    );
    envelope_node(ui, plan.amp, Some("Amp"), (ModSource::VolumeEnv, "Volume Env."), &params.vol_env, &controls, gui.live);
    voice(ui, plan.voice, params, &controls);
    effects(ui, &plan, params, &controls, gui.live);
    output(ui, plan.output, params, &controls, gui);
    lane(ui, &plan, params, &controls, gui.live);
    widgets::routes_window(ui.ctx(), &controls, gui);
}

/// Where everything goes in a window of a given size.
struct Plan {
    header: Rect,
    /// The columns of the diagram: the oscillators, the mixer, the filter, the amp and the
    /// effects.
    columns: [Rect; 5],
    /// The oscillators from the top: Osc 3, Osc 2, Osc 1.
    oscillators: [Rect; 3],
    /// The arrows between them, with their modulation's knobs: into Osc 2, and into Osc 1.
    links: [Rect; 2],
    mixers: [Rect; 3],
    filter: Rect,
    envelope_link: Rect,
    filter_env: Rect,
    amp: Rect,
    voice: Rect,
    effects: [Rect; 4],
    output: Rect,
    /// The modulation sources along the bottom: Env 1, Env 2, LFO 1 to 3, and the MIDI sources.
    lane: [Rect; 6],
}

impl Plan {
    /// The oscillator in a row of the diagram, from the top.
    fn oscillator_index(row: usize) -> usize {
        2 - row
    }

    fn column_widths(width: f32) -> [f32; 5] {
        let oscillators = (width * 0.28).max(330.0);
        let mixer = 124.0;
        let filter = (width * 0.21).max(248.0);
        let amp = (width * 0.18).max(212.0);
        let effects = width - oscillators - mixer - filter - amp - 4.0 * WIRE_GAP;
        [oscillators, mixer, filter, amp, effects]
    }

    /// The width of an oscillator's display, and of its knobs, in a column `width` wide.
    fn oscillator_cells(width: f32) -> (f32, f32) {
        let inner = width - 2.0 * PADDING;
        let display = (inner * 0.34).round();
        (display, (inner - display - INNER_GAP) / 4.0)
    }

    fn link_height() -> f32 {
        KNOBS.height(LINK_CELL)
    }

    /// The height an oscillator's node needs.
    fn oscillator_height(width: f32) -> f32 {
        let (_, cell) = Self::oscillator_cells(width);
        node_height(2.0 * KNOBS.height(cell) + ROW_GAP)
    }

    /// The height the diagram needs, in a window `width` wide: the oscillators' column, which
    /// the others fit beside.
    fn diagram_height(width: f32) -> f32 {
        3.0 * Self::oscillator_height(Self::column_widths(width)[0]) + 2.0 * Self::link_height()
    }

    fn needed_height(width: f32) -> f32 {
        HEADER_HEIGHT + GAP + CAPTION_HEIGHT + Self::diagram_height(width) + 2.0 * GAP + LANE_MIN
    }

    fn new(area: Rect) -> Self {
        let width = area.width();
        let spare = (area.height() - Self::needed_height(width)).max(0.0);
        let header = Rect::from_min_size(area.min, egui::vec2(width, HEADER_HEIGHT));
        let lane_height = LANE_MIN + spare * LANE_SHARE;
        let lane_top = area.bottom() - lane_height;
        let diagram = Rect::from_min_max(
            Pos2::new(area.left(), header.bottom() + GAP + CAPTION_HEIGHT),
            Pos2::new(area.right(), lane_top - 2.0 * GAP),
        );
        let widths = Self::column_widths(width);
        let mut left = area.left();
        let columns = widths.map(|width| {
            let column = Rect::from_x_y_ranges(left..=left + width, diagram.y_range());
            left += width + WIRE_GAP;
            column
        });
        let [oscillator_column, mixer_column, filter_column, amp_column, effects_column] = columns;

        // The oscillators, with the links between them
        let link = Self::link_height();
        let row_height = (diagram.height() - 2.0 * link) / 3.0;
        let row = |row: usize| {
            let top = diagram.top() + row as f32 * (row_height + link);
            Rect::from_x_y_ranges(oscillator_column.x_range(), top..=top + row_height)
        };
        let oscillators = [row(0), row(1), row(2)];
        let links = [0, 1].map(|row| {
            Rect::from_x_y_ranges(oscillator_column.x_range(), oscillators[row].bottom()..=oscillators[row + 1].top())
        });
        let mixer_height = node_height(KNOBS.height(LINK_CELL)) - TITLE_HEIGHT - INNER_GAP;
        let mixers = oscillators.map(|oscillator| {
            Rect::from_center_size(
                Pos2::new(mixer_column.center().x, oscillator.center().y),
                egui::vec2(mixer_column.width(), mixer_height),
            )
        });

        // The filter over its envelope, and the amp, the same height as the filter so that the
        // wire between them is straight, over the voice settings
        let filter_height = ((diagram.height() - link - 2.0 * GAP) * 0.55).round();
        let filter = Rect::from_min_size(filter_column.min, egui::vec2(filter_column.width(), filter_height));
        let envelope_link = Rect::from_x_y_ranges(filter_column.x_range(), filter.bottom() + GAP..=filter.bottom() + GAP + link);
        let filter_env = Rect::from_x_y_ranges(filter_column.x_range(), envelope_link.bottom() + GAP..=diagram.bottom());
        let amp = Rect::from_min_size(amp_column.min, egui::vec2(amp_column.width(), filter_height));
        let voice_height = node_height(KNOBS.height(cell_width(amp_column.width(), 5)));
        let voice = Rect::from_min_size(Pos2::new(amp_column.left(), amp.bottom() + 2.0 * GAP), egui::vec2(amp_column.width(), voice_height));

        // The effects down their column, and the output under them in the lane, from the amp's
        // column to the right
        let effect_height = (diagram.height() - 3.0 * CHAIN_GAP) / 4.0;
        let effects = [0, 1, 2, 3].map(|index| {
            let top = diagram.top() + index as f32 * (effect_height + CHAIN_GAP);
            Rect::from_x_y_ranges(effects_column.x_range(), top..=top + effect_height)
        });
        let output = Rect::from_x_y_ranges(amp_column.left()..=area.right(), lane_top..=area.bottom());

        let lane_width = amp_column.left() - WIRE_GAP - area.left() - 5.0 * GAP;
        let shares = [0.2, 0.2, 0.165, 0.165, 0.165, 0.105];
        let mut left = area.left();
        let lane = shares.map(|share| {
            let rect = Rect::from_x_y_ranges(left..=left + lane_width * share, lane_top..=area.bottom());
            left = rect.right() + GAP;
            rect
        });
        Self {
            header,
            columns,
            oscillators,
            links,
            mixers,
            filter,
            envelope_link,
            filter_env,
            amp,
            voice,
            effects,
            output,
            lane,
        }
    }

    /// The captions over the columns, and over the lane.
    fn captions(&self) -> Vec<(Rect, &'static str)> {
        let over = |rect: Rect| Rect::from_min_size(rect.min - egui::vec2(0.0, CAPTION_HEIGHT), egui::vec2(rect.width(), CAPTION_HEIGHT));
        let mut captions: Vec<(Rect, &'static str)> = self
            .columns
            .iter()
            .zip(["OSCILLATORS", "MIX", "FILTER", "AMP", "EFFECTS"])
            .map(|(column, caption)| (over(*column), caption))
            .collect();
        let lane = Rect::from_min_size(self.lane[0].min - egui::vec2(0.0, 2.0 * GAP), egui::vec2(self.lane[0].width() * 4.0, 2.0 * GAP));
        captions.push((lane, "MODULATION SOURCES  ·  drag a chip onto a knob"));
        captions
    }
}

/// The width of each of `cells` knobs across a node `width` wide.
fn cell_width(width: f32, cells: usize) -> f32 {
    (width - 2.0 * PADDING) / cells as f32
}

/// The height of a node whose contents are `body` tall.
fn node_height(body: f32) -> f32 {
    2.0 * PADDING + TITLE_HEIGHT + INNER_GAP + body
}

fn caption_font() -> FontId {
    FontId::monospace(11.0)
}

/// The blueprint's grid, under everything.
fn paint_grid(painter: &Painter, area: Rect) {
    let step = 24.0;
    let mut x = area.left();
    let mut column = 0;
    while x <= area.right() {
        let color = if column % 5 == 0 { GRID_MAJOR } else { GRID };
        painter.line_segment([Pos2::new(x, area.top()), Pos2::new(x, area.bottom())], Stroke::new(1.0_f32, color));
        x += step;
        column += 1;
    }
    let mut y = area.top();
    let mut row = 0;
    while y <= area.bottom() {
        let color = if row % 5 == 0 { GRID_MAJOR } else { GRID };
        painter.line_segment([Pos2::new(area.left(), y), Pos2::new(area.right(), y)], Stroke::new(1.0_f32, color));
        y += step;
        row += 1;
    }
}

/// A box of the diagram, with ticks at its corners like a drawing's: its title row and the area
/// under it.
struct Node {
    title: Rect,
    body: Rect,
}

fn node(ui: &Ui, rect: Rect) -> Node {
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(3), NODE, Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
    for (corner, direction) in [
        (rect.left_top(), egui::vec2(1.0, 1.0)),
        (rect.right_top(), egui::vec2(-1.0, 1.0)),
        (rect.left_bottom(), egui::vec2(1.0, -1.0)),
        (rect.right_bottom(), egui::vec2(-1.0, -1.0)),
    ] {
        let stroke = Stroke::new(1.5_f32, WIRE);
        painter.line_segment([corner, corner + egui::vec2(7.0 * direction.x, 0.0)], stroke);
        painter.line_segment([corner, corner + egui::vec2(0.0, 7.0 * direction.y)], stroke);
    }
    let inner = rect.shrink(PADDING);
    let title = Rect::from_min_size(inner.min, egui::vec2(inner.width(), TITLE_HEIGHT));
    let body = Rect::from_min_max(Pos2::new(inner.left(), title.bottom() + INNER_GAP), inner.max);
    Node { title, body }
}

/// A node's title, in the wires' colour.
fn title_text(ui: &mut Ui, title: &str) -> Response {
    ui.label(RichText::new(title.to_uppercase()).font(FontId::monospace(12.0)).color(WIRE).extra_letter_spacing(0.6))
}

/// Fills a node's title row: `left` from the left, then `right` from the right.
fn title_row(ui: &mut Ui, id_salt: impl Hash, rect: Rect, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    in_rect(ui, id_salt, rect, Layout::left_to_right(Align::Center), |ui| {
        left(ui);
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
}

/// Lays `knobs` out from the left of `rect`, each `cell` wide.
fn knob_row(ui: &mut Ui, id_salt: impl Hash, rect: Rect, knobs: &[Control], cell: f32, controls: &Controls) {
    in_rect(ui, id_salt, rect, Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        catalog::show_all(ui, knobs, cell, controls);
    });
}

/// An arrowhead pointing along `direction` with its tip at `tip`.
fn arrowhead(painter: &Painter, tip: Pos2, direction: Vec2, color: Color32) {
    let direction = direction.normalized();
    let side = direction.rot90() * 4.0;
    let back = tip - direction * 8.0;
    painter.add(Shape::convex_polygon(vec![tip, back + side, back - side], color, Stroke::NONE));
}

/// A wire of the sound's path through `points`, with an arrowhead at the end.
fn wire(painter: &Painter, points: &[Pos2]) {
    painter.add(Shape::line(points.to_vec(), Stroke::new(1.5_f32, WIRE)));
    if let [.., from, to] = points {
        arrowhead(painter, *to, *to - *from, WIRE);
    }
}

/// A dashed wire of modulation from `from` to `to`, with an arrowhead at the end.
fn modulation_wire(painter: &Painter, from: Pos2, to: Pos2) {
    painter.extend(Shape::dashed_line(&[from, to], Stroke::new(1.5_f32, MOD_COLOR), 5.0, 4.0));
    arrowhead(painter, to, to - from, MOD_COLOR);
}

/// The wires of the sound's path, from node to node, under the nodes.
fn wires(painter: &Painter, plan: &Plan) {
    // Each oscillator into its mixer channel, and the channels together into the filter
    let bus = plan.mixers[0].right() + WIRE_GAP / 2.0;
    for (oscillator, mixer) in plan.oscillators.iter().zip(&plan.mixers) {
        wire(painter, &[oscillator.right_center(), mixer.left_center()]);
        painter.line_segment([mixer.right_center(), Pos2::new(bus, mixer.center().y)], Stroke::new(1.5_f32, WIRE));
    }
    let into_filter = plan.filter.center().y;
    let top = plan.mixers[0].center().y.min(into_filter);
    let bottom = plan.mixers[2].center().y.max(into_filter);
    painter.line_segment([Pos2::new(bus, top), Pos2::new(bus, bottom)], Stroke::new(1.5_f32, WIRE));
    painter.circle_filled(Pos2::new(bus, into_filter), 3.0, WIRE);
    wire(painter, &[Pos2::new(bus, into_filter), plan.filter.left_center()]);
    // The filter into the amp, the amp into the first effect, the effects down into each other
    // and the last down into the output
    wire(painter, &[plan.filter.right_center(), plan.amp.left_center()]);
    let first = plan.effects[0].left_center();
    let turn = plan.amp.right() + WIRE_GAP / 2.0;
    wire(painter, &[plan.amp.right_center(), Pos2::new(turn, plan.amp.center().y), Pos2::new(turn, first.y), first]);
    for pair in plan.effects.windows(2) {
        wire(painter, &[pair[0].center_bottom(), pair[1].center_top()]);
    }
    let last = plan.effects[3].center_bottom();
    wire(painter, &[last, Pos2::new(last.x, plan.output.top())]);
    // The modulation between the oscillators, and the filter envelope up into the filter
    for (index, link) in plan.links.iter().enumerate() {
        let x = link.left() + 22.0;
        modulation_wire(painter, Pos2::new(x, plan.oscillators[index].bottom()), Pos2::new(x, plan.oscillators[index + 1].top()));
    }
    let x = plan.envelope_link.left() + 22.0;
    modulation_wire(painter, Pos2::new(x, plan.filter_env.top()), Pos2::new(x, plan.filter.bottom()));
}

/// The header: the plugin's name, the presets, the tuning, the routes and settings windows, and
/// the theme.
fn header(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    in_rect(ui, "header", rect, Layout::left_to_right(Align::Center), |ui| {
        ui.label(RichText::new("THREE OSC").font(FontId::monospace(17.0)).color(WIRE).extra_letter_spacing(1.5));
        ui.label(RichText::new(format!("v{}", ThreeOscPlugin::VERSION)).small().color(TEXT_WEAK));
        ui.add_space(16.0);
        title_bar::presets(ui, params, controls.setter, gui);
        ui.add_space(8.0);
        tuning_window::button(ui, params, &mut gui.state.tuning);
        ui.add_space(8.0);
        widgets::routes_button(ui, controls.matrix, gui);
        widgets::settings_button(ui, gui);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| theme_picker(ui, gui));
    });
}

fn oscillator(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let osc = params.oscillator(index);
    let file = params.wavetable_file(index);
    let node = node(ui, rect);
    title_row(
        ui,
        ("osc title", index),
        node.title,
        |ui| {
            widgets::chip(ui, ModSource::osc(index), &format!("Osc {}", index + 1), controls, &chips());
        },
        |ui| combo_cell(ui, &osc.wave, index, COMBO_WIDTH, help::OSC_WAVE, controls.setter),
    );
    let body = node.body;
    let (display_width, cell) = Plan::oscillator_cells(rect.width());
    let display = Rect::from_min_size(body.min, egui::vec2(display_width, body.height()));
    let playing_file = osc.wave.value() == OscillatorWave::File;
    let wave = WaveDisplay::new(index, osc, file, gui);
    in_rect(ui, ("osc display", index), display, Layout::top_down(Align::Min), |ui| {
        let files = if playing_file { FILE_ROWS } else { 0.0 };
        wave.show(ui, (display.height() - files).max(DISPLAY_MIN));
        if playing_file {
            wavetable_file_combo(ui, index, file, gui, display.width() - 8.0);
            if let Some(status) = wavetable_file_status(file) {
                ui.add(egui::Label::new(RichText::new(status).small().color(TEXT_WEAK)).truncate());
            }
        }
    });
    // Its pitch and its shape; its level and pan are in the mixer
    let knobs = catalog::oscillator(osc, index);
    let height = KNOBS.height(cell);
    for (row, knobs) in knobs[2..].chunks(4).enumerate() {
        let top = body.top() + row as f32 * (height + ROW_GAP);
        let rect = Rect::from_min_size(Pos2::new(display.right() + INNER_GAP, top), egui::vec2(4.0 * cell, height));
        knob_row(ui, ("osc knobs", row, index), rect, knobs, cell, controls);
    }
}

/// An oscillator's channel of the mixer: its level and its pan.
fn mixer(ui: &mut Ui, rect: Rect, index: usize, params: &ThreeOscParams, controls: &Controls) {
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(3), NODE, Stroke::new(1.0_f32, LINE), StrokeKind::Inside);
    let knobs = catalog::oscillator(params.oscillator(index), index);
    let cell = cell_width(rect.width(), 2);
    let row = Rect::from_center_size(rect.center(), egui::vec2(2.0 * cell, KNOBS.height(cell)));
    knob_row(ui, ("mixer", index), row, &knobs[..2], cell, controls);
}

/// Knobs of modulation on an arrow between two nodes, with a caption saying what goes where.
fn modulation_link(ui: &mut Ui, rect: Rect, caption: &str, knobs: &[Control], controls: &Controls) {
    let row = Rect::from_min_size(
        Pos2::new(rect.left() + 40.0, rect.center().y - KNOBS.height(LINK_CELL) / 2.0),
        egui::vec2(knobs.len() as f32 * LINK_CELL, KNOBS.height(LINK_CELL)),
    );
    knob_row(ui, ("link", caption), row, knobs, LINK_CELL, controls);
    ui.painter().text(
        Pos2::new(row.right() + GAP, rect.center().y),
        egui::Align2::LEFT_CENTER,
        caption,
        FontId::proportional(11.0),
        MOD_COLOR.gamma_multiply(0.85),
    );
}

fn filter(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let (filter, filter_env) = (&params.filter, &params.filter_env);
    let setter = controls.setter;
    let node = node(ui, rect);
    title_row(
        ui,
        "filter title",
        node.title,
        |ui| {
            title_text(ui, "Filter");
        },
        |ui| {
            combo_cell(ui, &filter.filter_type, 0, COMBO_WIDTH - 16.0, help::FILTER_TYPE, setter);
            combo_cell(ui, &filter.model, 0, COMBO_WIDTH - 16.0, help::FILTER_MODEL, setter);
        },
    );
    let body = node.body;
    let knobs = catalog::filter(filter);
    let cell = cell_width(rect.width(), knobs.len());
    let height = KNOBS.height(cell);
    let display_height = (body.height() - height - INNER_GAP).max(DISPLAY_MIN);
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
    let row = Rect::from_min_size(Pos2::new(body.left(), body.bottom() - height), egui::vec2(body.width(), height));
    knob_row(ui, "filter knobs", row, &knobs, cell, controls);
}

/// An envelope's node: its display over its knobs, titled by its chip (or by `title`, with the
/// chip at the right).
fn envelope_node(
    ui: &mut Ui,
    rect: Rect,
    title: Option<&str>,
    (source, name): (ModSource, &str),
    envelope: &EnvelopeParams,
    controls: &Controls,
    live: Live,
) {
    let node = node(ui, rect);
    title_row(
        ui,
        (name, "title"),
        node.title,
        |ui| match title {
            Some(title) => {
                title_text(ui, title);
            }
            None => {
                widgets::chip(ui, source, name, controls, &chips());
            }
        },
        |ui| {
            if title.is_some() {
                widgets::chip(ui, source, name, controls, &chips());
            }
        },
    );
    let body = node.body;
    let knobs = catalog::envelope(envelope);
    let cell = cell_width(rect.width(), knobs.len());
    let height = KNOBS.height(cell);
    let display_height = (body.height() - height - INNER_GAP).max(DISPLAY_MIN);
    in_rect(ui, (name, "display"), body, Layout::top_down(Align::Min), |ui| {
        envelope_display(ui, envelope, controls.setter, live, display_height);
    });
    let row = Rect::from_min_size(Pos2::new(body.left(), body.bottom() - height), egui::vec2(body.width(), height));
    knob_row(ui, (name, "knobs"), row, &knobs, cell, controls);
}

/// How notes are played.
fn voice(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls) {
    let global = &params.global;
    let node = node(ui, rect);
    title_row(
        ui,
        "voice title",
        node.title,
        |ui| {
            title_text(ui, "Voice");
        },
        |ui| combo_cell(ui, &global.polyphony, 0, COMBO_WIDTH, help::POLYPHONY, controls.setter),
    );
    let knobs = catalog::voice(global);
    let cell = cell_width(rect.width(), knobs.len());
    knob_row(ui, "voice knobs", node.body, &knobs, cell, controls);
}

/// What draws an effect's display, given its height.
type EffectDisplay = Box<dyn FnOnce(&mut Ui, f32)>;

/// The effects, down their column in the order they process the sound.
fn effects(ui: &mut Ui, plan: &Plan, params: &ThreeOscParams, controls: &Controls, live: Live) {
    let fx = &params.fx;
    // The displays show the settings as the newest note's modulation moves them
    let settings = match live.modulation() {
        Some(modulation) => fx.settings().modulated(&modulation),
        None => fx.settings(),
    };
    let setter = controls.setter;
    let (distortion, chorus, delay, reverb) = (settings.distortion, settings.chorus, settings.delay, settings.reverb);
    let effects: [(&str, &BoolParam, &str, Vec<Control>, EffectDisplay); 4] = [
        (
            "Distortion",
            &fx.distortion.enabled,
            help::DISTORTION,
            catalog::distortion(&fx.distortion).to_vec(),
            Box::new(move |ui, height| displays::distortion(ui, height, distortion.drive_db, distortion.mix, distortion.enabled)),
        ),
        (
            "Chorus",
            &fx.chorus.enabled,
            help::CHORUS,
            catalog::chorus(&fx.chorus).to_vec(),
            Box::new(move |ui, height| displays::chorus(ui, height, chorus.depth, chorus.enabled)),
        ),
        (
            "Delay",
            &fx.delay.enabled,
            help::DELAY,
            catalog::delay(&fx.delay).to_vec(),
            Box::new(move |ui, height| {
                displays::delay(ui, height, delay.time, delay.feedback, delay.mix, delay.ping_pong, delay.enabled)
            }),
        ),
        (
            "Reverb",
            &fx.reverb.enabled,
            help::REVERB,
            catalog::reverb(&fx.reverb).to_vec(),
            Box::new(move |ui, height| displays::reverb(ui, height, reverb.size, reverb.damping, reverb.enabled)),
        ),
    ];
    for ((name, enabled, about, knobs, display), rect) in effects.into_iter().zip(plan.effects) {
        let node = node(ui, rect);
        title_row(
            ui,
            (name, "title"),
            node.title,
            |ui| {
                widgets::switch(ui, enabled, about, setter, SOUND_COLOR);
                title_text(ui, name).on_hover_text(about);
            },
            |ui| {
                if name == "Delay" {
                    widgets::switch(ui, &fx.delay.ping_pong, help::DELAY_PING_PONG, setter, SOUND_COLOR);
                    ui.label(RichText::new("Ping-Pong").small().color(TEXT_WEAK)).on_hover_text(help::DELAY_PING_PONG);
                }
            },
        );
        let body = node.body;
        let cell = cell_width(rect.width(), 3);
        let height = KNOBS.height(cell);
        // A display above the knobs, when there's room for one
        let display_height = body.height() - height - INNER_GAP;
        if display_height >= 28.0 {
            let area = Rect::from_min_size(body.min, egui::vec2(body.width(), display_height));
            in_rect(ui, (name, "display"), area, Layout::top_down(Align::Min), |ui| display(ui, display_height));
        }
        let row = Rect::from_min_size(Pos2::new(body.left(), body.bottom() - height), egui::vec2(body.width(), height));
        in_rect(ui, (name, "knobs"), row, Layout::left_to_right(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if !enabled.value() {
                ui.multiply_opacity(0.55);
            }
            catalog::show_all(ui, &knobs, cell, controls);
        });
    }
}

/// What the synth plays, as a spectrogram and meters, with the output's level and width.
fn output(ui: &mut Ui, rect: Rect, params: &ThreeOscParams, controls: &Controls, gui: &mut Gui) {
    let node = node(ui, rect);
    title_row(
        ui,
        "output title",
        node.title,
        |ui| {
            title_text(ui, "Output").on_hover_text(analyser::HELP);
        },
        |_| {},
    );
    let body = node.body;
    let knobs = catalog::output(&params.global);
    let cell = 64.0;
    let height = KNOBS.height(cell);
    let row = Rect::from_min_size(
        Pos2::new(body.right() - 2.0 * cell, body.center().y - height / 2.0),
        egui::vec2(2.0 * cell, height),
    );
    knob_row(ui, "output knobs", row, &knobs, cell, controls);
    let analyser_rect = Rect::from_min_max(body.min, Pos2::new(row.left() - INNER_GAP, body.bottom()));
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

/// The lane of modulation sources: Env 1 and 2, the LFOs and the MIDI sources.
fn lane(ui: &mut Ui, plan: &Plan, params: &ThreeOscParams, controls: &Controls, live: Live) {
    for (index, rect) in plan.lane[..2].iter().enumerate() {
        let source = (ModSource::envelope(index), ["Env 1", "Env 2"][index]);
        envelope_node(ui, *rect, None, source, &params.mod_envs[index].envelope, controls, live);
    }
    for (index, (lfo, rect)) in params.lfos.iter().zip(&plan.lane[2..5]).enumerate() {
        let node = node(ui, *rect);
        let setter = controls.setter;
        title_row(
            ui,
            ("lfo title", index),
            node.title,
            |ui| {
                widgets::chip(ui, ModSource::lfo(index), &format!("LFO {}", index + 1), controls, &chips());
            },
            |ui| {
                // (A lit button rather than a labelled switch, which wouldn't fit beside the chip)
                let synced = lfo.sync.value();
                let text = RichText::new("Sync").small().color(if synced { BACKGROUND } else { TEXT_WEAK });
                let button = egui::Button::new(text).small().fill(if synced { MOD_COLOR } else { WIDGET });
                if ui.add(button).on_hover_text(help::LFO_SYNC).clicked() {
                    set_param(setter, &lfo.sync, !synced);
                }
            },
        );
        // Its wave and trigger over its display, and its knobs under it
        let body = node.body;
        let combos = Rect::from_min_size(body.min, egui::vec2(body.width(), 2.0 * 22.0 + ROW_GAP));
        in_rect(ui, ("lfo choices", index), combos, Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = ROW_GAP;
            combo_cell(ui, &lfo.wave, index, body.width(), help::LFO_WAVE, setter);
            combo_cell(ui, &lfo.trigger, index, body.width(), help::LFO_TRIGGER, setter);
        });
        let knobs = catalog::lfo(lfo, index);
        let cell = (body.width() / knobs.len() as f32).min(LINK_CELL);
        let height = KNOBS.height(cell);
        let row = Rect::from_min_size(Pos2::new(body.left(), body.bottom() - height), egui::vec2(body.width(), height));
        knob_row(ui, ("lfo knobs", index), row, &knobs, cell, controls);
        let display = Rect::from_min_max(Pos2::new(body.left(), combos.bottom() + INNER_GAP), Pos2::new(body.right(), row.top() - 2.0));
        in_rect(ui, ("lfo display", index), display, Layout::top_down(Align::Min), |ui| {
            lfo_display(ui, lfo, index, live, display.height().max(24.0));
        });
    }
    let midi = node(ui, plan.lane[5]);
    title_row(
        ui,
        "midi title",
        midi.title,
        |ui| {
            title_text(ui, "MIDI");
        },
        |_| {},
    );
    in_rect(ui, "midi", midi.body, Layout::top_down(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.y = 6.0;
        for source in ModSource::MIDI {
            widgets::chip(ui, source, ModSource::variants()[source.to_index()], controls, &chips());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_diagram_fits_and_nothing_overlaps() {
        let default = egui::vec2(crate::editor::DEFAULT_WIDTH as f32, crate::editor::DEFAULT_HEIGHT as f32);
        for size in [default, egui::vec2(MIN_WIDTH, 700.0), egui::vec2(1920.0, 1200.0)] {
            let width = size.x.max(MIN_WIDTH);
            let area = Rect::from_min_size(Pos2::ZERO, egui::vec2(width, size.y.max(Plan::needed_height(width))));
            let plan = Plan::new(area);
            let mut rects = vec![plan.header, plan.filter, plan.envelope_link, plan.filter_env, plan.amp, plan.voice, plan.output];
            rects.extend(plan.oscillators);
            rects.extend(plan.links);
            rects.extend(plan.mixers);
            rects.extend(plan.effects);
            rects.extend(plan.lane);
            for (index, rect) in rects.iter().enumerate() {
                assert!(area.expand(0.5).contains_rect(*rect), "{size:?}: {rect:?}");
                assert!(rect.width() > 40.0 && rect.height() > 20.0, "{size:?}: {rect:?}");
                for other in &rects[index + 1..] {
                    assert!(!rect.shrink(0.5).intersects(other.shrink(0.5)), "{size:?}: {rect:?} {other:?}");
                }
            }
            // The oscillators have room for their knobs, and the effects for theirs
            for oscillator in plan.oscillators {
                assert!(oscillator.height() >= Plan::oscillator_height(oscillator.width()) - 0.5);
            }
            for effect in plan.effects {
                assert!(effect.height() >= node_height(KNOBS.height(cell_width(effect.width(), 3))) - 0.5);
            }
        }
        // The default window holds it without scrolling
        let inside = default - egui::vec2(16.0, 16.0) - egui::vec2(2.0 * MARGIN, 2.0 * MARGIN);
        assert!(inside.y >= Plan::needed_height(inside.x), "{}", Plan::needed_height(inside.x));
    }
}
