//! Small displays of what the controls do: each oscillator's wave, each LFO's shape, the
//! envelopes and the filter's frequency response. They are drawn with the synth's own code (the
//! wavetables' spectra, the LFO generator, the envelope curve and the filters themselves), so
//! they show what is played.

use nih_plug_egui::egui::{self, Color32, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

use nih_plug::prelude::{FloatParam, Param, ParamSetter};

use crate::params::EnvelopeParams;
use crate::synth::envelopes::AdsrEnvelope;
use crate::synth::filter::{
    ladder_oversampling, Filter, FilterContainer, FilterController, FilterModel, FilterType, LadderFilter,
    LADDER_LANES,
};
use crate::synth::oscillator::{OscWave, WavePreview};

/// The displays' colours, which the theme sets (see [`set_palette()`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// The displays' background, and their edge.
    pub screen: Color32,
    pub edge: Color32,
    /// The oscillators', the filter's and the effects' curves. (The envelopes and LFOs, which
    /// are modulation sources, are drawn in the modulation colour.)
    pub signal: Color32,
    /// What moves with the notes that are playing: where each is in its envelopes, its LFOs'
    /// phases, its filter's response.
    pub live: Color32,
    /// The guide lines, and the labels on them.
    pub guide: Color32,
    pub label: Color32,
}

impl Palette {
    /// Dark screens with blue curves, as the Classic theme has them.
    pub const DARK: Self = Self {
        screen: Color32::from_gray(10),
        edge: Color32::from_gray(60),
        signal: Color32::from_rgb(90, 180, 255),
        live: Color32::from_rgb(255, 238, 205),
        guide: Color32::from_gray(60),
        label: Color32::from_gray(110),
    };
}

fn palette_id() -> egui::Id {
    egui::Id::new("display palette")
}

/// Sets the displays' colours.
pub fn set_palette(ctx: &egui::Context, palette: Palette) {
    ctx.data_mut(|data| data.insert_temp(palette_id(), palette));
}

/// The displays' colours.
pub fn palette(ctx: &egui::Context) -> Palette {
    ctx.data(|data| data.get_temp(palette_id())).unwrap_or(Palette::DARK)
}

/// The heights the Classic layout gives the displays.
pub const WAVE_HEIGHT: f32 = 50.0;
/// The LFO displays sit beside their two rows of controls.
pub const LFO_HEIGHT: f32 = 46.0;
pub const ENVELOPE_HEIGHT: f32 = 46.0;
pub const FILTER_HEIGHT: f32 = 76.0;

/// A display's painter (clipped to it), and the colours to paint with.
struct Canvas {
    painter: Painter,
    palette: Palette,
}

impl std::ops::Deref for Canvas {
    type Target = Painter;
    fn deref(&self) -> &Painter {
        &self.painter
    }
}

impl Canvas {
    /// A dot for a note that is playing, at a point of the display.
    fn live_dot(&self, at: Pos2) {
        self.circle_filled(at, 3.5, self.palette.live);
        self.circle_stroke(at, 3.5, Stroke::new(1.0_f32, Color32::from_black_alpha(160)));
    }

    /// A faint guide line across or down the display.
    fn guide(&self, from: Pos2, to: Pos2) {
        self.line_segment([from, to], Stroke::new(1.0_f32, self.palette.guide));
    }
}

/// Allocates a display `height` pixels tall across the available width and draws its
/// background. Returns the display's response (for its hover text), what to paint it with and
/// the area to plot in.
fn frame(ui: &mut Ui, height: f32) -> (egui::Response, Canvas, Rect) {
    frame_sensing(ui, height, Sense::hover())
}

/// A [`frame()`] that senses what `sense` says, for displays that can be dragged.
fn frame_sensing(ui: &mut Ui, height: f32, sense: Sense) -> (egui::Response, Canvas, Rect) {
    let size = egui::vec2(ui.available_width(), height);
    let (response, painter) = ui.allocate_painter(size, sense);
    let rect = response.rect;
    let palette = palette(ui.ctx());
    painter.rect_filled(rect, 3.0, palette.screen);
    painter.rect_stroke(rect, 3.0, Stroke::new(1.0_f32, palette.edge), StrokeKind::Inside);
    let plot = rect.shrink2(egui::vec2(4.0, 5.0));
    let canvas = Canvas {
        painter: painter.with_clip_rect(rect.shrink(1.0)),
        palette,
    };
    (response, canvas, plot)
}

/// Draws a curve through `points`, from left to right, with the area between it and the
/// horizontal line at `base` shaded.
fn curve(painter: &Painter, points: Vec<Pos2>, base: f32, color: Color32, width: f32) {
    let fill = color.gamma_multiply(0.15);
    let mut mesh = Mesh::default();
    for (i, point) in points.iter().enumerate() {
        mesh.colored_vertex(*point, fill);
        mesh.colored_vertex(Pos2::new(point.x, base), fill);
        if i > 0 {
            let top = 2 * i as u32;
            mesh.add_triangle(top - 2, top - 1, top);
            mesh.add_triangle(top - 1, top, top + 1);
        }
    }
    painter.add(Shape::mesh(mesh));
    painter.add(Shape::line(points, Stroke::new(width, color)));
}

/// The x coordinate of a fraction of the way across `rect`.
fn x_at(rect: Rect, fraction: f32) -> f32 {
    rect.left() + fraction * rect.width()
}

/// One cycle of an oscillator's wave at its wavetable position, starting where notes start
/// (its Phase, in cycles), so the wave moves along as the Phase does. When a random part of a
/// cycle is added to each note's start (Phase Rand, short of a whole cycle), the stretch the
/// notes start in is shaded. A silent oscillator is drawn dimmed.
pub fn wave(ui: &mut Ui, preview: &WavePreview, position: f32, phase: f32, phase_rand: f32, silent: bool, height: f32) {
    let (response, painter, rect) = frame(ui, height);
    response.on_hover_text(
        "One cycle of the wave, at the wavetable position, from where notes start (the Phase). \
         The shaded stretch is where Phase Rand lets them start.",
    );
    let centre = rect.center().y;
    painter.guide(Pos2::new(rect.left(), centre), Pos2::new(rect.right(), centre));
    let signal = painter.palette.signal;
    if phase_rand > 0.0 && phase_rand < 1.0 {
        let random = Rect::from_x_y_ranges(rect.left()..=x_at(rect, phase_rand), rect.y_range());
        painter.rect_filled(random, 0.0, signal.gamma_multiply(0.08));
    }
    let cycle = preview.cycle(position);
    // From the phase on, interpolated between the cycle's samples, so that the wave moves
    // smoothly with it, and round to where the next cycle starts
    let len = cycle.len();
    let start = phase.rem_euclid(1.0) * len as f32;
    let points = (0..=len)
        .map(|i| {
            let at = start + i as f32;
            let (index, fraction) = (at.floor() as usize, at.fract());
            let sample = cycle[index % len] * (1.0 - fraction) + cycle[(index + 1) % len] * fraction;
            Pos2::new(x_at(rect, i as f32 / len as f32), centre - sample * rect.height() / 2.0)
        })
        .collect();
    let color = if silent { signal.gamma_multiply(0.35) } else { signal };
    curve(&painter, points, centre, color, 1.5);
}

/// One cycle of an LFO's waveform, with a dot where the newest note's copy of it is (`phase`, in
/// cycles).
pub fn lfo(ui: &mut Ui, wave: &OscWave, color: Color32, phase: Option<f32>, height: f32) {
    let (response, painter, rect) = frame(ui, height);
    response.on_hover_text(
        "One cycle of the LFO, from -100% to 100%. The dot is where the newest note's copy of it is.",
    );
    let centre = rect.center().y;
    painter.guide(Pos2::new(rect.left(), centre), Pos2::new(rect.right(), centre));
    let steps = (rect.width() as usize).max(2) * 2;
    let points = (0..=steps)
        .map(|step| {
            let phase = step as f32 / steps as f32;
            // The end of the cycle is the start of the next one
            let value = wave.generate(if step == steps { 0.0 } else { phase });
            Pos2::new(x_at(rect, phase), centre - value * rect.height() / 2.0)
        })
        .collect();
    curve(&painter, points, centre, color, 1.5);
    if let Some(phase) = phase {
        let phase = phase.rem_euclid(1.0);
        painter.live_dot(Pos2::new(x_at(rect, phase), centre - wave.generate(phase) * rect.height() / 2.0));
    }
}

/// An envelope's shape as the envelope display draws it: the attack, the decay down to the
/// sustain level (a point, where held notes wait) and the release. Each stage is as wide as the
/// square root of its time times a scale, which fits the stages to the width (except while a
/// point is being dragged, when it stays as it was, so that the point follows the pointer), so
/// that short stages stay visible next to long ones; within a stage, time runs evenly, so the
/// curves have their true shapes.
struct EnvelopeShape<'a> {
    envelope: &'a AdsrEnvelope,
    /// A stage's width (as a fraction of the display's) per square root of a second.
    scale: f32,
    /// The attack's, the decay's and the release's widths.
    widths: [f32; 3],
}

impl<'a> EnvelopeShape<'a> {
    /// When the decay or the release falls through less than this share of the height, its
    /// slope handle works over the whole height instead, so that it stays in reach.
    const MIN_SPAN: f32 = 0.1;

    fn roots(envelope: &AdsrEnvelope) -> [f32; 3] {
        [envelope.attack_time, envelope.decay_time, envelope.release_time].map(|time| time.max(0.0).sqrt())
    }

    /// The shape fitted to the width.
    fn new(envelope: &'a AdsrEnvelope) -> Self {
        let total: f32 = Self::roots(envelope).iter().sum();
        Self::with_scale(envelope, if total > 0.0 { 1.0 / total } else { 0.0 })
    }

    fn with_scale(envelope: &'a AdsrEnvelope, scale: f32) -> Self {
        Self {
            envelope,
            scale,
            widths: Self::roots(envelope).map(|root| root * scale),
        }
    }

    fn decay_start(&self) -> f32 {
        self.widths[0]
    }

    /// Where the sustain point is, and the release starts.
    fn release_start(&self) -> f32 {
        self.widths[0] + self.widths[1]
    }

    fn end(&self) -> f32 {
        self.release_start() + self.widths[2]
    }

    /// How long a stage this wide lasts, in seconds.
    fn time_for(&self, width: f32) -> f32 {
        if self.scale > 0.0 {
            (width.max(0.0) / self.scale).powi(2)
        } else {
            0.0
        }
    }

    /// Where a held note is across the width, `time` seconds into it: in the sustain, at the
    /// sustain point.
    fn held_fraction(&self, time: f32) -> f32 {
        let envelope = self.envelope;
        let (attack, decay) = (envelope.attack_time, envelope.decay_time);
        if time < attack {
            self.widths[0] * time / attack
        } else if time - attack < decay {
            self.decay_start() + self.widths[1] * (time - attack) / decay
        } else {
            self.release_start()
        }
    }

    /// Where a note is on the display, `time` seconds into it and released at `released`
    /// seconds (if it has been): the fraction of the width and its level. A released note
    /// carries on from where it was when it was released, across as far as the release is
    /// wide and down from its own level (so a note released before the sustain falls on its
    /// own, lower, curve). `None` once the release is over.
    fn locate(&self, time: f32, released: Option<f32>) -> Option<(f32, f32)> {
        let envelope = self.envelope;
        match released {
            None => Some((self.held_fraction(time), envelope.sample_held(time))),
            Some(released) => {
                let since = time - released;
                (since < envelope.release_time).then(|| {
                    let fraction = self.held_fraction(released) + self.widths[2] * since / envelope.release_time;
                    (fraction, envelope.sample_released(released, time))
                })
            }
        }
    }

    /// The envelope's level at a fraction of the width, worked out by the synth's envelope.
    fn level(&self, fraction: f32) -> f32 {
        let envelope = self.envelope;
        let (attack, decay) = (envelope.attack_time, envelope.decay_time);
        let (decay_start, release_start) = (self.decay_start(), self.release_start());
        if fraction < decay_start {
            envelope.sample_held(attack * fraction / decay_start)
        } else if fraction < release_start {
            envelope.sample_held(attack + decay * (fraction - decay_start) / self.widths[1])
        } else if self.widths[2] <= 0.0 {
            envelope.sample_held(attack + decay)
        } else {
            let released = attack + decay;
            let into = ((fraction - release_start) / self.widths[2]).min(1.0);
            envelope.sample_released(released, released + envelope.release_time * into)
        }
    }

    /// Whether a slope handle's curve falls through too little of the height to bend, so that
    /// the handle works over the whole height instead (see [`decay_span()`]).
    ///
    /// [`decay_span()`]: Self::decay_span()
    fn spans_whole_height(&self, handle: EnvelopeHandle) -> bool {
        let sustain = self.envelope.sustain_level;
        match handle {
            EnvelopeHandle::DecaySlope => 1.0 - sustain < Self::MIN_SPAN,
            EnvelopeHandle::ReleaseSlope => sustain < Self::MIN_SPAN,
            _ => false,
        }
    }

    /// The curve a slope handle that works over the whole height bends, from the top of the
    /// height to the bottom across its stage: the fraction of the width and the level at
    /// `steps` points.
    fn whole_height_curve(&self, handle: EnvelopeHandle, steps: usize) -> Vec<(f32, f32)> {
        let envelope = self.envelope;
        let (start, width, exponent) = match handle {
            EnvelopeHandle::DecaySlope => (self.decay_start(), self.widths[1], envelope.decay_slope),
            _ => (self.release_start(), self.widths[2], envelope.release_slope),
        };
        (0..=steps)
            .map(|step| {
                let into = step as f32 / steps as f32;
                (start + width * into, (1.0 - into).powf(exponent))
            })
            .collect()
    }

    /// The levels the decay's slope handle moves between: the decay's, or the whole height
    /// when that is too small to reach.
    fn decay_span(&self) -> (f32, f32) {
        let sustain = self.envelope.sustain_level;
        if 1.0 - sustain < Self::MIN_SPAN {
            (1.0, 0.0)
        } else {
            (1.0, sustain)
        }
    }

    /// The levels the release's slope handle moves between, like [`decay_span()`].
    ///
    /// [`decay_span()`]: Self::decay_span()
    fn release_span(&self) -> (f32, f32) {
        let sustain = self.envelope.sustain_level;
        if sustain < Self::MIN_SPAN {
            (1.0, 0.0)
        } else {
            (sustain, 0.0)
        }
    }

    /// Where a handle is: the fraction of the width and the level. A slope handle sits on its
    /// stage's curve, halfway through the stage.
    fn handle(&self, handle: EnvelopeHandle) -> (f32, f32) {
        let envelope = self.envelope;
        // Halfway through, a stage's curve has gone 0.5 to the power of its exponent of the way
        let halfway = |exponent: f32| 0.5_f32.powf(exponent);
        let between = |(top, bottom): (f32, f32), exponent: f32| bottom + (top - bottom) * halfway(exponent);
        match handle {
            EnvelopeHandle::Attack => (self.decay_start(), 1.0),
            EnvelopeHandle::Sustain => (self.release_start(), envelope.sustain_level),
            EnvelopeHandle::Release => (self.end(), 0.0),
            EnvelopeHandle::AttackSlope => (self.widths[0] / 2.0, halfway(envelope.attack_slope)),
            EnvelopeHandle::DecaySlope => (
                self.decay_start() + self.widths[1] / 2.0,
                between(self.decay_span(), envelope.decay_slope),
            ),
            EnvelopeHandle::ReleaseSlope => (
                self.release_start() + self.widths[2] / 2.0,
                between(self.release_span(), envelope.release_slope),
            ),
        }
    }
}

/// The points of an envelope display that can be dragged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvelopeHandle {
    /// The top of the attack: its time.
    Attack,
    /// The sustain point: the decay's time across, and the sustain level up and down.
    Sustain,
    /// The end of the release: its time.
    Release,
    /// The middle of a stage's curve: its slope.
    AttackSlope,
    DecaySlope,
    ReleaseSlope,
}

impl EnvelopeHandle {
    /// The points first, which win over the slope handles when they overlap.
    const ALL: [Self; 6] = [
        Self::Attack,
        Self::Sustain,
        Self::Release,
        Self::AttackSlope,
        Self::DecaySlope,
        Self::ReleaseSlope,
    ];

    fn is_slope(self) -> bool {
        matches!(self, Self::AttackSlope | Self::DecaySlope | Self::ReleaseSlope)
    }

    /// The parameters it moves.
    fn params(self, params: &EnvelopeParams) -> Vec<&FloatParam> {
        match self {
            Self::Attack => vec![&params.attack],
            Self::Sustain => vec![&params.decay, &params.sustain],
            Self::Release => vec![&params.release],
            Self::AttackSlope => vec![&params.attack_slope],
            Self::DecaySlope => vec![&params.decay_slope],
            Self::ReleaseSlope => vec![&params.release_slope],
        }
    }

    fn cursor(self) -> egui::CursorIcon {
        match self {
            Self::Attack | Self::Release => egui::CursorIcon::ResizeHorizontal,
            Self::Sustain => egui::CursorIcon::Move,
            _ => egui::CursorIcon::ResizeVertical,
        }
    }
}

/// A drag of an envelope display's handle, kept in egui's memory while it lasts.
#[derive(Debug, Clone, Copy)]
struct EnvelopeDrag {
    handle: EnvelopeHandle,
    /// The shape's scale when the drag started, kept so that the dragged point follows the
    /// pointer.
    scale: f32,
    /// Where the drag started, and the dragged slope's value then.
    start: Pos2,
    start_slope: f32,
}

/// How far a pixel of dragging moves a slope.
const SLOPE_PER_PIXEL: f32 = 0.04;
/// Stages narrower than this many pixels don't show their slope handles, which would sit on
/// top of the points.
const MIN_BENDABLE_PIXELS: f32 = 6.0;
/// How close to a handle the pointer has to be to take hold of it, in pixels.
const GRAB_RADIUS: f32 = 7.0;

/// An envelope's shape, see [`EnvelopeShape`], with a dot for each note playing (its time into
/// the note and the time it was released at, if it has been, in seconds), and handles to drag:
/// the attack's top, the sustain point and the release's end set the times and the sustain
/// level, and a handle in the middle of each stage's curve bends it (its slope). The changes
/// go through the host, as moving the sliders would.
pub fn envelope(
    ui: &mut Ui,
    envelope: &AdsrEnvelope,
    color: Color32,
    notes: &[(f32, Option<f32>)],
    params: &EnvelopeParams,
    setter: &ParamSetter,
    height: f32,
) {
    let (response, painter, rect) = frame_sensing(ui, height, Sense::click_and_drag());
    let drag_id = response.id.with("drag");
    let drag: Option<EnvelopeDrag> = ui.memory(|memory| memory.data.get_temp(drag_id));
    let shape = match drag {
        Some(drag) => EnvelopeShape::with_scale(envelope, drag.scale),
        None => EnvelopeShape::new(envelope),
    };
    let bottom = rect.bottom();
    let to_screen = |(fraction, level): (f32, f32)| Pos2::new(x_at(rect, fraction), bottom - level * rect.height());
    // A slope handle shows when its stage is wide enough to bend, and the decay's when there's
    // a decay at all
    let bendable = |stage: usize| shape.widths[stage] * rect.width() >= MIN_BENDABLE_PIXELS;
    let shown = |handle: &EnvelopeHandle| match handle {
        EnvelopeHandle::AttackSlope => bendable(0),
        EnvelopeHandle::DecaySlope => bendable(1) && envelope.sustain_level < 0.999,
        EnvelopeHandle::ReleaseSlope => bendable(2),
        _ => true,
    };
    let nearest = |pointer: Pos2| {
        EnvelopeHandle::ALL
            .into_iter()
            .filter(shown)
            .map(|handle| (handle, to_screen(shape.handle(handle)).distance(pointer)))
            .filter(|(_, distance)| *distance <= GRAB_RADIUS)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(handle, _)| handle)
    };
    let hovered = response.hover_pos().and_then(nearest);

    // Dragging: a point follows the pointer across (and the sustain point up and down) at the
    // scale the drag started at, and a slope moves with how far the pointer has moved up or down
    if response.drag_started() {
        let origin = ui.input(|input| input.pointer.press_origin());
        if let Some((handle, start)) = origin.and_then(|origin| nearest(origin).map(|handle| (handle, origin))) {
            let start_slope = match handle {
                EnvelopeHandle::AttackSlope => params.attack_slope.value(),
                EnvelopeHandle::DecaySlope => params.decay_slope.value(),
                EnvelopeHandle::ReleaseSlope => params.release_slope.value(),
                _ => 0.0,
            };
            for param in handle.params(params) {
                setter.begin_set_parameter(param);
            }
            let drag = EnvelopeDrag { handle, scale: shape.scale, start, start_slope };
            ui.memory_mut(|memory| memory.data.insert_temp(drag_id, drag));
        }
    }
    if let (Some(drag), Some(pointer)) = (drag, response.interact_pointer_pos()) {
        let fraction = (pointer.x - rect.left()) / rect.width();
        let level = ((bottom - pointer.y) / rect.height()).clamp(0.0, 1.0);
        let set = |param: &FloatParam, value: f32| setter.set_parameter(param, param.preview_plain(param.preview_normalized(value)));
        // Down makes the decay and release punchier, up the attack
        let slope = |sign: f32| drag.start_slope + sign * (pointer.y - drag.start.y) * SLOPE_PER_PIXEL;
        match drag.handle {
            EnvelopeHandle::Attack => set(&params.attack, shape.time_for(fraction)),
            EnvelopeHandle::Sustain => {
                set(&params.decay, shape.time_for(fraction - shape.decay_start()));
                set(&params.sustain, level);
            }
            EnvelopeHandle::Release => set(&params.release, shape.time_for(fraction - shape.release_start())),
            EnvelopeHandle::AttackSlope => set(&params.attack_slope, slope(-1.0)),
            EnvelopeHandle::DecaySlope => set(&params.decay_slope, slope(1.0)),
            EnvelopeHandle::ReleaseSlope => set(&params.release_slope, slope(1.0)),
        }
    }
    if response.drag_stopped() {
        if let Some(drag) = drag {
            for param in drag.handle.params(params) {
                setter.end_set_parameter(param);
            }
            ui.memory_mut(|memory| memory.data.remove::<EnvelopeDrag>(drag_id));
        }
    }
    // A double click puts a handle's controls back to their defaults
    if response.double_clicked() {
        if let Some(handle) = hovered {
            for param in handle.params(params) {
                setter.begin_set_parameter(param);
                setter.set_parameter(param, param.default_plain_value());
                setter.end_set_parameter(param);
            }
        }
    }
    let active = drag.map(|drag| drag.handle).or(hovered);
    if let Some(handle) = active {
        ui.ctx().set_cursor_icon(handle.cursor());
    }
    // (Not while dragging, when it would only get in the way)
    if drag.is_none() {
        response.on_hover_text(
            "The envelope's shape: attack, decay down to the sustain level (where held notes \
             wait) and release. The stages are drawn as wide as the square roots of their times, \
             so that short ones stay visible next to long ones. Drag the points to set the times \
             and the sustain level, and the small handles in the middle of the stages to bend \
             their curves (their slopes: up for a punchier attack, down for a punchier decay or \
             release). Double-click one to reset it. The dots are the notes playing.",
        );
    }

    for boundary in [shape.decay_start(), shape.release_start()] {
        let x = x_at(rect, boundary);
        painter.guide(Pos2::new(x, rect.top()), Pos2::new(x, bottom));
    }
    // A point per pixel, plus the stage boundaries themselves for crisp corners
    let steps = (rect.width() as usize).max(2);
    let mut fractions: Vec<f32> = (0..=steps).map(|step| step as f32 / steps as f32).collect();
    fractions.extend([shape.decay_start(), shape.release_start(), shape.end()].into_iter().filter(|fraction| *fraction <= 1.0));
    fractions.sort_by(f32::total_cmp);
    let points = fractions.into_iter().map(|fraction| to_screen((fraction, shape.level(fraction)))).collect();
    curve(&painter, points, bottom, color, 1.5);
    // The curve a whole height slope handle bends, while it is hovered or dragged
    if let Some(handle) = active.filter(|handle| shape.spans_whole_height(*handle)) {
        let points = shape.whole_height_curve(handle, 32).into_iter().map(to_screen).collect();
        painter.add(Shape::line(points, Stroke::new(1.0_f32, color.gamma_multiply(0.5))));
    }
    for handle in EnvelopeHandle::ALL.into_iter().filter(shown) {
        let at = to_screen(shape.handle(handle));
        let lit = active == Some(handle);
        let radius = if handle.is_slope() { 2.5 } else { 3.5 };
        let fill = if lit { color } else { painter.palette.screen };
        painter.circle(at, radius + if lit { 1.0 } else { 0.0 }, fill, Stroke::new(1.0_f32, color));
    }
    for &(time, released) in notes {
        if let Some(located) = shape.locate(time, released) {
            painter.live_dot(to_screen(located));
        }
    }
}

/// The colour of an effect's display: dimmed while the effect is off.
fn effect_color(palette: &Palette, on: bool) -> Color32 {
    if on {
        palette.signal
    } else {
        palette.signal.gamma_multiply(0.35)
    }
}

/// The distortion's curve: the output level (up) for each input level (across), from -1 to 1,
/// at the drive and mix, with the clean straight line for comparison.
pub fn distortion(ui: &mut Ui, height: f32, drive_db: f32, mix: f32, on: bool) {
    let (response, painter, rect) = frame(ui, height);
    response.on_hover_text(
        "The distortion's curve: the output level (up) for each input level (across), from -1 to \
         1. The faint line is the clean sound.",
    );
    let centre = rect.center();
    painter.guide(Pos2::new(rect.left(), centre.y), Pos2::new(rect.right(), centre.y));
    painter.guide(Pos2::new(centre.x, rect.top()), Pos2::new(centre.x, rect.bottom()));
    painter.guide(rect.left_bottom(), rect.right_top());
    let gain = 10.0_f32.powf(drive_db * 0.05);
    let steps = (rect.width() as usize).max(2);
    let points = (0..=steps)
        .map(|step| {
            let input = step as f32 / steps as f32 * 2.0 - 1.0;
            let output = input + ((gain * input).tanh() - input) * mix;
            Pos2::new(x_at(rect, step as f32 / steps as f32), centre.y - output * rect.height() / 2.0)
        })
        .collect();
    painter.add(Shape::line(points, Stroke::new(1.5_f32, effect_color(&painter.palette, on))));
}

/// One cycle of the chorus's sweep: how the delay of each channel's copy moves (left solid,
/// right faint), at the depth.
pub fn chorus(ui: &mut Ui, height: f32, depth: f32, on: bool) {
    let (response, painter, rect) = frame(ui, height);
    response.on_hover_text(
        "One cycle of the chorus: how the delay of the left channel's copy (solid) and the \
         right's (faint) sweeps, from 6 to 18 ms at full depth.",
    );
    let centre = rect.center().y;
    painter.guide(Pos2::new(rect.left(), centre), Pos2::new(rect.right(), centre));
    let steps = (rect.width() as usize).max(2);
    let sweep = |offset: f32| -> Vec<Pos2> {
        (0..=steps)
            .map(|step| {
                let phase = step as f32 / steps as f32;
                let value = depth * (std::f32::consts::TAU * (phase + offset)).sin();
                Pos2::new(x_at(rect, phase), centre - value * rect.height() / 2.0)
            })
            .collect()
    };
    let color = effect_color(&painter.palette, on);
    painter.add(Shape::line(sweep(0.25), Stroke::new(1.0_f32, color.gamma_multiply(0.5))));
    painter.add(Shape::line(sweep(0.0), Stroke::new(1.5_f32, color)));
}

/// The delay's echoes over the first seconds: a line for the sound itself and one for each
/// echo, as high as its level. Ping-ponging, the left channel's go up and the right's down.
pub fn delay(ui: &mut Ui, height: f32, time: f32, feedback: f32, mix: f32, ping_pong: bool, on: bool) {
    let (response, painter, rect) = frame(ui, height);
    // Long enough for a few echoes of the longest times
    let span = (time * 6.0).max(1.0);
    response.on_hover_text(format!(
        "The sound (the first line) and its echoes over {span:.1} s, each as high as its level{}.",
        if ping_pong { ", the left channel's up and the right's down" } else { "" }
    ));
    let color = effect_color(&painter.palette, on);
    let (top, bottom) = (rect.top(), rect.bottom());
    let base = if ping_pong { rect.center().y } else { bottom };
    let height = if ping_pong { rect.height() / 2.0 } else { rect.height() };
    painter.guide(Pos2::new(rect.left(), base), Pos2::new(rect.right(), base));
    let tick = |painter: &Painter, at: f32, level: f32, up: bool, color: Color32| {
        let x = x_at(rect, at / span);
        let reach = level.clamp(0.0, 1.0) * height;
        let end = if up { (base - reach).max(top) } else { (base + reach).min(bottom) };
        painter.line_segment([Pos2::new(x, base), Pos2::new(x, end)], Stroke::new(2.0_f32, color));
    };
    tick(&painter, 0.0, 1.0 - mix, true, color.gamma_multiply(0.5));
    let mut level = mix;
    for echo in 1.. {
        let at = echo as f32 * time;
        if at > span || level < 1e-3 {
            break;
        }
        tick(&painter, at, level, !ping_pong || echo % 2 == 1, color);
        level *= feedback;
    }
}

/// The reverb's decay after a short sound, in decibels from 0 at the top to -60 at the bottom:
/// the lows (solid) and the highs (faint), which the damping makes die away sooner.
pub fn reverb(ui: &mut Ui, height: f32, size: f32, damping: f32, on: bool) {
    let (response, painter, rect) = frame(ui, height);
    // Freeverb's loop gain for the lows (see `effects::Reverb`), and for the highs through the
    // damping's lowpass at the Nyquist frequency, over the combs' average loop
    let feedback = 0.7 + 0.28 * size;
    let damp = 0.4 * damping;
    let highs = feedback * (1.0 - damp) / (1.0 + damp);
    let loop_seconds = 1366.0 / 44100.0;
    let decay_time = |gain: f32| 3.0 * loop_seconds / -gain.log10();
    let rt60 = decay_time(feedback);
    let span = rt60.max(0.5) * 1.1;
    response.on_hover_text(format!(
        "How the reverb dies away after a short sound, over {span:.1} s, from 0 dB (top) to -60 dB \
         (bottom): the lows (solid) take {rt60:.1} s, the highs (faint) {:.1} s.",
        decay_time(highs)
    ));
    let color = effect_color(&painter.palette, on);
    let steps = (rect.width() as usize).max(2);
    let curve_for = |gain: f32| -> Vec<Pos2> {
        (0..=steps)
            .map(|step| {
                let t = step as f32 / steps as f32 * span;
                let db = 20.0 * gain.log10() * t / loop_seconds;
                Pos2::new(x_at(rect, step as f32 / steps as f32), rect.top() + (-db / 60.0).min(1.0) * rect.height())
            })
            .collect()
    };
    painter.add(Shape::line(curve_for(highs), Stroke::new(1.0_f32, color.gamma_multiply(0.5))));
    painter.add(Shape::line(curve_for(feedback), Stroke::new(1.5_f32, color)));
}

/// What the filter display shows, and what it was measured for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterSettings {
    pub model: FilterModel,
    pub filter_type: FilterType,
    pub cutoff: f32,
    pub resonance: f32,
    /// The cutoff at the peak of the filter envelope, when the envelope moves it.
    pub envelope_cutoff: Option<f32>,
    /// The rate the voices' filters run at: the sample rate times the oversampling.
    pub sample_rate: f32,
    /// How many times that rate the Ladder filter runs at.
    pub ladder_oversampling: i32,
}

impl FilterSettings {
    /// The settings for a cutoff control, with the filter envelope's amount (as the synth uses
    /// it, squared) added at the envelope's peak, for voices running at `oversampling` times
    /// the sample rate.
    pub fn new(
        model: FilterModel,
        filter_type: FilterType,
        cutoff: f32,
        resonance: f32,
        envelope_amount: f32,
        sample_rate: f32,
        oversampling: usize,
    ) -> Self {
        let sample_rate = sample_rate * oversampling as f32;
        let mut controller = FilterController::new();
        controller.cutoff = cutoff;
        controller.envelope_amount = envelope_amount;
        let base = controller.get_cutoff(1.0, 0.0, sample_rate);
        let peak = controller.get_cutoff(1.0, 1.0, sample_rate);
        Self {
            model,
            filter_type,
            cutoff: base,
            resonance,
            envelope_cutoff: (peak > base * 1.01).then_some(peak),
            sample_rate,
            ladder_oversampling: ladder_oversampling(oversampling),
        }
    }
}

/// The lowest and highest frequencies the filter display shows.
const LOW_HZ: f32 = 20.0;
const HIGH_HZ: f32 = 20000.0;
/// The levels at the bottom and the top of the filter display.
const FLOOR_DB: f32 = -48.0;
const CEILING_DB: f32 = 24.0;
/// How many frequencies (evenly spaced in pitch) the response is worked out at.
const RESPONSE_POINTS: usize = 256;

/// The frequency of a point of the response, from [`LOW_HZ`] to [`HIGH_HZ`].
fn point_frequency(point: usize) -> f32 {
    LOW_HZ * (HIGH_HZ / LOW_HZ).powf(point as f32 / (RESPONSE_POINTS - 1) as f32)
}

/// Measures filter responses, keeping the last one until the settings change.
pub struct FilterResponse {
    settings: Option<FilterSettings>,
    /// The response at each point, in dB, and at the envelope's peak.
    levels: Vec<f32>,
    envelope_levels: Option<Vec<f32>>,
    /// The response at the newest note's cutoff and resonance, and what it was measured for.
    live: Option<(FilterSettings, Vec<f32>)>,
    fft: Option<Arc<dyn Fft<f32>>>,
}

impl FilterResponse {
    /// The length of the impulse responses that are measured.
    const LEN: usize = 16384;
    /// The size of the impulse, small enough that the nonlinear filters stay close to linear.
    const IMPULSE: f32 = 0.25;
    /// The responses are measured a block of samples at a time, and the measurement stops once
    /// a block shows that they have died away.
    const BLOCK: usize = 1024;

    pub fn new() -> Self {
        Self {
            settings: None,
            levels: Vec::new(),
            envelope_levels: None,
            live: None,
            fft: None,
        }
    }

    /// The response at a playing note's cutoff and resonance. A cutoff within a quarter of a
    /// semitone, and a resonance within a hundredth, of the last one's keeps that response
    /// rather than measuring again (once a frame at most, and at most half a millisecond, see
    /// `benchmark_filter_response()`).
    fn update_live(&mut self, settings: FilterSettings, cutoff: f32, resonance: f32) -> &[f32] {
        let cutoff = ((cutoff.max(1.0).log2() * 48.0).round() / 48.0).exp2();
        let resonance = (resonance * 100.0).round() / 100.0;
        let live = FilterSettings {
            cutoff,
            resonance,
            envelope_cutoff: None,
            ..settings
        };
        if self.live.as_ref().map(|(measured, _)| *measured) != Some(live) {
            let fft = self
                .fft
                .get_or_insert_with(|| FftPlanner::new().plan_fft_forward(Self::LEN))
                .clone();
            let levels = Self::measure(&live, &[cutoff], &*fft).remove(0);
            self.live = Some((live, levels));
        }
        self.live.as_ref().map(|(_, levels)| &levels[..]).unwrap_or_default()
    }

    fn update(&mut self, settings: FilterSettings) {
        if self.settings == Some(settings) {
            return;
        }
        let fft = self
            .fft
            .get_or_insert_with(|| FftPlanner::new().plan_fft_forward(Self::LEN))
            .clone();
        let cutoffs: Vec<f32> = std::iter::once(settings.cutoff).chain(settings.envelope_cutoff).collect();
        let mut levels = Self::measure(&settings, &cutoffs, &*fft).into_iter();
        self.levels = levels.next().unwrap_or_default();
        self.envelope_levels = levels.next();
        self.settings = Some(settings);
    }

    /// The response of the filter at each of the cutoffs, in dB at each point: the spectrum of
    /// the filter's response to an impulse, run through the synth's filter code. The nonlinear
    /// filters are run with a positive and a negative impulse and the difference is taken,
    /// which cancels their dither noise and their even order distortion.
    fn measure(settings: &FilterSettings, cutoffs: &[f32], fft: &dyn Fft<f32>) -> Vec<Vec<f32>> {
        if matches!(settings.model, FilterModel::None) {
            return vec![vec![0.0; RESPONSE_POINTS]; cutoffs.len()];
        }
        let sample_rate = settings.sample_rate;
        // A pair of filters per cutoff, for the positive and the negative impulse
        let mut filters: Vec<FilterContainer> = cutoffs
            .iter()
            .flat_map(|&cutoff| {
                let mut filter = FilterContainer::None;
                filter.set(settings.model, cutoff, settings.resonance, sample_rate, settings.filter_type);
                filter.set_ladder_oversampling(settings.ladder_oversampling);
                filter.set_filter_type(settings.filter_type);
                filter.set_params(sample_rate, cutoff, settings.resonance);
                [filter.clone(), filter]
            })
            .collect();

        // Tiny numbers are flushed to zero, as on the audio thread, rather than slowing the
        // decaying filters down to a crawl
        let _flush = FlushToZero::enable();
        let mut responses = vec![vec![0.0_f32; Self::LEN]; cutoffs.len()];
        let mut blocks = vec![[0.0_f32; Self::BLOCK]; filters.len()];
        let mut peaks = vec![0.0_f32; cutoffs.len()];
        for start in (0..Self::LEN).step_by(Self::BLOCK) {
            for (index, block) in blocks.iter_mut().enumerate() {
                block.fill(0.0);
                if start == 0 {
                    block[0] = if index % 2 == 0 { Self::IMPULSE } else { -Self::IMPULSE };
                }
            }
            process_blocks(&mut filters, &mut blocks);

            // The pairs' differences, until every response has died away
            let mut decayed = true;
            for (cutoff, response) in responses.iter_mut().enumerate() {
                let (positive, negative) = (&blocks[2 * cutoff], &blocks[2 * cutoff + 1]);
                let mut recent_peak = 0.0_f32;
                for (n, sample) in response[start..start + Self::BLOCK].iter_mut().enumerate() {
                    let difference = (positive[n] - negative[n]) / (2.0 * Self::IMPULSE);
                    *sample = if difference.is_finite() { difference } else { 0.0 };
                    recent_peak = recent_peak.max(sample.abs());
                }
                peaks[cutoff] = peaks[cutoff].max(recent_peak);
                decayed &= recent_peak < peaks[cutoff] * 1e-7;
            }
            if decayed {
                break;
            }
        }

        let bins_per_hz = Self::LEN as f32 / sample_rate;
        responses
            .into_iter()
            .map(|response| {
                // The second half of the response fades out, so that a filter that is still
                // ringing (or oscillating) at the end doesn't smear the whole spectrum
                let fade_start = Self::LEN / 2;
                let mut buffer: Vec<Complex32> = response
                    .iter()
                    .enumerate()
                    .map(|(n, sample)| {
                        let fade = if n < fade_start {
                            1.0
                        } else {
                            let t = (n - fade_start) as f32 / (Self::LEN - fade_start) as f32;
                            0.5 + 0.5 * (std::f32::consts::PI * t).cos()
                        };
                        Complex32::new(sample * fade, 0.0)
                    })
                    .collect();
                fft.process(&mut buffer);
                (0..RESPONSE_POINTS)
                    .map(|point| {
                        // Interpolated between the two nearest bins
                        let bin = point_frequency(point) * bins_per_hz;
                        let below = (bin as usize).min(Self::LEN / 2 - 1);
                        let fraction = (bin - below as f32).clamp(0.0, 1.0);
                        let magnitude = buffer[below].norm() * (1.0 - fraction) + buffer[below + 1].norm() * fraction;
                        20.0 * magnitude.max(1e-6).log10()
                    })
                    .collect()
            })
            .collect()
    }
}

/// Runs each filter over its block of samples, in place. Ladder filters run eight at a time
/// through the synth's lockstep kernel when the CPU can, which is several times quicker than
/// one at a time.
fn process_blocks(filters: &mut [FilterContainer], blocks: &mut [[f32; FilterResponse::BLOCK]]) {
    #[cfg(target_arch = "x86_64")]
    if is_x86_feature_detected!("avx2") && is_x86_feature_detected!("fma") {
        let mut ladders = Vec::new();
        let mut others = Vec::new();
        for (filter, block) in filters.iter_mut().zip(blocks.iter_mut()) {
            let lockstep = matches!(filter, FilterContainer::LadderFilter(ladder) if ladder.supports_lockstep());
            if !lockstep {
                others.push((filter, block));
            } else if let FilterContainer::LadderFilter(ladder) = filter {
                ladders.push((ladder, &mut block[..]));
            }
        }
        let mut ladders = ladders.into_iter();
        loop {
            let lanes: [_; LADDER_LANES] = std::array::from_fn(|_| ladders.next());
            if lanes[0].is_none() {
                break;
            }
            // SAFETY: AVX2 and FMA were just detected
            unsafe { LadderFilter::process_block_x8_fma(lanes) };
        }
        for (filter, block) in others {
            block.iter_mut().for_each(|sample| *sample = filter.process(*sample));
        }
        return;
    }
    for (filter, block) in filters.iter_mut().zip(blocks.iter_mut()) {
        block.iter_mut().for_each(|sample| *sample = filter.process(*sample));
    }
}

/// The controls a filter display can be dragged to set: the cutoff across, and the resonance up
/// and down.
pub struct FilterEdit<'a> {
    pub cutoff: &'a FloatParam,
    pub resonance: &'a FloatParam,
    pub setter: &'a ParamSetter<'a>,
}

/// A drag of a filter display, kept in egui's memory while it lasts: where it has taken the
/// cutoff, in Hz, and the resonance (from 0 to 1 across its range), before they are clamped.
#[derive(Debug, Clone, Copy)]
struct FilterDrag {
    cutoff: f32,
    resonance: f32,
}

/// How many pixels of dragging up or down move the resonance across its whole range.
const RESONANCE_DRAG_PIXELS: f32 = 160.0;

impl FilterEdit<'_> {
    /// Drags the cutoff and the resonance relative to where they were, so that grabbing the
    /// display anywhere doesn't make them jump: the cutoff moves as the display's frequencies
    /// do, and the resonance by its range per [`RESONANCE_DRAG_PIXELS`]. A double click resets
    /// them.
    fn interact(&self, ui: &Ui, response: &egui::Response, rect: Rect) {
        let drag_id = response.id.with("drag");
        let params = [self.cutoff, self.resonance];
        if response.drag_started() {
            for param in params {
                self.setter.begin_set_parameter(param);
            }
            let drag = FilterDrag {
                cutoff: self.cutoff.value(),
                resonance: self.resonance.unmodulated_normalized_value(),
            };
            ui.memory_mut(|memory| memory.data.insert_temp(drag_id, drag));
        }
        let drag: Option<FilterDrag> = ui.memory(|memory| memory.data.get_temp(drag_id));
        if let (true, Some(mut drag)) = (response.dragged(), drag) {
            let delta = response.drag_delta();
            drag.cutoff *= (HIGH_HZ / LOW_HZ).powf(delta.x / rect.width());
            drag.resonance -= delta.y / RESONANCE_DRAG_PIXELS;
            let cutoff = self.cutoff.preview_plain(self.cutoff.preview_normalized(drag.cutoff));
            self.setter.set_parameter(self.cutoff, cutoff);
            self.setter.set_parameter(self.resonance, self.resonance.preview_plain(drag.resonance.clamp(0.0, 1.0)));
            ui.memory_mut(|memory| memory.data.insert_temp(drag_id, drag));
        }
        if response.drag_stopped() {
            for param in params {
                self.setter.end_set_parameter(param);
            }
            ui.memory_mut(|memory| memory.data.remove::<FilterDrag>(drag_id));
        }
        if response.double_clicked() {
            for param in params {
                self.setter.begin_set_parameter(param);
                self.setter.set_parameter(param, param.default_plain_value());
                self.setter.end_set_parameter(param);
            }
        }
        if response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        } else if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }
}

/// The filter's frequency response, from 20 Hz to 20 kHz, with its cutoff marked, and the
/// response at the peak of the filter envelope drawn more faintly when the envelope moves the
/// cutoff. `live` is the newest note's cutoff and resonance, with its envelope, keytracking and
/// modulation: while a note plays, its response is drawn over the controls' and moves with it.
/// With `edit`, the display can be dragged to set the cutoff and the resonance, and has a
/// handle at the cutoff to show it.
pub fn filter(
    ui: &mut Ui,
    settings: FilterSettings,
    response: &mut FilterResponse,
    live: Option<(f32, f32)>,
    height: f32,
    edit: Option<FilterEdit>,
) {
    response.update(settings);
    let sense = if edit.is_some() { Sense::click_and_drag() } else { Sense::hover() };
    let (hover, painter, rect) = frame_sensing(ui, height, sense);
    if let Some(edit) = &edit {
        edit.interact(ui, &hover, rect);
    }
    // (Not while dragging, when it would only get in the way)
    if !hover.dragged() {
        hover.clone().on_hover_text(format!(
            "The filter's frequency response, from 20 Hz to 20 kHz (the line marks the cutoff), \
             measured from the filter itself with a quiet signal, so the drive's distortion \
             doesn't show. The fainter curve is the response at the peak of the filter envelope. \
             With keytracking, the cutoff is for the note A4. While notes play, the light curve \
             is the newest note's response, with its envelope, keytracking and modulation moving \
             it.{}",
            if edit.is_some() {
                " Drag it across to move the cutoff, and up or down for the resonance; \
                 double-click it to reset them."
            } else {
                ""
            }
        ));
    }
    let x_for = |hz: f32| x_at(rect, (hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln());
    let y_for = |db: f32| {
        let fraction = (db.clamp(FLOOR_DB, CEILING_DB) - FLOOR_DB) / (CEILING_DB - FLOOR_DB);
        rect.bottom() - fraction * rect.height()
    };
    // The level of a response at a frequency, at the point nearest to it
    let level_at = |levels: &[f32], hz: f32| {
        let point = ((hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln() * (RESPONSE_POINTS - 1) as f32).round();
        levels.get(point.clamp(0.0, (RESPONSE_POINTS - 1) as f32) as usize).copied().unwrap_or(0.0)
    };

    // A guide every decade and at 0 dB, labelled
    let label_color = painter.palette.label;
    let signal = painter.palette.signal;
    let font = egui::FontId::proportional(9.0);
    for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let x = x_for(hz);
        painter.guide(Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()));
        painter.text(Pos2::new(x + 2.0, rect.bottom()), egui::Align2::LEFT_BOTTOM, label, font.clone(), label_color);
    }
    let zero = y_for(0.0);
    painter.guide(Pos2::new(rect.left(), zero), Pos2::new(rect.right(), zero));
    painter.text(Pos2::new(rect.left() + 1.0, zero - 1.0), egui::Align2::LEFT_BOTTOM, "0 dB", font, label_color);

    // Frequencies above the Nyquist frequency don't exist at this sample rate
    let nyquist = settings.sample_rate / 2.0;
    let points = |levels: &[f32]| -> Vec<Pos2> {
        levels
            .iter()
            .enumerate()
            .take_while(|(point, _)| point_frequency(*point) < nyquist)
            .map(|(point, level)| Pos2::new(x_for(point_frequency(point)), y_for(*level)))
            .collect()
    };
    let filtered = !matches!(settings.model, FilterModel::None);
    let live = live.filter(|_| filtered);
    if let Some(levels) = &response.envelope_levels {
        painter.add(Shape::line(points(levels), Stroke::new(1.0_f32, signal.gamma_multiply(0.45))));
    }
    if filtered {
        let x = x_for(settings.cutoff.clamp(LOW_HZ, HIGH_HZ));
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0_f32, signal.gamma_multiply(0.5)),
        );
    }
    // The controls' response, behind the playing note's when there is one
    let levels = response.levels.clone();
    let color = if live.is_some() { signal.gamma_multiply(0.6) } else { signal };
    curve(&painter, points(&levels), rect.bottom(), color, 1.5);
    if edit.is_some() && filtered {
        let cutoff = settings.cutoff.clamp(LOW_HZ, HIGH_HZ);
        let at = Pos2::new(x_for(cutoff), y_for(level_at(&levels, cutoff)));
        let lit = hover.hovered() || hover.dragged();
        let fill = if lit { signal } else { painter.palette.screen };
        painter.circle(at, if lit { 5.0 } else { 4.0 }, fill, Stroke::new(1.5_f32, signal));
    }
    if let Some((cutoff, resonance)) = live {
        let levels = response.update_live(settings, cutoff, resonance).to_vec();
        painter.add(Shape::line(points(&levels), Stroke::new(1.5_f32, painter.palette.live)));
        // A dot on it at the note's cutoff
        let x = x_for(cutoff.clamp(LOW_HZ, HIGH_HZ));
        painter.live_dot(Pos2::new(x, y_for(level_at(&levels, cutoff))));
    }
}

/// Sets the CPU to flush denormal results to zero while it exists, as audio threads run (nih-plug
/// does the same around the plugin's processing), and restores the previous mode when dropped.
struct FlushToZero {
    previous: Option<u64>,
    /// It changes the current thread's floating point mode, so it must stay on this thread.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl FlushToZero {
    #[allow(unreachable_code)] // on the architectures with a flush to zero mode
    fn enable() -> Self {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            const FLUSH_TO_ZERO: u32 = 1 << 15;
            let mut csr: u32 = 0;
            // SAFETY: only sets the flush to zero bit of this thread's SSE control register
            unsafe { std::arch::asm!("stmxcsr [{}]", in(reg) &mut csr) };
            if csr & FLUSH_TO_ZERO != 0 {
                return Self { previous: None, _not_send: std::marker::PhantomData };
            }
            unsafe { std::arch::asm!("ldmxcsr [{}]", in(reg) &(csr | FLUSH_TO_ZERO)) };
            return Self { previous: Some(csr as u64), _not_send: std::marker::PhantomData };
        }
        #[cfg(target_arch = "aarch64")]
        {
            const FLUSH_TO_ZERO: u64 = 1 << 24;
            let fpcr: u64;
            // SAFETY: only sets the flush to zero bit of this thread's floating point control
            // register
            unsafe { std::arch::asm!("mrs {}, fpcr", out(reg) fpcr) };
            if fpcr & FLUSH_TO_ZERO != 0 {
                return Self { previous: None, _not_send: std::marker::PhantomData };
            }
            unsafe { std::arch::asm!("msr fpcr, {}", in(reg) fpcr | FLUSH_TO_ZERO) };
            return Self { previous: Some(fpcr), _not_send: std::marker::PhantomData };
        }
        Self { previous: None, _not_send: std::marker::PhantomData }
    }
}

impl Drop for FlushToZero {
    fn drop(&mut self) {
        let Some(_previous) = self.previous else {
            return;
        };
        // SAFETY: restores the control register as it was
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        unsafe {
            std::arch::asm!("ldmxcsr [{}]", in(reg) &(_previous as u32))
        };
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) _previous)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::oscillator::{builtin_spectra, CycleRenderer};

    /// The envelope display goes through the envelope's levels at the stage boundaries, with
    /// the sustain a point, and puts its handles at the corners and on the curves.
    #[test]
    fn envelope_shape_follows_the_envelope() {
        // Attack, decay, release, sustain, slope
        let envelope = AdsrEnvelope::new(0.01, 0.25, 0.3, 0.6, 1.0);
        let shape = EnvelopeShape::new(&envelope);
        let (decay_start, release_start) = (shape.decay_start(), shape.release_start());
        assert!(0.0 < decay_start && decay_start < release_start && release_start < 1.0);
        assert!((shape.end() - 1.0).abs() < 1e-6);
        // The stages are as wide as the square roots of their times, and fill the width
        let width = |time: f32| time.sqrt() / (0.01_f32.sqrt() + 0.25_f32.sqrt() + 0.3_f32.sqrt());
        assert!((decay_start - width(0.01)).abs() < 1e-6);
        assert!((release_start - decay_start - width(0.25)).abs() < 1e-6);
        assert!((shape.widths[2] - width(0.3)).abs() < 1e-6);
        assert!((shape.time_for(width(0.25)) - 0.25).abs() < 1e-5);

        // (The envelope's power function is approximate)
        assert!(shape.level(0.0) < 1e-6);
        assert!((shape.level(decay_start) - 1.0).abs() < 1e-4);
        assert!((shape.level(release_start) - 0.6).abs() < 1e-4);
        assert!(shape.level(1.0) < 1e-6);
        // The decay falls and the release falls
        assert!(shape.level(decay_start + 0.05) < 1.0);
        assert!(shape.level(release_start + 0.05) < 0.6);

        // The points are at the corners, and the slope handles on the curves, halfway across
        let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-5 && (a.1 - b.1).abs() < 1e-3;
        assert!(close(shape.handle(EnvelopeHandle::Attack), (decay_start, 1.0)));
        assert!(close(shape.handle(EnvelopeHandle::Sustain), (release_start, 0.6)));
        assert!(close(shape.handle(EnvelopeHandle::Release), (1.0, 0.0)));
        for (handle, start, end) in [
            (EnvelopeHandle::AttackSlope, 0.0, decay_start),
            (EnvelopeHandle::DecaySlope, decay_start, release_start),
            (EnvelopeHandle::ReleaseSlope, release_start, 1.0),
        ] {
            let (fraction, level) = shape.handle(handle);
            assert!((fraction - (start + end) / 2.0).abs() < 1e-6, "{handle:?}");
            assert!((shape.level(fraction) - level).abs() < 2e-3, "{handle:?}");
        }

        // Without a sustain to release from, the release's handle bends a curve over the whole
        // height, through the handle
        let plucked = AdsrEnvelope::new(0.0, 0.3, 0.2, 0.0, 1.0);
        let shape = EnvelopeShape::new(&plucked);
        assert!(shape.spans_whole_height(EnvelopeHandle::ReleaseSlope));
        assert!(!shape.spans_whole_height(EnvelopeHandle::DecaySlope));
        let curve = shape.whole_height_curve(EnvelopeHandle::ReleaseSlope, 32);
        assert!(close(curve[16], shape.handle(EnvelopeHandle::ReleaseSlope)));
        assert!(close(curve[0], (shape.release_start(), 1.0)) && close(curve[32], (shape.end(), 0.0)));

        // A zero length attack starts at the top, and without any stages it's all sustain
        let instant = AdsrEnvelope::new(0.0, 0.5, 0.1, 0.2, 1.0);
        assert!((EnvelopeShape::new(&instant).level(0.0) - 1.0).abs() < 1e-6);
        let flat = AdsrEnvelope::new(0.0, 0.0, 0.0, 0.7, 1.0);
        let shape = EnvelopeShape::new(&flat);
        for fraction in [0.0, 0.3, 1.0] {
            assert!((shape.level(fraction) - 0.7).abs() < 1e-6, "{fraction}");
        }
    }

    /// Notes move through the envelope display: a held one waits at the sustain point, and a
    /// released one carries on from wherever it was, rather than jumping to the release.
    #[test]
    fn released_notes_carry_on_from_where_they_were() {
        let envelope = AdsrEnvelope::new(0.1, 0.25, 0.4, 0.6, 1.0);
        let shape = EnvelopeShape::new(&envelope);
        let held = |time: f32| shape.locate(time, None).unwrap();
        assert!((held(0.05).0 - shape.decay_start() / 2.0).abs() < 1e-6);
        assert_eq!(held(10.0).0, shape.release_start());
        // Released halfway through the attack: no jump, then as far across as the release is
        // wide, falling to nothing
        for released in [0.05, 0.2, 3.0] {
            let at_release = shape.locate(released, Some(released)).unwrap();
            assert!((at_release.0 - held(released).0).abs() < 1e-6 && (at_release.1 - held(released).1).abs() < 1e-4);
            let halfway = shape.locate(released + 0.2, Some(released)).unwrap();
            assert!((halfway.0 - (held(released).0 + shape.widths[2] / 2.0)).abs() < 1e-6);
            assert!(halfway.1 < at_release.1);
            assert!(shape.locate(released + 0.4, Some(released)).is_none());
        }
        // While a point is dragged the scale stays put: times map to the same widths
        let frozen = EnvelopeShape::with_scale(&envelope, shape.scale);
        assert_eq!(frozen.widths, shape.widths);
        assert!((frozen.time_for(frozen.widths[1]) - 0.25).abs() < 1e-5);
    }

    /// The playing note's response follows its cutoff and resonance, without measuring again
    /// for a cutoff that has barely moved.
    #[test]
    fn live_filter_response_follows_the_note() {
        let settings = FilterSettings::new(FilterModel::SvfSimperFilter, FilterType::Lowpass, 1000.0, 0.7, 0.0, 44100.0, 1);
        let mut response = FilterResponse::new();
        let at_2k = response.update_live(settings, 2000.0, 0.7).to_vec();
        let measured = response.live.as_ref().unwrap().0;
        response.update_live(settings, 2001.0, 0.7);
        assert_eq!(response.live.as_ref().unwrap().0, measured);
        // An octave higher lets more through above the cutoff, and resonance makes a peak
        let at_4k = response.update_live(settings, 4000.0, 0.7).to_vec();
        assert!(level_at(&at_4k, 4000.0) > level_at(&at_2k, 4000.0) + 6.0);
        let resonant = response.update_live(settings, 2000.0, 8.0).to_vec();
        assert!(level_at(&resonant, 2000.0) > level_at(&at_2k, 2000.0) + 6.0);
    }

    /// How long measuring a filter's response takes, for each model at a few settings, which
    /// is what animating the curve costs for each new cutoff or resonance.
    #[test]
    #[ignore]
    fn benchmark_filter_response() {
        let fft = FftPlanner::new().plan_fft_forward(FilterResponse::LEN);
        println!();
        for model in [FilterModel::SvfSimperFilter, FilterModel::RcFilter, FilterModel::LadderFilter] {
            for (cutoff, resonance) in [(100.0, 0.7), (1000.0, 0.7), (1000.0, 5.0), (10000.0, 0.7)] {
                let settings = FilterSettings::new(model, FilterType::Lowpass, cutoff, resonance, 0.0, 44100.0, 1);
                let start = std::time::Instant::now();
                let rounds = 20;
                for _ in 0..rounds {
                    std::hint::black_box(FilterResponse::measure(&settings, &[cutoff], &*fft));
                }
                let ms = start.elapsed().as_secs_f64() * 1000.0 / rounds as f64;
                println!("{model:?} at {cutoff} Hz, resonance {resonance}: {ms:.3} ms");
            }
        }
    }

    fn level_at(levels: &[f32], hz: f32) -> f32 {
        let point = ((hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln() * (RESPONSE_POINTS - 1) as f32).round();
        levels[point as usize]
    }

    fn measure(model: FilterModel, filter_type: FilterType, cutoff: f32, resonance: f32, sample_rate: f32) -> Vec<f32> {
        thread_local! {
            static FFT: Arc<dyn Fft<f32>> = FftPlanner::new().plan_fft_forward(FilterResponse::LEN);
        }
        let settings = FilterSettings::new(model, filter_type, cutoff, resonance, 0.0, sample_rate, 1);
        FFT.with(|fft| FilterResponse::measure(&settings, &[settings.cutoff], &**fft).remove(0))
    }

    /// The measured responses have the shapes of the filters: flat without a filter, a second
    /// order Digital filter falling 12 dB per octave, a fourth order ladder whose passband drops
    /// with the resonance.
    #[test]
    fn filter_responses_have_the_expected_shapes() {
        use FilterModel::*;
        use FilterType::*;
        assert!(measure(None, Lowpass, 1000.0, 0.7, 44100.0).iter().all(|level| *level == 0.0));

        // The Digital filter with little resonance (a resonance of 0.1 is k = 1.98)
        let lowpass = measure(SvfSimperFilter, Lowpass, 1000.0, 0.1, 44100.0);
        assert!(level_at(&lowpass, 30.0).abs() < 0.5, "{}", level_at(&lowpass, 30.0));
        let falloff = level_at(&lowpass, 4000.0) - level_at(&lowpass, 8000.0);
        assert!((falloff - 12.0).abs() < 1.5, "{falloff}");
        let highpass = measure(SvfSimperFilter, Highpass, 1000.0, 0.1, 44100.0);
        assert!(level_at(&highpass, 15000.0).abs() < 0.5, "{}", level_at(&highpass, 15000.0));
        assert!(level_at(&highpass, 100.0) < -35.0, "{}", level_at(&highpass, 100.0));
        // Resonance makes a peak at the cutoff
        let resonant = measure(SvfSimperFilter, Lowpass, 1000.0, 9.0, 44100.0);
        assert!(level_at(&resonant, 1000.0) > 10.0, "{}", level_at(&resonant, 1000.0));

        // The ladder's passband is 1.8 / (1 + k), with k = 8 * resonance / 16.6
        for resonance in [0.5, 4.0] {
            let ladder = measure(LadderFilter, Lowpass, 2000.0, resonance, 44100.0);
            let k = 8.0 * resonance / 16.6;
            let want = 20.0 * (1.8 / (1.0 + k)).log10();
            let got = level_at(&ladder, 40.0);
            assert!((got - want).abs() < 0.5, "resonance {resonance}: {got} vs {want}");
            let falloff = level_at(&ladder, 6000.0) - level_at(&ladder, 12000.0);
            assert!((falloff - 24.0).abs() < 4.0, "resonance {resonance}: {falloff}");
        }
    }

    /// Every model and type gives a finite response at the extremes of the controls, also when
    /// the filter is self-oscillating, and working one out is quick enough to do while a
    /// slider is dragged.
    #[test]
    fn filter_responses_are_finite_and_quick() {
        use FilterModel::*;
        measure(SvfSimperFilter, FilterType::Lowpass, 1000.0, 1.0, 44100.0);
        let mut slowest = std::time::Duration::ZERO;
        for model in [RcFilter, LadderFilter, SvfSimperFilter] {
            for filter_type in [FilterType::Lowpass, FilterType::Bandpass, FilterType::Highpass] {
                for (cutoff, resonance) in [(10.0, 0.1), (22000.0, 10.0), (500.0, 10.0)] {
                    for sample_rate in [44100.0, 96000.0] {
                        let start = std::time::Instant::now();
                        let levels = measure(model, filter_type, cutoff, resonance, sample_rate);
                        slowest = slowest.max(start.elapsed());
                        assert_eq!(levels.len(), RESPONSE_POINTS);
                        assert!(
                            levels.iter().all(|level| level.is_finite()),
                            "{model:?} {filter_type:?} {cutoff} {resonance} {sample_rate}"
                        );
                    }
                }
            }
        }
        println!("slowest filter response: {slowest:?}");
    }

    /// The wave previews hold every frame, peak at 1, and crossfade between frames.
    #[test]
    fn wave_previews_follow_the_wavetables() {
        let mut renderer = CycleRenderer::new();
        let previews: Vec<WavePreview> = builtin_spectra()
            .iter()
            .map(|(_, spectra)| WavePreview::from_spectra(spectra, &mut renderer))
            .collect();
        for (preview, (name, spectra)) in previews.iter().zip(builtin_spectra()) {
            assert_eq!(preview.frames.len(), spectra.len(), "{name}");
            let peak = preview.frames.iter().flatten().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
            assert!((peak - 1.0).abs() < 1e-6, "{name}: {peak}");
        }
        // A sine starts at zero and peaks a quarter of the way in
        let sine = previews[0].cycle(0.0);
        assert_eq!(sine.len(), WavePreview::LEN);
        assert!(sine[0].abs() < 1e-3 && (sine[WavePreview::LEN / 4].abs() - 1.0).abs() < 1e-3);
        // Halfway between two frames of the PW sweep is the average of the two
        let sweep = &previews[crate::params::OscillatorWave::PwSweep.wavetable_index().unwrap()];
        let frames = sweep.frames.len();
        let position = 2.5 / (frames - 1) as f32;
        let mixed = sweep.cycle(position);
        for (n, ((got, a), b)) in mixed.iter().zip(&sweep.frames[2]).zip(&sweep.frames[3]).enumerate() {
            assert!((got - (a + b) / 2.0).abs() < 1e-4, "{n}");
        }
        // Without frames it's silence
        let empty = WavePreview { frames: Vec::new() };
        assert!(empty.cycle(0.5).iter().all(|sample| *sample == 0.0));
    }
}
