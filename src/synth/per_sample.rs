//! Modulation at the sample rate (see [`ModulationRate::Sample`][super::ModulationRate]): the
//! control values of every sample of a block, worked out eight samples at a time.
//!
//! The control rate update ([`Voice::update_controls()`]) works the controls out for the end
//! of a block, and they ramp there from the last block's. Here each sample of the block gets
//! the values for its own time instead: envelopes, LFOs, the modulation matrix, pitch, gains,
//! wavetable positions, modulation depths and filter coefficients, with the same arithmetic as
//! the control rate update, in vectors of eight consecutive samples (see `fastmath::x8`). The
//! oscillators, filters and mixing then take the values sample by sample, and still render the
//! whole block in one go.

use nih_plug::prelude::Enum;
use wide::{f32x8, i32x8};

use super::envelopes::AdsrEnvelope;
use super::fastmath::{self, x8};
use super::lfo::LfoTrigger;
use super::filter::{Filter, FilterContainer, FilterModel, SvfSimper};
use super::modulation::{ModSource, ModTarget, Route, Sources, N_LFOS, N_SLOTS, N_SOURCES, N_TARGETS};
use super::oscillator::{frame_indices, OscillatorParams, WavetableNotes, MULTIPLIER_RANGE, OCTAVE_RANGE};
use super::{
    modulated_pan_gains, Voice, VoiceContext, CONTROL_BLOCK_SAMPLES, FM_DEPTH, LFO_MAX_HZ, LFO_MIN_HZ, PM_DEPTH_CYCLES,
};

/// Vectors of eight samples in a control rate block.
const BLOCK_VECTORS: usize = CONTROL_BLOCK_SAMPLES / 8;
/// A value for each sample of a control rate block.
pub(super) type Samples = [f32; CONTROL_BLOCK_SAMPLES];
/// A value for each sample of a control rate block, eight samples per vector.
type Vectors = [f32x8; BLOCK_VECTORS];

/// The control values of each sample of a voice's current block.
pub(super) struct SampleControls {
    /// Each oscillator's base phase delta in cycles per sample.
    pub osc_delta: [Samples; 3],
    /// Each oscillator's gain times the filter drive, before amplitude modulation.
    pub osc_gain: [Samples; 3],
    /// Each oscillator's crossfade between the two frames of its pair.
    pub osc_mix: [Samples; 3],
    /// The first frame of each oscillator's pair. Usually the same throughout a block, which is
    /// rendered in runs of samples with the same pair.
    pub osc_frame: [[i32; CONTROL_BLOCK_SAMPLES]; 3],
    /// Each oscillator's FM, PM and AM depths, in the units [`Modulation::scale()`] takes.
    ///
    /// [`Modulation::scale()`]: super::oscillator::Modulation::scale()
    pub osc_depths: [[Samples; 3]; 3],
    /// Whether each of an oscillator's depths is above zero at any sample.
    pub osc_modulated: [[bool; 3]; 3],
    /// Whether each oscillator's gain is zero throughout the block.
    pub osc_silent: [bool; 3],
    /// Whether each oscillator's delta, gain or frames change within the block. Those that
    /// don't are rendered like a control rate block without ramps, which is cheaper.
    pub osc_varies: [bool; 3],
    /// Whether each oscillator's first frame changes within the block, which then renders in
    /// runs of samples with the same frames.
    pub osc_frames_vary: [bool; 3],
    /// Whether each oscillator reads the second frame of its pair at any sample of the block.
    pub osc_blend: [bool; 3],
    /// The voice's gain: amplitude envelope times velocity and volume.
    pub gain: Samples,
    /// The filter's coefficients: `c1`, `c2`, `c3` and `k` for the Digital filter, the
    /// integration rate and resonance for the ladder, and the cutoff and resonance for the
    /// other models, which work theirs out sample by sample.
    pub filter: [Samples; 4],
    /// Whether the filter's coefficients change within the block. When they don't, the
    /// filters are set to them and run as at the control rate, without a ramp.
    pub filter_varies: bool,
    /// The filter model, cutoff, resonance and sample rate whose coefficients `filter` holds
    /// at every sample, if it does, so that they aren't worked out again while they stay the
    /// same.
    filter_cached: Option<(FilterModel, f32, f32, f32)>,
    /// The modulation sources, while the block's values are worked out.
    sources: [Vectors; N_SOURCES],
    /// Whether each source is the same throughout the block.
    constant: [bool; N_SOURCES],
}

impl SampleControls {
    /// Every source's value at sample `n` of the block just worked out.
    pub fn sources_at(&self, n: usize) -> Sources {
        std::array::from_fn(|source| sample(&self.sources[source], n))
    }

    pub fn new() -> Self {
        Self {
            osc_delta: [[0.0; CONTROL_BLOCK_SAMPLES]; 3],
            osc_gain: [[0.0; CONTROL_BLOCK_SAMPLES]; 3],
            osc_mix: [[0.0; CONTROL_BLOCK_SAMPLES]; 3],
            osc_frame: [[0; CONTROL_BLOCK_SAMPLES]; 3],
            osc_depths: [[[0.0; CONTROL_BLOCK_SAMPLES]; 3]; 3],
            osc_modulated: [[false; 3]; 3],
            osc_silent: [true; 3],
            osc_varies: [false; 3],
            osc_frames_vary: [false; 3],
            osc_blend: [false; 3],
            gain: [0.0; CONTROL_BLOCK_SAMPLES],
            filter: [[0.0; CONTROL_BLOCK_SAMPLES]; 4],
            filter_varies: false,
            filter_cached: None,
            sources: [[f32x8::ZERO; BLOCK_VECTORS]; N_SOURCES],
            constant: [false; N_SOURCES],
        }
    }
}

/// The routes grouped by target, in the order they come in (which is the order the control
/// rate update adds them up in), so that one target's modulation can be worked out at a time.
pub(super) struct RoutesByTarget {
    routes: [Route; N_SLOTS],
    /// Each target's range of `routes`.
    ranges: [(u8, u8); N_TARGETS],
}

impl RoutesByTarget {
    pub fn new(routes: &[Route]) -> Self {
        let routes = &routes[..routes.len().min(N_SLOTS)];
        let mut counts = [0_u8; N_TARGETS];
        for route in routes {
            counts[route.target.to_index()] += 1;
        }
        let mut ranges = [(0, 0); N_TARGETS];
        let mut start = 0;
        for (range, count) in ranges.iter_mut().zip(counts) {
            *range = (start, start);
            start += count;
        }
        let none = Route::new(ModSource::None, ModTarget::None, 0.0);
        let mut sorted = [none; N_SLOTS];
        for route in routes {
            let range = &mut ranges[route.target.to_index()];
            sorted[range.1 as usize] = *route;
            range.1 += 1;
        }
        Self {
            routes: sorted,
            ranges,
        }
    }

    /// The routes to a target.
    pub fn get(&self, target: ModTarget) -> &[Route] {
        let (start, end) = self.ranges[target.to_index()];
        &self.routes[start as usize..end as usize]
    }

    /// Whether any route modulates an LFO's rate, which makes the LFOs depend on the matrix
    /// from one sample to the next.
    pub fn modulate_lfo_rates(&self) -> bool {
        (0..N_LFOS).any(|lfo| !self.get(ModTarget::lfo_rate(lfo)).is_empty())
    }
}

impl AdsrEnvelope {
    /// The level throughout a block that starts at `start` (in seconds), if it doesn't change
    /// within the block: all of it in the sustain (or in a decay to a full sustain, which stays
    /// at 1), or released to silence. `release` is the release's time and the level it started
    /// from. The level is exactly what [`sample_held()`][Self::sample_held()] or
    /// [`sample_released()`][Self::sample_released()] gives at every sample.
    fn constant_level(&self, start: f32, release: Option<(f32, f32)>) -> Option<f32> {
        match release {
            None => {
                let decayed = start >= self.attack_time
                    && (start - self.attack_time >= self.decay_time || self.sustain_level == 1.0);
                decayed.then_some(self.sustain_level)
            }
            Some((release_index, level)) => {
                (start - release_index >= self.release_time || level == 0.0).then_some(0.0)
            }
        }
    }

    /// [`sample_held()`][Self::sample_held()] of eight times at once, with exactly the same
    /// results.
    fn sample_held8(&self, index: f32x8) -> f32x8 {
        let splat = f32x8::splat;
        let attack = splat(self.attack_time);
        let since_attack = index - attack;
        let sustain = splat(self.sustain_level);
        let decay = |since_attack: f32x8| {
            let curve = x8::pow(splat(1.0) - since_attack / splat(self.decay_time), splat(self.decay_slope));
            curve * (splat(1.0) - sustain) + sustain
        };
        // The times go up, so when the first is past the attack, they all are, and when the
        // last is in the attack, they all are
        let [first, .., last] = index.to_array();
        if first >= self.attack_time {
            if first - self.attack_time >= self.decay_time {
                return sustain;
            }
            return since_attack.simd_lt(splat(self.decay_time)).select(decay(since_attack), sustain);
        }
        if last < self.attack_time {
            return x8::pow(index / attack, splat(self.attack_slope));
        }
        let in_attack = index.simd_lt(attack);
        let in_decay = since_attack.simd_lt(splat(self.decay_time));
        // One power for whichever stage each sample is in
        let base = in_attack.select(index / attack, splat(1.0) - since_attack / splat(self.decay_time));
        let exponent = in_attack.select(splat(self.attack_slope), splat(self.decay_slope));
        let curve = x8::pow(base, exponent);
        let decaying = curve * (splat(1.0) - sustain) + sustain;
        in_attack.select(curve, in_decay.select(decaying, sustain))
    }

    /// [`sample_released()`][Self::sample_released()] of eight times at once, given the level
    /// at the release, with exactly the same results.
    fn sample_released8(&self, release_index: f32, level: f32, index: f32x8) -> f32x8 {
        let splat = f32x8::splat;
        // The times go up, so when the first is past the release, they all are
        if index.as_array()[0] - release_index >= self.release_time {
            return f32x8::ZERO;
        }
        let since = index - splat(release_index);
        let curve = x8::pow(splat(1.0) - since / splat(self.release_time), splat(self.release_slope)) * splat(level);
        since.simd_ge(splat(self.release_time)).select(f32x8::ZERO, curve)
    }
}

/// A target's modulation over a block: the sum of the routes to it, at each sample.
struct Modulated<'a> {
    routes: &'a [Route],
    sources: &'a [Vectors; N_SOURCES],
    /// Whether any of its sources changes within the block. If none do, every sample's
    /// modulation is the same.
    varies: bool,
}

impl Modulated<'_> {
    /// The modulation at the samples of vector `v`, summed over the routes in their order.
    #[inline(always)]
    fn at(&self, v: usize) -> f32x8 {
        let mut sum = f32x8::ZERO;
        for route in self.routes {
            let mut value = self.sources[route.source.to_index()][v];
            if !route.is_plain() {
                value = route.reshape8(value, self.sources[route.remap.via.to_index()][v]);
            }
            sum += value * f32x8::splat(route.amount);
        }
        sum
    }

    /// The modulation at the first sample.
    fn first(&self) -> f32 {
        first(self.at(0))
    }
}

/// [`OscillatorParams::stepped_pitch_factor()`] for eight samples' modulation.
fn stepped_pitch_factor8(osc: &OscillatorParams, octave_mod: f32x8, multiplier_mod: f32x8) -> f32x8 {
    let splat = f32x8::splat;
    let (low, high) = OCTAVE_RANGE;
    let octave = (splat(osc.octave as f32) + octave_mod * splat(ModTarget::OCTAVE_SPAN))
        .round()
        .fast_max(splat(low as f32))
        .fast_min(splat(high as f32));
    let (low, high) = MULTIPLIER_RANGE;
    let step = (splat(osc.multiplier as f32) + multiplier_mod * splat(ModTarget::MULTIPLIER_SPAN))
        .round()
        .fast_max(splat(low as f32))
        .fast_min(splat(high as f32));
    let ratio = step.simd_ge(f32x8::ZERO).select(splat(1.0) + step, splat(1.0) / (splat(1.0) - step));
    x8::exp2(octave - splat(osc.octave as f32)) * ratio / splat(osc.pitch_multiplier)
}

/// Whether a target's modulation changes within the block.
fn varies(modulation: &Option<Modulated>) -> bool {
    modulation.as_ref().is_some_and(|modulation| modulation.varies)
}

/// A target's modulation at the samples of a vector, zero without routes.
fn at(modulation: &Option<Modulated>, v: usize) -> f32x8 {
    modulation.as_ref().map_or(f32x8::ZERO, |modulation| modulation.at(v))
}

/// `0, 1, ..., 7`
fn lane_indices() -> i32x8 {
    i32x8::new([0, 1, 2, 3, 4, 5, 6, 7])
}

/// Copies a vector into its eight samples of a block.
fn store(samples: &mut Samples, vector: usize, values: f32x8) {
    samples[vector * 8..][..8].copy_from_slice(values.as_array());
}

/// Sample `n` of a block of vectors.
fn sample(vectors: &Vectors, n: usize) -> f32 {
    vectors[n / 8].as_array()[n % 8]
}

/// The first value of a vector.
fn first(vector: f32x8) -> f32 {
    vector.as_array()[0]
}

/// A vector's values, with those of the samples past the end of a `block_len` sample block set
/// to zero.
fn in_block(values: f32x8, vector: usize, block_len: usize) -> f32x8 {
    let remaining = block_len as i32 - 8 * vector as i32;
    if remaining >= 8 {
        return values;
    }
    f32x8::from_i32x8(lane_indices())
        .simd_lt(f32x8::splat(remaining as f32))
        .select(values, f32x8::ZERO)
}

impl Voice {
    /// The control values of every sample of the next `block_len` samples, for modulation at
    /// the sample rate (see [`SampleControls`]). The voice's own control rate state is left as
    /// the last sample's values, so that the control rate update can carry on from them.
    ///
    /// The controls that don't change within the block (because nothing that moves them does)
    /// are only worked out once, for its first vector.
    pub(super) fn update_controls_per_sample(&mut self, context: &VoiceContext, block_len: usize) {
        let splat = f32x8::splat;
        let sample_rate = context.sample_rate;
        let block_start = self.runtime;
        self.runtime += block_len as u32;
        // (The filters take each block's coefficients afresh here)
        self.rate_changed = false;
        let vectors = block_len.div_ceil(8);
        let last = block_len - 1;
        // The vectors to work a control out for: all of them, or the first if it doesn't change
        let distinct = |varies: bool| if varies { vectors } else { 1 };

        // Portamento: the pitch offset at each sample, from where the last block left it
        let detune = self.semitone_detune;
        let sliding = detune != 0.0;
        let mut semitone_detune = [f32x8::ZERO; BLOCK_VECTORS];
        if sliding {
            for (v, detunes) in semitone_detune.iter_mut().enumerate().take(vectors) {
                let powers: [f32; 8] = context.portamento_powers[v * 8..][..8].try_into().unwrap();
                *detunes = splat(detune) * f32x8::new(powers);
            }
        }
        self.semitone_detune = detune * context.portamento_powers[block_len];
        self.pitch_multiply = context.octave_detune;

        // The base phase delta of each sample, as `delta()` works it out
        let key = self.pitch - 69.0;
        let delta = if sliding {
            let mut delta = [f32x8::ZERO; BLOCK_VECTORS];
            for (delta, detune) in delta.iter_mut().zip(&semitone_detune).take(vectors) {
                *delta = splat(440.0) * x8::exp2(((splat(key) + *detune) * splat(self.pitch_multiply)) / splat(12.0))
                    / splat(sample_rate);
            }
            delta
        } else {
            [splat(440.0 * fastmath::exp2((key * self.pitch_multiply) / 12.0) / sample_rate); BLOCK_VECTORS]
        };

        self.update_sources(context, block_start, block_len);

        let routes = &context.routes_by_target;
        let samples = &mut self.samples;
        let sources = &samples.sources;
        let constant = samples.constant;
        // A target's modulation, or `None` if no route modulates it
        let modulation = |target: ModTarget| -> Option<Modulated> {
            let routes = routes.get(target);
            (!routes.is_empty()).then(|| Modulated {
                routes,
                sources,
                varies: routes.iter().any(|route| route.sources().any(|source| !constant[source.to_index()])),
            })
        };

        // The filter controls, first, since the drive scales the oscillators' gains
        let controller = context.filter_controller;
        let filter_level = &sources[ModSource::FilterEnv.to_index()];
        let max_cutoff = (sample_rate * 0.49).clamp(10.0, 22000.0);
        let cutoff_mod = modulation(ModTarget::Cutoff);
        let resonance_mod = modulation(ModTarget::Resonance);
        // (Without a filter there's nothing to work out)
        let filtered = controller.filter_model != FilterModel::None;
        let coefficients_vary = filtered
            && (varies(&cutoff_mod)
                || varies(&resonance_mod)
                || (sliding && controller.keytrack != 0.0)
                || (controller.envelope_amount != 0.0 && !constant[ModSource::FilterEnv.to_index()]));
        let mut cutoff = [f32x8::ZERO; BLOCK_VECTORS];
        let mut resonance = [splat(controller.resonance); BLOCK_VECTORS];
        for v in 0..if filtered { distinct(coefficients_vary) } else { 0 } {
            let mut mult = if controller.keytrack != 0.0 {
                x8::exp2(
                    (splat(self.pitch - 69.0) + semitone_detune[v]) / splat(12.0)
                        * splat(controller.keytrack),
                )
            } else {
                splat(1.0)
            };
            if let Some(cutoff_mod) = &cutoff_mod {
                mult *= x8::exp2(cutoff_mod.at(v) * splat(ModTarget::CUTOFF_OCTAVES));
            }
            // `FilterController::get_cutoff()`
            let base = splat(controller.cutoff) * mult;
            let envelope = filter_level[v]
                * splat(controller.envelope_amount)
                * (splat(440.0) + base)
                * splat(50.0);
            cutoff[v] = (base + envelope).fast_max(splat(10.0)).fast_min(splat(max_cutoff));
            if let Some(resonance_mod) = &resonance_mod {
                resonance[v] = (splat(controller.resonance)
                    * x8::exp2(resonance_mod.at(v) * splat(ModTarget::RESONANCE_OCTAVES)))
                .fast_max(splat(0.1))
                .fast_min(splat(10.0));
            }
        }
        let drive_mod = modulation(ModTarget::Drive);
        let drive_varies = varies(&drive_mod);
        let mut drive = [f32x8::ZERO; BLOCK_VECTORS];
        for (v, drive) in drive.iter_mut().enumerate().take(distinct(drive_varies)) {
            let modulated = match &drive_mod {
                Some(drive_mod) => (splat(controller.drive) * x8::exp2(drive_mod.at(v) * splat(ModTarget::DRIVE_OCTAVES)))
                    .fast_max(splat(0.01))
                    .fast_min(splat(10.0)),
                None => splat(controller.drive),
            };
            // Biquad/SVF/none filters are unaffected by drive, so it is clamped between 0 and 1
            // to keep the levels the same when switching filter (see `update_controls()`).
            *drive = match controller.filter_model {
                FilterModel::BiquadFilter | FilterModel::SvfSimperFilter | FilterModel::None => modulated.fast_min(splat(1.0)),
                FilterModel::LadderFilter => modulated / splat(2.0),
                _ => modulated,
            };
        }
        if !drive_varies {
            drive = [drive[0]; BLOCK_VECTORS];
        }
        self.drive = sample(&drive, last);

        // The filter itself, set up for the block, and its coefficients for every sample
        let first_cutoff = first(cutoff[0]);
        self.cutoff = if coefficients_vary { sample(&cutoff, last) } else { first_cutoff };
        let first_resonance = first(resonance[0]);
        let configure = |filter: &mut FilterContainer| {
            filter.set(
                controller.filter_model,
                first_cutoff,
                first_resonance,
                sample_rate,
                controller.filter_type,
            );
            filter.set_ladder_oversampling(context.ladder_oversampling);
            filter.set_filter_type(controller.filter_type);
        };
        configure(&mut self.filter);
        if context.stereo {
            configure(&mut self.filter_right);
        }
        // When the cutoff and the resonance stay the same throughout the block, so do the
        // coefficients: they are worked out once (or not at all if the last block's were the
        // same), and the filters then run on them as at the control rate, without a ramp
        let unchanged = !coefficients_vary
            || (0..vectors).all(|v| {
                (cutoff[v].simd_eq(splat(first_cutoff)) & resonance[v].simd_eq(splat(first_resonance))).all()
            });
        let key = (controller.filter_model, first_cutoff, first_resonance, sample_rate);
        let cached = unchanged && samples.filter_cached == Some(key);
        let computed = if unchanged { 1 } else { vectors };
        // Into every sample of the block, so that they stay valid for the next
        let store_all = |samples: &mut Samples, values: &Vectors| {
            if unchanged {
                samples.fill(first(values[0]));
            } else {
                for (v, values) in values.iter().enumerate().take(vectors) {
                    store(samples, v, *values);
                }
            }
        };
        let [c0, c1, c2, c3] = &mut samples.filter;
        match &mut self.filter {
            FilterContainer::SvfSimperFilter(_) if !cached => {
                let mut values = [[f32x8::ZERO; BLOCK_VECTORS]; 4];
                for v in 0..computed {
                    let coefficients = SvfSimper::coefficients8(cutoff[v], resonance[v] / splat(10.0), sample_rate);
                    for (values, coefficient) in values.iter_mut().zip(coefficients) {
                        values[v] = coefficient;
                    }
                }
                for (samples, values) in [&mut *c0, &mut *c1, &mut *c2, &mut *c3].into_iter().zip(&values) {
                    store_all(samples, values);
                }
            }
            FilterContainer::LadderFilter(ladder) => {
                // Keeps the resampling filters' design up to date (the samples' values then
                // replace the cutoff and resonance this sets)
                ladder.set_params(sample_rate, first_cutoff, first_resonance);
                if let FilterContainer::LadderFilter(right) = &mut self.filter_right {
                    right.set_params(sample_rate, first_cutoff, first_resonance);
                }
                if !cached {
                    let mut values = [[f32x8::ZERO; BLOCK_VECTORS]; 2];
                    for v in 0..computed {
                        let coefficients = ladder.coefficients8(cutoff[v], resonance[v], sample_rate);
                        for (values, coefficient) in values.iter_mut().zip(coefficients) {
                            values[v] = coefficient;
                        }
                    }
                    store_all(c0, &values[0]);
                    store_all(c1, &values[1]);
                }
            }
            FilterContainer::SvfSimperFilter(_) | FilterContainer::None => {}
            _ if !cached => {
                store_all(c0, &cutoff);
                store_all(c1, &resonance);
            }
            _ => {}
        }
        samples.filter_varies = !unchanged;
        samples.filter_cached = unchanged.then_some(key);
        if unchanged {
            let set = |filter: &mut FilterContainer| match filter {
                FilterContainer::SvfSimperFilter(svf) => svf.set_coefficients([c0[0], c1[0], c2[0], c3[0]]),
                FilterContainer::LadderFilter(ladder) => ladder.set_coefficients(c0[0], c1[0]),
                FilterContainer::None => {}
                other => other.set_params(sample_rate, first_cutoff, first_resonance),
            };
            set(&mut self.filter);
            if context.stereo {
                set(&mut self.filter_right);
            }
        }

        // The oscillators, with the pitch modulation they share worked out once, and its factor
        // on the deltas of those without pitch routes of their own
        let pitch_mod = modulation(ModTarget::Pitch);
        let fine_pitch_mod = modulation(ModTarget::FinePitch);
        let shared_routed = pitch_mod.is_some() || fine_pitch_mod.is_some();
        let shared_varies = varies(&pitch_mod) || varies(&fine_pitch_mod);
        let mut shared_semitones = [f32x8::ZERO; BLOCK_VECTORS];
        let mut shared_factor = [splat(1.0); BLOCK_VECTORS];
        if shared_routed {
            for v in 0..distinct(shared_varies) {
                shared_semitones[v] = at(&pitch_mod, v) * splat(ModTarget::PITCH_SEMITONES)
                    + at(&fine_pitch_mod, v) * splat(ModTarget::FINE_PITCH_SEMITONES);
                shared_factor[v] = x8::exp2(shared_semitones[v] / splat(12.0));
            }
            if !shared_varies {
                shared_semitones = [shared_semitones[0]; BLOCK_VECTORS];
                shared_factor = [shared_factor[0]; BLOCK_VECTORS];
            }
        }
        for (i, osc) in context.oscillators.iter().enumerate() {
            if !context.active[i] {
                // Nothing of it is rendered this block, and the ramps start afresh if it
                // becomes active at the control rate.
                self.osc_ready[i] = false;
                continue;
            }
            self.osc_ready[i] = true;

            // What can change within the block. An oscillator without any of it is rendered
            // from its first sample's values, which are all that is filled in for it.
            let osc_pitch_mod = modulation(ModTarget::osc_pitch(i));
            let (octave_mod, multiplier_mod) = match context.stepped[i] {
                true => (modulation(ModTarget::osc_octave(i)), modulation(ModTarget::osc_multiplier(i))),
                false => (None, None),
            };
            let stepped = octave_mod.is_some() || multiplier_mod.is_some();
            let amp_mod = modulation(ModTarget::osc_amp(i));
            let position_mod = modulation(ModTarget::osc_position(i));
            let pitch_varies = sliding
                || shared_varies
                || varies(&osc_pitch_mod)
                || varies(&octave_mod)
                || varies(&multiplier_mod);
            let gain_varies = drive_varies || varies(&amp_mod);
            let position_varies = varies(&position_mod);
            let fill = pitch_varies || gain_varies || position_varies;
            let fill_with = |samples: &mut Samples, value: f32| {
                if fill {
                    samples.fill(value);
                } else {
                    samples[0] = value;
                }
            };

            // Pitch. (Whether it changes takes the samples of the last vector past the block
            // into account too, as does whether the gain does, or is silent: at worst a
            // block takes a dearer path than it needs.)
            let osc_delta = |v: usize| {
                let osc_delta = splat(osc.total_pitch_multiplier()) * delta[v];
                let osc_delta = match &osc_pitch_mod {
                    Some(osc_pitch_mod) => {
                        let semitones = shared_semitones[v] + osc_pitch_mod.at(v) * splat(ModTarget::PITCH_SEMITONES);
                        osc_delta * x8::exp2(semitones / splat(12.0))
                    }
                    None if shared_routed => osc_delta * shared_factor[v],
                    None => osc_delta,
                };
                if stepped {
                    osc_delta * stepped_pitch_factor8(osc, at(&octave_mod, v), at(&multiplier_mod, v))
                } else {
                    osc_delta
                }
            };
            let mut changes = false;
            let (max_delta, last_delta) = if pitch_varies {
                let (mut differs, mut max) = (f32x8::ZERO, f32x8::ZERO);
                for v in 0..vectors {
                    let osc_delta = osc_delta(v);
                    store(&mut samples.osc_delta[i], v, osc_delta);
                    differs |= osc_delta.simd_ne(splat(samples.osc_delta[i][0]));
                    max = max.fast_max(in_block(osc_delta, v, block_len));
                }
                changes |= differs.any();
                (max.to_array().into_iter().fold(0.0, f32::max), samples.osc_delta[i][last])
            } else {
                let osc_delta = first(osc_delta(0));
                fill_with(&mut samples.osc_delta[i], osc_delta);
                (osc_delta, osc_delta)
            };
            // The table bandlimited enough for the highest pitch in the block
            self.osc_note[i] = WavetableNotes::cycles_to_note(max_delta, sample_rate);
            self.osc_delta[i] = last_delta;
            self.osc_delta_step[i] = 0.0;

            // Amplitude, with the drive
            if gain_varies {
                let (mut audible, mut differs) = (f32x8::ZERO, f32x8::ZERO);
                for (v, drive) in drive.iter().enumerate().take(vectors) {
                    let amp = match &amp_mod {
                        Some(amp_mod) => (splat(osc.amp) + amp_mod.at(v)).fast_max(f32x8::ZERO).fast_min(splat(1.0)),
                        None => splat(osc.amp.clamp(0.0, 1.0)),
                    };
                    let gain = splat(osc.unison_amp) * amp;
                    if v == last / 8 {
                        self.osc_gain[i] = gain.as_array()[last % 8];
                    }
                    audible |= gain.simd_ne(f32x8::ZERO);
                    store(&mut samples.osc_gain[i], v, gain * *drive);
                    differs |= (gain * *drive).simd_ne(splat(samples.osc_gain[i][0]));
                }
                samples.osc_silent[i] = !audible.any();
                changes |= differs.any();
            } else {
                let amp = match &amp_mod {
                    Some(amp_mod) => (osc.amp + amp_mod.first()).clamp(0.0, 1.0),
                    None => osc.amp.clamp(0.0, 1.0),
                };
                let gain = osc.unison_amp * amp;
                self.osc_gain[i] = gain;
                samples.osc_silent[i] = gain == 0.0;
                fill_with(&mut samples.osc_gain[i], gain * first(drive[0]));
            }
            self.osc_gain_step[i] = 0.0;

            // Wavetable position
            let frames = context.frames[i].frames.len();
            match position_mod {
                Some(position_mod) if position_varies => {
                    // `frame_indices()`
                    let top = frames.saturating_sub(1);
                    let (mut frames_vary, mut blend) = (f32x8::ZERO, f32x8::ZERO);
                    for v in 0..vectors {
                        let position = splat(osc.position) + position_mod.at(v);
                        // (At most `top`, so the frame needs no clamping)
                        let scaled = position.fast_max(f32x8::ZERO).fast_min(splat(1.0)) * splat(top as f32);
                        let a = scaled.fast_trunc_int();
                        let mix = scaled - a.round_float();
                        store(&mut samples.osc_mix[i], v, mix);
                        samples.osc_frame[i][v * 8..][..8].copy_from_slice(a.as_array());
                        changes |= mix.simd_ne(splat(samples.osc_mix[i][0])).any();
                        frames_vary |= a.round_float().simd_ne(splat(samples.osc_frame[i][0] as f32));
                        blend |= mix.simd_gt(f32x8::ZERO);
                    }
                    samples.osc_frames_vary[i] = frames_vary.any();
                    samples.osc_blend[i] = blend.any();
                    changes |= frames_vary.any();
                    let a = samples.osc_frame[i][last] as usize;
                    self.osc_frames[i] = (a, (a + 1).min(top), samples.osc_mix[i][last]);
                }
                position_mod => {
                    // The same throughout the block, so only the first frame is read
                    let position = match position_mod {
                        Some(position_mod) => osc.position + position_mod.first(),
                        None => osc.position,
                    };
                    let (a, b, mix) = frame_indices(frames, position);
                    samples.osc_frame[i][0] = a as i32;
                    fill_with(&mut samples.osc_mix[i], mix);
                    samples.osc_frames_vary[i] = false;
                    samples.osc_blend[i] = mix > 0.0;
                    self.osc_frames[i] = (a, b, mix);
                }
            }
            samples.osc_varies[i] = changes;

            // Unison detune and pan, which only change once per block: their first sample's
            let first_sample = |target: ModTarget| {
                routes
                    .get(target)
                    .iter()
                    .fold(0.0, |sum, route| sum + first(sources[route.source.to_index()][0]) * route.amount)
            };
            let detune_mod = first_sample(ModTarget::osc_detune(i));
            self.detune_modulated[i] = detune_mod != 0.0;
            if detune_mod != 0.0 {
                self.detune_factors[i] =
                    osc.detune_factors_for((osc.voices_detune + detune_mod).clamp(0.0, 1.0));
            }
            let pan_gains = modulated_pan_gains(context, i, first_sample(ModTarget::osc_pan(i)));
            self.pan_modulated[i] = pan_gains.is_some();
            if let Some(gains) = pan_gains {
                self.pan_gains[i] = gains;
            }

            // Modulation by the next oscillator (the amounts are squared for finer control at
            // the low end), in the kernels' units. A kind that is zero throughout isn't read.
            let depths = [
                (osc.fm, ModTarget::osc_fm(i), FM_DEPTH),
                (osc.pm, ModTarget::osc_pm(i), PM_DEPTH_CYCLES),
                (osc.am, ModTarget::osc_am(i), 1.0),
            ];
            for (kind, (amount, target, depth)) in depths.into_iter().enumerate() {
                let (last_scaled, modulated) = match target.and_then(modulation) {
                    Some(depth_mod) if depth_mod.varies => {
                        // (As for the gain, the samples past the block count too)
                        let (mut positive, mut last_scaled) = (f32x8::ZERO, 0.0);
                        for v in 0..vectors {
                            let x = (splat(amount) + depth_mod.at(v)).fast_max(f32x8::ZERO).fast_min(splat(1.0));
                            let scaled = x * x;
                            if v == last / 8 {
                                last_scaled = scaled.as_array()[last % 8];
                            }
                            positive |= scaled.simd_gt(f32x8::ZERO);
                            store(&mut samples.osc_depths[i][kind], v, scaled * splat(depth));
                        }
                        (last_scaled, positive.any())
                    }
                    depth_mod => {
                        let x = match depth_mod {
                            Some(depth_mod) => (amount + depth_mod.first()).clamp(0.0, 1.0),
                            None => amount.clamp(0.0, 1.0),
                        };
                        let scaled = x * x;
                        if scaled > 0.0 {
                            samples.osc_depths[i][kind].fill(scaled * depth);
                        }
                        (scaled, scaled > 0.0)
                    }
                };
                match kind {
                    0 => self.osc_fm[i] = last_scaled,
                    1 => self.osc_pm[i] = last_scaled,
                    _ => self.osc_am[i] = last_scaled,
                }
                samples.osc_modulated[i][kind] = modulated;
            }
        }

        // Amplitude envelope, velocity and volume modulation
        let velocity = self.velocity as f32 / 128.0;
        let gain_level = &sources[ModSource::VolumeEnv.to_index()];
        let volume_mod = modulation(ModTarget::Volume);
        let gain_varies = !constant[ModSource::VolumeEnv.to_index()] || varies(&volume_mod);
        let mut gain = [f32x8::ZERO; BLOCK_VECTORS];
        for (v, gain) in gain.iter_mut().enumerate().take(distinct(gain_varies)) {
            let volume = match &volume_mod {
                Some(volume_mod) => (splat(1.0) + volume_mod.at(v)).fast_max(f32x8::ZERO) * splat(velocity),
                None => splat(velocity),
            };
            *gain = gain_level[v] * volume;
        }
        if gain_varies {
            for (v, gain) in gain.iter().enumerate().take(vectors) {
                store(&mut samples.gain, v, *gain);
            }
        } else {
            samples.gain.fill(first(gain[0]));
        }
        self.gain = samples.gain[last];
        self.gain_step = 0.0;
    }

    /// The modulation sources at each sample of the block (only those that are used, and
    /// the two envelopes the voice needs anyway), into `samples.sources`, and whether each is
    /// the same throughout the block into `samples.constant`. The LFOs advance.
    fn update_sources(&mut self, context: &VoiceContext, block_start: u32, block_len: usize) {
        let splat = f32x8::splat;
        let sample_rate = context.sample_rate;
        let vectors = block_len.div_ceil(8);
        let used = |source: ModSource| context.sources_used[source.to_index()];
        let controller = context.filter_controller;
        let release = self.release_time.map(|release| release as f32 / sample_rate);
        let SampleControls { sources, constant, .. } = &mut self.samples;
        // (No route has no source, which is zero)
        *constant = [false; N_SOURCES];
        constant[ModSource::None.to_index()] = true;

        // The envelopes, sampled at each sample's time, or once for a block they don't move in
        let times = |v: usize| {
            f32x8::from_i32x8(i32x8::splat(block_start as i32 + 8 * v as i32) + lane_indices())
                / splat(sample_rate)
        };
        let start = block_start as i32 as f32 / sample_rate;
        let levels = |envelope: &AdsrEnvelope, levels: &mut Vectors| -> bool {
            let release = release.map(|release| (release, envelope.sample_held(release)));
            if let Some(level) = envelope.constant_level(start, release) {
                *levels = [splat(level); BLOCK_VECTORS];
                return true;
            }
            for (v, levels) in levels.iter_mut().enumerate().take(vectors) {
                *levels = match release {
                    Some((release, level)) => envelope.sample_released8(release, level, times(v)),
                    None => envelope.sample_held8(times(v)),
                };
            }
            false
        };
        let volume = ModSource::VolumeEnv.to_index();
        constant[volume] = levels(context.gain_envelope, &mut sources[volume]);
        let filter = ModSource::FilterEnv.to_index();
        if controller.envelope_amount != 0.0 || used(ModSource::FilterEnv) {
            constant[filter] = levels(&controller.cutoff_envelope, &mut sources[filter]);
        } else {
            sources[filter] = [f32x8::ZERO; BLOCK_VECTORS];
            constant[filter] = true;
        }
        for (i, envelope) in context.mod_envelopes.iter().enumerate() {
            let source = ModSource::envelope(i).to_index();
            if used(ModSource::envelope(i)) {
                constant[source] = levels(envelope, &mut sources[source]);
            }
        }
        let constants = [
            (ModSource::Velocity, self.velocity as f32 / 127.0),
            (ModSource::Key, (self.id as f32 - 60.0) / 60.0),
            (ModSource::ModWheel, context.mod_wheel),
            (ModSource::Aftertouch, context.aftertouch.max(self.pressure)),
            (ModSource::Random, self.random),
        ];
        for (source, value) in constants {
            if used(source) {
                sources[source.to_index()] = [splat(value); BLOCK_VECTORS];
                constant[source.to_index()] = true;
            }
        }
        // The oscillators, a block's worth of samples late (see `Voice::osc_history`)
        if context.any_sourced {
            for (i, history) in self.osc_history.iter().enumerate().filter(|(i, _)| context.sourced[*i]) {
                for (v, values) in sources[ModSource::osc(i).to_index()].iter_mut().enumerate().take(vectors) {
                    *values = f32x8::new(history[v * 8..][..8].try_into().unwrap());
                }
            }
        }

        if context.routes_by_target.modulate_lfo_rates() {
            self.update_lfos_one_by_one(context, block_start, block_len);
            return;
        }
        // The LFOs keep running whether they are used or not, so they don't jump when routed.
        // Without their rates modulated, each sample's phase follows from the block's first.
        let offsets = f32x8::from_i32x8(lane_indices());
        for (i, lfo) in context.lfos.iter().enumerate() {
            let delta = context.lfo_rates[i] / sample_rate;
            let one_shot = lfo.trigger == LfoTrigger::OneShot;
            if used(ModSource::lfo(i)) {
                let copy = self.lfos[i];
                let first = splat(copy.phase as f32);
                for (v, values) in sources[ModSource::lfo(i).to_index()].iter_mut().enumerate().take(vectors) {
                    let offset = (offsets + splat(8.0 * v as f32)) * splat(delta);
                    let wave = if lfo.wave.is_random() {
                        // (Each sample's cycle has its own value)
                        let offset = offset.to_array();
                        f32x8::new(std::array::from_fn(|lane| copy.value_at(lfo.wave, offset[lane], one_shot)))
                    } else {
                        let phase = first + offset;
                        let phase = if one_shot {
                            phase.fast_min(splat(1.0))
                        } else {
                            phase - phase.fast_trunc_int().round_float()
                        };
                        lfo.wave.value8(phase)
                    };
                    *values = if lfo.fade > 0.0 { wave * lfo.fade_gain8(times(v)) } else { wave };
                }
            }
            self.lfos[i].advance(delta * block_len as f32, one_shot);
        }
    }

    /// The LFOs one sample at a time, when routes modulate their rates: each sample's rate
    /// depends on the sources at the sample before, the LFOs among them. Like the control rate
    /// update, a sample's LFO values come from the phases reached by the sample before.
    fn update_lfos_one_by_one(&mut self, context: &VoiceContext, block_start: u32, block_len: usize) {
        let routes = &context.routes_by_target;
        let sources = &mut self.samples.sources;
        // Each sample's LFO values
        let mut values = [[0.0_f32; N_LFOS]; CONTROL_BLOCK_SAMPLES];
        let mut rates = [0.0_f32; N_LFOS];
        for (n, values) in values.iter_mut().enumerate().take(block_len) {
            let time = (block_start + n as u32) as f32 / context.sample_rate;
            for (i, lfo) in context.lfos.iter().enumerate() {
                values[i] = self.lfos[i].value(lfo.wave) * lfo.fade_gain(time);
            }
            // The rates' modulation at this sample, which moves the phases on to the next
            let lfo_values = *values;
            let value = |source: ModSource| match (0..N_LFOS).find(|&lfo| ModSource::lfo(lfo) == source) {
                Some(lfo) => lfo_values[lfo],
                None => sample(&sources[source.to_index()], n),
            };
            for (i, rate) in rates.iter_mut().enumerate() {
                *rate = 0.0;
                for route in routes.get(ModTarget::lfo_rate(i)) {
                    *rate += route.reshape(value(route.source), value(route.remap.via)) * route.amount;
                }
            }
            for (i, lfo) in context.lfos.iter().enumerate() {
                let mut freq = context.lfo_rates[i];
                if rates[i] != 0.0 {
                    freq = (freq * fastmath::exp2(rates[i] * ModTarget::LFO_RATE_OCTAVES)).clamp(LFO_MIN_HZ, LFO_MAX_HZ);
                }
                self.lfos[i].advance(freq / context.sample_rate, lfo.trigger == LfoTrigger::OneShot);
            }
        }
        for (i, rate) in rates.iter().enumerate() {
            self.mods[ModTarget::lfo_rate(i).to_index()] = *rate;
            let lfo = ModSource::lfo(i).to_index();
            for (v, vector) in sources[lfo].iter_mut().enumerate().take(block_len.div_ceil(8)) {
                *vector = f32x8::new(std::array::from_fn(|lane| values[v * 8 + lane][i]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Envelopes with every stage long, short and missing, with full, partial and no sustain,
    /// and with each kind of slope.
    fn envelopes() -> Vec<AdsrEnvelope> {
        let shapes = [
            (0.0, 0.0, 0.5),
            (0.002, 0.01, 1.0),
            (0.0, 0.01, 0.0),
            (0.004, 0.003, 0.4),
            (0.001, 0.005, 0.9),
            (0.0007, 0.0, 0.8),
        ];
        let mut envelopes = Vec::new();
        for (attack, decay, sustain) in shapes {
            for slope in [-3.0, 0.0, 1.0, 5.0] {
                let mut envelope = AdsrEnvelope::new(attack, decay, 0.004, sustain, 0.0);
                envelope.set_slope(slope);
                envelopes.push(envelope);
            }
            // Each stage its own way
            let mut envelope = AdsrEnvelope::new(attack, decay, 0.004, sustain, 0.0);
            envelope.set_slopes([2.0, -1.5, 4.0]);
            envelopes.push(envelope);
        }
        envelopes
    }

    /// The envelopes eight samples at a time give exactly the levels of one sample at a time,
    /// for blocks starting anywhere in every stage, held and released, and the blocks they
    /// stay the same throughout (as `update_sources()` finds them) do.
    #[test]
    fn envelope_vectors_match_the_scalar_envelope() {
        let sample_rate = 44100.0;
        let mut constant_blocks = 0;
        for envelope in envelopes() {
            for release in [None, Some(0_u32), Some(60), Some(300), Some(500)] {
                // As `update_sources()` works the times out
                let release_index = release.map(|release| release as f32 / sample_rate);
                let release_level = release_index.map(|index| (index, envelope.sample_held(index)));
                let first_block = release.unwrap_or(0);
                for block_start in (first_block..first_block + 900).step_by(7) {
                    let start = block_start as i32 as f32 / sample_rate;
                    let constant = envelope.constant_level(start, release_level);
                    constant_blocks += constant.is_some() as usize;
                    for v in 0..BLOCK_VECTORS {
                        let times = f32x8::from_i32x8(i32x8::splat(block_start as i32 + 8 * v as i32) + lane_indices())
                            / f32x8::splat(sample_rate);
                        let levels = match release_level {
                            Some((index, level)) => envelope.sample_released8(index, level, times),
                            None => envelope.sample_held8(times),
                        };
                        for (time, level) in times.to_array().into_iter().zip(levels.to_array()) {
                            let expected = match release_index {
                                Some(index) => envelope.sample_released(index, time),
                                None => envelope.sample_held(time),
                            };
                            let case = format!("{envelope:?}, released at {release:?}, at {time}");
                            assert_eq!(level.to_bits(), expected.to_bits(), "{case}: {level} vs {expected}");
                            if let Some(constant) = constant {
                                assert_eq!(constant.to_bits(), expected.to_bits(), "{case}: {constant} vs {expected}");
                            }
                        }
                    }
                }
            }
        }
        // (Many of them are)
        assert!(constant_blocks > 1000, "{constant_blocks}");
    }
}
