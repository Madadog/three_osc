//! Small displays of what the controls do: each oscillator's wave, each LFO's shape, the
//! envelopes and the filter's frequency response. They are drawn with the synth's own code (the
//! wavetables' spectra, the LFO generator, the envelope curve and the filters themselves), so
//! they show what is played.

use nih_plug_egui::egui::{self, Color32, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui};
use rustfft::num_complex::Complex32;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

use crate::synth::envelopes::AdsrEnvelope;
use crate::synth::filter::{
    Filter, FilterContainer, FilterController, FilterModel, FilterType, LadderFilter, LADDER_LANES,
};
use crate::synth::oscillator::{OscWave, WavePreview};

/// The colour of the oscillator and filter displays (the envelopes and LFOs, which are
/// modulation sources, use the modulation colour).
const SIGNAL_COLOR: Color32 = Color32::from_rgb(90, 180, 255);

const WAVE_HEIGHT: f32 = 50.0;
/// The LFO displays sit beside their two rows of controls.
const LFO_HEIGHT: f32 = 46.0;
const ENVELOPE_HEIGHT: f32 = 46.0;
const FILTER_HEIGHT: f32 = 76.0;

/// Allocates a display `height` pixels tall across the available width and draws its
/// background. Returns the display's response (for its hover text), a painter clipped to it and
/// the area to plot in.
fn frame(ui: &mut Ui, height: f32) -> (egui::Response, Painter, Rect) {
    let size = egui::vec2(ui.available_width(), height);
    let (response, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = response.rect;
    let visuals = ui.visuals();
    painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
    painter.rect_stroke(rect, 3.0, visuals.widgets.noninteractive.bg_stroke, StrokeKind::Inside);
    let plot = rect.shrink2(egui::vec2(4.0, 5.0));
    (response, painter.with_clip_rect(rect.shrink(1.0)), plot)
}

/// A faint guide line across or down a display.
fn guide(painter: &Painter, from: Pos2, to: Pos2) {
    painter.line_segment([from, to], Stroke::new(1.0_f32, Color32::from_gray(60)));
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

/// One cycle of an oscillator's wave at its wavetable position. `phase` marks where the wave
/// starts, when the start isn't fully random. A silent oscillator is drawn dimmed.
pub fn wave(ui: &mut Ui, preview: &WavePreview, position: f32, phase: Option<f32>, silent: bool) {
    let (response, painter, rect) = frame(ui, WAVE_HEIGHT);
    response.on_hover_text(if phase.is_some() {
        "One cycle of the wave, at the wavetable position. The line marks where it starts (Phase)."
    } else {
        "One cycle of the wave, at the wavetable position."
    });
    let centre = rect.center().y;
    guide(&painter, Pos2::new(rect.left(), centre), Pos2::new(rect.right(), centre));
    if let Some(phase) = phase {
        let x = x_at(rect, phase);
        guide(&painter, Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()));
    }
    let cycle = preview.cycle(position);
    // The cycle's first sample again at the end, where the next cycle starts
    let len = cycle.len();
    let points = (0..=len)
        .map(|i| {
            let sample = cycle[i % len];
            Pos2::new(x_at(rect, i as f32 / len as f32), centre - sample * rect.height() / 2.0)
        })
        .collect();
    let color = if silent { SIGNAL_COLOR.gamma_multiply(0.35) } else { SIGNAL_COLOR };
    curve(&painter, points, centre, color, 1.5);
}

/// One cycle of an LFO's waveform.
pub fn lfo(ui: &mut Ui, wave: &OscWave, color: Color32) {
    let (response, painter, rect) = frame(ui, LFO_HEIGHT);
    response.on_hover_text("One cycle of the LFO, from -100% to 100%.");
    let centre = rect.center().y;
    guide(&painter, Pos2::new(rect.left(), centre), Pos2::new(rect.right(), centre));
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
}

/// An envelope's shape as the envelope display draws it: attack, decay, a stretch of sustain,
/// then the release. The stages are as wide as the square roots of their times, so that short
/// stages stay visible next to long ones; within a stage, time runs evenly, so the curves have
/// their true shapes.
struct EnvelopeShape<'a> {
    envelope: &'a AdsrEnvelope,
    /// Where the decay, the sustain and the release start, as fractions of the width.
    boundaries: [f32; 3],
    release_width: f32,
}

impl<'a> EnvelopeShape<'a> {
    /// The share of the width the sustain gets.
    const SUSTAIN_WIDTH: f32 = 0.2;

    fn new(envelope: &'a AdsrEnvelope) -> Self {
        let times = [envelope.attack_time, envelope.decay_time, envelope.release_time];
        let roots = times.map(|time| time.max(0.0).sqrt());
        let total: f32 = roots.iter().sum();
        let sustain = if total > 0.0 { Self::SUSTAIN_WIDTH } else { 1.0 };
        let [attack, decay, release] = roots.map(|root| if total > 0.0 { root / total * (1.0 - sustain) } else { 0.0 });
        Self {
            envelope,
            boundaries: [attack, attack + decay, attack + decay + sustain],
            release_width: release,
        }
    }

    /// The envelope's level at a fraction of the width, worked out by the synth's envelope.
    fn level(&self, fraction: f32) -> f32 {
        let envelope = self.envelope;
        let (attack, decay) = (envelope.attack_time, envelope.decay_time);
        let [decay_start, sustain_start, release_start] = self.boundaries;
        if fraction < decay_start {
            envelope.sample_held(attack * fraction / decay_start)
        } else if fraction < sustain_start {
            envelope.sample_held(attack + decay * (fraction - decay_start) / (sustain_start - decay_start))
        } else if fraction < release_start || self.release_width <= 0.0 {
            envelope.sample_held(attack + decay)
        } else {
            let released = attack + decay;
            let into = ((fraction - release_start) / self.release_width).min(1.0);
            envelope.sample_released(released, released + envelope.release_time * into)
        }
    }
}

/// An envelope's shape, see [`EnvelopeShape`].
pub fn envelope(ui: &mut Ui, envelope: &AdsrEnvelope, color: Color32) {
    let (response, painter, rect) = frame(ui, ENVELOPE_HEIGHT);
    response.on_hover_text(
        "The envelope's shape: attack, decay, sustain (for as long as the note is held) and \
         release. The stages are drawn as wide as the square roots of their times, so that short \
         ones stay visible next to long ones.",
    );
    let shape = EnvelopeShape::new(envelope);
    let bottom = rect.bottom();
    for boundary in shape.boundaries {
        let x = x_at(rect, boundary);
        guide(&painter, Pos2::new(x, rect.top()), Pos2::new(x, bottom));
    }
    // A point per pixel, plus the stage boundaries themselves for crisp corners
    let steps = (rect.width() as usize).max(2);
    let mut fractions: Vec<f32> = (0..=steps).map(|step| step as f32 / steps as f32).collect();
    fractions.extend(shape.boundaries);
    fractions.sort_by(f32::total_cmp);
    let points = fractions
        .into_iter()
        .map(|fraction| Pos2::new(x_at(rect, fraction), bottom - shape.level(fraction) * rect.height()))
        .collect();
    curve(&painter, points, bottom, color, 1.5);
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
    pub sample_rate: f32,
}

impl FilterSettings {
    /// The settings for a cutoff control, with the filter envelope's amount (as the synth uses
    /// it, squared) added at the envelope's peak.
    pub fn new(
        model: FilterModel,
        filter_type: FilterType,
        cutoff: f32,
        resonance: f32,
        envelope_amount: f32,
        sample_rate: f32,
    ) -> Self {
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
            fft: None,
        }
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

/// The filter's frequency response, from 20 Hz to 20 kHz, with its cutoff marked, and the
/// response at the peak of the filter envelope drawn more faintly when the envelope moves the
/// cutoff.
pub fn filter(ui: &mut Ui, settings: FilterSettings, response: &mut FilterResponse) {
    response.update(settings);
    let (hover, painter, rect) = frame(ui, FILTER_HEIGHT);
    hover.on_hover_text(
        "The filter's frequency response, from 20 Hz to 20 kHz (the line marks the cutoff), \
         measured from the filter itself with a quiet signal, so the drive's distortion doesn't \
         show. The fainter curve is the response at the peak of the filter envelope. With \
         keytracking, the cutoff is for the note A4.",
    );
    let x_for = |hz: f32| x_at(rect, (hz / LOW_HZ).ln() / (HIGH_HZ / LOW_HZ).ln());
    let y_for = |db: f32| {
        let fraction = (db.clamp(FLOOR_DB, CEILING_DB) - FLOOR_DB) / (CEILING_DB - FLOOR_DB);
        rect.bottom() - fraction * rect.height()
    };

    // A guide every decade and at 0 dB, labelled
    let label_color = Color32::from_gray(110);
    let font = egui::FontId::proportional(9.0);
    for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10000.0, "10k")] {
        let x = x_for(hz);
        guide(&painter, Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom()));
        painter.text(Pos2::new(x + 2.0, rect.bottom()), egui::Align2::LEFT_BOTTOM, label, font.clone(), label_color);
    }
    let zero = y_for(0.0);
    guide(&painter, Pos2::new(rect.left(), zero), Pos2::new(rect.right(), zero));
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
    if let Some(levels) = &response.envelope_levels {
        painter.add(Shape::line(points(levels), Stroke::new(1.0_f32, SIGNAL_COLOR.gamma_multiply(0.45))));
    }
    if !matches!(settings.model, FilterModel::None) {
        let x = x_for(settings.cutoff.clamp(LOW_HZ, HIGH_HZ));
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            Stroke::new(1.0_f32, SIGNAL_COLOR.gamma_multiply(0.5)),
        );
    }
    curve(&painter, points(&response.levels), rect.bottom(), SIGNAL_COLOR, 1.5);
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

    /// The envelope display goes through the envelope's levels at the stage boundaries.
    #[test]
    fn envelope_shape_follows_the_envelope() {
        // Attack, decay, release, sustain, slope
        let envelope = AdsrEnvelope::new(0.01, 0.25, 0.3, 0.6, 1.0);
        let shape = EnvelopeShape::new(&envelope);
        let [decay_start, sustain_start, release_start] = shape.boundaries;
        assert!(0.0 < decay_start && decay_start < sustain_start && sustain_start < release_start);
        assert!(release_start < 1.0);
        // The stages are as wide as the square roots of their times
        let width = |time: f32| time.sqrt() / (0.01_f32.sqrt() + 0.25_f32.sqrt() + 0.3_f32.sqrt()) * 0.8;
        assert!((decay_start - width(0.01)).abs() < 1e-6);
        assert!((sustain_start - decay_start - width(0.25)).abs() < 1e-6);
        assert!((shape.release_width - width(0.3)).abs() < 1e-6);

        // (The envelope's power function is approximate)
        assert!(shape.level(0.0) < 1e-6);
        assert!((shape.level(decay_start) - 1.0).abs() < 1e-4);
        assert!((shape.level(sustain_start) - 0.6).abs() < 1e-4);
        assert!((shape.level((sustain_start + release_start) / 2.0) - 0.6).abs() < 1e-6);
        assert!((shape.level(release_start) - 0.6).abs() < 1e-4);
        assert!(shape.level(1.0) < 1e-6);
        // The decay falls and the release falls
        assert!(shape.level(decay_start + 0.05) < 1.0);
        assert!(shape.level(release_start + 0.05) < 0.6);

        // A zero length attack starts at the top, and without any stages it's all sustain
        let instant = AdsrEnvelope::new(0.0, 0.5, 0.1, 0.2, 1.0);
        assert!((EnvelopeShape::new(&instant).level(0.0) - 1.0).abs() < 1e-6);
        let flat = AdsrEnvelope::new(0.0, 0.0, 0.0, 0.7, 1.0);
        let shape = EnvelopeShape::new(&flat);
        for fraction in [0.0, 0.3, 1.0] {
            assert!((shape.level(fraction) - 0.7).abs() < 1e-6, "{fraction}");
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
        let settings = FilterSettings::new(model, filter_type, cutoff, resonance, 0.0, sample_rate);
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
        let sweep = previews.last().unwrap();
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
