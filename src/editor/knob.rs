//! The Studio layout's rotary control for a parameter: a knob with its name above it and its
//! value below.
//!
//! Dragging a knob up (or to the right) turns it up, ten times finer with Shift held; the mouse
//! wheel steps it; a double-click (or Ctrl/Cmd+click) puts it back to its default; and clicking
//! its value lets one be typed in. Right-clicking it opens a menu for all of that and for the
//! modulation routes that move it.
//!
//! A knob that can be modulated is a drop target for the modulation sources' handles. Around it
//! an arc shows how far its routes can move it, with a dot where the newest note's modulation
//! has it, and it lights up while the handle of a source that modulates it is hovered.

use std::f32::consts::PI;

use nih_plug::prelude::*;
use nih_plug_egui::egui::text::{CCursor, CCursorRange};
use nih_plug_egui::egui::text_edit::TextEditState;
use nih_plug_egui::egui::{self, Color32, FontId, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, Ui, Vec2};
use nih_plug_egui::widgets::ParamSlider;

use super::{set_param, Matrix, NumericParam, MOD_COLOR};
use crate::synth::modulation::{ModSource, ModTarget, N_SOURCES};

/// What the knobs need besides their parameters: a way to set them, the modulation matrix, and
/// the modulation source whose routes to light up (the one whose handle is hovered).
pub struct Controls<'a> {
    pub setter: &'a ParamSetter<'a>,
    pub matrix: &'a Matrix<'a>,
    pub highlight: Option<ModSource>,
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

const LABEL_HEIGHT: f32 = 15.0;
const VALUE_HEIGHT: f32 = 17.0;
const MIN_DIAMETER: f32 = 26.0;
const MAX_DIAMETER: f32 = 44.0;
const FONT_SIZE: f32 = 11.5;

/// The height of a knob `width` wide, with its name and its value.
pub fn height(width: f32) -> f32 {
    LABEL_HEIGHT + diameter(width) + VALUE_HEIGHT + 2.0
}

fn diameter(width: f32) -> f32 {
    (width * 0.58).clamp(MIN_DIAMETER, MAX_DIAMETER)
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
    painter.add(Shape::line(points, stroke));
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

/// How far `routes` (each a source and its amount) can move a knob for `target`, from its value:
/// the knob positions at the sum of the routes' lowest and highest modulation, if there are
/// routes.
fn modulation_span<P: NumericParam>(
    param: &P,
    target: ModTarget,
    routes: impl Iterator<Item = (ModSource, f32)>,
) -> Option<(f32, f32)> {
    let (mut low, mut high, mut any) = (0.0, 0.0, false);
    for (source, amount) in routes {
        if source.is_bipolar() {
            low -= amount.abs();
            high += amount.abs();
        } else {
            low += amount.min(0.0);
            high += amount.max(0.0);
        }
        any = true;
    }
    let value = param.value_f32();
    let scaling = target.scaling();
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
            color,
        }
    }

    /// Makes the knob a target for modulation.
    pub fn modulated(mut self, target: ModTarget) -> Self {
        self.target = target;
        self
    }

    /// Adds the knob, `width` wide (and [`height()`] tall).
    pub fn show(self, ui: &mut Ui, width: f32, controls: &Controls) -> Response {
        let Self { param, label, help, target, color } = self;
        let setter = controls.setter;
        let diameter = diameter(width);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height(width)), Sense::hover());
        let id = ui.id().with(("knob", param.name()));
        let center = Pos2::new(rect.center().x, rect.top() + LABEL_HEIGHT + diameter / 2.0);
        // The name is part of what can be grabbed, and the value below is for typing one in
        let grab = Rect::from_min_max(rect.min, Pos2::new(rect.right(), center.y + diameter / 2.0 + 1.0));
        let value_rect = Rect::from_min_max(Pos2::new(rect.left(), grab.bottom()), rect.max);
        let response = ui.interact(grab, id, Sense::click_and_drag());
        let modulatable = target != ModTarget::None;
        let fine = ui.input(|input| input.modifiers.shift);

        // Dragging turns it from where it was, with the unrounded position kept so that stepped
        // parameters move evenly
        let drag_id = id.with("drag");
        if response.drag_started() {
            setter.begin_set_parameter(param);
            let start = param.unmodulated_normalized_value();
            ui.memory_mut(|memory| memory.data.insert_temp(drag_id, start));
        }
        if response.dragged() {
            let delta = response.drag_delta();
            let speed = if fine { FINE } else { 1.0 } / DRAG_PIXELS;
            let start = ui.memory(|memory| memory.data.get_temp::<f32>(drag_id));
            let position = (start.unwrap_or_else(|| param.unmodulated_normalized_value()) + (delta.x - delta.y) * speed)
                .clamp(0.0, 1.0);
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
        let editing = value_entry(ui, entry_id, value_rect, param, setter);
        if !editing {
            let value = ui.interact(value_rect, id.with("value"), Sense::click());
            if value.clicked() {
                start_entry(ui, entry_id, param);
            }
            if value.hovered() {
                ui.painter().rect_filled(value_rect.shrink2(egui::vec2(4.0, 1.0)), 3.0, ui.visuals().widgets.hovered.bg_fill);
                ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
            }
            let text = param.normalized_value_to_string(param.modulated_normalized_value(), true);
            let text_color = if response.dragged() || value.hovered() {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().text_color()
            };
            ui.painter().text(value_rect.center(), egui::Align2::CENTER_CENTER, text, FontId::proportional(FONT_SIZE), text_color);
            value.on_hover_text("Click to type in a value.");
        }

        paint(ui, &response, center, diameter, param, target, color, controls);
        let label_color = if response.hovered() || response.dragged() {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        ui.painter().text(
            Pos2::new(rect.center().x, rect.top() + LABEL_HEIGHT / 2.0),
            egui::Align2::CENTER_CENTER,
            label,
            FontId::proportional(FONT_SIZE),
            label_color,
        );

        if response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        context_menu(&response, entry_id, param, target, controls);
        // (Not while it's being turned, when it would only get in the way)
        if response.dragged() {
            response
        } else {
            let routes: Vec<String> = controls
                .matrix
                .routes_for(target)
                .map(|(source, amount)| format!("{} {:+.0}%", ModSource::variants()[source.to_index()], amount * 100.0))
                .collect();
            response.on_hover_ui(|ui| {
                ui.label(RichText::new(param.name()).strong());
                ui.label(help);
                if !routes.is_empty() {
                    ui.label(RichText::new(format!("Modulated by {}.", routes.join(", "))).color(MOD_COLOR));
                }
                if modulatable {
                    ui.label(RichText::new("Right-click to edit its modulation.").weak());
                }
            })
        }
    }
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
fn value_entry<P: Param>(ui: &mut Ui, entry_id: egui::Id, rect: Rect, param: &P, setter: &ParamSetter) -> bool {
    let Some(mut text) = ui.memory(|memory| memory.data.get_temp::<String>(entry_id)) else {
        return false;
    };
    let field = entry_id.with("field");
    let edit = ui.put(
        rect,
        egui::TextEdit::singleline(&mut text)
            .id(field)
            .font(FontId::proportional(FONT_SIZE))
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

/// Draws a knob: its track, the arc from its origin to its value, its pointer, and its
/// modulation around it.
#[allow(clippy::too_many_arguments)]
fn paint<P: NumericParam>(
    ui: &Ui,
    response: &Response,
    center: Pos2,
    diameter: f32,
    param: &P,
    target: ModTarget,
    color: Color32,
    controls: &Controls,
) {
    let painter = ui.painter();
    let visuals = ui.visuals();
    let radius = diameter / 2.0;
    let ring = radius - 2.0;
    let active = response.hovered() || response.dragged();
    let normalized = param.modulated_normalized_value();

    // Lit while a source is dragged over it (and dimly while one is dragged at all), or while
    // the handle of a source that modulates it is hovered
    if target != ModTarget::None {
        let dragged = egui::DragAndDrop::has_payload_of_type::<ModSource>(ui.ctx());
        let lit = response.dnd_hover_payload::<ModSource>().is_some()
            || controls
                .highlight
                .is_some_and(|source| controls.matrix.routes_for(target).any(|(routed, _)| routed == source));
        if dragged || lit {
            let stroke_color = if lit { MOD_COLOR } else { MOD_COLOR.gamma_multiply(0.35) };
            painter.circle_stroke(center, radius + 5.0, Stroke::new(1.5_f32, stroke_color));
        }
    }

    arc(painter, center, ring, 0.0, 1.0, Stroke::new(3.0_f32, visuals.extreme_bg_color));
    let value_color = if active { color } else { color.gamma_multiply(0.85) };
    arc(painter, center, ring, origin(param), normalized, Stroke::new(3.0_f32, value_color));
    let body = if active { visuals.widgets.hovered.bg_fill } else { visuals.widgets.inactive.bg_fill };
    painter.circle_filled(center, radius - 5.5, body);
    let direction = Vec2::angled(angle(normalized));
    painter.line_segment(
        [center + direction * (radius * 0.2), center + direction * (radius - 7.0)],
        Stroke::new(2.0_f32, visuals.strong_text_color()),
    );

    if target == ModTarget::None {
        return;
    }
    let matrix = controls.matrix;
    if let Some((low, high)) = modulation_span(param, target, matrix.routes_for(target)) {
        let outer = radius + 1.5;
        arc(painter, center, outer, low, high, Stroke::new(2.0_f32, MOD_COLOR));
        painter.circle_filled(center + outer * Vec2::angled(angle(normalized)), 2.0, MOD_COLOR);
        if let Some(modulation) = matrix.monitor.modulation(target) {
            let value = param.value_f32();
            let at = param.normalized_at(target.scaling().apply(value, modulation));
            let dot = center + outer * Vec2::angled(angle(at));
            painter.circle(dot, 2.5, LIVE_COLOR, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
        }
    }
}

/// A knob's right-click menu: resetting it, typing in a value, and its modulation routes, with
/// their amounts, a way to remove them and a way to add another.
fn context_menu<P: NumericParam>(
    response: &Response,
    entry_id: egui::Id,
    param: &P,
    target: ModTarget,
    controls: &Controls,
) {
    let setter = controls.setter;
    response.context_menu(|ui| {
        ui.set_min_width(240.0);
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
        ui.separator();
        ui.label(RichText::new("Modulation").color(MOD_COLOR));
        let matrix = controls.matrix;
        let mut routed = [false; N_SOURCES];
        egui::Grid::new("routes").num_columns(3).spacing([8.0, 4.0]).show(ui, |ui| {
            for slot in matrix.slots_for(target) {
                let source = slot.source.value();
                routed[source.to_index()] = true;
                ui.label(ModSource::variants()[source.to_index()]).on_hover_text(source.help());
                ui.add(ParamSlider::for_param(&slot.amount, setter).with_width(110.0))
                    .on_hover_text(super::help::MOD_AMOUNT);
                if ui.small_button("Remove").clicked() {
                    Matrix::clear(setter, slot);
                }
                ui.end_row();
            }
        });
        if !routed.contains(&true) {
            ui.label(RichText::new("Nothing modulates it yet: drag a source's handle onto the knob, or add one here.").weak());
        }
        let free = matrix.slots.iter().any(|slot| !Matrix::is_used(slot));
        ui.add_enabled_ui(free, |ui| {
            ui.menu_button("Add a source", |ui| {
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
        let span = |param: &FloatParam, target: ModTarget, routes: &[(ModSource, f32)]| {
            modulation_span(param, target, routes.iter().copied())
        };
        assert_eq!(span(&params.filter.cutoff, ModTarget::Cutoff, &[]), None);

        // An LFO swings both ways: from the top of the cutoff's range, 5 octaves down and up
        // (past the top)
        let cutoff = &params.filter.cutoff;
        let (low, high) = span(cutoff, ModTarget::Cutoff, &[(ModSource::Lfo1, 0.5)]).unwrap();
        assert!(close(low, cutoff.preview_normalized(cutoff.value() / 32.0)) && close(high, 1.0));

        // An envelope only one way, even at a negative amount: Osc 1 is at 100%
        let amp = &params.osc1.amp;
        let (low, high) = span(amp, ModTarget::Osc1Amp, &[(ModSource::Env1, -0.5)]).unwrap();
        assert!(close(low, 0.5) && close(high, 1.0));

        // And the routes add up: from the middle of the pan, a quarter either way plus half more
        // up, of its whole range
        let pan = &params.osc1.pan;
        let routes = [(ModSource::Lfo2, 0.25), (ModSource::Velocity, 0.5)];
        let (low, high) = span(pan, ModTarget::Osc1Pan, &routes).unwrap();
        assert!(close(low, 0.25) && close(high, 1.0), "{low} {high}");
    }
}
