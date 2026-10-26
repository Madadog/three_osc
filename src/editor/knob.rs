//! A control for a parameter, as the themes other than Classic have them: a knob that turns,
//! or a fader that slides, with its name above it and its value below (or, in a compact style,
//! in place of its name while it's hovered).
//!
//! Dragging a knob up (or to the right) turns it up, ten times finer with Shift held; the mouse
//! wheel steps it; a double-click (or Ctrl/Cmd+click) puts it back to its default; and clicking
//! its value lets one be typed in. Right-clicking it opens a menu for all of that and for the
//! modulation routes that move it.
//!
//! A knob that can be modulated is a drop target for the modulation sources' handles. Around it
//! (or beside a fader) an arc shows how far its routes can move it, with a dot where the newest
//! note's modulation has it, and it lights up while the handle of a source that modulates it is
//! hovered.

use std::cell::RefCell;
use std::f32::consts::PI;

use nih_plug::prelude::*;
use nih_plug_egui::egui::text::{CCursor, CCursorRange};
use nih_plug_egui::egui::text_edit::TextEditState;
use nih_plug_egui::egui::{
    self, Color32, CornerRadius, FontId, Pos2, Rect, Response, RichText, Sense, Shape as EguiShape, Stroke, StrokeKind,
    Ui, Vec2,
};
use nih_plug_egui::widgets::ParamSlider;

use super::catalog::Colors;
use super::{set_param, Matrix, NumericParam, MOD_COLOR};
use crate::synth::modulation::{ModSource, ModTarget, Scaling, N_SOURCES};

/// What the knobs need besides their parameters: a way to set them, the modulation matrix, the
/// modulation source whose routes to light up (the one whose handle is hovered), and how the
/// layout wants them to look.
pub struct Controls<'a> {
    pub setter: &'a ParamSetter<'a>,
    pub matrix: &'a Matrix<'a>,
    pub highlight: Option<ModSource>,
    pub style: Style,
    /// The colour of each group's knobs.
    pub colors: Colors,
    /// Where to note down where each knob that can be modulated is, for layouts that draw the
    /// routes to them.
    pub positions: Option<&'a RefCell<Vec<(ModTarget, Pos2)>>>,
}

/// How far a drag turns a knob across its whole range, in pixels.
const DRAG_PIXELS: f32 = 220.0;
/// How much finer dragging (and the wheel) gets with Shift held.
const FINE: f32 = 0.1;
/// How far a step of the mouse wheel turns a knob that doesn't have steps of its own.
const WHEEL_STEP: f32 = 0.02;
/// A knob's arc goes from the bottom left round to the bottom right, in radians clockwise from
/// the right (egui's y axis points down).
const START_ANGLE: f32 = 0.75 * PI;
const SWEEP: f32 = 1.5 * PI;
/// The colour of where the newest note's modulation has a knob.
const LIVE_COLOR: Color32 = Color32::from_rgb(255, 238, 205);
const MIN_DIAMETER: f32 = 22.0;
/// The smallest a knob's name or value gets to fit its cell.
const MIN_FONT_SIZE: f32 = 7.0;
/// The height of a fader's cap.
const CAP_HEIGHT: f32 = 12.0;

/// What a control looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// A knob that turns.
    Rotary,
    /// A vertical fader whose cap travels this many pixels.
    Fader(f32),
}

/// How a layout's knobs look: their shape, their text, and what they show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub shape: Shape,
    pub font_size: f32,
    pub monospace: bool,
    /// Whether the value is shown under the knob all the time, or only in place of its name
    /// while it is hovered or turned, which takes less room.
    pub value_below: bool,
    /// Whether hovering a knob shows what it does, for layouts that don't show that elsewhere.
    pub tooltips: bool,
    /// The largest a knob gets across.
    pub max_diameter: f32,
}

impl Style {
    /// Studio's knobs.
    pub const STUDIO: Self = Self {
        shape: Shape::Rotary,
        font_size: 11.5,
        monospace: false,
        value_below: true,
        tooltips: true,
        max_diameter: 44.0,
    };

    fn font(&self) -> FontId {
        if self.monospace {
            FontId::monospace(self.font_size)
        } else {
            FontId::proportional(self.font_size)
        }
    }

    fn label_height(&self) -> f32 {
        (self.font_size * 1.3).round()
    }

    fn value_height(&self) -> f32 {
        if self.value_below {
            self.label_height() + 2.0
        } else {
            0.0
        }
    }

    /// How tall the knob, or the fader, is in a cell `width` wide.
    fn control_height(&self, width: f32) -> f32 {
        match self.shape {
            Shape::Rotary => self.diameter(width),
            Shape::Fader(travel) => travel + CAP_HEIGHT,
        }
    }

    fn diameter(&self, width: f32) -> f32 {
        (width * 0.58).clamp(MIN_DIAMETER.min(self.max_diameter), self.max_diameter)
    }

    /// The height of a knob `width` wide, with its name and its value.
    pub fn height(&self, width: f32) -> f32 {
        self.label_height() + self.control_height(width) + self.value_height() + 2.0
    }
}

/// The angle of a knob turned to `normalized`.
fn angle(normalized: f32) -> f32 {
    START_ANGLE + SWEEP * normalized.clamp(0.0, 1.0)
}

/// Draws an arc around `center` between two knob positions.
fn arc(painter: &egui::Painter, center: Pos2, radius: f32, from: f32, to: f32, stroke: Stroke) {
    let (from, to) = (angle(from.min(to)), angle(from.max(to)));
    let steps = ((to - from) / 0.06).ceil().max(1.0) as usize;
    let points = (0..=steps)
        .map(|step| center + radius * Vec2::angled(from + (to - from) * step as f32 / steps as f32))
        .collect();
    painter.add(EguiShape::line(points, stroke));
}

/// Where a knob's arc starts: at zero, for a parameter that goes either side of it, or else at
/// the bottom of its range.
fn origin<P: NumericParam>(param: &P) -> f32 {
    let (low, high) = (param.plain_at(0.0), param.plain_at(1.0));
    if low < 0.0 && high > 0.0 {
        param.normalized_at(0.0)
    } else {
        0.0
    }
}

/// How far `routes` (each whether it swings both ways, and its amount) can move a knob whose
/// modulation moves it as `scaling` says, from its value: the knob positions at the sum of the
/// routes' lowest and highest modulation, if there are routes.
fn modulation_span<P: NumericParam>(
    param: &P,
    scaling: Scaling,
    routes: impl Iterator<Item = (bool, f32)>,
) -> Option<(f32, f32)> {
    let (mut low, mut high, mut any) = (0.0, 0.0, false);
    for (bipolar, amount) in routes {
        if bipolar {
            low -= amount.abs();
            high += amount.abs();
        } else {
            low += amount.min(0.0);
            high += amount.max(0.0);
        }
        any = true;
    }
    let value = param.value_f32();
    any.then(|| (param.normalized_at(scaling.apply(value, low)), param.normalized_at(scaling.apply(value, high))))
}

/// Sets a parameter to the value at a position of its control, during a gesture.
fn set_normalized<P: Param>(setter: &ParamSetter, param: &P, normalized: f32) {
    let value = param.preview_plain(normalized);
    if value != param.unmodulated_plain_value() {
        setter.set_parameter(param, value);
    }
}

/// A knob for a parameter, see the module's documentation.
pub struct Knob<'a, P: NumericParam> {
    param: &'a P,
    label: &'a str,
    help: &'a str,
    target: ModTarget,
    /// How the target's modulation moves the knob: as it moves the target's own control,
    /// unless the knob says otherwise.
    scaling: Scaling,
    color: Color32,
}

impl<'a, P: NumericParam> Knob<'a, P> {
    /// A knob showing `label` above it, `help` when hovered, and its value in `color`.
    pub fn new(param: &'a P, label: &'a str, help: &'a str, color: Color32) -> Self {
        Self {
            param,
            label,
            help,
            target: ModTarget::None,
            scaling: ModTarget::None.scaling(),
            color,
        }
    }

    /// Makes the knob a target for modulation, which moves it as `scaling` says (or as it moves
    /// the target's own control).
    pub fn modulated(mut self, target: ModTarget, scaling: Option<Scaling>) -> Self {
        self.target = target;
        self.scaling = scaling.unwrap_or_else(|| target.scaling());
        self
    }

    /// Adds the knob, `width` wide (and as tall as the style's [`Style::height()`]).
    pub fn show(self, ui: &mut Ui, width: f32, controls: &Controls) -> Response {
        let Self { param, label, help, target, scaling, color } = self;
        let setter = controls.setter;
        let style = controls.style;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, style.height(width)), Sense::hover());
        let id = ui.id().with(("knob", param.name()));
        let label_rect = Rect::from_min_size(rect.min, egui::vec2(width, style.label_height()));
        let control_rect = Rect::from_min_size(
            Pos2::new(rect.left(), label_rect.bottom()),
            egui::vec2(width, style.control_height(width)),
        );
        // The name is part of what can be grabbed, and the value (or, without a value below, the
        // name) is for typing one in
        let grab = Rect::from_min_max(
            if style.value_below { rect.min } else { control_rect.min },
            Pos2::new(rect.right(), control_rect.bottom() + 1.0),
        );
        let value_rect = if style.value_below {
            Rect::from_min_max(Pos2::new(rect.left(), grab.bottom()), rect.max)
        } else {
            label_rect
        };
        let response = ui.interact(grab, id, Sense::click_and_drag());
        let modulatable = target != ModTarget::None;
        let fine = ui.input(|input| input.modifiers.shift);
        if let (true, Some(positions)) = (modulatable, controls.positions) {
            positions.borrow_mut().push((target, control_rect.center()));
        }

        // Dragging turns it from where it was, with the unrounded position kept so that stepped
        // parameters move evenly. A fader's cap follows the pointer.
        let drag_id = id.with("drag");
        if response.drag_started() {
            setter.begin_set_parameter(param);
            let start = param.unmodulated_normalized_value();
            ui.memory_mut(|memory| memory.data.insert_temp(drag_id, start));
        }
        if response.dragged() {
            let delta = response.drag_delta();
            let (distance, pixels) = match style.shape {
                Shape::Rotary => (delta.x - delta.y, DRAG_PIXELS),
                Shape::Fader(travel) => (-delta.y, travel),
            };
            let speed = if fine { FINE } else { 1.0 } / pixels;
            let start = ui.memory(|memory| memory.data.get_temp::<f32>(drag_id));
            let position =
                (start.unwrap_or_else(|| param.unmodulated_normalized_value()) + distance * speed).clamp(0.0, 1.0);
            ui.memory_mut(|memory| memory.data.insert_temp(drag_id, position));
            set_normalized(setter, param, position);
        }
        if response.drag_stopped() {
            setter.end_set_parameter(param);
            ui.memory_mut(|memory| memory.data.remove::<f32>(drag_id));
        }
        if response.double_clicked() || (response.clicked() && ui.input(|input| input.modifiers.command)) {
            set_param(setter, param, param.default_plain_value());
        }
        if response.hovered() {
            wheel(ui, id, param, setter, fine);
        }
        if modulatable {
            if let Some(source) = response.dnd_release_payload::<ModSource>() {
                controls.matrix.add_route(setter, *source, target);
            }
        }

        // The value, or the field it's being typed into
        let entry_id = id.with("entry");
        let editing = value_entry(ui, entry_id, value_rect, param, setter, style.font());
        let value = (!editing).then(|| ui.interact(value_rect, id.with("value"), Sense::click()));
        if let Some(value) = &value {
            if value.clicked() {
                start_entry(ui, entry_id, param);
            }
        }
        let value_hovered = value.as_ref().is_some_and(|value| value.hovered());
        let active = response.hovered() || response.dragged() || value_hovered;
        let text_color = |strong: bool| if strong { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
        if !editing {
            if value_hovered {
                ui.painter().rect_filled(value_rect.shrink2(egui::vec2(4.0, 1.0)), 3.0, ui.visuals().widgets.hovered.bg_fill);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            let shown = if style.value_below || !active {
                None
            } else {
                Some(param.normalized_value_to_string(param.modulated_normalized_value(), true))
            };
            // The name, or in its place the value while the knob is in use
            let (text, color) = match shown {
                Some(text) => (text, text_color(true)),
                None if active => (label.to_string(), text_color(true)),
                None => (label.to_string(), ui.visuals().weak_text_color()),
            };
            fitted_text(ui.painter(), label_rect, text, style.font(), color);
            if style.value_below {
                let text = param.normalized_value_to_string(param.modulated_normalized_value(), true);
                let color = text_color(response.dragged() || value_hovered);
                fitted_text(ui.painter(), value_rect, text, style.font(), color);
            }
        } else if style.value_below {
            fitted_text(ui.painter(), label_rect, label.to_string(), style.font(), text_color(true));
        }

        match style.shape {
            Shape::Rotary => paint_rotary(ui, &response, control_rect, param, target, scaling, color, controls),
            Shape::Fader(_) => paint_fader(ui, &response, control_rect, param, target, scaling, color, controls),
        }

        if response.dragged() {
            let cursor = match style.shape {
                Shape::Rotary => egui::CursorIcon::ResizeVertical,
                Shape::Fader(_) => egui::CursorIcon::ResizeRow,
            };
            ui.ctx().set_cursor_icon(cursor);
        }
        context_menu(&response, entry_id, param, target, controls);
        // The routes that move it, over it while it's hovered
        let hovered = response.hovered() || value_hovered;
        super::widgets::routes_overlay(ui, id.with("routes"), rect, hovered, target, controls.matrix, setter);
        if let (Some(value), true) = (value, style.value_below) {
            value.on_hover_text("Click to type in a value.");
        }
        // (Not while it's being turned, when it would only get in the way)
        if response.dragged() || !style.tooltips {
            response
        } else {
            response.on_hover_ui(|ui| {
                ui.label(RichText::new(param.name()).strong());
                ui.label(help);
                if modulatable {
                    ui.label(RichText::new("Right-click to edit its modulation.").weak());
                }
            })
        }
    }
}

/// Writes `text` in the middle of `rect`, in a smaller font if it would be wider than the rect.
fn fitted_text(painter: &egui::Painter, rect: Rect, text: String, font: FontId, color: Color32) {
    let galley = painter.layout_no_wrap(text.clone(), font.clone(), color);
    let room = rect.width() - 2.0;
    let galley = if galley.size().x > room {
        let size = (font.size * room / galley.size().x).max(MIN_FONT_SIZE);
        painter.layout_no_wrap(text, FontId::new(size, font.family), color)
    } else {
        galley
    };
    painter.galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// Turns a hovered knob by the mouse wheel's steps: one step of the parameter's at a time if it
/// has steps, else [`WHEEL_STEP`] (or a tenth of that, finely). Under a knob the page doesn't
/// scroll.
fn wheel<P: Param>(ui: &mut Ui, id: egui::Id, param: &P, setter: &ParamSetter, fine: bool) {
    let steps: f32 = ui.input(|input| {
        input
            .events
            .iter()
            .map(|event| match event {
                egui::Event::MouseWheel { unit, delta, .. } => {
                    // (Shift may turn the wheel sideways)
                    let delta = delta.x + delta.y;
                    match unit {
                        egui::MouseWheelUnit::Line => delta,
                        egui::MouseWheelUnit::Point => delta / 50.0,
                        egui::MouseWheelUnit::Page => delta * 10.0,
                    }
                }
                _ => 0.0,
            })
            .sum()
    });
    ui.input_mut(|input| {
        input.smooth_scroll_delta = Vec2::ZERO;
        input.raw_scroll_delta = Vec2::ZERO;
    });
    if steps == 0.0 {
        return;
    }
    let step = match param.step_count() {
        Some(count) => 1.0 / count as f32,
        None => WHEEL_STEP * if fine { FINE } else { 1.0 },
    };
    // The unrounded position, kept while it's still where the parameter is, so that small
    // steps (from a touchpad) add up on stepped parameters
    let wheel_id = id.with("wheel");
    let kept = ui.memory(|memory| memory.data.get_temp::<f32>(wheel_id));
    let start = kept
        .filter(|position| param.preview_plain(*position) == param.unmodulated_plain_value())
        .unwrap_or_else(|| param.unmodulated_normalized_value());
    let position = (start + steps * step).clamp(0.0, 1.0);
    ui.memory_mut(|memory| memory.data.insert_temp(wheel_id, position));
    setter.begin_set_parameter(param);
    set_normalized(setter, param, position);
    setter.end_set_parameter(param);
}

/// Starts typing a value into a knob's value field, with the current value selected.
fn start_entry<P: Param>(ui: &Ui, entry_id: egui::Id, param: &P) {
    let text = param.normalized_value_to_string(param.unmodulated_normalized_value(), true);
    let field = entry_id.with("field");
    let mut state = TextEditState::load(ui.ctx(), field).unwrap_or_default();
    let all = CCursorRange::two(CCursor::new(0), CCursor::new(text.chars().count()));
    state.cursor.set_char_range(Some(all));
    state.store(ui.ctx(), field);
    ui.memory_mut(|memory| {
        memory.data.insert_temp(entry_id, text);
        memory.request_focus(field);
    });
}

/// The field a value is being typed into, if one is: Enter sets the parameter to it, and
/// Escape or clicking elsewhere leaves the parameter as it was. Returns whether there is one.
fn value_entry<P: Param>(
    ui: &mut Ui,
    entry_id: egui::Id,
    rect: Rect,
    param: &P,
    setter: &ParamSetter,
    font: FontId,
) -> bool {
    let Some(mut text) = ui.memory(|memory| memory.data.get_temp::<String>(entry_id)) else {
        return false;
    };
    let field = entry_id.with("field");
    let edit = ui.put(
        rect,
        egui::TextEdit::singleline(&mut text)
            .id(field)
            .font(font)
            .horizontal_align(egui::Align::Center)
            .margin(egui::Margin::symmetric(2, 1)),
    );
    let entered = ui.input(|input| input.key_pressed(egui::Key::Enter));
    if edit.lost_focus() || !edit.has_focus() {
        if entered && edit.lost_focus() {
            if let Some(normalized) = param.string_to_normalized_value(&text) {
                setter.begin_set_parameter(param);
                set_normalized(setter, param, normalized);
                setter.end_set_parameter(param);
            }
        }
        ui.memory_mut(|memory| memory.data.remove::<String>(entry_id));
    } else {
        ui.memory_mut(|memory| memory.data.insert_temp(entry_id, text));
    }
    true
}

/// Lights a knob up (`around` its control) while a source is dragged over it, dimly while one
/// is dragged at all, and while the handle of a source that modulates it is hovered.
fn paint_drop_highlight(ui: &Ui, response: &Response, target: ModTarget, controls: &Controls, around: impl FnOnce(Color32)) {
    if target == ModTarget::None {
        return;
    }
    let dragged = egui::DragAndDrop::has_payload_of_type::<ModSource>(ui.ctx());
    let lit = response.dnd_hover_payload::<ModSource>().is_some()
        || controls
            .highlight
            .is_some_and(|source| controls.matrix.routes_for(target).any(|(routed, ..)| routed == source));
    if dragged || lit {
        around(if lit { MOD_COLOR } else { MOD_COLOR.gamma_multiply(0.35) });
    }
}

/// Where the routes to a control can move it, and where the newest note's modulation has it, as
/// positions of the control from 0 to 1, if anything modulates it.
fn modulation_marks<P: NumericParam>(
    param: &P,
    target: ModTarget,
    scaling: Scaling,
    matrix: &Matrix,
) -> Option<((f32, f32), Option<f32>)> {
    if target == ModTarget::None {
        return None;
    }
    let routes = matrix.routes_for(target).map(|(_, bipolar, amount)| (bipolar, amount));
    let span = modulation_span(param, scaling, routes)?;
    let live = matrix
        .monitor
        .modulation(target)
        .map(|modulation| param.normalized_at(scaling.apply(param.value_f32(), modulation)));
    Some((span, live))
}

/// Draws a knob in `rect`: its track, the arc from its origin to its value, its pointer, and its
/// modulation around it.
#[allow(clippy::too_many_arguments)]
fn paint_rotary<P: NumericParam>(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    param: &P,
    target: ModTarget,
    scaling: Scaling,
    color: Color32,
    controls: &Controls,
) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let center = rect.center();
    let radius = rect.height() / 2.0;
    let ring = radius - 2.0;
    let active = response.hovered() || response.dragged();
    let normalized = param.modulated_normalized_value();

    paint_drop_highlight(ui, response, target, controls, |color| {
        painter.circle_stroke(center, radius + 5.0, Stroke::new(1.5_f32, color));
    });
    arc(painter, center, ring, 0.0, 1.0, Stroke::new(3.0_f32, visuals.extreme_bg_color));
    let value_color = if active { color } else { color.gamma_multiply(0.85) };
    arc(painter, center, ring, origin(param), normalized, Stroke::new(3.0_f32, value_color));
    let body = if active { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.bg_fill };
    painter.circle_filled(center, radius - 5.5, body);
    let direction = Vec2::angled(angle(normalized));
    painter.line_segment(
        [center + direction * (radius * 0.2), center + direction * (radius - 7.0)],
        Stroke::new(2.0_f32, visuals.widgets.inactive.fg_stroke.color),
    );

    if let Some(((low, high), live)) = modulation_marks(param, target, scaling, controls.matrix) {
        let outer = radius + 1.5;
        arc(painter, center, outer, low, high, Stroke::new(2.0_f32, MOD_COLOR));
        painter.circle_filled(center + outer * Vec2::angled(angle(normalized)), 2.0, MOD_COLOR);
        if let Some(at) = live {
            let dot = center + outer * Vec2::angled(angle(at));
            painter.circle(dot, 2.5, LIVE_COLOR, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
        }
    }
}

/// Draws a fader in `rect`: its slot with a scale, the stretch from its origin to its value, its
/// cap, and its modulation beside it.
#[allow(clippy::too_many_arguments)]
fn paint_fader<P: NumericParam>(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    param: &P,
    target: ModTarget,
    scaling: Scaling,
    color: Color32,
    controls: &Controls,
) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let active = response.hovered() || response.dragged();
    let normalized = param.modulated_normalized_value();
    let x = rect.center().x;
    let (top, bottom) = (rect.top() + CAP_HEIGHT / 2.0, rect.bottom() - CAP_HEIGHT / 2.0);
    let y = |position: f32| bottom - (bottom - top) * position.clamp(0.0, 1.0);

    paint_drop_highlight(ui, response, target, controls, |color| {
        let outline = Rect::from_x_y_ranges((x - 16.0)..=(x + 16.0), rect.y_range()).expand(3.0);
        painter.rect_stroke(outline, 4.0, Stroke::new(1.5_f32, color), StrokeKind::Outside);
    });
    // A tick every tenth of the way, longer at the ends and the middle
    for tick in 0..=10 {
        let length = if tick % 5 == 0 { 7.0 } else { 4.0 };
        let at = y(tick as f32 / 10.0);
        painter.line_segment(
            [Pos2::new(x - 7.0 - length, at), Pos2::new(x - 7.0, at)],
            Stroke::new(1.0_f32, visuals.weak_text_color().gamma_multiply(0.6)),
        );
    }
    let slot = Rect::from_x_y_ranges((x - 2.5)..=(x + 2.5), top..=bottom);
    painter.rect_filled(slot, 2.5, visuals.extreme_bg_color);
    let (from, to) = (y(origin(param)), y(normalized));
    let fill = Rect::from_x_y_ranges((x - 2.5)..=(x + 2.5), from.min(to)..=from.max(to));
    painter.rect_filled(fill, 2.5, if active { color } else { color.gamma_multiply(0.85) });

    if let Some(((low, high), live)) = modulation_marks(param, target, scaling, controls.matrix) {
        let bar_x = x + 9.0;
        painter.line_segment([Pos2::new(bar_x, y(low)), Pos2::new(bar_x, y(high))], Stroke::new(3.0_f32, MOD_COLOR));
        if let Some(at) = live {
            painter.circle(Pos2::new(bar_x, y(at)), 2.5, LIVE_COLOR, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
        }
    }

    let cap_width = (rect.width() * 0.62).clamp(18.0, 30.0);
    let cap = Rect::from_center_size(Pos2::new(x, y(normalized)), egui::vec2(cap_width, CAP_HEIGHT));
    let body = if active { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.bg_fill };
    painter.rect(cap, CornerRadius::same(2), body, Stroke::new(1.0_f32, Color32::from_black_alpha(140)), StrokeKind::Inside);
    painter.line_segment(
        [Pos2::new(cap.left() + 3.0, cap.center().y), Pos2::new(cap.right() - 3.0, cap.center().y)],
        Stroke::new(2.0_f32, color),
    );
}

/// A knob's right-click menu: resetting it, typing in a value, and its modulation: the routes
/// that move it (under a thin line, if there are any), with their amounts and a way to remove
/// them, and a menu to add another.
fn context_menu<P: NumericParam>(
    response: &Response,
    entry_id: egui::Id,
    param: &P,
    target: ModTarget,
    controls: &Controls,
) {
    let setter = controls.setter;
    response.context_menu(|ui| {
        ui.set_min_width(220.0);
        let value = param.normalized_value_to_string(param.unmodulated_normalized_value(), true);
        ui.label(RichText::new(format!("{}: {value}", param.name())).strong());
        let default = param.normalized_value_to_string(param.default_normalized_value(), true);
        if ui.button(format!("Reset to {default}")).clicked() {
            set_param(setter, param, param.default_plain_value());
            ui.close_menu();
        }
        if ui.button("Type in a value").clicked() {
            start_entry(ui, entry_id, param);
            ui.close_menu();
        }
        if target == ModTarget::None {
            return;
        }
        let matrix = controls.matrix;
        let mut routed = [false; N_SOURCES];
        let slots: Vec<_> = matrix.indexed_slots_for(target).collect();
        if !slots.is_empty() {
            ui.scope(|ui| {
                ui.visuals_mut().widgets.noninteractive.bg_stroke.color = MOD_COLOR.gamma_multiply(0.45);
                ui.add(egui::Separator::default().spacing(4.0));
            });
            egui::Grid::new("routes").num_columns(5).min_col_width(0.0).spacing([6.0, 4.0]).show(ui, |ui| {
                for (index, slot) in slots {
                    let source = slot.source.value();
                    routed[source.to_index()] = true;
                    ui.label(RichText::new(ModSource::variants()[source.to_index()]).color(MOD_COLOR))
                        .on_hover_text(source.help());
                    ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(110.0))
                        .on_hover_text(super::help::MOD_AMOUNT);
                    super::widgets::polarity_button(ui, slot, setter);
                    if super::widgets::remap_button(ui, index, slot).clicked() {
                        // (For the remap's window, which opens where the button was)
                        ui.close_menu();
                    }
                    if ui.small_button("×").on_hover_text(super::help::REMOVE_ROUTE).clicked() {
                        Matrix::clear(setter, slot);
                    }
                    ui.end_row();
                }
            });
        }
        let free = matrix.slots.iter().any(|slot| !Matrix::is_used(slot));
        ui.add_enabled_ui(free, |ui| {
            ui.menu_button("Add modulation", |ui| {
                for (index, name) in ModSource::variants().iter().enumerate().skip(1) {
                    let source = ModSource::from_index(index);
                    let button = ui.add_enabled(!routed[index], egui::Button::new(*name)).on_hover_text(source.help());
                    if button.clicked() {
                        matrix.add_route(setter, source, target);
                        ui.close_menu();
                    }
                }
            });
        })
        .response
        .on_disabled_hover_text("All of the modulation matrix's routes are in use.");
    });
}

#[cfg(test)]
mod tests {
    use super::super::catalog;
    use super::*;
    use crate::params::ThreeOscParams;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn arcs_start_at_zero_for_parameters_either_side_of_it() {
        let params = ThreeOscParams::default();
        assert_eq!(origin(&params.osc1.amp), 0.0);
        assert!(close(origin(&params.osc1.pan), 0.5));
        assert!(close(origin(&params.osc1.octave), 0.5));
        assert!(close(origin(&params.osc1.multiplier), 0.5));
        assert_eq!(origin(&params.filter.cutoff), 0.0);
        // (A range that ends at zero starts at its bottom)
        assert_eq!(origin(&params.global.output_gain), 0.0);
    }

    #[test]
    fn the_modulation_span_covers_every_routes_swing() {
        let params = ThreeOscParams::default();
        let span = |param: &FloatParam, target: ModTarget, routes: &[(bool, f32)]| {
            modulation_span(param, target.scaling(), routes.iter().copied())
        };
        assert_eq!(span(&params.filter.cutoff, ModTarget::Cutoff, &[]), None);

        // A route that swings both ways (an LFO's, say): from the top of the cutoff's range, 5
        // octaves down and up (past the top)
        let cutoff = &params.filter.cutoff;
        let (low, high) = span(cutoff, ModTarget::Cutoff, &[(true, 0.5)]).unwrap();
        assert!(close(low, cutoff.preview_normalized(cutoff.value() / 32.0)) && close(high, 1.0));

        // One way only (an envelope's, or a unipolar LFO's), even at a negative amount: Osc 1
        // is at 100%
        let amp = &params.osc1.amp;
        let (low, high) = span(amp, ModTarget::Osc1Amp, &[(false, -0.5)]).unwrap();
        assert!(close(low, 0.5) && close(high, 1.0));

        // And the routes add up: from the middle of the pan, a quarter either way plus half more
        // up, of its whole range
        let pan = &params.osc1.pan;
        let routes = [(true, 0.25), (false, 0.5)];
        let (low, high) = span(pan, ModTarget::Osc1Pan, &routes).unwrap();
        assert!(close(low, 0.25) && close(high, 1.0), "{low} {high}");

        // A synced LFO's rate knob moves through about three note lengths an octave
        let lfo = &crate::params::LfoParams {
            sync: BoolParam::new("Sync", true),
            ..crate::params::LfoParams::new()
        };
        let routes = [(false, 0.25)];
        let steps = catalog::lfo_rate(lfo, 0);
        let (low, high) = modulation_span(&lfo.division, steps.scaling.unwrap(), routes.iter().copied()).unwrap();
        assert!(close(low, lfo.division.unmodulated_normalized_value()), "{low}");
        assert!(close(high, lfo.division.preview_normalized(lfo.division.value() + 3)), "{high}");
    }
}
