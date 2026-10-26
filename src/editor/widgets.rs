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
    analyser, combo_cell, displays, envelope_display, filter_display_inputs, help, lfo_display, load_wavetable_file,
    reset_remap, set_param, switch_theme, title_bar, wavetable_folder_rows, Gui, Live, Matrix, WaveDisplay, MOD_COLOR,
};
use crate::params::{CurrentValues, GlobalParams, ModSlotParams, OscillatorWave, ThreeOscParams};
use crate::synth::modulation::{ModSource, ModTarget, Polarity, Route, N_SLOTS};

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
/// `slider_width` wide), its polarity and remap, and a button to remove it. Returns whether it
/// was removed.
pub fn route_row(ui: &mut Ui, index: usize, slot: &ModSlotParams, setter: &ParamSetter, widths: [f32; 3]) -> bool {
    let [source_width, target_width, slider_width] = widths;
    combo_cell(ui, &slot.source, index, source_width, slot.source.value().help(), setter);
    ui.label(RichText::new("›").weak());
    combo_cell(ui, &slot.target, index, target_width, slot.target.value().help(), setter);
    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(slider_width))
        .on_hover_text(help::MOD_AMOUNT);
    ui.horizontal(|ui| {
        polarity_button(ui, slot, setter);
        remap_button(ui, index, slot);
    });
    let removed = ui.button("Remove").on_hover_text(help::CLEAR_ROUTE).clicked();
    if removed {
        Matrix::clear(setter, slot);
    }
    ui.end_row();
    removed
}

/// The height of a row of the routes over a modulated control (see [`routes_overlay()`]), and
/// the widths of its names and sliders.
const OVERLAY_ROW: f32 = 14.0;
const OVERLAY_NAME: f32 = 50.0;
const OVERLAY_SLIDER: f32 = 66.0;

/// While a modulated control is hovered (`hovered`), or the overlay itself is, the routes that
/// move it as small sliders with their sources' names, just above it (`anchor`), on a
/// transparent background. They show at once, and go as soon as the pointer leaves both the
/// control and them. Dragging a slider sets its route's amount (finer with Shift held), and a
/// double-click sets it to zero.
pub fn routes_overlay(
    ui: &Ui,
    id: egui::Id,
    anchor: Rect,
    hovered: bool,
    target: ModTarget,
    matrix: &Matrix,
    setter: &ParamSetter,
) {
    let ctx = ui.ctx();
    // Where it was in the last frame, which keeps it up while the pointer is on it
    let last: Option<Rect> = ctx.data(|data| data.get_temp(id));
    if !hovered && last.is_none() {
        return;
    }
    let on_overlay = last
        .zip(ctx.input(|input| input.pointer.hover_pos()))
        .is_some_and(|(rect, pointer)| rect.expand(3.0).contains(pointer));
    let sliding = ctx.dragged_id().is_some_and(|dragged| (0..N_SLOTS).any(|index| dragged == id.with(index)));
    let slots: Vec<(usize, &ModSlotParams)> = matrix.indexed_slots_for(target).collect();
    let dragging_source = egui::DragAndDrop::has_payload_of_type::<ModSource>(ctx);
    if !(hovered || on_overlay || sliding) || slots.is_empty() || dragging_source {
        ctx.data_mut(|data| data.remove::<Rect>(id));
        return;
    }
    // Above the control, or under it where there's no room above
    let size = egui::vec2(OVERLAY_NAME + 4.0 + OVERLAY_SLIDER, slots.len() as f32 * OVERLAY_ROW);
    let screen = ctx.screen_rect();
    let x = (anchor.center().x - size.x / 2.0).clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    let y = if anchor.top() - size.y >= screen.top() { anchor.top() - size.y } else { anchor.bottom() };
    let rect = Rect::from_min_size(Pos2::new(x, y), size);
    ctx.data_mut(|data| data.insert_temp(id, rect));
    egui::Area::new(id.with("area"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_min_size(size);
            for (row, (index, slot)) in slots.into_iter().enumerate() {
                let top = rect.top() + row as f32 * OVERLAY_ROW;
                let name = Rect::from_min_size(Pos2::new(rect.left(), top), egui::vec2(OVERLAY_NAME, OVERLAY_ROW));
                let track = Rect::from_min_size(
                    Pos2::new(name.right() + 4.0, top + 4.0),
                    egui::vec2(OVERLAY_SLIDER, OVERLAY_ROW - 8.0),
                );
                route_slider(ui, id.with(index), name, track, slot, setter);
            }
        });
}

/// One of [`routes_overlay()`]'s sliders: the route's source's name (or, while the slider is
/// hovered, its amount) in `name`, and its amount as a bar from the middle of `track`.
fn route_slider(ui: &Ui, id: egui::Id, name: Rect, track: Rect, slot: &ModSlotParams, setter: &ParamSetter) {
    let amount = &slot.amount;
    let response = ui.interact(track.expand2(egui::vec2(2.0, 4.0)), id, Sense::click_and_drag());
    // The position while dragging is kept unrounded, as the knobs keep theirs
    let position_id = id.with("position");
    if response.drag_started() {
        setter.begin_set_parameter(amount);
        ui.ctx().data_mut(|data| data.insert_temp(position_id, amount.unmodulated_normalized_value()));
    }
    if response.dragged() {
        let fine = ui.input(|input| input.modifiers.shift);
        let speed = if fine { 0.1 } else { 1.0 } / track.width();
        let start = ui.ctx().data(|data| data.get_temp::<f32>(position_id)).unwrap_or_else(|| amount.unmodulated_normalized_value());
        let position = (start + response.drag_delta().x * speed).clamp(0.0, 1.0);
        ui.ctx().data_mut(|data| data.insert_temp(position_id, position));
        setter.set_parameter_normalized(amount, position);
    }
    if response.drag_stopped() {
        setter.end_set_parameter(amount);
        ui.ctx().data_mut(|data| data.remove::<f32>(position_id));
    }
    if response.double_clicked() {
        set_param(setter, amount, 0.0);
    }
    let active = response.hovered() || response.dragged();
    if active {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }

    let painter = ui.painter();
    let shadow = Color32::from_black_alpha(190);
    painter.rect_filled(track, CornerRadius::same(3), Color32::from_black_alpha(if active { 200 } else { 150 }));
    let x = |normalized: f32| track.left() + track.width() * normalized.clamp(0.0, 1.0);
    let zero = x(amount.preview_normalized(0.0));
    let value = x(amount.unmodulated_normalized_value());
    let bar = Rect::from_x_y_ranges(zero.min(value)..=zero.max(value), track.y_range());
    painter.rect_filled(bar, CornerRadius::same(2), if active { MOD_COLOR } else { MOD_COLOR.gamma_multiply(0.85) });
    painter.line_segment([Pos2::new(zero, track.top() - 1.0), Pos2::new(zero, track.bottom() + 1.0)], Stroke::new(1.0_f32, shadow));
    let text = if active {
        amount.normalized_value_to_string(amount.unmodulated_normalized_value(), true)
    } else {
        String::from(ModSource::variants()[slot.source.value().to_index()])
    };
    let font = FontId::proportional(10.0);
    let at = name.right_center() - egui::vec2(0.0, 0.5);
    // Outlined, so that it reads over whatever is under the overlay
    for x in [-1.0, 0.0, 1.0] {
        for y in [-1.0, 0.0, 1.0] {
            if x != 0.0 || y != 0.0 {
                painter.text(at + egui::vec2(x, y), egui::Align2::RIGHT_CENTER, &text, font.clone(), shadow);
            }
        }
    }
    painter.text(at, egui::Align2::RIGHT_CENTER, &text, font, if active { Color32::WHITE } else { MOD_COLOR });
    let source = ModSource::variants()[slot.source.value().to_index()];
    response.on_hover_text(format!(
        "How much of {source} moves this control: drag to set it (Shift for finer), double-click for none."
    ));
}

/// A button showing whether a route swings both ways (`±`) or one way (`+`), which switches it
/// to the other (see [`Polarity`]).
pub fn polarity_button(ui: &mut Ui, slot: &ModSlotParams, setter: &ParamSetter) -> Response {
    let source = slot.source.value();
    let bipolar = slot.polarity.value().is_bipolar(source);
    let text = RichText::new(if bipolar { "±" } else { "+" }).strong().color(MOD_COLOR);
    let response = ui
        .add(egui::Button::new(text).small().min_size(egui::vec2(18.0, 0.0)))
        .on_hover_text(format!(
            "{} {}",
            if bipolar { "Swings both ways: click for one way." } else { "Swings one way: click for both ways." },
            help::MOD_POLARITY
        ));
    if response.clicked() {
        // (The source's own polarity is left as the default, so that the route stays plain)
        let polarity = match (!bipolar, source.is_bipolar()) {
            (wanted, native) if wanted == native => Polarity::Source,
            (true, _) => Polarity::Bipolar,
            (false, _) => Polarity::Unipolar,
        };
        set_param(setter, &slot.polarity, polarity);
    }
    response
}

/// A route's remap as the synth applies it, with a full amount.
fn remap_route(slot: &ModSlotParams) -> Route {
    Route::remapped(slot.source.value(), slot.target.value(), 1.0, slot.remap(&CurrentValues))
}

/// Whether a route reshapes its source in a way the polarity button doesn't show.
fn is_remapped(slot: &ModSlotParams) -> bool {
    slot.curve.value() != 0.0 || slot.steps.value() > 1 || slot.via.value() != ModSource::None
}

/// Draws how a route maps its source in `rect`: across, the source from its bottom to its top;
/// up, what the route applies, from -1 to 1 (with the scaling source at its top).
fn paint_remap(painter: &Painter, rect: Rect, route: &Route, color: Color32, width: f32) {
    let bipolar = route.source.is_bipolar();
    let y = |value: f32| rect.center().y - value.clamp(-1.0, 1.0) * rect.height() / 2.0;
    painter.hline(rect.x_range(), y(0.0), Stroke::new(1.0_f32, color.gamma_multiply(0.3)));
    let steps = 64;
    let points = (0..=steps)
        .map(|step| {
            let across = step as f32 / steps as f32;
            let value = if bipolar { across * 2.0 - 1.0 } else { across };
            Pos2::new(rect.left() + across * rect.width(), y(route.reshape(value, 1.0)))
        })
        .collect();
    painter.add(egui::Shape::line(points, Stroke::new(width, color)));
}

/// What the remap window shows: the slot it's for, the button that opened it, and whether it's
/// been put beside that button yet (after which it can be moved).
#[derive(Debug, Clone, Copy, PartialEq)]
struct RemapWindow {
    slot: usize,
    button: Rect,
    placed: bool,
}

fn remap_window_id() -> egui::Id {
    egui::Id::new("remap window")
}

/// Where a window of `size` goes beside the button at `button`: under it (or over it, where
/// there's no room under it), with their right edges lined up, and on the screen.
fn beside(button: Rect, size: egui::Vec2, screen: Rect) -> Pos2 {
    let x = (button.right() - size.x).clamp(screen.left(), (screen.right() - size.x).max(screen.left()));
    let under = button.bottom() + 4.0;
    let y = if under + size.y <= screen.bottom() { under } else { (button.top() - 4.0 - size.y).max(screen.top()) };
    Pos2::new(x, y)
}

/// A button for a route's remap (see [`Remap`][crate::synth::modulation::Remap]): a small
/// picture of how the route maps its source, lit while it does more than its polarity says,
/// which opens the remap's window (see [`remap_window()`]).
pub fn remap_button(ui: &mut Ui, index: usize, slot: &ModSlotParams) -> Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 16.0), Sense::click());
    let visuals = ui.style().interact(&response);
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(3), visuals.weak_bg_fill, visuals.bg_stroke, StrokeKind::Inside);
    let color = if is_remapped(slot) { MOD_COLOR } else { visuals.fg_stroke.color.gamma_multiply(0.7) };
    paint_remap(painter, rect.shrink2(egui::vec2(4.0, 3.0)), &remap_route(slot), color, 1.2);
    if response.clicked() {
        ui.ctx().data_mut(|data| {
            let open = data.get_temp::<Option<RemapWindow>>(remap_window_id()).flatten();
            let next = match open {
                Some(window) if window.slot == index => None,
                _ => Some(RemapWindow { slot: index, button: rect, placed: false }),
            };
            data.insert_temp(remap_window_id(), next);
        });
    }
    response.on_hover_text(
        "How the route reshapes its source: click to set how it swings, bend it, step it, or have \
         another source scale it.",
    )
}

/// The window for the remap of the route whose remap button was clicked last, while it's open:
/// a picture of how the route maps its source, its polarity, curve and steps, the source that
/// scales it, its amount, and a button to put it back as it was.
pub fn remap_window(ctx: &egui::Context, params: &ThreeOscParams, setter: &ParamSetter) {
    let Some(RemapWindow { slot: index, button, placed }) = ctx.data(|data| data.get_temp(remap_window_id())).flatten()
    else {
        return;
    };
    let slot = &params.mod_slots[index];
    let mut open = Matrix::is_used(slot);
    let name = |variants: &'static [&'static str], index: usize| variants[index];
    let title = format!(
        "{} › {}",
        name(ModSource::variants(), slot.source.value().to_index()),
        name(ModTarget::variants(), slot.target.value().to_index())
    );
    // It's put beside its button until it has been shown at the size it was measured at (the
    // first time it's shown, egui only measures it)
    let size_id = remap_window_id().with("size");
    let measured: Option<egui::Vec2> = ctx.data(|data| data.get_temp(size_id));
    let mut window = egui::Window::new(title).id(remap_window_id()).open(&mut open).collapsible(false).resizable(false);
    if !placed {
        let size = measured.unwrap_or(egui::vec2(280.0, 320.0));
        window = window.current_pos(beside(button, size, ctx.screen_rect()));
    }
    let shown = window.show(ctx, |ui| {
            ui.set_width(260.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(260.0, 70.0), Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, CornerRadius::same(4), ui.visuals().extreme_bg_color);
            paint_remap(painter, rect.shrink(6.0), &remap_route(slot), MOD_COLOR, 2.0);
            let source = slot.source.value();
            let via = slot.via.value();
            let caption = if via == ModSource::None {
                String::from("The source across, what the route applies up.")
            } else {
                let via = name(ModSource::variants(), via.to_index());
                format!("The source across, what the route applies up, with {via} at its top.")
            };
            ui.label(RichText::new(caption).small().weak());
            ui.add_space(4.0);
            egui::Grid::new(("remap", index)).num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                ui.label("Polarity").on_hover_text(help::MOD_POLARITY);
                ui.horizontal(|ui| {
                    let own = if source.is_bipolar() { "As source (±)" } else { "As source (+)" };
                    for (polarity, label) in [(Polarity::Source, own), (Polarity::Unipolar, "+"), (Polarity::Bipolar, "±")] {
                        let chosen = slot.polarity.value() == polarity;
                        if ui.selectable_label(chosen, label).on_hover_text(help::MOD_POLARITY).clicked() && !chosen {
                            set_param(setter, &slot.polarity, polarity);
                        }
                    }
                });
                ui.end_row();
                ui.label("Curve").on_hover_text(help::MOD_CURVE);
                ui.add(ParamSlider::for_param(&slot.curve, setter).with_width(150.0)).on_hover_text(help::MOD_CURVE);
                ui.end_row();
                ui.label("Steps").on_hover_text(help::MOD_STEPS);
                ui.add(ParamSlider::for_param(&slot.steps, setter).with_width(150.0)).on_hover_text(help::MOD_STEPS);
                ui.end_row();
                ui.label("Scale by").on_hover_text(help::MOD_VIA);
                combo_cell(ui, &slot.via, index, 150.0, help::MOD_VIA, setter);
                ui.end_row();
                ui.label("Amount").on_hover_text(help::MOD_AMOUNT);
                ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(150.0)).on_hover_text(help::MOD_AMOUNT);
                ui.end_row();
            });
            ui.add_space(4.0);
            let reset = ui.button("Reset the shape").on_hover_text("Puts the polarity, curve, steps and scaling back as they were.");
            if reset.clicked() {
                reset_remap(setter, slot);
            }
        });
    let size = shown.map(|shown| shown.response.rect.size());
    ctx.data_mut(|data| {
        if let Some(size) = size {
            data.insert_temp(size_id, size);
        }
        let next = open.then_some(RemapWindow { slot: index, button, placed: placed || (size.is_some() && size == measured) });
        data.insert_temp(remap_window_id(), next);
    });
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
        egui::Grid::new("routes").num_columns(6).spacing([10.0, 6.0]).show(ui, |ui| {
            for heading in ["Source", "", "Target", "Amount", "Shape", ""] {
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
        .default_size(egui::vec2(780.0, 360.0))
        .default_pos(below_the_header(ctx, 780.0))
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
            Self::Lfo(index) => lfo_display(ui, &params.lfos[index], index, live, height),
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
